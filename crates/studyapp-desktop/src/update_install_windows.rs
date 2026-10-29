//! Ordinary-user Windows NSIS supervisor with exact process and restart receipts.
use super::*;
use std::{
    ffi::OsString,
    fs,
    io::Write,
    os::windows::{ffi::OsStringExt, fs::MetadataExt, process::CommandExt},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, FILETIME, HANDLE, WAIT_OBJECT_0},
    Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation},
    System::{
        Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW},
        Threading::{
            GetCurrentProcess, GetProcessTimes, OpenProcess, OpenProcessToken, WaitForSingleObject,
        },
    },
};
type Result<T> = std::result::Result<T, String>;
const NO_WINDOW: u32 = 0x08000000;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WindowsTransaction {
    transaction: InstallTransaction,
    executable: PathBuf,
    old_hash: String,
}
fn io<T>(r: std::io::Result<T>) -> Result<T> {
    r.map_err(|e| e.to_string())
}
struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
fn ordinary_user() -> Result<()> {
    let mut token = std::ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let token = Handle(token);
    let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut size = 0;
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenElevation,
            &mut elevation as *mut _ as *mut _,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut size,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().to_string());
    }
    if elevation.TokenIsElevated != 0 {
        return Err("Restart normally without administrator privileges before updating".into());
    }
    Ok(())
}
fn process(pid: u32) -> Result<(Handle, u64)> {
    let handle = unsafe { OpenProcess(0x00100000 | 0x1000, 0, pid) };
    if handle.is_null() {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let handle = Handle(handle);
    let mut creation = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut exit = creation;
    let mut kernel = creation;
    let mut user = creation;
    if unsafe { GetProcessTimes(handle.0, &mut creation, &mut exit, &mut kernel, &mut user) } == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok((
        handle,
        (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime),
    ))
}
fn registry(name: &str) -> Result<OsString> {
    let key: Vec<u16> = "Software\\WW-StudyApp\\Installer"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let mut size = 0;
    if unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    } != 0
        || size > 32768
    {
        return Err("Installed NSIS registration is missing".into());
    }
    let mut buffer = vec![0u16; size as usize / 2];
    if unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr() as *mut _,
            &mut size,
        )
    } != 0
    {
        return Err("Cannot read installed NSIS registration".into());
    }
    while buffer.last() == Some(&0) {
        buffer.pop();
    }
    Ok(OsString::from_wide(&buffer))
}
fn safe_path(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        let meta = io(fs::symlink_metadata(ancestor))?;
        if meta.file_attributes() & 0x400 != 0 {
            return Err("Reparse-point updater paths are unsupported".into());
        }
    }
    Ok(())
}
fn installed() -> Result<PathBuf> {
    let directory = PathBuf::from(registry("InstallLocation")?);
    safe_path(&directory)?;
    io(fs::canonicalize(directory.join("WW-StudyApp.exe")))
}
fn version() -> Result<String> {
    registry("Version")?
        .into_string()
        .map_err(|_| "Invalid installed version".into())
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = io(fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path))?;
    io(file.write_all(bytes))?;
    io(file.sync_all())
}
fn atomic(dir: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let temp = dir.join(format!("receipt-{}.tmp", uuid::Uuid::new_v4()));
    write_new(&temp, bytes)?;
    io(fs::rename(temp, dir.join(name)))
}
fn record(dir: &Path, state: &str, message: &str) -> Result<()> {
    write_new(
        &dir.join(format!("status-{state}-{}.json", uuid::Uuid::new_v4())),
        &serde_json::to_vec(&serde_json::json!({"state":state,"message":message}))
            .map_err(|e| e.to_string())?,
    )
}
fn pointer() -> Result<PathBuf> {
    directories::ProjectDirs::from("io", "StudyApp", "StudyApp")
        .map(|p| p.data_local_dir().join("update-last.txt"))
        .ok_or("Missing update record location".into())
}
fn read_transaction(dir: &Path) -> Result<WindowsTransaction> {
    safe_path(dir)?;
    let tx: WindowsTransaction =
        serde_json::from_slice(&read_bounded(&dir.join("transaction.json"), 16384)?)
            .map_err(|e| e.to_string())?;
    if tx.transaction.schema != 1
        || !valid_id(&tx.transaction.id)
        || tx.transaction.old_pid == 0
        || tx.transaction.old_start == 0
        || tx.executable != installed()?
    {
        return Err("Invalid installed update transaction".into());
    }
    Ok(tx)
}
pub fn prepare(package: &VerifiedPackage, data: Option<PathBuf>) -> Result<PathBuf> {
    ordinary_user()?;
    package.reverify()?;
    let executable = io(std::env::current_exe())?;
    if io(fs::canonicalize(&executable))? != installed()? {
        return Err("Automatic installation requires the registered installed application".into());
    }
    let dir = package
        .directory
        .with_file_name(format!("studyapp-install-{}", uuid::Uuid::new_v4()));
    io(fs::create_dir(&dir))?;
    write_new(
        &dir.join("manifest.json"),
        &read_bounded(&package.directory.join("manifest.json"), 2 * 1024 * 1024)?,
    )?;
    write_new(
        &dir.join("package.exe"),
        &read_bounded(&package.directory.join("package.exe"), 256 * 1024 * 1024)?,
    )?;
    let staged = VerifiedPackage::verify(&dir)?;
    if staged.digest != package.digest || staged.version != package.version {
        return Err("Approved update changed".into());
    }
    let (_, created) = process(std::process::id())?;
    let tx = WindowsTransaction {
        transaction: InstallTransaction {
            schema: 1,
            id: uuid::Uuid::new_v4().to_string(),
            old_pid: std::process::id(),
            old_start: created,
            uid: 0,
            target_version: package.version.clone(),
            data_directory: data,
        },
        executable: installed()?,
        old_hash: format!(
            "{:x}",
            Sha256::digest(read_bounded(&executable, 512 * 1024 * 1024)?)
        ),
    };
    write_new(
        &dir.join("transaction.json"),
        &serde_json::to_vec(&tx).map_err(|e| e.to_string())?,
    )?;
    stage_supervisor(&executable, &dir)?;
    let location = pointer()?;
    io(fs::create_dir_all(
        location.parent().ok_or("Missing status parent")?,
    ))?;
    io(fs::write(location, dir.to_string_lossy().as_bytes()))?;
    Ok(dir)
}
fn stage_supervisor(executable: &Path, dir: &Path) -> Result<()> {
    let source = executable
        .parent()
        .ok_or("Missing installed executable parent")?;
    for name in ["vcruntime140.dll", "vcruntime140_1.dll"] {
        let runtime = source.join(name);
        safe_path(&runtime)?;
        write_new(&dir.join(name), &read_bounded(&runtime, 4 * 1024 * 1024)?)?;
    }
    io(fs::copy(executable, dir.join("supervisor.exe")))?;
    Ok(())
}
pub fn spawn(dir: &Path) -> Result<()> {
    ordinary_user()?;
    let tx = read_transaction(dir)?;
    if dir.join("ready").exists() {
        return Err("Prepare a fresh update attempt".into());
    }
    write_new(&dir.join("launch-claimed"), tx.transaction.id.as_bytes())?;
    let mut child = io(Command::new(dir.join("supervisor.exe"))
        .arg("--update-supervisor")
        .arg(dir)
        .creation_flags(NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn())?;
    let started = Instant::now();
    loop {
        if io(child.try_wait())?.is_some() {
            return Err("Update supervisor exited before readiness".into());
        }
        if let Ok(bytes) = read_bounded(&dir.join("ready"), 36) {
            if bytes == tx.transaction.id.as_bytes() {
                return Ok(());
            }
            let _ = child.kill();
            return Err("Readiness identity mismatch".into());
        }
        if started.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            return Err("Update supervisor readiness timeout".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
fn restart(tx: &WindowsTransaction, health: Option<&Path>) -> Result<std::process::Child> {
    ordinary_user()?;
    let mut command = Command::new(&tx.executable);
    command.env_remove("STUDYAPP_UPDATE_TRANSACTION");
    if let Some(dir) = health {
        command.env("STUDYAPP_UPDATE_TRANSACTION", dir);
    }
    if let Some(data) = &tx.transaction.data_directory {
        command.arg("--data-dir").arg(data);
    }
    io(command
        .creation_flags(NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn())
}
pub fn supervise(dir: &Path) -> Result<()> {
    safe_path(dir)?;
    let result = supervise_inner(dir);
    if let Err(error) = &result {
        record(dir, "failed", error)?;
    }
    result
}
fn supervise_inner(dir: &Path) -> Result<()> {
    ordinary_user()?;
    let tx = read_transaction(dir)?;
    let package = VerifiedPackage::verify(dir)?;
    if package.version != tx.transaction.target_version {
        return Err("Approved update target changed".into());
    }
    let (old, created) = process(tx.transaction.old_pid)?;
    if created != tx.transaction.old_start {
        return Err("Original process identity changed".into());
    }
    atomic(dir, "ready", tx.transaction.id.as_bytes())?;
    record(dir, "waiting", "Waiting for original application exit")?;
    if unsafe { WaitForSingleObject(old.0, 120000) } != WAIT_OBJECT_0 {
        return Err("Original process did not exit; installation not started".into());
    }
    package.reverify()?;
    record(dir, "installing", "Running verified NSIS installer")?;
    // /UPDATED remains supported for legacy clients. This supervisor owns restart/data context.
    let status = io(Command::new(dir.join("package.exe"))
        .arg("/S")
        .creation_flags(NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status())?;
    if !status.success() {
        if read_bounded(&tx.executable, 512 * 1024 * 1024)
            .is_ok_and(|bytes| format!("{:x}", Sha256::digest(bytes)) == tx.old_hash)
        {
            let _ = restart(&tx, None);
        }
        return Err(format!(
            "NSIS installation failed ({status}); no rollback is claimed"
        ));
    }
    if installed()? != tx.executable || version()? != tx.transaction.target_version {
        return Err("Installed destination/version mismatch".into());
    }
    record(dir, "installed", &package.version)?;
    let mut child = restart(&tx, Some(dir))?;
    record(
        dir,
        "relaunching",
        "Waiting for restored library and drafts",
    )?;
    let started = Instant::now();
    loop {
        if io(child.try_wait())?.is_some() {
            return Err("Updated app exited before health confirmation".into());
        }
        if let Ok(bytes) = read_bounded(&dir.join("health.json"), 4096) {
            let value: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            if health_matches(
                &value,
                &tx.transaction.id,
                &tx.transaction.target_version,
                child.id(),
            ) {
                return record(dir, "healthy", "Updated app and restored drafts confirmed");
            }
            return Err("Health identity mismatch".into());
        }
        if started.elapsed() > Duration::from_secs(60) {
            return Err("Updated app startup confirmation timed out".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
pub fn acknowledge_health() -> Result<()> {
    let Some(dir) = std::env::var_os("STUDYAPP_UPDATE_TRANSACTION") else {
        return Ok(());
    };
    ordinary_user()?;
    let dir = PathBuf::from(dir);
    let tx = read_transaction(&dir)?;
    if io(fs::canonicalize(io(std::env::current_exe())?))? != tx.executable
        || tx.transaction.target_version != env!("CARGO_PKG_VERSION")
    {
        return Err("Health target mismatch".into());
    }
    atomic(&dir,"health.json",&serde_json::to_vec(&serde_json::json!({"id":tx.transaction.id,"version":env!("CARGO_PKG_VERSION"),"pid":std::process::id()})).map_err(|e|e.to_string())?)
}
pub fn previous_failure() -> Result<Option<String>> {
    let path = pointer()?;
    if !path.exists() {
        return Ok(None);
    }
    let dir =
        PathBuf::from(String::from_utf8(read_bounded(&path, 4096)?).map_err(|e| e.to_string())?);
    safe_path(&dir)?;
    let mut records = Vec::new();
    for entry in io(fs::read_dir(&dir))? {
        let entry = io(entry)?;
        if entry.file_name().to_string_lossy().starts_with("status-") {
            records.push((io(io(entry.metadata())?.modified())?, entry.path()));
        }
    }
    records.sort_by_key(|v| v.0);
    if let Some((_, path)) = records.last() {
        let value: serde_json::Value =
            serde_json::from_slice(&read_bounded(path, 16384)?).map_err(|e| e.to_string())?;
        if value["state"] == "failed" {
            return Ok(Some(format!(
                "上次更新未完成：{}。记录：{}",
                value["message"].as_str().unwrap_or("未知错误"),
                dir.display()
            )));
        }
    }
    Ok(None)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn update_install_windows_supervisor_keeps_bundled_runtime() {
        let root =
            std::env::temp_dir().join(format!("studyapp-win-runtime-{}", uuid::Uuid::new_v4()));
        let source = root.join("installed");
        let stage = root.join("stage");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir(&stage).unwrap();
        fs::write(source.join("WW-StudyApp.exe"), b"executable").unwrap();
        for name in ["vcruntime140.dll", "vcruntime140_1.dll"] {
            fs::write(source.join(name), name.as_bytes()).unwrap();
        }
        stage_supervisor(&source.join("WW-StudyApp.exe"), &stage).unwrap();
        for name in ["vcruntime140.dll", "vcruntime140_1.dll"] {
            assert_eq!(fs::read(stage.join(name)).unwrap(), name.as_bytes());
        }
        let missing = root.join("missing");
        fs::create_dir(&missing).unwrap();
        fs::remove_file(source.join("vcruntime140_1.dll")).unwrap();
        assert!(stage_supervisor(&source.join("WW-StudyApp.exe"), &missing).is_err());
    }
    #[test]
    fn update_install_windows_process_identity_is_stable() {
        let (_, first) = process(std::process::id()).unwrap();
        let (_, second) = process(std::process::id()).unwrap();
        assert_eq!(first, second);
        assert_ne!(first, 0);
    }
    #[test]
    fn update_install_windows_atomic_receipt_is_complete() {
        let dir =
            std::env::temp_dir().join(format!("studyapp-win-health-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        atomic(&dir, "health.json", br#"{"ready":true}"#).unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.join("health.json")).unwrap()).unwrap();
        assert_eq!(value["ready"], true);
    }
}
