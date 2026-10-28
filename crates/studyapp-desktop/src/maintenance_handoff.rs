//! Private inherited-pipe maintenance handoff and process-bound startup health.
use crate::maintenance_execute::{AuthenticatedUninstall, PreparedMaintenance};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, OnceLock, mpsc},
    time::Duration,
};
use studyapp_desktop::maintenance::{
    FileIdentity, FileRole, InstallReceipt, OwnedFile, PayloadKind, same_absolute_path,
    valid_sha256, validate_receipt,
};

#[path = "maintenance_handoff_cleanup.rs"]
mod cleanup;
pub(crate) use cleanup::claim_recovery_stage;
pub(crate) use cleanup::{
    abandon_current, claim_stage, initialize_local_stage, mark_recoverable, record_publication,
    recorded_files, retry_local_stages,
};
pub(crate) use cleanup::{cleanup_download_stage, cleanup_stage_on_error, create_stage_file};

const PROTOCOL: u32 = 1;
const MAX_FRAME: usize = 1024 * 1024;
const HEALTH_ENV: &str = "WW_STUDYAPP_MAINTENANCE_HEALTH";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Hello {
    protocol: u32,
    transaction: String,
    nonce: String,
    parent_pid: u32,
    parent_created: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    hello: Hello,
    source_executable: PathBuf,
    source_identity: FileIdentity,
    intent: Intent,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Intent {
    Update(Box<PreparedMaintenance>),
    Uninstall(Box<AuthenticatedUninstall>),
    Recover(Box<crate::maintenance_execute::RecoveryRequest>),
}
impl Intent {
    fn receipt(&self) -> &InstallReceipt {
        match self {
            Self::Update(p) => &p.receipt,
            Self::Uninstall(p) => &p.receipt,
            Self::Recover(p) => &p.source_receipt,
        }
    }
    fn transaction(&self) -> &str {
        match self {
            Self::Update(p) => &p.transaction_id,
            Self::Uninstall(p) => &p.transaction_id,
            Self::Recover(p) => &p.prepared.transaction_id,
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ready {
    protocol: u32,
    nonce: String,
    transaction: String,
    child_pid: u32,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Health {
    protocol: u32,
    nonce: String,
    version: String,
    child_pid: u32,
}
static HEALTH: OnceLock<Mutex<Option<Health>>> = OnceLock::new();

fn valid_reference(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
fn validate_hello(hello: &Hello, transaction: &str, actual_parent: u32) -> Result<()> {
    ensure!(
        hello.protocol == PROTOCOL
            && valid_reference(transaction)
            && hello.transaction == transaction
            && valid_sha256(&hello.nonce)
            && actual_parent > 0
            && hello.parent_pid == actual_parent
            && hello.parent_created > 0,
        "维护交接身份或事务无效"
    );
    Ok(())
}
fn validate_health(response: &Health, challenge: &Health) -> Result<()> {
    ensure!(
        response.protocol == PROTOCOL
            && response.nonce == challenge.nonce
            && valid_sha256(&response.nonce)
            && response.version == challenge.version
            && response.child_pid == challenge.child_pid
            && response.child_pid > 0,
        "应用启动确认无效"
    );
    Ok(())
}
fn validate_restart_args(args: &[String]) -> Result<()> {
    ensure!(
        args.is_empty() || (args.len() == 2 && args[0] == "--data-dir"),
        "重启参数无效"
    );
    if args.len() == 2 {
        studyapp_desktop::maintenance::absolute_components(Path::new(&args[1]))?;
    }
    Ok(())
}
fn write_frame<T: Serialize>(writer: &mut impl Write, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    ensure!(bytes.len() <= MAX_FRAME, "维护交接数据过大");
    writer.write_all(&(bytes.len() as u32).to_le_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()?;
    Ok(())
}
fn read_frame<T: DeserializeOwned>(reader: &mut impl Read) -> Result<T> {
    let mut length = [0; 4];
    reader.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    ensure!(length > 0 && length <= MAX_FRAME, "维护交接数据长度无效");
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    Ok(serde_json::from_slice(&bytes)?)
}
fn receive<T: DeserializeOwned + Send + 'static>(
    mut reader: impl Read + Send + 'static,
    timeout: Duration,
) -> Result<T> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(read_frame::<T>(&mut reader));
    });
    receiver.recv_timeout(timeout).context("维护进程响应超时")?
}

/// Create one new private directory; never follows or replaces an existing path.
pub(crate) fn create_private_directory(path: &Path) -> Result<()> {
    studyapp_desktop::maintenance::absolute_components(path)?;
    let parent = path.parent().context("维护目录没有父目录")?;
    let _guards = crate::storage_backend::guard_directories(parent, false)?;
    native::create_private_directory(path)?;
    let _guard = crate::storage_backend::plain_handle(path, true, false)?;
    Ok(())
}
pub(crate) fn create_download_stage() -> Result<PathBuf> {
    let root = std::env::temp_dir();
    create_download_stage_in(&root)
}
fn create_download_stage_in(root: &Path) -> Result<PathBuf> {
    cleanup::retry_before_create(root)?;
    let path = root.join(format!(
        "WW-StudyApp-maintenance-{}",
        uuid::Uuid::new_v4().simple()
    ));
    create_private_directory(&path)?;
    cleanup::initialize_stage(&path)?;
    Ok(path)
}
fn copy_verified(source: &Path, destination: &Path, expected: &OwnedFile) -> Result<File> {
    let _guards =
        crate::storage_backend::guard_directories(source.parent().context("资源目录无效")?, false)?;
    let mut input = crate::storage_backend::plain_handle(source, false, true)?;
    let (identity, links) = crate::maintenance_runtime::handle_identity(&input, false)?;
    ensure!(
        links == 1 && identity.sha256 == expected.sha256 && identity.size == expected.size,
        "维护资源身份已改变"
    );
    std::io::Seek::rewind(&mut input)?;
    let mut output = create_stage_file(
        destination.parent().context("维护暂存目录无效")?,
        destination
            .file_name()
            .and_then(|n| n.to_str())
            .context("维护资源名无效")?,
    )?;
    std::io::copy(&mut input, &mut output)?;
    output.seal()?;
    drop(output);
    let output = crate::storage_backend::plain_handle(destination, false, true)?;
    let (actual, links) = crate::maintenance_runtime::handle_identity(&output, false)?;
    ensure!(
        links == 1 && actual.sha256 == expected.sha256 && actual.size == expected.size,
        "维护资源复制校验失败"
    );
    Ok(output)
}
pub(crate) struct AcceptanceHandoff {
    pub(crate) stage: PathBuf,
    pub(crate) helper_pid: u32,
    pub(crate) helper_created: u64,
}
fn launch(intent: Intent) -> Result<AcceptanceHandoff> {
    let receipt = intent.receipt();
    validate_receipt(receipt)?;
    ensure!(valid_reference(intent.transaction()), "维护事务引用无效");
    let source = std::env::current_exe()?;
    let root = &receipt.install_root;
    if let Intent::Recover(request) = &intent {
        crate::maintenance_execute::validate_recovery_source(request, &source)?;
    } else {
        ensure!(
            same_absolute_path(source.parent().context("应用目录无效")?, root)?,
            "维护发起程序不属于此安装"
        );
    }
    let root_guards = crate::storage_backend::guard_directories(root, false)?;
    let root_identity = crate::maintenance_runtime::handle_identity(
        root_guards.last().context("缺少安装目录句柄")?,
        true,
    )?
    .0;
    ensure!(root_identity == receipt.root_identity, "安装目录身份已改变");
    let basename = source
        .file_name()
        .and_then(|p| p.to_str())
        .context("应用入口无效")?;
    let source_record = receipt
        .files
        .iter()
        .find(|f| {
            f.relative_path.eq_ignore_ascii_case(basename)
                && matches!(f.role, FileRole::Application | FileRole::Uninstaller)
        })
        .context("维护入口没有可信资源记录")?;
    let observed = crate::maintenance_runtime::observe_file(&source, basename)?;
    ensure!(
        observed.identity.sha256 == source_record.sha256
            && observed.identity.size == source_record.size
            && observed.link_count == 1,
        "维护入口已改变"
    );
    let stage = create_download_stage()?;
    let mut cleanup_on_error = cleanup_stage_on_error(&stage)?;
    ensure!(
        !studyapp_desktop::maintenance::absolute_components(&stage)?
            .starts_with(&studyapp_desktop::maintenance::absolute_components(root)?),
        "维护进程必须位于安装目录外"
    );
    let mut stage_guards = crate::storage_backend::guard_directories(&stage, false)?;
    let helper_receipt = if let Intent::Recover(request) = &intent {
        &request.prepared.receipt
    } else {
        receipt
    };
    let helper_root = if let Intent::Recover(request) = &intent {
        crate::maintenance_execute::recovery_snapshot(request)?
    } else {
        root.to_owned()
    };
    let helper_record = if matches!(&intent, Intent::Recover(_)) {
        helper_receipt
            .files
            .iter()
            .find(|f| f.role == FileRole::Uninstaller)
            .context("恢复入口缺失")?
    } else {
        source_record
    };
    let helper_source = if matches!(&intent, Intent::Recover(_)) {
        helper_root.join("Uninstall.exe")
    } else {
        source.clone()
    };
    stage_guards.push(copy_verified(
        &helper_source,
        &stage.join("WW-StudyApp.exe"),
        helper_record,
    )?);
    for runtime in helper_receipt
        .files
        .iter()
        .filter(|f| f.role == FileRole::Runtime)
    {
        stage_guards.push(copy_verified(
            &helper_root.join(&runtime.relative_path),
            &stage.join(&runtime.relative_path),
            runtime,
        )?);
    }
    let hello = Hello {
        protocol: PROTOCOL,
        transaction: intent.transaction().into(),
        nonce: format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        ),
        parent_pid: std::process::id(),
        parent_created: native::current_process_created()?,
    };
    let envelope = Envelope {
        hello: hello.clone(),
        source_executable: source,
        source_identity: observed.identity,
        intent,
    };
    let mut command = Command::new(stage.join("WW-StudyApp.exe"));
    command
        .args(["--maintenance-apply", &hello.transaction])
        .current_dir(&stage)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env_remove(HEALTH_ENV);
    native::hide_console(&mut command);
    let mut child = command.spawn().context("无法启动维护进程")?;
    let exchange = (|| -> Result<u64> {
        let mut pipe = child.stdin.take().context("缺少维护交接管道")?;
        write_frame(&mut pipe, &envelope)?;
        drop(pipe);
        let ready: Ready = receive(
            child.stdout.take().context("缺少维护响应管道")?,
            Duration::from_secs(20),
        )?;
        ensure!(
            ready.protocol == PROTOCOL
                && ready.nonce == hello.nonce
                && ready.transaction == hello.transaction
                && ready.child_pid == child.id(),
            "维护进程未确认正确事务"
        );
        native::child_process_created(&child)
    })();
    if exchange.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    drop(stage_guards);
    drop(root_guards);
    if exchange.is_ok() {
        cleanup_on_error.disarm();
    }
    exchange.map(|helper_created| AcceptanceHandoff {
        stage,
        helper_pid: child.id(),
        helper_created,
    })
}
pub(crate) fn begin_update_handoff(prepared: &PreparedMaintenance) -> Result<()> {
    launch(Intent::Update(Box::new(prepared.clone()))).map(|h| {
        let _ = (h.stage, h.helper_pid, h.helper_created);
    })
}
pub(crate) fn begin_uninstall_handoff(intent: AuthenticatedUninstall) -> Result<()> {
    launch(Intent::Uninstall(Box::new(intent))).map(|h| {
        let _ = (h.stage, h.helper_pid, h.helper_created);
    })
}
pub(crate) fn begin_recovery_handoff(
    request: crate::maintenance_execute::RecoveryRequest,
) -> Result<AcceptanceHandoff> {
    launch(Intent::Recover(Box::new(request)))
}
#[cfg(all(windows, feature = "ui-test"))]
pub(crate) fn begin_update_handoff_for_acceptance(
    prepared: &PreparedMaintenance,
) -> Result<AcceptanceHandoff> {
    launch(Intent::Update(Box::new(prepared.clone())))
}
#[cfg(all(windows, feature = "ui-test"))]
pub(crate) fn begin_uninstall_handoff_for_acceptance(
    intent: AuthenticatedUninstall,
) -> Result<AcceptanceHandoff> {
    launch(Intent::Uninstall(Box::new(intent)))
}

/// No editable on-disk plan or command-line flag supplies handoff authority.
pub(crate) fn run_apply(transaction: &str) -> Result<()> {
    native::require_stdin_pipe()?;
    let envelope: Envelope = receive(std::io::stdin(), Duration::from_secs(20))?;
    let actual_parent = native::parent_process_id()?;
    validate_hello(&envelope.hello, transaction, actual_parent)?;
    native::require_stdin_owner(actual_parent)?;
    ensure!(
        envelope.intent.transaction() == transaction,
        "维护事务不匹配"
    );
    let parent = native::bind_parent(
        actual_parent,
        envelope.hello.parent_created,
        &envelope.source_executable,
    )?;
    let receipt = envelope.intent.receipt();
    validate_receipt(receipt)?;
    if let Intent::Recover(request) = &envelope.intent {
        crate::maintenance_execute::validate_recovery_source(request, &envelope.source_executable)?;
    } else {
        ensure!(
            same_absolute_path(
                envelope
                    .source_executable
                    .parent()
                    .context("维护来源目录无效")?,
                &receipt.install_root
            )?,
            "维护来源安装目录不符"
        );
    }
    let source = crate::maintenance_runtime::observe_file(&envelope.source_executable, "source")?;
    ensure!(
        source.identity == envelope.source_identity && source.link_count == 1,
        "维护来源文件已改变"
    );
    let source_name = envelope
        .source_executable
        .file_name()
        .and_then(|p| p.to_str())
        .context("维护来源名称无效")?;
    let record = receipt
        .files
        .iter()
        .find(|f| {
            f.relative_path.eq_ignore_ascii_case(source_name)
                && matches!(f.role, FileRole::Application | FileRole::Uninstaller)
        })
        .context("维护来源不属于安装记录")?;
    ensure!(
        record.sha256 == source.identity.sha256 && record.size == source.identity.size,
        "维护来源未验证"
    );
    let executable = std::env::current_exe()?;
    let own = crate::maintenance_runtime::observe_file(&executable, "self")?;
    let helper_receipt = if let Intent::Recover(request) = &envelope.intent {
        &request.prepared.receipt
    } else {
        receipt
    };
    let helper_record = if matches!(&envelope.intent, Intent::Recover(_)) {
        helper_receipt
            .files
            .iter()
            .find(|f| f.role == FileRole::Uninstaller)
            .context("恢复入口缺失")?
    } else {
        record
    };
    ensure!(
        own.identity.sha256 == helper_record.sha256
            && own.identity.size == helper_record.size
            && own.link_count == 1,
        "维护程序与来源不相同"
    );
    ensure!(
        !studyapp_desktop::maintenance::absolute_components(&executable)?.starts_with(
            &studyapp_desktop::maintenance::absolute_components(&receipt.install_root)?
        ),
        "维护程序仍位于安装目录内"
    );
    let stage_guards = crate::storage_backend::guard_directories(
        executable.parent().context("维护程序目录无效")?,
        false,
    )?;
    let mut runtime_handles = Vec::new();
    for runtime in helper_receipt
        .files
        .iter()
        .filter(|f| f.role == FileRole::Runtime)
    {
        let handle = crate::storage_backend::plain_handle(
            &executable.parent().unwrap().join(&runtime.relative_path),
            false,
            true,
        )?;
        let (actual, links) = crate::maintenance_runtime::handle_identity(&handle, false)?;
        ensure!(
            links == 1 && actual.sha256 == runtime.sha256 && actual.size == runtime.size,
            "维护运行库已改变"
        );
        runtime_handles.push(handle);
    }
    cleanup::claim_stage(
        executable.parent().context("维护目录无效")?,
        actual_parent,
        envelope.hello.parent_created,
    )?;
    if let Intent::Update(prepared) = &envelope.intent {
        crate::maintenance_execute::claim_prepared(
            prepared,
            actual_parent,
            envelope.hello.parent_created,
        )?;
    }
    write_frame(
        &mut std::io::stdout(),
        &Ready {
            protocol: PROTOCOL,
            nonce: envelope.hello.nonce,
            transaction: transaction.into(),
            child_pid: std::process::id(),
        },
    )?;
    parent.wait_for_exit(Duration::from_secs(120))?;
    // All root observations above have released their file/directory handles.
    let stage = executable.parent().context("维护程序目录无效")?;
    let mut cleanup_files = vec![executable.clone()];
    for runtime in receipt.files.iter().filter(|f| f.role == FileRole::Runtime) {
        cleanup_files.push(stage.join(&runtime.relative_path));
    }
    cleanup_files.push(stage.join("result.json"));
    let show_result = !matches!(&envelope.intent, Intent::Update(_));
    let failure_version = receipt.version.clone();
    let failure_root = receipt.install_root.clone();
    let operation = match envelope.intent {
        Intent::Update(prepared) => match prepared.kind {
            PayloadKind::PortableZip => crate::maintenance_execute::apply_portable_plan(&prepared),
            PayloadKind::Msi => crate::maintenance_execute::apply_msi_update(&prepared),
            _ => Err(anyhow::anyhow!("维护载荷类型不受支持")),
        },
        Intent::Uninstall(intent) => {
            crate::maintenance_execute::apply_authenticated_uninstall(*intent)
        }
        Intent::Recover(request) => crate::maintenance_execute::apply_recovery(&request),
    };
    let mut status = operation.unwrap_or_else(|error| {
        eprintln!("WW-StudyApp: 维护事务未完成：{error:#}");
        crate::maintenance_execute::MaintenanceStatus {
            transaction_id: transaction.into(),
            state: crate::maintenance_execute::MaintenanceState::Failed,
            version: failure_version,
            code: "maintenance_failed".into(),
            retained_paths: vec![failure_root],
        }
    });
    status.retained_paths.extend(cleanup_files.iter().cloned());
    let mut result = create_stage_file(stage, "result.json")?;
    serde_json::to_writer(&mut result, &status)?;
    result.seal()?;
    drop(result);
    drop(runtime_handles);
    drop(stage_guards);
    if show_result || status.state == crate::maintenance_execute::MaintenanceState::Failed {
        crate::maintenance_ui::run_result(status, stage.to_owned())?;
    }
    #[cfg(all(windows, feature = "ui-test"))]
    if stage.join("maintenance-result.png").try_exists()? {
        cleanup_files.push(stage.join("maintenance-result.png"));
    }
    cleanup::defer_helper_stage(stage, &cleanup_files)?;
    Ok(())
}

/// Called before normal storage startup only when our private health channel exists.
pub(crate) fn initialize_health_channel() -> Result<bool> {
    let Some(nonce) = std::env::var_os(HEALTH_ENV) else {
        return Ok(false);
    };
    let nonce = nonce
        .into_string()
        .map_err(|_| anyhow::anyhow!("启动确认参数无效"))?;
    ensure!(valid_sha256(&nonce), "启动确认参数无效");
    native::require_stdin_pipe()?;
    let challenge: Health = receive(std::io::stdin(), Duration::from_secs(20))?;
    ensure!(
        challenge.protocol == PROTOCOL
            && challenge.nonce == nonce
            && challenge.version == env!("CARGO_PKG_VERSION")
            && challenge.child_pid == std::process::id(),
        "启动确认挑战不匹配"
    );
    let mut state = HEALTH
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map_err(|_| anyhow::anyhow!("启动确认状态不可用"))?;
    ensure!(state.is_none(), "启动确认不能重复初始化");
    *state = Some(challenge);
    Ok(true)
}
/// A snapshot is emitted only after the worker opened storage successfully.
pub(crate) fn acknowledge_storage_ready() {
    let Some(state) = HEALTH.get() else {
        return;
    };
    let Ok(mut state) = state.lock() else {
        return;
    };
    if let Some(challenge) = state.take() {
        let _ = write_frame(&mut std::io::stdout(), &challenge);
    }
}
pub(crate) fn restart_and_wait_for_health(
    executable: &Path,
    expected: &OwnedFile,
    version: &str,
    restart_args: &[String],
) -> Result<()> {
    validate_restart_args(restart_args)?;
    let _guards = crate::storage_backend::guard_directories(
        executable.parent().context("重启目录无效")?,
        false,
    )?;
    let handle = crate::storage_backend::plain_handle(executable, false, true)?;
    let (identity, links) = crate::maintenance_runtime::handle_identity(&handle, false)?;
    ensure!(
        links == 1 && identity.sha256 == expected.sha256 && identity.size == expected.size,
        "重启程序校验失败"
    );
    let nonce = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let mut child = Command::new(executable)
        .args(restart_args)
        .current_dir(executable.parent().unwrap())
        .env(HEALTH_ENV, &nonce)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("更新后应用无法启动")?;
    let result = (|| -> Result<()> {
        let challenge = Health {
            protocol: PROTOCOL,
            nonce,
            version: version.into(),
            child_pid: child.id(),
        };
        let mut pipe = child.stdin.take().context("缺少启动确认管道")?;
        write_frame(&mut pipe, &challenge)?;
        drop(pipe);
        let response: Health = receive(
            child.stdout.take().context("缺少启动响应管道")?,
            Duration::from_secs(90),
        )?;
        validate_health(&response, &challenge)?;
        ensure!(child.try_wait()?.is_none(), "更新后应用已退出");
        Ok(())
    })();
    if result.is_err() {
        // This handle belongs to the exact child we created; never kill by name/PID.
        if child.try_wait()?.is_none() {
            child
                .kill()
                .context("启动失败的应用仍在运行，无法安全回滚")?;
            child.wait().context("未能等待启动失败的应用退出")?;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hello() -> Hello {
        Hello {
            protocol: PROTOCOL,
            transaction: "a".repeat(32),
            nonce: "b".repeat(64),
            parent_pid: 123,
            parent_created: 456,
        }
    }
    #[test]
    fn maintenance_handoff_protocol_binds_nonce_transaction_and_os_parent() {
        let good = hello();
        assert!(validate_hello(&good, &good.transaction, 123).is_ok());
        assert!(validate_hello(&good, &"c".repeat(32), 123).is_err());
        assert!(validate_hello(&good, &good.transaction, 124).is_err());
        assert!(
            validate_hello(
                &Hello {
                    protocol: 2,
                    ..hello()
                },
                &good.transaction,
                123
            )
            .is_err()
        );
        assert!(
            validate_hello(
                &Hello {
                    nonce: "b".repeat(63),
                    ..hello()
                },
                &good.transaction,
                123
            )
            .is_err()
        );
        assert!(
            validate_hello(
                &Hello {
                    parent_created: 0,
                    ..hello()
                },
                &good.transaction,
                123
            )
            .is_err()
        );
    }
    #[test]
    fn maintenance_handoff_frames_are_bounded_and_reject_unknown_fields() {
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &hello()).unwrap();
        assert_eq!(
            read_frame::<Hello>(&mut bytes.as_slice())
                .unwrap()
                .parent_pid,
            123
        );
        assert!(
            read_frame::<Hello>(&mut ((MAX_FRAME + 1) as u32).to_le_bytes().as_slice()).is_err()
        );
        assert!(read_frame::<Hello>(&mut [0u8; 4].as_slice()).is_err());
        assert!(read_frame::<Hello>(&mut [1u8, 0, 0, 0, b'{'].as_slice()).is_err());
    }
    #[test]
    fn maintenance_handoff_health_rejects_wrong_process_version_nonce_and_arguments() {
        let expected = Health {
            protocol: PROTOCOL,
            nonce: "a".repeat(64),
            version: "0.8.0".into(),
            child_pid: 123,
        };
        assert!(validate_health(&expected, &expected).is_ok());
        assert!(
            validate_health(
                &Health {
                    child_pid: 124,
                    ..expected.clone()
                },
                &expected
            )
            .is_err()
        );
        assert!(
            validate_health(
                &Health {
                    version: "0.7.3".into(),
                    ..expected.clone()
                },
                &expected
            )
            .is_err()
        );
        assert!(
            validate_health(
                &Health {
                    nonce: "b".repeat(64),
                    ..expected.clone()
                },
                &expected
            )
            .is_err()
        );
        assert!(validate_restart_args(&[]).is_ok());
        assert!(validate_restart_args(&["--uninstall".into()]).is_err());
        assert!(validate_restart_args(&["--data-dir".into(), "../outside".into()]).is_err());
    }
    #[test]
    fn maintenance_handoff_private_directory_never_replaces_existing_content() {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .join(".scratch/installation-20261026/test-temp");
        std::fs::create_dir_all(&parent).unwrap();
        let target = parent.join(format!("handoff-{}", uuid::Uuid::new_v4().simple()));
        create_private_directory(&target).unwrap();
        std::fs::write(target.join("retained.txt"), b"retained").unwrap();
        assert!(create_private_directory(&target).is_err());
        assert_eq!(
            std::fs::read(target.join("retained.txt")).unwrap(),
            b"retained"
        );
    }
    #[cfg(windows)]
    #[test]
    fn maintenance_handoff_anonymous_pipe_has_bound_creator_and_process_timestamp() {
        native::test_pipe_owner().unwrap();
        let created = native::current_process_created().unwrap();
        let path = std::env::current_exe().unwrap();
        assert!(native::bind_parent(std::process::id(), created, &path).is_ok());
        assert!(native::bind_parent(std::process::id(), created + 1, &path).is_err());
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::{
        ffi::c_void,
        os::windows::{
            ffi::OsStrExt,
            io::{AsRawHandle, FromRawHandle, OwnedHandle},
            process::CommandExt,
        },
    };
    use windows_sys::Win32::{
        Foundation::{FILETIME, HANDLE, INVALID_HANDLE_VALUE, LocalFree, WAIT_OBJECT_0},
        Security::{GetTokenInformation, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser},
        Storage::FileSystem::{CreateDirectoryW, FILE_TYPE_PIPE, GetFileType},
        System::Threading::{
            GetCurrentProcess, GetProcessTimes, OpenProcess, OpenProcessToken,
            PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, QueryFullProcessImageNameW,
            WaitForSingleObject,
        },
    };
    #[repr(C)]
    struct ProcessEntry {
        size: u32,
        usage: u32,
        process_id: u32,
        heap: usize,
        module_id: u32,
        threads: u32,
        parent_process_id: u32,
        priority: i32,
        flags: u32,
        name: [u16; 260],
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateToolhelp32Snapshot(flags: u32, process: u32) -> HANDLE;
        fn Process32FirstW(snapshot: HANDLE, entry: *mut ProcessEntry) -> i32;
        fn Process32NextW(snapshot: HANDLE, entry: *mut ProcessEntry) -> i32;
        fn GetNamedPipeServerProcessId(pipe: HANDLE, process_id: *mut u32) -> i32;
        #[cfg(test)]
        fn CreatePipe(
            read: *mut HANDLE,
            write: *mut HANDLE,
            attributes: *const SECURITY_ATTRIBUTES,
            size: u32,
        ) -> i32;
    }
    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn ConvertSidToStringSidW(sid: *const c_void, text: *mut *mut u16) -> i32;
        fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
            text: *const u16,
            revision: u32,
            descriptor: *mut *mut c_void,
            size: *mut u32,
        ) -> i32;
    }
    struct LocalAllocation(*mut c_void);
    impl Drop for LocalAllocation {
        fn drop(&mut self) {
            // SAFETY: these pointers are allocated by the documented LocalAlloc APIs.
            unsafe {
                LocalFree(self.0);
            }
        }
    }
    fn wide(text: &std::ffi::OsStr) -> Vec<u16> {
        text.encode_wide().chain(Some(0)).collect()
    }
    fn sid(process: HANDLE) -> Result<String> {
        let mut token = std::ptr::null_mut();
        // SAFETY: valid borrowed process handle, writable token handle slot.
        ensure!(
            unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } != 0,
            "无法验证维护用户"
        );
        // SAFETY: successful API returned an owned token handle.
        let token = unsafe { OwnedHandle::from_raw_handle(token) };
        let mut length = 0;
        // SAFETY: querying required length with an intentionally null buffer.
        unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                std::ptr::null_mut(),
                0,
                &mut length,
            );
        }
        ensure!(length > 0 && length < 65536, "用户令牌长度无效");
        // usize alignment is sufficient for TOKEN_USER and SID_AND_ATTRIBUTES.
        let mut storage = vec![0usize; (length as usize).div_ceil(std::mem::size_of::<usize>())];
        // SAFETY: aligned storage has at least length bytes; live owned token handle.
        ensure!(
            unsafe {
                GetTokenInformation(
                    token.as_raw_handle(),
                    TokenUser,
                    storage.as_mut_ptr().cast(),
                    length,
                    &mut length,
                )
            } != 0,
            "无法读取维护用户令牌"
        );
        // SAFETY: successful GetTokenInformation(TokenUser) initializes this layout.
        let user = unsafe { &*storage.as_ptr().cast::<TOKEN_USER>() };
        let mut text = std::ptr::null_mut();
        // SAFETY: the SID points inside live token information storage.
        ensure!(
            unsafe { ConvertSidToStringSidW(user.User.Sid, &mut text) } != 0,
            "无法读取维护用户身份"
        );
        let _allocation = LocalAllocation(text.cast());
        let mut count = 0;
        // SAFETY: returned NUL-terminated SID string remains allocated until scope exit.
        unsafe {
            while count < 184 && *text.add(count) != 0 {
                count += 1;
            }
        }
        ensure!(count < 184, "用户身份长度无效");
        // SAFETY: count was bounded by the terminating NUL above.
        Ok(String::from_utf16(unsafe {
            std::slice::from_raw_parts(text, count)
        })?)
    }
    pub(super) fn create_private_directory(path: &Path) -> Result<()> {
        // SAFETY: GetCurrentProcess returns a valid pseudo-handle.
        let user = sid(unsafe { GetCurrentProcess() })?;
        let acl = format!("D:P(A;OICI;FA;;;{user})(A;OICI;FA;;;SY)");
        let text = wide(std::ffi::OsStr::new(&acl));
        let mut descriptor = std::ptr::null_mut();
        // SAFETY: NUL-terminated SDDL, correct revision, writable output.
        ensure!(
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    text.as_ptr(),
                    1,
                    &mut descriptor,
                    std::ptr::null_mut(),
                )
            } != 0,
            "无法创建私有维护权限"
        );
        let _allocation = LocalAllocation(descriptor);
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let path = wide(path.as_os_str());
        // SAFETY: live security descriptor and terminated path. Existing paths fail.
        ensure!(
            unsafe { CreateDirectoryW(path.as_ptr(), &attributes) } != 0,
            "无法创建私有维护目录"
        );
        Ok(())
    }
    pub(super) fn hide_console(command: &mut Command) {
        command.creation_flags(0x0800_0000);
    }
    pub(super) fn require_stdin_pipe() -> Result<()> {
        // SAFETY: stdin borrows its live standard handle.
        ensure!(
            unsafe { GetFileType(std::io::stdin().as_raw_handle()) } == FILE_TYPE_PIPE,
            "内部维护入口需要专用交接管道"
        );
        Ok(())
    }
    pub(super) fn require_stdin_owner(expected: u32) -> Result<()> {
        pipe_owner(std::io::stdin().as_raw_handle(), expected)
    }
    fn pipe_owner(pipe: HANDLE, expected: u32) -> Result<()> {
        let mut owner = 0;
        // SAFETY: pipe is a live borrowed handle and owner is writable output.
        ensure!(
            unsafe { GetNamedPipeServerProcessId(pipe, &mut owner) } != 0 && owner == expected,
            "维护交接管道不属于父进程"
        );
        Ok(())
    }
    #[cfg(test)]
    pub(super) fn test_pipe_owner() -> Result<()> {
        let mut read = std::ptr::null_mut();
        let mut write = std::ptr::null_mut();
        // SAFETY: writable handle outputs and null security attributes are documented.
        ensure!(
            unsafe { CreatePipe(&mut read, &mut write, std::ptr::null(), 0) } != 0,
            "无法创建测试管道"
        );
        // SAFETY: successful CreatePipe returns two distinct owned handles.
        let read = unsafe { OwnedHandle::from_raw_handle(read) };
        // SAFETY: successful CreatePipe returns two distinct owned handles.
        let _write = unsafe { OwnedHandle::from_raw_handle(write) };
        pipe_owner(read.as_raw_handle(), std::process::id())?;
        ensure!(
            pipe_owner(read.as_raw_handle(), std::process::id().wrapping_add(1)).is_err(),
            "错误管道用户未被拒绝"
        );
        Ok(())
    }
    fn created(handle: HANDLE) -> Result<u64> {
        let mut creation = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        // SAFETY: borrowed process handle and correctly typed output storage.
        ensure!(
            unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) }
                != 0,
            "无法验证维护父进程时间"
        );
        Ok((u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime))
    }
    pub(super) fn current_process_created() -> Result<u64> {
        // SAFETY: process pseudo-handle remains valid throughout the call.
        created(unsafe { GetCurrentProcess() })
    }
    pub(super) fn process_is_live(pid: u32, expected: u64) -> Result<bool> {
        // Query-only process identity prevents PID reuse from holding an old lease.
        let raw = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                pid,
            )
        };
        if raw.is_null() {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(87) {
                return Ok(false);
            }
            return Err(error.into());
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        Ok(created(handle.as_raw_handle())? == expected
            && unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) } != WAIT_OBJECT_0)
    }
    pub(super) fn child_process_created(child: &std::process::Child) -> Result<u64> {
        created(child.as_raw_handle())
    }
    pub(super) fn parent_process_id() -> Result<u32> {
        // SAFETY: documented process snapshot flag, no borrowed memory.
        let raw = unsafe { CreateToolhelp32Snapshot(2, 0) };
        ensure!(
            raw != INVALID_HANDLE_VALUE && !raw.is_null(),
            "无法验证实际父进程"
        );
        // SAFETY: successful API returned an owned snapshot handle.
        let snapshot = unsafe { OwnedHandle::from_raw_handle(raw) };
        // SAFETY: PROCESSENTRY32W is an all-integer C layout.
        let mut entry: ProcessEntry = unsafe { std::mem::zeroed() };
        entry.size = std::mem::size_of::<ProcessEntry>() as u32;
        // SAFETY: correct dwSize and writable process entry buffer.
        let mut found = unsafe { Process32FirstW(snapshot.as_raw_handle(), &mut entry) } != 0;
        while found {
            if entry.process_id == std::process::id() {
                return Ok(entry.parent_process_id);
            }
            // SAFETY: same live snapshot and initialized entry buffer.
            found = unsafe { Process32NextW(snapshot.as_raw_handle(), &mut entry) } != 0;
        }
        anyhow::bail!("无法确认维护父进程")
    }
    pub(super) struct Process(OwnedHandle);
    impl Process {
        pub(super) fn wait_for_exit(&self, timeout: Duration) -> Result<()> {
            // SAFETY: handle includes SYNCHRONIZE and remains alive while waiting.
            ensure!(
                unsafe {
                    WaitForSingleObject(
                        self.0.as_raw_handle(),
                        timeout.as_millis().min(u128::from(u32::MAX - 1)) as u32,
                    )
                } == WAIT_OBJECT_0,
                "应用尚未退出，维护操作已停止"
            );
            Ok(())
        }
    }
    pub(super) fn bind_parent(
        pid: u32,
        expected_created: u64,
        expected_path: &Path,
    ) -> Result<Process> {
        // SAFETY: only query and synchronize rights, never terminate the parent.
        let raw = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                pid,
            )
        };
        ensure!(!raw.is_null(), "无法打开维护父进程");
        // SAFETY: successful API returned an owned process handle.
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        ensure!(
            created(handle.as_raw_handle())? == expected_created,
            "维护父进程身份已变化"
        );
        // SAFETY: current-process pseudo handle is valid; both queries are read-only.
        ensure!(
            sid(handle.as_raw_handle())? == sid(unsafe { GetCurrentProcess() })?,
            "维护父进程用户不匹配"
        );
        let mut name = vec![0u16; 32768];
        let mut length = name.len() as u32;
        // SAFETY: live query-capable process handle, output buffer capacity in chars.
        ensure!(
            unsafe {
                QueryFullProcessImageNameW(
                    handle.as_raw_handle(),
                    0,
                    name.as_mut_ptr(),
                    &mut length,
                )
            } != 0,
            "无法验证维护父进程路径"
        );
        let actual = PathBuf::from(String::from_utf16(&name[..length as usize])?);
        ensure!(
            same_absolute_path(&actual, expected_path)?,
            "维护调用者不是指定应用"
        );
        Ok(Process(handle))
    }
}

