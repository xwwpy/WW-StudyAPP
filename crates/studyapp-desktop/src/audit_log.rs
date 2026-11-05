//! Local, allowlisted diagnostics. No API here accepts user text, paths, URLs or errors.
#[path = "audit_log_files.rs"]
mod files;
#[path = "audit_log_writer.rs"]
mod writer;

pub(crate) use files::inventory;
use serde::Serialize;
use studyapp_core::{LogLevel, NoticeCategory, NoticeEventId, NoticeSeverity};
pub(crate) use writer::{Configuration, Health, Logger, Runtime};

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase {
    ApplicationStart,
    ApplicationStop,
    WorkerStart,
    WorkerStop,
    LibraryLockAttempt,
    LibraryLockAcquired,
    LibraryLockRejected,
    LibraryLockReleased,
    LibraryOwnerService,
    LibraryOwnerRequest,
    EditorCreated,
    EditorReady,
    EditorVisibility,
    BusinessOutcome,
    UpdateCheck,
    UpdateDownload,
    UpdateVerify,
    UpdateInstall,
    NotificationRoute,
    NotificationPresented,
    NotificationAction,
    NotificationMerged,
    MigrationSuspend,
    MigrationResume,
    QueueOverflow,
}
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Outcome {
    Attempt,
    Success,
    Failure,
    Rejected,
    Deferred,
    Suppressed,
}

// Identity comes only from a validated UUID wrapper, never an arbitrary String.
fn event_id<S: serde::Serializer>(
    id: &Option<NoticeEventId>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match id {
        Some(id) => serializer.serialize_some(&id.to_string()),
        None => serializer.serialize_none(),
    }
}
#[derive(Clone, Copy, Debug, Serialize)]
pub(crate) struct Record {
    pub level: LogLevel,
    pub phase: Phase,
    pub outcome: Outcome,
    #[serde(serialize_with = "event_id", skip_serializing_if = "Option::is_none")]
    pub event: Option<NoticeEventId>,
    #[serde(serialize_with = "event_id", skip_serializing_if = "Option::is_none")]
    pub related_event: Option<NoticeEventId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<NoticeCategory>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os_code: Option<i32>,
}
impl Record {
    pub(crate) fn new(level: LogLevel, phase: Phase, outcome: Outcome) -> Self {
        Self {
            level,
            phase,
            outcome,
            event: None,
            related_event: None,
            category: None,
            count: None,
            duration_ms: None,
            os_code: None,
        }
    }
    pub(crate) fn notice(event: &studyapp_core::NoticeEvent) -> Self {
        let level = match event.severity {
            NoticeSeverity::Critical | NoticeSeverity::Error => LogLevel::Error,
            NoticeSeverity::Warning => LogLevel::Warn,
            _ => LogLevel::Info,
        };
        Self {
            event: Some(event.id),
            category: Some(event.category),
            ..Self::new(
                level,
                Phase::BusinessOutcome,
                if event.severity >= NoticeSeverity::Warning {
                    Outcome::Failure
                } else {
                    Outcome::Success
                },
            )
        }
    }
}

static INITIALIZED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static GLOBAL: std::sync::OnceLock<Logger> = std::sync::OnceLock::new();
static LOCATION: std::sync::OnceLock<(std::path::PathBuf, bool)> = std::sync::OnceLock::new();

pub(crate) fn initialize(database: &std::path::Path, managed: bool) -> anyhow::Result<Runtime> {
    INITIALIZED.store(true, std::sync::atomic::Ordering::Release);
    let default = if managed {
        crate::app_usage::runtime_paths()?
            .effective
            .local_data
            .join("logs")
    } else {
        database
            .parent()
            .ok_or_else(|| anyhow::anyhow!("资料库没有父目录"))?
            .join("logs")
    };
    let runtime = Runtime::start(None)?;
    let _ = LOCATION.set((default, managed));
    let _ = GLOBAL.set(runtime.logger.clone());
    Ok(runtime)
}
pub(crate) fn global() -> Option<&'static Logger> {
    GLOBAL.get()
}
pub(crate) fn record(record: Record) {
    if let Some(log) = global() {
        log.record(record);
    }
}
pub(crate) fn stage(level: LogLevel, phase: Phase, outcome: Outcome) {
    record(Record::new(level, phase, outcome));
}
pub(crate) fn configuration(settings: &studyapp_core::LoggingSettings) -> Option<Configuration> {
    let (directory, managed) = LOCATION.get()?;
    let default_directory = if *managed {
        crate::app_usage::runtime_paths()
            .map(|paths| paths.effective.local_data.join("logs"))
            .unwrap_or_else(|_| directory.clone())
    } else {
        directory.clone()
    };
    Some(Configuration {
        settings: settings.clone(),
        default_directory,
    })
}
pub(crate) fn configure(settings: &studyapp_core::LoggingSettings) {
    if let (Some(log), Some(config)) = (global(), configuration(settings)) {
        log.configure(config);
    }
}
pub(crate) fn flush() -> bool {
    global().is_none_or(|log| log.flush(std::time::Duration::from_secs(2)))
}

pub(crate) struct MigrationPause {
    config: Option<Configuration>,
}
pub(crate) fn pause_for_migration() -> anyhow::Result<MigrationPause> {
    let config = global().and_then(Logger::configuration);
    stage(LogLevel::Info, Phase::MigrationSuspend, Outcome::Attempt);
    if let Some(log) = global()
        && !log.suspend(std::time::Duration::from_secs(2))
    {
        if let Some(config) = &config {
            log.resume(config.clone());
        }
        anyhow::bail!("日志句柄尚未释放，本次迁移未开始，请稍后重试");
    }
    Ok(MigrationPause { config })
}
impl Drop for MigrationPause {
    fn drop(&mut self) {
        if let (Some(log), Some(previous)) = (global(), self.config.take()) {
            let next = configuration(&previous.settings).unwrap_or(previous);
            log.resume(next);
            stage(LogLevel::Info, Phase::MigrationResume, Outcome::Success);
        }
    }
}

pub(crate) fn health() -> Option<Health> {
    INITIALIZED
        .load(std::sync::atomic::Ordering::Acquire)
        .then(|| global().map_or(Health::Unavailable, Logger::health))
}
