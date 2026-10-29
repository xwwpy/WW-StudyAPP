//! Linux deb update supervisor and privileged installed worker.
use super::*;
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
const EXE: &str = "/usr/bin/studyapp";
type Result<T> = std::result::Result<T, String>;
fn io<T>(r: std::io::Result<T>) -> Result<T> {
    r.map_err(|e| e.to_string())
}
fn output(program: &str, args: &[&str]) -> Result<String> {
    let out = io(Command::new(program).args(args).env("LC_ALL", "C").output())?;
    if !out.status.success() {
        return Err(format!(
            "{program} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}
fn uid() -> Result<u32> {
    output("/usr/bin/id", &["-u"])?
        .parse()
        .map_err(|_| "Invalid uid".into())
}
fn safe_dir(path: &Path, owner: u32) -> Result<()> {
    let meta = io(fs::symlink_metadata(path))?;
    if !meta.is_dir() || meta.uid() != owner || meta.mode() & 0o022 != 0 {
        return Err("Unsafe staging directory".into());
    }
    Ok(())
}
fn bounded(path: &Path, owner: u32, limit: u64) -> Result<Vec<u8>> {
    let f = io(OpenOptions::new()
        .read(true)
        .custom_flags(0x20000 | 0x800)
        .open(path))?; // O_NOFOLLOW | O_NONBLOCK (reject FIFOs without waiting for writers)
    let meta = io(f.metadata())?;
    if !meta.is_file() || meta.uid() != owner || meta.nlink() != 1 || meta.len() > limit {
        return Err("Unsafe update file".into());
    }
    let mut bytes = Vec::new();
    io(f.take(limit + 1).read_to_end(&mut bytes))?;
    if bytes.len() as u64 > limit {
        return Err("Update file too large".into());
    }
    Ok(bytes)
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut f = io(OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path))?;
    io(f.write_all(bytes))?;
    io(f.sync_all())
}
fn record(dir: &Path, state: &str, message: &str) -> Result<()> {
    // Immutable unique records preserve failures across repeated attempts.
    let bytes = serde_json::to_vec(&serde_json::json!({"state":state,"message":message}))
        .map_err(|e| e.to_string())?;
    write_new(
        &dir.join(format!("status-{}-{}.json", state, uuid::Uuid::new_v4())),
        &bytes,
    )
}
fn identity(pid: u32) -> Result<Option<u64>> {
    match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(s) => process_start(&s)
            .map(Some)
            .ok_or("Invalid process identity".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}
fn wait_old(tx: &InstallTransaction) -> Result<()> {
    let start = Instant::now();
    while identity(tx.old_pid)? == Some(tx.old_start) {
        if start.elapsed() > Duration::from_secs(120) {
            return Err("Application did not close; installation not started".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}
fn installed() -> Result<String> {
    let meta = io(fs::symlink_metadata(EXE))?;
    if !meta.is_file() || meta.uid() != 0 || meta.mode() & 0o022 != 0 {
        return Err("Not a protected installed executable".into());
    }
    if output("/usr/bin/dpkg-query", &["-S", EXE])? != "studyapp: /usr/bin/studyapp" {
        return Err("Executable is not owned by studyapp deb".into());
    }
    let value = output(
        "/usr/bin/dpkg-query",
        &["-W", "-f=${Status}|${Architecture}|${Version}", "studyapp"],
    )?;
    value
        .strip_prefix("install ok installed|amd64|")
        .map(str::to_owned)
        .ok_or("Unsupported package state or architecture".into())
}
fn transaction(dir: &Path, owner: u32) -> Result<InstallTransaction> {
    safe_dir(dir, owner)?;
    let tx: InstallTransaction =
        serde_json::from_slice(&bounded(&dir.join("transaction.json"), owner, 16384)?)
            .map_err(|e| e.to_string())?;
    if tx.schema != 1
        || tx.uid != owner
        || owner == 0
        || !valid_id(&tx.id)
        || tx.old_pid < 2
        || tx.old_start == 0
    {
        return Err("Invalid installation transaction".into());
    }
    Ok(tx)
}

/// Called before close; only installed deb deployments are accepted.
pub fn prepare(package: &VerifiedPackage, data_directory: Option<PathBuf>) -> Result<PathBuf> {
    if io(std::env::current_exe())? != Path::new(EXE) {
        return Err("Automatic updates require the installed deb application".into());
    }
    installed()?;
    if !Path::new("/usr/bin/pkexec").is_file() {
        return Err("Install polkit/pkexec before updating".into());
    }
    let owner = uid()?;
    safe_dir(&package.directory, owner)?;
    package.reverify()?;
    // Each user retry gets independent readiness and status records.
    let attempt = package
        .directory
        .with_file_name(format!("studyapp-install-{}", uuid::Uuid::new_v4()));
    io(fs::DirBuilder::new().mode(0o700).create(&attempt))?;
    write_new(
        &attempt.join("manifest.json"),
        &bounded(
            &package.directory.join("manifest.json"),
            owner,
            2 * 1024 * 1024,
        )?,
    )?;
    write_new(
        &attempt.join("package"),
        &bounded(&package.directory.join("package"), owner, 256 * 1024 * 1024)?,
    )?;
    let staged = VerifiedPackage::verify(&attempt)?;
    if staged.version != package.version || staged.digest != package.digest {
        return Err("Approved package changed during preparation".into());
    }
    let package = &staged;
    let tx = InstallTransaction {
        schema: 1,
        id: uuid::Uuid::new_v4().to_string(),
        old_pid: std::process::id(),
        old_start: identity(std::process::id())?.ok_or("Missing process")?,
        uid: owner,
        target_version: package.version.clone(),
        data_directory,
    };
    write_new(
        &package.directory.join("transaction.json"),
        &serde_json::to_vec(&tx).map_err(|e| e.to_string())?,
    )?;
    io(fs::copy(EXE, package.directory.join("supervisor")))?;
    io(fs::set_permissions(
        package.directory.join("supervisor"),
        fs::Permissions::from_mode(0o700),
    ))?;
    let pointer = pointer_path()?;
    io(fs::create_dir_all(
        pointer.parent().ok_or("Invalid update record path")?,
    ))?;
    io(fs::write(
        pointer,
        package.directory.to_string_lossy().as_bytes(),
    ))?;
    Ok(package.directory.clone())
}
/// Readiness must succeed before the caller closes its window.
pub fn spawn(dir: &Path) -> Result<()> {
    // Attempts are single-use. A retained readiness marker must never authorize a new close.
    if fs::symlink_metadata(dir.join("ready")).is_ok() {
        return Err("This update attempt was already started; prepare a fresh transaction".into());
    }
    let tx = transaction(dir, uid()?)?;
    write_new(&dir.join("launch-claimed"), tx.id.as_bytes())?;
    let mut child = io(Command::new(dir.join("supervisor"))
        .arg("--update-supervisor")
        .arg(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn())?;
    let start = Instant::now();
    while !dir.join("ready").is_file() {
        if io(child.try_wait())?.is_some() {
            return Err("Update supervisor exited before readiness".into());
        }
        if start.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            return Err("Update supervisor readiness timeout".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if io(child.try_wait())?.is_some() {
        return Err("Update supervisor exited during readiness".into());
    }
    if bounded(&dir.join("ready"), tx.uid, 36)? != tx.id.as_bytes() {
        let _ = child.kill();
        return Err("Update supervisor readiness identity mismatch".into());
    }
    Ok(())
}
pub fn supervise(dir: &Path) -> Result<()> {
    safe_dir(dir, uid()?)?;
    persist_failure(dir, || supervise_inner(dir))
}
fn persist_failure(dir: &Path, action: impl FnOnce() -> Result<()>) -> Result<()> {
    let result = action();
    if let Err(error) = &result {
        record(dir, "failed", error)?;
    }
    result
}
fn supervise_inner(dir: &Path) -> Result<()> {
    let owner = uid()?;
    let tx = transaction(dir, owner)?;
    let package = VerifiedPackage::verify(dir)?;
    if package.version != tx.target_version {
        return Err("Approved target version changed".into());
    }
    let previous_version = installed()?;
    write_new(&dir.join("ready"), tx.id.as_bytes())?;
    record(dir, "waiting", "Waiting for saved application to exit")?;
    wait_old(&tx)?;
    record(dir, "installing", "Waiting for system authorization")?;
    let result = io(Command::new("/usr/bin/pkexec")
        .arg(EXE)
        .arg("--update-worker")
        .arg(dir)
        .status());
    match result {
        Ok(status) if status.success() => {
            if installed()? != package.version {
                record(
                    dir,
                    "failed",
                    "Installed version mismatch; repair package installation",
                )?;
                return Err("Installed version mismatch".into());
            }
            record(dir, "installed", &package.version)?;
            let mut child = restart(&tx, Some(dir))?;
            record(
                dir,
                "relaunching",
                "Waiting for application and library startup",
            )?;
            let start = Instant::now();
            loop {
                if io(child.try_wait())?.is_some() {
                    return Err("Updated application exited before health confirmation".into());
                }
                if let Ok(bytes) = bounded(&dir.join("health.json"), tx.uid, 4096) {
                    let value: serde_json::Value =
                        serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                    if health_matches(&value, &tx.id, &tx.target_version, child.id()) {
                        return record(
                            dir,
                            "healthy",
                            "Updated application and library startup confirmed",
                        );
                    }
                    return Err("Updated application health identity mismatch".into());
                }
                if start.elapsed() > Duration::from_secs(60) {
                    return Err("Updated application did not confirm startup; inspect the retained update records".into());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        Ok(status) if matches!(status.code(), Some(126 | 127)) => {
            record(
                dir,
                "cancelled",
                "System authorization cancelled or denied; installation was not started",
            )?;
            if installed()? == previous_version {
                restart(&tx, None)?;
            }
            Err("System authorization cancelled or denied".into())
        }
        other => {
            record(
                dir,
                "failed",
                &format!(
                    "Authorization or package installation failed: {other:?}. Package manager may require repair; no rollback claimed."
                ),
            )?;
            // A failed apt transaction may leave a partially configured package: never launch it blindly.
            Err("Update failed; inspect the retained transaction records".into())
        }
    }
}
fn restart(tx: &InstallTransaction, health: Option<&Path>) -> Result<std::process::Child> {
    if uid()? != tx.uid || tx.uid == 0 {
        return Err("Refusing elevated GUI restart".into());
    }
    let mut command = Command::new(EXE);
    command.env_remove("STUDYAPP_UPDATE_TRANSACTION");
    if let Some(path) = health {
        command.env("STUDYAPP_UPDATE_TRANSACTION", path);
    }
    if let Some(path) = &tx.data_directory {
        command.arg("--data-dir").arg(path);
    }
    io(command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn())
}

/// Invoked by pkexec using /usr/bin/studyapp, never a user-writable helper.
pub fn worker(dir: &Path) -> Result<()> {
    if uid()? != 0 || io(std::env::current_exe())? != Path::new(EXE) {
        return Err(
            "Worker requires the installed root-owned executable and system authorization".into(),
        );
    }
    installed()?;
    let owner: u32 = std::env::var("PKEXEC_UID")
        .map_err(|_| "Missing initiating uid")?
        .parse()
        .map_err(|_| "Invalid initiating uid")?;
    let tx = transaction(dir, owner)?;
    // Read all untrusted inputs into bounded memory; create root-owned snapshots before verification.
    let manifest = bounded(&dir.join("manifest.json"), owner, 2 * 1024 * 1024)?;
    let payload = bounded(&dir.join("package"), owner, 256 * 1024 * 1024)?;
    let root = Path::new("/var/lib/studyapp-updates");
    if !root.exists() {
        io(fs::DirBuilder::new().mode(0o700).create(root))?;
    }
    safe_dir(root, 0)?;
    let stage = root.join(uuid::Uuid::new_v4().to_string());
    io(fs::DirBuilder::new().mode(0o700).create(&stage))?;
    write_new(&stage.join("manifest.json"), &manifest)?;
    write_new(&stage.join("package"), &payload)?;
    let package = VerifiedPackage::verify(&stage)?;
    if package.version != tx.target_version {
        return Err("Approved target version changed".into());
    }
    let current = semver::Version::parse(&installed()?).map_err(|e| e.to_string())?;
    let offered = semver::Version::parse(&package.version).map_err(|e| e.to_string())?;
    if offered <= current {
        return Err("Refusing non-newer package".into());
    }
    let deb = stage.join("candidate.deb");
    io(fs::rename(stage.join("package"), &deb))?;
    let deb_text = deb.to_str().ok_or("Invalid root staging path")?;
    for (field, expected) in [
        ("Package", "studyapp"),
        ("Architecture", "amd64"),
        ("Version", package.version.as_str()),
    ] {
        if output("/usr/bin/dpkg-deb", &["-f", deb_text, field])? != expected {
            return Err(format!("Unexpected deb {field}"));
        }
    }
    wait_old(&tx)?;
    // No sudo fallback and no GUI is launched in this elevated process.
    let status = io(Command::new("/usr/bin/apt-get")
        .args(["install", "-y", "--", deb_text])
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .env("DEBIAN_FRONTEND", "noninteractive")
        .status())?;
    if !status.success() {
        record(
            &stage,
            "failed",
            "apt failed; package manager repair may be required",
        )?;
        return Err("apt installation failed; no rollback performed".into());
    }
    if installed()? != package.version {
        return Err("Installed version mismatch".into());
    }
    record(&stage, "installed", &package.version)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn directory() -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("studyapp-updater-test-{}", uuid::Uuid::new_v4()));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        path
    }
    #[test]
    fn update_install_private_inputs_reject_symlinks_hardlinks_and_oversize() {
        let dir = directory();
        let original = dir.join("original");
        write_new(&original, b"test").unwrap();
        assert_eq!(bounded(&original, uid().unwrap(), 4).unwrap(), b"test");
        assert!(bounded(&original, uid().unwrap(), 3).is_err());
        let link = dir.join("link");
        std::os::unix::fs::symlink(&original, &link).unwrap();
        assert!(bounded(&link, uid().unwrap(), 4).is_err());
        fs::hard_link(&original, dir.join("hard")).unwrap();
        assert!(bounded(&original, uid().unwrap(), 4).is_err());
    }
    #[test]
    fn update_install_world_writable_stage_is_rejected() {
        let dir = directory();
        safe_dir(&dir, uid().unwrap()).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(safe_dir(&dir, uid().unwrap()).is_err());
    }
    #[test]
    fn update_install_pid_reuse_does_not_wait_for_unrelated_process() {
        let pid = std::process::id();
        let start = identity(pid).unwrap().unwrap();
        let tx = InstallTransaction {
            schema: 1,
            id: uuid::Uuid::new_v4().to_string(),
            old_pid: pid,
            old_start: start + 1,
            uid: uid().unwrap(),
            target_version: "0.9.0".into(),
            data_directory: None,
        };
        wait_old(&tx).unwrap();
    }
    #[test]
    fn update_install_stale_readiness_never_authorizes_close() {
        let dir = directory();
        write_new(&dir.join("ready"), b"previous-attempt").unwrap();
        fs::copy("/usr/bin/false", dir.join("supervisor")).unwrap();
        let error = spawn(&dir).unwrap_err();
        assert!(error.contains("fresh transaction"));
    }
    #[test]
    fn update_install_fifo_rejection_is_bounded() {
        let dir = directory();
        let fifo = dir.join("fifo");
        assert!(
            Command::new("/usr/bin/mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        let (sender, receiver) = std::sync::mpsc::channel();
        let owner = uid().unwrap();
        std::thread::spawn(move || {
            let _ = sender.send(bounded(&fifo, owner, 16));
        });
        assert!(
            receiver
                .recv_timeout(Duration::from_secs(2))
                .expect("FIFO open blocked")
                .is_err()
        );
    }
    #[test]
    fn update_install_all_supervisor_failures_are_retained() {
        for message in ["wait timeout", "installed query failed", "relaunch failed"] {
            let dir = directory();
            assert!(persist_failure(&dir, || Err(message.into())).is_err());
            let records: Vec<_> = fs::read_dir(&dir)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect();
            assert_eq!(records.len(), 1);
            let record: serde_json::Value =
                serde_json::from_slice(&fs::read(&records[0]).unwrap()).unwrap();
            assert_eq!(record["state"], "failed");
            assert_eq!(record["message"], message);
        }
    }
    #[test]
    fn update_install_source_binary_cannot_run_privileged_worker() {
        assert!(worker(&directory()).is_err());
    }
}

fn pointer_path() -> Result<PathBuf> {
    directories::ProjectDirs::from("io", "StudyApp", "StudyApp")
        .map(|p| p.data_local_dir().join("update-last.txt"))
        .ok_or("Cannot locate update status".into())
}
pub fn acknowledge_health() -> Result<()> {
    let Some(path) = std::env::var_os("STUDYAPP_UPDATE_TRANSACTION") else {
        return Ok(());
    };
    if io(std::env::current_exe())? != Path::new(EXE) {
        return Err("Health acknowledgement requires installed executable".into());
    }
    let dir = PathBuf::from(path);
    let tx = transaction(&dir, uid()?)?;
    if tx.target_version != env!("CARGO_PKG_VERSION") {
        return Err("Health version mismatch".into());
    }
    write_new(&dir.join("health.json"), &serde_json::to_vec(&serde_json::json!({"id":tx.id,"version":env!("CARGO_PKG_VERSION"),"pid":std::process::id()})).map_err(|e|e.to_string())?)
}
pub fn previous_failure() -> Result<Option<String>> {
    let pointer = pointer_path()?;
    if !pointer.exists() {
        return Ok(None);
    }
    let owner = uid()?;
    let path = String::from_utf8(bounded(&pointer, owner, 4096)?).map_err(|e| e.to_string())?;
    let dir = PathBuf::from(path);
    safe_dir(&dir, owner)?;
    let mut records = Vec::new();
    for entry in io(fs::read_dir(&dir))? {
        let entry = io(entry)?;
        if entry.file_name().to_string_lossy().starts_with("status-") {
            let meta = io(entry.metadata())?;
            records.push((io(meta.modified())?, entry.path()));
        }
    }
    records.sort_by_key(|entry| entry.0);
    if let Some((_, path)) = records.last() {
        let value: serde_json::Value =
            serde_json::from_slice(&bounded(path, owner, 16384)?).map_err(|e| e.to_string())?;
        if value["state"] == "failed" {
            return Ok(Some(format!(
                "上次更新未完成：{}。更新记录：{}",
                value["message"].as_str().unwrap_or("未知错误"),
                dir.display()
            )));
        }
    }
    Ok(None)
}