#[cfg(not(windows))]
mod native {
    use super::*;
    pub(super) fn create_private_directory(path: &Path) -> Result<()> {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(path)?;
        Ok(())
    }
    pub(super) fn hide_console(_: &mut Command) {}
    pub(super) fn require_stdin_pipe() -> Result<()> {
        anyhow::bail!("维护进程仅支持 Windows")
    }
    pub(super) fn require_stdin_owner(_: u32) -> Result<()> {
        anyhow::bail!("维护进程仅支持 Windows")
    }
    pub(super) fn current_process_created() -> Result<u64> {
        anyhow::bail!("维护进程仅支持 Windows")
    }
    pub(super) fn child_process_created(_: &std::process::Child) -> Result<u64> {
        anyhow::bail!("维护进程仅支持 Windows")
    }
    pub(super) fn process_is_live(_: u32, _: u64) -> Result<bool> {
        anyhow::bail!("维护进程仅支持 Windows")
    }
    pub(super) fn parent_process_id() -> Result<u32> {
        anyhow::bail!("维护进程仅支持 Windows")
    }
    pub(super) struct Process;
    impl Process {
        pub(super) fn wait_for_exit(&self, _: Duration) -> Result<()> {
            anyhow::bail!("维护进程仅支持 Windows")
        }
    }
    pub(super) fn bind_parent(_: u32, _: u64, _: &Path) -> Result<Process> {
        anyhow::bail!("维护进程仅支持 Windows")
    }
}
