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
            GetCurrentProcess, GetProcessTimes, OpenProcess, OpenProcessToken,
            QueryFullProcessImageNameW, WaitForSingleObject,
        },
    },
};
#[path = "update_user_windows.rs"]
mod user_launch;
type Result<T> = std::result::Result<T, String>;
const NO_WINDOW: u32 = 0x08000000;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WindowsTransaction {
    transaction: InstallTransaction,
    executable: PathBuf,
    old_hash: String,
    #[serde(default)]
    mode: WindowsInstallMode,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WindowsInstallMode {
    #[default]
    Installed,
    Portable,
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
// NSIS requires ordinary local DOS paths, not canonical verbatim spellings.
// Library paths and filesystem identity are deliberately not rewritten.
fn local_dos_path(path: &Path) -> Result<PathBuf> {
    let text = path
        .to_str()
        .ok_or("Windows update path is not valid Unicode")?;
    let text = text.strip_prefix(r"\\?\").unwrap_or(text);
    let bytes = text.as_bytes();
    if bytes.len() < 3
        || !bytes[0].is_ascii_alphabetic()
        || bytes[1] != b':'
        || bytes[2] != b'\\'
        || text[3..]
            .split('\\')
            .any(|part| part.ends_with(['.', ' ']) || part == "..")
    {
        return Err("自动更新需要普通本地磁盘路径；请从本地目录重新启动应用。".into());
    }
    Ok(PathBuf::from(text))
}
fn installed() -> Result<PathBuf> {
    let directory = PathBuf::from(registry("InstallLocation")?);
    safe_path(&directory)?;
    local_dos_path(&io(fs::canonicalize(directory.join("WW-StudyApp.exe")))?)
}
fn install_mode(executable: &Path) -> Result<WindowsInstallMode> {
    safe_path(executable)?;
    if installed().is_ok_and(|path| path == executable) {
        return Ok(WindowsInstallMode::Installed);
    }
    let root = executable.parent().ok_or("Missing executable directory")?;
    if executable.file_name() != Some(std::ffi::OsStr::new("WW-StudyApp.exe"))
        || root.join("Uninstall.exe").exists()
    {
        return Err(
            "安装信息与当前程序不一致。请使用安装包修复，或从完整的免安装版目录启动。".into(),
        );
    }
    for name in ["vcruntime140.dll", "vcruntime140_1.dll"] {
        let path = root.join(name);
        safe_path(&path)?;
        if !io(fs::metadata(&path))?.is_file() {
            return Err("免安装版运行文件不完整，请重新解压完整发行包。".into());
        }
    }
    Ok(WindowsInstallMode::Portable)
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
    status::publish(dir, state, message, write_new)
}
fn pointer() -> Result<PathBuf> {
    crate::app_usage::runtime_paths()
        .and_then(|paths| paths.update_pointer())
        .map_err(|error| error.to_string())
}
fn read_transaction(dir: &Path) -> Result<WindowsTransaction> {
    safe_path(dir)?;
    let mut tx: WindowsTransaction =
        serde_json::from_slice(&read_bounded(&dir.join("transaction.json"), 16384)?)
            .map_err(|e| e.to_string())?;
    tx.executable = local_dos_path(&tx.executable)?;
    if tx.transaction.schema != 1
        || !valid_id(&tx.transaction.id)
        || tx.transaction.old_pid == 0
        || tx.transaction.old_start == 0
        || install_mode(&tx.executable)? != tx.mode
    {
        return Err("Invalid installed update transaction".into());
    }
    Ok(tx)
}
pub fn prepare(package: &VerifiedPackage, data: Option<PathBuf>) -> Result<PathBuf> {
    user_launch::ensure_available()?;
    package.reverify()?;
    let original = io(std::env::current_exe())?;
    safe_path(&original)?;
    let executable = local_dos_path(&io(fs::canonicalize(&original))?)?;
    let mode = install_mode(&executable)?;
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
        executable: executable.clone(),
        mode,
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
    crate::app_usage::runtime_paths()
        .and_then(|paths| paths.publish_update_pointer(&dir))
        .map_err(|error| error.to_string())?;
    local_dos_path(&dir)
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
    let tx = read_transaction(dir)?;
    if dir.join("ready").exists() {
        return Err("Prepare a fresh update attempt".into());
    }
    write_new(&dir.join("launch-claimed"), tx.transaction.id.as_bytes())?;
    let result: Result<()> = (|| {
        let mut child = user_launch::spawn(dir)?;
        let started = Instant::now();
        loop {
            if let Some(code) = child.exit_code()? {
                return Err(format!(
                    "Update supervisor exited before readiness (0x{code:08X})"
                ));
            }
            if let Ok(bytes) = read_bounded(&dir.join("ready"), 36) {
                if bytes == tx.transaction.id.as_bytes() {
                    return Ok(());
                }
                child.kill();
                return Err("Readiness identity mismatch".into());
            }
            if started.elapsed() > Duration::from_secs(10) {
                child.kill();
                return Err("Update supervisor readiness timeout".into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    })();
    if let Err(error) = &result {
        record(dir, "failed", error)?;
    }
    result
}
fn restart(tx: &WindowsTransaction, health: Option<&Path>) -> Result<std::process::Child> {
    ordinary_user()?;
    let mut command = Command::new(&tx.executable);
    command.current_dir(tx.executable.parent().ok_or("Missing restart directory")?);
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
    #[cfg(feature = "update-acceptance")]
    user_launch::record_process(dir, "supervisor", std::process::id())?;
    let tx = read_transaction(dir)?;
    let package = VerifiedPackage::verify(dir)?;
    if package.version != tx.transaction.target_version {
        return Err("Approved update target changed".into());
    }
    let (old, created) = process(tx.transaction.old_pid)?;
    if created != tx.transaction.old_start {
        return Err("Original process identity changed".into());
    }
    let mut name = vec![0u16; 32768];
    let mut size = name.len() as u32;
    if unsafe { QueryFullProcessImageNameW(old.0, 0, name.as_mut_ptr(), &mut size) } == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let running = PathBuf::from(OsString::from_wide(&name[..size as usize]));
    if local_dos_path(&io(fs::canonicalize(running))?)? != tx.executable {
        return Err("Original process executable does not match update destination".into());
    }
    atomic(dir, "ready", tx.transaction.id.as_bytes())?;
    record(dir, "waiting", "Waiting for original application exit")?;
    if unsafe { WaitForSingleObject(old.0, 120000) } != WAIT_OBJECT_0 {
        return Err("Original process did not exit; installation not started".into());
    }
    package.reverify()?;
    record(dir, "installing", "Running verified NSIS installer")?;
    // /UPDATED remains supported for legacy clients. This supervisor owns restart/data context.
    if format!(
        "{:x}",
        Sha256::digest(read_bounded(&tx.executable, 512 * 1024 * 1024)?)
    ) != tx.old_hash
    {
        return Err("Original application changed before installation".into());
    }
    let mut installer = Command::new(dir.join("package.exe"));
    installer.arg("/S").current_dir(dir);
    if tx.mode == WindowsInstallMode::Portable {
        installer.arg("/PORTABLEUPDATE");
    }
    // NSIS requires /D= to be the last unquoted remainder. No shell is used.
    installer.raw_arg(format!(
        "/D={}",
        tx.executable
            .parent()
            .ok_or("Missing install directory")?
            .display()
    ));
    let mut installer_child = io(installer
        .creation_flags(NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn())?;
    #[cfg(feature = "update-acceptance")]
    user_launch::record_process(dir, "installer", installer_child.id())?;
    let status = io(installer_child.wait())?;
    if !status.success() {
        let message = format!("NSIS installation failed ({status}); no rollback is claimed");
        if read_bounded(&tx.executable, 512 * 1024 * 1024)
            .is_ok_and(|bytes| format!("{:x}", Sha256::digest(bytes)) == tx.old_hash)
        {
            status::recover(dir, &message, write_new, || restart(&tx, None))?;
        }
        return Err(message);
    }
    if install_mode(&tx.executable)? != tx.mode
        || (tx.mode == WindowsInstallMode::Installed && version()? != tx.transaction.target_version)
    {
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
    if local_dos_path(&io(fs::canonicalize(io(std::env::current_exe())?))?)? != tx.executable
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
    status::failure(&dir, |path| read_bounded(path, 16384))
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
    #[test]
    fn update_shell_paths_accept_unicode_and_verbatim_local_disks_only() {
        for text in [r"C:\学习 软件\WW-StudyApp.exe", r"D:\a & b\应用.exe"] {
            assert_eq!(
                local_dos_path(Path::new(text)).unwrap(),
                PathBuf::from(text)
            );
            assert_eq!(
                local_dos_path(Path::new(&format!(r"\\?\{text}"))).unwrap(),
                PathBuf::from(text)
            );
        }
        for text in [
            r"\\?\UNC\server\share\app.exe",
            r"\\server\share\app.exe",
            r"\\?\GLOBALROOT\Device\app.exe",
            r"\\?\Volume{abc}\app.exe",
            r"C:\folder.\app.exe",
            r"C:\folder \app.exe",
            r"C:\a\..\app.exe",
            "relative.exe",
            "",
        ] {
            assert!(local_dos_path(Path::new(text)).is_err(), "{text}");
        }
    }
    #[test]
    fn portable_mode_requires_complete_bundle_without_an_uninstaller() {
        let root = tempfile::tempdir().unwrap();
        let executable = root.path().join("WW-StudyApp.exe");
        fs::write(&executable, b"portable fixture").unwrap();
        let executable = local_dos_path(&fs::canonicalize(executable).unwrap()).unwrap();
        assert!(install_mode(&executable).is_err());
        for name in ["vcruntime140.dll", "vcruntime140_1.dll"] {
            fs::write(root.path().join(name), b"runtime").unwrap();
        }
        assert_eq!(
            install_mode(&executable).unwrap(),
            WindowsInstallMode::Portable
        );
        fs::write(root.path().join("Uninstall.exe"), b"installed owner").unwrap();
        assert!(install_mode(&executable).is_err());
    }
    #[test]
    fn legacy_update_transaction_defaults_to_registered_installation() {
        let value = serde_json::json!({"transaction":{"schema":1,"id":uuid::Uuid::new_v4(),
            "old_pid":1,"old_start":1,"uid":0,"target_version":"0.2.3","data_directory":null},
            "executable":r"\\?\C:\App\WW-StudyApp.exe","old_hash":"a".repeat(64)});
        let tx: WindowsTransaction = serde_json::from_value(value).unwrap();
        assert_eq!(tx.mode, WindowsInstallMode::Installed);
        assert_eq!(
            local_dos_path(&tx.executable).unwrap(),
            Path::new(r"C:\App\WW-StudyApp.exe")
        );
    }
}
