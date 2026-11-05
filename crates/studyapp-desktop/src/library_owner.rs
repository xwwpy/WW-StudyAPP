//! Authenticated, local-only normal-close requests. Discovery files are hints,
//! never PID authority. The OS peer and held lock identity are verified anew.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
#[cfg(windows)]
#[path = "library_owner_windows.rs"]
mod native;
#[cfg(unix)]
#[path = "library_owner_unix.rs"]
mod native;
const LIMIT: usize = 4096;
const DEADLINE: Duration = Duration::from_millis(750);
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Phase {
    Idle,
    Requested,
    Waiting,
    Cancelled,
    Closing,
    Busy,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Hint {
    schema: u8,
    session: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Op {
    Inspect,
    Close,
    Status,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    schema: u8,
    session: String,
    lock: String,
    op: Op,
    ticket: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    schema: u8,
    session: String,
    lock: String,
    phase: Phase,
    ticket: Option<String>,
}
#[derive(Clone, Debug)]
pub(crate) struct Offer {
    pub pid: u32,
    session: String,
    lock: String,
    database: PathBuf,
}
#[derive(Clone, Debug)]
pub(crate) enum Progress {
    Phase(Phase),
    WaitingForRelease,
    Released,
}
fn valid_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn identity(file: &File) -> Result<String> {
    let (device, inode) = crate::app_usage::identity(file)?;
    Ok(format!("{device:016x}:{inode:032x}"))
}
fn hint_path(database: &Path) -> PathBuf {
    database.with_extension("ww-owner.json")
}
fn locked_file(database: &Path) -> Result<File> {
    let path = database.with_extension("lock");
    let pinned = crate::storage_backend::plain_handle(&path, false, false)?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    ensure!(identity(&file)? == identity(&pinned)?, "锁文件身份已改变");
    Ok(file)
}
fn is_held(file: &File) -> Result<bool> {
    match file.try_lock() {
        Ok(()) => {
            file.unlock()?;
            Ok(false)
        }
        Err(std::fs::TryLockError::WouldBlock) => Ok(true),
        Err(std::fs::TryLockError::Error(error)) => Err(error.into()),
    }
}
fn packet<T: Serialize>(stream: &mut impl Write, value: &T) -> Result<()> {
    let mut data = serde_json::to_vec(value)?;
    ensure!(data.len() < LIMIT, "关闭请求过大");
    data.push(b'\n');
    let deadline = Instant::now() + DEADLINE;
    let mut offset = 0;
    while offset < data.len() {
        match stream.write(&data[offset..]) {
            Ok(0) => anyhow::bail!("占用者连接已关闭"),
            Ok(n) => offset += n,
            Err(e) if native::pending(&e) => {}
            Err(e) => return Err(e.into()),
        }
        ensure!(Instant::now() < deadline, "占用者暂未响应");
        if offset < data.len() {
            thread::sleep(Duration::from_millis(5));
        }
    }
    Ok(())
}
fn read_packet<T: serde::de::DeserializeOwned>(stream: &mut impl Read) -> Result<T> {
    let deadline = Instant::now() + DEADLINE;
    let mut data = Vec::new();
    let mut buffer = [0u8; 512];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => anyhow::bail!("占用者连接已关闭"),
            Ok(n) => {
                data.extend_from_slice(&buffer[..n]);
                ensure!(data.len() <= LIMIT, "占用者响应过大");
                if let Some(end) = data.iter().position(|b| *b == b'\n') {
                    ensure!(end + 1 == data.len(), "占用者响应格式无效");
                    return Ok(serde_json::from_slice(&data[..end])?);
                }
            }
            Err(e) if native::pending(&e) => {}
            Err(e) => return Err(e.into()),
        }
        ensure!(Instant::now() < deadline, "占用者暂未响应");
        thread::sleep(Duration::from_millis(5));
    }
}
#[derive(Debug)]
pub(crate) struct Control {
    pub database: PathBuf,
    state: Mutex<(Phase, Option<String>)>,
}
impl Control {
    pub fn mark(&self, phase: Phase) {
        if let Ok(mut state) = self.state.lock() {
            state.0 = phase;
        }
    }
    #[cfg(test)]
    pub fn phase(&self) -> Phase {
        self.state
            .lock()
            .map(|s| s.0.clone())
            .unwrap_or(Phase::Busy)
    }
    fn reply(&self, session: &str, lock: &str) -> Reply {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        Reply {
            schema: 1,
            session: session.into(),
            lock: lock.into(),
            phase: state.0.clone(),
            ticket: state.1.clone(),
        }
    }
    fn apply(&self, request: &Request) -> bool {
        if !matches!(request.op, Op::Close) {
            return false;
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.1 == request.ticket {
            return false;
        }
        if matches!(state.0, Phase::Idle | Phase::Cancelled | Phase::Busy) {
            *state = (Phase::Requested, request.ticket.clone());
            true
        } else {
            false
        }
    }
}
pub(crate) struct Server {
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
impl Server {
    /// The caller must retain the exact locked File until this server is dropped.
    pub fn start(
        database: &Path,
        file: &File,
        notify: impl Fn(Arc<Control>) + Send + 'static,
    ) -> Result<Self> {
        let session = uuid::Uuid::new_v4().simple().to_string();
        let lock = identity(file)?;
        let mut listener = native::Listener::bind(&session)?;
        crate::app_migration::atomic_json(
            &hint_path(database),
            &Hint {
                schema: 1,
                session: session.clone(),
            },
        )?;
        let control = Arc::new(Control {
            database: database.to_owned(),
            state: Mutex::new((Phase::Idle, None)),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let thread = thread::spawn(move || {
            while !stopped.load(Ordering::Acquire) {
                if let Ok(Some(mut connection)) = listener.accept() {
                    let result = (|| -> Result<()> {
                        native::verify_peer(&connection)?;
                        let request: Request = read_packet(&mut connection)?;
                        ensure!(
                            request.schema == 1
                                && request.session == session
                                && request.lock == lock,
                            "占用者或资料库身份不匹配"
                        );
                        ensure!(
                            !matches!(request.op, Op::Close | Op::Status)
                                || request.ticket.as_deref().is_some_and(valid_id),
                            "关闭请求身份无效"
                        );
                        if control.apply(&request) {
                            crate::audit_log::stage(
                                studyapp_core::LogLevel::Info,
                                crate::audit_log::Phase::LibraryOwnerRequest,
                                crate::audit_log::Outcome::Attempt,
                            );
                            notify(control.clone());
                        }
                        packet(&mut connection, &control.reply(&session, &lock))?;
                        // Keep named-pipe replies alive until the client has consumed them.
                        let _ = read_packet::<bool>(&mut connection);
                        Ok(())
                    })();
                    let _ = result; // Invalid/untrusted requests never reach UI or process signals.
                }
                thread::sleep(Duration::from_millis(15));
            }
        });
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}
fn exchange(offer: &Offer, op: Op, ticket: Option<&str>) -> Result<Reply> {
    let mut connection = native::Connection::connect(&offer.session)?;
    ensure!(
        native::verify_peer(&connection)? == offer.pid,
        "占用者进程身份已变化"
    );
    exchange_on(offer, &mut connection, op, ticket)
}
fn exchange_on(
    offer: &Offer,
    connection: &mut native::Connection,
    op: Op,
    ticket: Option<&str>,
) -> Result<Reply> {
    packet(
        connection,
        &Request {
            schema: 1,
            session: offer.session.clone(),
            lock: offer.lock.clone(),
            op,
            ticket: ticket.map(str::to_owned),
        },
    )?;
    let response: Reply = read_packet(connection)?;
    ensure!(
        response.schema == 1 && response.session == offer.session && response.lock == offer.lock,
        "占用者或资料库身份已变化"
    );
    if let Some(ticket) = ticket {
        ensure!(
            response.ticket.as_deref() == Some(ticket),
            "占用者正在处理另一项关闭请求"
        );
    }
    packet(connection, &true)?;
    Ok(response)
}
pub(crate) fn inspect(database: &Path, file: &File) -> Result<Offer> {
    let path = hint_path(database);
    let file_hint = crate::storage_backend::plain_handle(&path, false, true)?;
    ensure!(
        file_hint.metadata()?.len() <= LIMIT as u64,
        "占用者提示过大"
    );
    let mut data = Vec::new();
    file_hint.take(LIMIT as u64 + 1).read_to_end(&mut data)?;
    ensure!(data.len() <= LIMIT, "占用者提示过大");
    let hint: Hint = serde_json::from_slice(&data)?;
    ensure!(
        hint.schema == 1 && valid_id(&hint.session),
        "占用者提示无效"
    );
    let mut connection = native::Connection::connect(&hint.session)?;
    let pid = native::verify_peer(&connection)?;
    let offer = Offer {
        pid,
        session: hint.session,
        lock: identity(file)?,
        database: database.to_owned(),
    };
    exchange_on(&offer, &mut connection, Op::Inspect, None)?;
    ensure!(pid != std::process::id(), "不能请求当前应用关闭自己");
    ensure!(is_held(file)?, "资料库已释放，请直接重试");
    Ok(offer)
}
impl Offer {
    pub fn request_close(&self, ticket: &str) -> Result<Progress> {
        let file = locked_file(&self.database)?;
        ensure!(identity(&file)? == self.lock, "资料库锁身份已改变");
        if !is_held(&file)? {
            return Ok(Progress::Released);
        }
        Ok(Progress::Phase(
            exchange(self, Op::Close, Some(ticket))?.phase,
        ))
    }
    pub fn progress(&self, ticket: &str) -> Result<Progress> {
        let file = locked_file(&self.database)?;
        ensure!(identity(&file)? == self.lock, "资料库锁身份已改变");
        if !is_held(&file)? {
            return Ok(Progress::Released);
        }
        match exchange(self, Op::Status, Some(ticket)) {
            Ok(reply) => Ok(Progress::Phase(reply.phase)),
            Err(_) => {
                // Shutdown can close IPC just before the worker releases the lock.
                // A lost/unverifiable peer is never evidence of lock release, nor
                // authority to contact a different instance. Only watch this lock.
                if is_held(&file)? {
                    Ok(Progress::WaitingForRelease)
                } else {
                    Ok(Progress::Released)
                }
            }
        }
    }
}
#[cfg(test)]
#[path = "library_owner_tests.rs"]
mod tests;
