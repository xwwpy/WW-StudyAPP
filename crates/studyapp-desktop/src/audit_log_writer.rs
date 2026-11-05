//! Bounded data queue with an independent, coalescing control/flush channel.
use super::{Outcome, Phase, Record, files::Sink};
use serde::Serialize;
use std::{
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU8, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use studyapp_core::{LogLevel, LoggingSettings};
const CAPACITY: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Configuration {
    pub settings: LoggingSettings,
    pub default_directory: PathBuf,
}
impl Configuration {
    pub(crate) fn directory(&self) -> PathBuf {
        self.settings
            .directory
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.default_directory.clone())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum Health {
    Pending = 0,
    Healthy = 1,
    Disabled = 2,
    Unavailable = 3,
}
#[derive(Clone)]
struct Desired {
    revision: u64,
    config: Option<Arc<Configuration>>,
    suspended: bool,
    stop: bool,
}
struct Control {
    desired: Desired,
    accepted: u64,
    request: u64,
    target: u64,
    target_revision: u64,
    completed_request: u64,
    flush_ok: bool,
    finished: bool,
}
struct Shared {
    control: Mutex<Control>,
    wake: Condvar,
    dropped: AtomicU64,
    failures: AtomicU64,
    health: AtomicU8,
}
struct Message {
    serial: u64,
    revision: u64,
    config: Arc<Configuration>,
    record: Record,
}
#[derive(Clone)]
pub(crate) struct Logger {
    sender: SyncSender<Message>,
    shared: Arc<Shared>,
}
pub(crate) struct Runtime {
    pub logger: Logger,
    thread: Option<thread::JoinHandle<()>>,
}
impl Logger {
    pub(crate) fn record(&self, record: Record) -> bool {
        let mut c = self
            .shared
            .control
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(config) = &c.desired.config else {
            return false;
        };
        if c.desired.stop || c.desired.suspended || !config.settings.records(record.level) {
            return false;
        }
        let serial = c.accepted + 1;
        let message = Message {
            serial,
            revision: c.desired.revision,
            config: config.clone(),
            record,
        };
        match self.sender.try_send(message) {
            Ok(()) => {
                c.accepted = serial;
                self.shared.wake.notify_all();
                true
            }
            Err(mpsc::TrySendError::Full(_)) => {
                self.shared.dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                self.shared
                    .health
                    .store(Health::Unavailable as u8, Ordering::Release);
                false
            }
        }
    }
    pub(crate) fn configure(&self, config: Configuration) {
        let mut c = self
            .shared
            .control
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if c.desired.stop || c.desired.config.as_deref() == Some(&config) {
            return;
        }
        c.desired.config = Some(Arc::new(config));
        c.desired.revision += 1;
        self.shared.wake.notify_all();
    }
    pub(crate) fn configuration(&self) -> Option<Configuration> {
        self.shared
            .control
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .desired
            .config
            .as_deref()
            .cloned()
    }
    pub(crate) fn health(&self) -> Health {
        match self.shared.health.load(Ordering::Acquire) {
            1 => Health::Healthy,
            2 => Health::Disabled,
            3 => Health::Unavailable,
            _ => Health::Pending,
        }
    }
    pub(crate) fn flush(&self, timeout: Duration) -> bool {
        self.drain(timeout, true)
    }
    fn drain(&self, timeout: Duration, require_persistence: bool) -> bool {
        let deadline = Instant::now() + timeout;
        let mut c = self
            .shared
            .control
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if c.finished {
            return c.flush_ok || !require_persistence;
        }
        c.request += 1;
        let request = c.request;
        c.target = c.accepted;
        c.target_revision = c.desired.revision;
        self.shared.wake.notify_all();
        loop {
            if c.completed_request >= request {
                return c.flush_ok || !require_persistence;
            }
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            let (next, timed) = self
                .shared
                .wake
                .wait_timeout(c, remaining)
                .unwrap_or_else(|e| e.into_inner());
            c = next;
            if timed.timed_out() && c.completed_request < request {
                return false;
            }
        }
    }
    // Success confirms the drain completed and every sink/parent guard is closed.
    // Earlier persistence failures remain visible via health and flush(); they do
    // not permanently prevent migration once handles are released. Timeout MUST abort migration.
    pub(crate) fn suspend(&self, timeout: Duration) -> bool {
        {
            let mut c = self
                .shared
                .control
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if !c.desired.suspended {
                c.desired.suspended = true;
                c.desired.revision += 1;
            }
        }
        self.drain(timeout, false)
    }
    pub(crate) fn resume(&self, config: Configuration) {
        let mut c = self
            .shared
            .control
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if c.desired.stop {
            return;
        }
        c.desired.config = Some(Arc::new(config));
        c.desired.suspended = false;
        c.desired.revision += 1;
        self.shared.wake.notify_all();
    }
}
impl Runtime {
    pub(crate) fn start(config: Option<Configuration>) -> anyhow::Result<Self> {
        Self::with_start(config, || {})
    }
    fn with_start(
        config: Option<Configuration>,
        before: impl FnOnce() + Send + 'static,
    ) -> anyhow::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(CAPACITY);
        let shared = Arc::new(Shared {
            control: Mutex::new(Control {
                desired: Desired {
                    revision: 1,
                    config: config.map(Arc::new),
                    suspended: false,
                    stop: false,
                },
                accepted: 0,
                request: 0,
                target: 0,
                target_revision: 0,
                completed_request: 0,
                flush_ok: false,
                finished: false,
            }),
            wake: Condvar::new(),
            dropped: AtomicU64::new(0),
            failures: AtomicU64::new(0),
            health: AtomicU8::new(Health::Pending as u8),
        });
        let logger = Logger {
            sender,
            shared: shared.clone(),
        };
        let thread = thread::Builder::new()
            .name("studyapp-log".into())
            .spawn(move || {
                before();
                run(receiver, shared)
            })?;
        Ok(Self {
            logger,
            thread: Some(thread),
        })
    }
    pub(crate) fn shutdown(&mut self, timeout: Duration) -> bool {
        {
            let mut c = self
                .logger
                .shared
                .control
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if !c.desired.stop {
                c.desired.stop = true;
                c.desired.revision += 1;
            }
        }
        let flushed = self.logger.flush(timeout);
        if self
            .thread
            .as_ref()
            .is_some_and(|thread| thread.is_finished())
            && let Some(thread) = self.thread.take()
        {
            return thread.join().is_ok() && flushed;
        }
        // Never turn a bounded flush into an unbounded JoinHandle::join.
        flushed
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        let _ = self.shutdown(Duration::from_secs(2));
    }
}
#[derive(Serialize)]
struct Line {
    schema: u8,
    timestamp_ms: u128,
    process: u32,
    version: &'static str,
    os: &'static str,
    arch: &'static str,
    #[serde(flatten)]
    record: Record,
}
fn encode(record: Record) -> serde_json::Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(&Line {
        schema: 1,
        timestamp_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
        process: std::process::id(),
        version: env!("CARGO_PKG_VERSION"),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        record,
    })?;
    bytes.push(b'\n');
    Ok(bytes)
}
fn failed(shared: &Shared) {
    shared.failures.fetch_add(1, Ordering::Relaxed);
    shared
        .health
        .store(Health::Unavailable as u8, Ordering::Release);
}
fn flush_sink(sink: &mut Option<Sink>, shared: &Shared) {
    if sink.as_mut().is_some_and(|sink| sink.flush().is_err()) {
        *sink = None;
        failed(shared);
    }
}
fn apply(
    revision: u64,
    config: Option<&Configuration>,
    sink: &mut Option<Sink>,
    applied: &mut u64,
    shared: &Shared,
) {
    if *applied == revision {
        return;
    }
    flush_sink(sink, shared);
    *sink = None;
    *applied = revision;
    if let Some(config) = config.filter(|config| config.settings.enabled) {
        if config.settings.validate().is_err() {
            failed(shared);
            return;
        }
        match Sink::open(&config.directory()) {
            Ok(next) => {
                *sink = Some(next);
                shared
                    .health
                    .store(Health::Healthy as u8, Ordering::Release);
            }
            Err(_) => failed(shared),
        }
    } else {
        shared
            .health
            .store(Health::Disabled as u8, Ordering::Release);
    }
}
fn write(record: Record, sink: &mut Option<Sink>, shared: &Shared) {
    let Some(file) = sink.as_mut() else {
        failed(shared);
        return;
    };
    if encode(record)
        .ok()
        .is_none_or(|bytes| file.write(&bytes).is_err())
    {
        *sink = None;
        failed(shared);
    }
}
fn run(receiver: Receiver<Message>, shared: Arc<Shared>) {
    let mut sink = None;
    let mut applied = 0;
    let mut completed = 0;
    let mut last_flush = Instant::now();
    loop {
        // At most one queue capacity per pass; controls never compete for a data slot.
        for _ in 0..CAPACITY {
            let Ok(message) = receiver.try_recv() else {
                break;
            };
            apply(
                message.revision,
                Some(&message.config),
                &mut sink,
                &mut applied,
                &shared,
            );
            write(message.record, &mut sink, &shared);
            completed = message.serial;
        }
        let desired = shared
            .control
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .desired
            .clone();
        let config = if desired.suspended || desired.stop {
            None
        } else {
            desired.config.as_deref()
        };
        if sink.is_some() && shared.dropped.load(Ordering::Relaxed) > 0 {
            // Lost eligible records are an error at Error-only, otherwise a warning.
            let level = config.map_or(LogLevel::Warn, |config| {
                std::cmp::min(config.settings.minimum_level, LogLevel::Warn)
            });
            let count = shared.dropped.swap(0, Ordering::Relaxed);
            write(
                Record {
                    count: Some(count),
                    ..Record::new(level, Phase::QueueOverflow, Outcome::Rejected)
                },
                &mut sink,
                &shared,
            );
        }
        apply(desired.revision, config, &mut sink, &mut applied, &shared);
        let request_ready = {
            let c = shared.control.lock().unwrap_or_else(|e| e.into_inner());
            c.request > c.completed_request && completed >= c.target && applied >= c.target_revision
        };
        if request_ready || last_flush.elapsed() >= Duration::from_secs(1) {
            flush_sink(&mut sink, &shared);
            last_flush = Instant::now();
        }
        let mut c = shared.control.lock().unwrap_or_else(|e| e.into_inner());
        if request_ready && completed >= c.target && applied >= c.target_revision {
            c.completed_request = c.request;
            c.flush_ok = shared.failures.load(Ordering::Relaxed) == 0;
            shared.wake.notify_all();
        }
        if desired.stop && completed >= c.accepted {
            drop(sink);
            c.finished = true;
            shared.wake.notify_all();
            break;
        }
        // The mutex is released by the condvar; it is NEVER held during filesystem I/O.
        let _ = shared
            .wake
            .wait_timeout(c, Duration::from_millis(100))
            .unwrap_or_else(|e| e.into_inner());
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    const WAIT: Duration = Duration::from_secs(5);
    fn config(path: &Path) -> Configuration {
        Configuration {
            settings: LoggingSettings::default(),
            default_directory: path.to_owned(),
        }
    }
    fn text(path: &Path) -> String {
        let mut result = String::new();
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|ext| ext == "jsonl") {
                result += &std::fs::read_to_string(path).unwrap();
            }
        }
        result
    }
    fn record(level: LogLevel, count: u64) -> Record {
        Record {
            count: Some(count),
            ..Record::new(level, Phase::BusinessOutcome, Outcome::Success)
        }
    }
    #[test]
    fn default_info_filters_debug_disable_preserves_notifications_and_shutdown_drains() {
        let root =
            tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
        let mut runtime = Runtime::start(Some(config(root.path()))).unwrap();
        let log = &runtime.logger;
        assert!(log.record(record(LogLevel::Info, 11)));
        assert!(!log.record(record(LogLevel::Debug, 22)));
        let mut disabled = config(root.path());
        disabled.settings.enabled = false;
        log.configure(disabled);
        assert!(!log.record(record(LogLevel::Error, 33)));
        assert!(log.flush(WAIT));
        let bytes = text(root.path());
        assert!(bytes.contains("\"count\":11"));
        assert!(!bytes.contains("\"count\":22"));
        assert!(!bytes.contains("\"count\":33"));
        assert_eq!(log.health(), Health::Disabled);
        let mut enabled = config(root.path());
        enabled.settings.minimum_level = LogLevel::Trace;
        log.configure(enabled);
        assert!(log.record(record(LogLevel::Trace, 44)));
        assert!(runtime.shutdown(WAIT));
        assert!(text(root.path()).contains("\"count\":44"));
    }
    #[test]
    fn full_queue_does_not_block_flush_controls_and_overflow_has_a_bounded_summary() {
        let root =
            tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
        let gate = Arc::new(std::sync::Barrier::new(2));
        let thread_gate = gate.clone();
        let runtime = Runtime::with_start(Some(config(root.path())), move || {
            thread_gate.wait();
        })
        .unwrap();
        let mut accepted = 0;
        for index in 0..1000 {
            accepted += usize::from(runtime.logger.record(record(LogLevel::Info, index)));
        }
        assert_eq!(accepted, CAPACITY);
        assert!(!runtime.logger.flush(Duration::from_millis(10)));
        gate.wait();
        assert!(runtime.logger.flush(WAIT));
        let bytes = text(root.path());
        assert_eq!(bytes.matches("\"phase\":\"queue_overflow\"").count(), 1);
        assert!(bytes.contains("\"count\":744"));
        assert_eq!(
            bytes
                .lines()
                .filter(|line| line.contains(r#""phase":"#))
                .count(),
            CAPACITY + 1
        );
    }
    #[test]
    fn suspend_releases_parent_guards_and_custom_directory_can_resume_after_move() {
        let root =
            tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
        let before = root.path().join("before");
        let after = root.path().join("after");
        let runtime = Runtime::start(Some(config(&before))).unwrap();
        let log = &runtime.logger;
        log.record(record(LogLevel::Info, 1));
        assert!(log.suspend(WAIT));
        assert!(!log.record(record(LogLevel::Error, 2)));
        std::fs::rename(&before, &after).unwrap();
        let mut custom = config(&root.path().join("unused-default"));
        custom.settings.directory = Some(after.to_string_lossy().into_owned());
        log.resume(custom);
        assert!(log.record(record(LogLevel::Warn, 3)));
        assert!(log.flush(WAIT));
        assert!(!before.exists());
        let bytes = text(&after);
        assert!(bytes.contains("\"count\":1"));
        assert!(!bytes.contains("\"count\":2"));
        assert!(bytes.contains("\"count\":3"));
    }
    #[test]
    fn unavailable_writer_does_not_retry_or_recurse_and_reconfiguration_can_recover() {
        let root =
            tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
        let blocked = root.path().join("file");
        std::fs::write(&blocked, "unknown").unwrap();
        let runtime = Runtime::start(Some(config(&blocked))).unwrap();
        assert!(!runtime.logger.flush(WAIT));
        assert_eq!(runtime.logger.health(), Health::Unavailable);
        assert!(runtime.logger.record(record(LogLevel::Error, 1)));
        assert!(!runtime.logger.flush(WAIT));
        assert_eq!(std::fs::read_to_string(blocked).unwrap(), "unknown");
        let valid = root.path().join("valid");
        runtime.logger.configure(config(&valid));
        runtime.logger.record(record(LogLevel::Info, 2));
        // Previous lost records remain honestly reflected in the process flush status.
        assert!(!runtime.logger.flush(WAIT));
        assert_eq!(runtime.logger.health(), Health::Healthy);
        assert!(text(&valid).contains("\"count\":2"));
    }
    #[test]
    fn rapid_configuration_changes_keep_accepted_records_in_their_original_directories() {
        let root =
            tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
        let a = root.path().join("a");
        let b = root.path().join("b");
        let gate = Arc::new(std::sync::Barrier::new(2));
        let thread_gate = gate.clone();
        let runtime = Runtime::with_start(None, move || {
            thread_gate.wait();
        })
        .unwrap();
        for index in 0..12 {
            runtime
                .logger
                .configure(config(if index % 2 == 0 { &a } else { &b }));
            assert!(runtime.logger.record(record(LogLevel::Info, index)));
        }
        let mut disabled = config(&root.path().join("must-not-create"));
        disabled.settings.enabled = false;
        runtime.logger.configure(disabled);
        gate.wait();
        assert!(runtime.logger.flush(WAIT));
        assert!(!root.path().join("must-not-create").exists());
        for (path, remainder) in [(&a, 0), (&b, 1)] {
            let records: Vec<serde_json::Value> = text(path)
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .filter(|line: &serde_json::Value| line.get("phase").is_some())
                .collect();
            let counts: Vec<_> = records
                .iter()
                .map(|record| record["count"].as_u64().unwrap())
                .collect();
            assert_eq!(
                counts,
                (0..12)
                    .filter(|value| value % 2 == remainder)
                    .collect::<Vec<_>>()
            );
        }
    }
    #[test]
    fn timed_out_suspend_cannot_claim_closed_handles_then_later_drain_closes_them() {
        let root =
            tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
        let directory = root.path().join("before");
        let after = root.path().join("after");
        let gate = Arc::new(std::sync::Barrier::new(2));
        let thread_gate = gate.clone();
        let runtime = Runtime::with_start(Some(config(&directory)), move || {
            thread_gate.wait();
        })
        .unwrap();
        runtime.logger.record(record(LogLevel::Info, 1));
        assert!(!runtime.logger.suspend(Duration::from_millis(10)));
        gate.wait();
        assert!(runtime.logger.flush(WAIT));
        std::fs::rename(directory, &after).unwrap();
        assert!(text(&after).contains(r#""count":1"#));
    }
    #[test]
    fn disabled_start_creates_no_files_and_notification_suppression_does_not_disable_logging() {
        let root =
            tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
        let directory = root.path().join("logs");
        let mut disabled = config(&directory);
        disabled.settings.enabled = false;
        let runtime = Runtime::start(Some(disabled)).unwrap();
        assert!(runtime.logger.flush(WAIT));
        assert!(!directory.exists());
        runtime.logger.configure(config(&directory));
        let mut policy = studyapp_core::NotificationSettings {
            do_not_disturb: true,
            ..Default::default()
        };
        policy
            .rules
            .entry(studyapp_core::NoticeCategory::SaveFailure)
            .or_default()
            .enabled = false;
        let event = studyapp_core::NoticeEvent::new(
            studyapp_core::NoticeCategory::SaveFailure,
            studyapp_core::NoticeSeverity::Error,
            "private source not serialized",
        );
        assert_eq!(
            policy.route(
                event.category,
                event.severity,
                &Default::default(),
                studyapp_core::NoticeEnvironment {
                    foreground: true,
                    desktop_available: false,
                    anchor_available: false,
                    minute: 720
                }
            ),
            studyapp_core::NoticeRoute::Suppressed
        );
        runtime.logger.record(Record::notice(&event));
        assert!(runtime.logger.flush(WAIT));
        assert!(text(&directory).contains(&event.id.to_string()));
        assert!(!text(&directory).contains("private source"));
    }
    #[test]
    fn source_error_and_summary_secrets_cannot_enter_allowlisted_records() {
        let root =
            tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
        let runtime = Runtime::start(Some(config(root.path()))).unwrap();
        let secret = "private body https://host.invalid/?token=secret C:/Users/private/error";
        let mut event = studyapp_core::NoticeEvent::new(
            studyapp_core::NoticeCategory::SaveFailure,
            studyapp_core::NoticeSeverity::Error,
            secret,
        );
        event.dedupe_key = Some("private-secret-key".into());
        runtime.logger.record(Record::notice(&event));
        assert!(runtime.logger.flush(WAIT));
        let bytes = text(root.path());
        assert!(bytes.contains(&event.id.to_string()));
        assert!(bytes.contains("save_failure"));
        for forbidden in [
            "private body",
            "https:",
            "token=",
            "C:/Users",
            "private-secret-key",
        ] {
            assert!(!bytes.contains(forbidden));
        }
    }
}
