//! Installed macOS bundle updates. Keep backups and preserve extended attributes.
use super::*;
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
type Result<T> = std::result::Result<T, String>;
const APP: &str = "WW-StudyApp.app";
const BUNDLE_ID: &str = "io.github.xwwpy.studyapp";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MacTransaction {
    transaction: InstallTransaction,
    target: PathBuf,
    elevated: bool,
}
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
        .custom_flags(0x100 | 0x4)
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

fn bundle(exe: &Path) -> Result<PathBuf> {
    if exe.file_name().and_then(|s| s.to_str()) != Some("studyapp")
        || exe
            .parent()
            .and_then(Path::file_name)
            .and_then(|s| s.to_str())
            != Some("MacOS")
    {
        return Err("Automatic updates require an installed app bundle".into());
    }
    let app = exe
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or("Invalid bundle path")?;
    if app.file_name().and_then(|s| s.to_str()) != Some(APP) {
        return Err("Unsupported app bundle name".into());
    }
    let text = app.to_string_lossy();
    if text.starts_with("/Volumes/") || text.contains("/AppTranslocation/") {
        return Err("Move the app from the disk image into Applications before updating".into());
    }
    safe_ancestors(app)?;
    Ok(app.to_owned())
}
fn safe_ancestors(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        return Err("Absolute bundle path required".into());
    }
    for ancestor in path.ancestors() {
        if io(fs::symlink_metadata(ancestor))?.file_type().is_symlink() {
            return Err("Symlinked bundle paths are not supported".into());
        }
    }
    Ok(())
}
fn text(path: &Path) -> Result<&str> {
    path.to_str().ok_or("Non-UTF8 path is unsupported".into())
}
fn plist(bundle: &Path, key: &str) -> Result<String> {
    output(
        "/usr/bin/plutil",
        &[
            "-extract",
            key,
            "raw",
            "-o",
            "-",
            text(&bundle.join("Contents/Info.plist"))?,
        ],
    )
}
fn validate_bundle(app: &Path, version: &str) -> Result<()> {
    safe_ancestors(app)?;
    if plist(app, "CFBundleIdentifier")? != BUNDLE_ID
        || plist(app, "CFBundleExecutable")? != "studyapp"
        || plist(app, "CFBundleShortVersionString")? != version
        || plist(app, "CFBundleVersion")? != version
    {
        return Err("Unexpected bundle identity or version".into());
    }
    let executable = app.join("Contents/MacOS/studyapp");
    let architecture = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x86_64"
    };
    if output("/usr/bin/lipo", &["-archs", text(&executable)?])? != architecture {
        return Err("Unexpected bundle architecture".into());
    }
    output(
        "/usr/bin/codesign",
        &["--verify", "--deep", "--strict", text(app)?],
    )?;
    Ok(())
}
fn protected_bundle(app: &Path) -> Result<()> {
    safe_ancestors(app)?;
    for ancestor in app.ancestors().skip(1) {
        let meta = io(fs::symlink_metadata(ancestor))?;
        if meta.uid() != 0 || meta.mode() & 0o022 != 0 {
            return Err(format!(
                "Authorization path is replaceable at {}; move to a writable per-user Applications folder",
                ancestor.display()
            ));
        }
    }
    let mut stack = vec![app.to_owned()];
    while let Some(path) = stack.pop() {
        let meta = io(fs::symlink_metadata(&path))?;
        if meta.uid() != 0 || meta.mode() & 0o022 != 0 {
            return Err("Authorization requires a root-protected installed app; use a writable Applications folder".into());
        }
        if meta.is_dir() {
            for child in io(fs::read_dir(path))? {
                stack.push(io(child)?.path());
            }
        }
    }
    Ok(())
}
fn start_identity(pid: u32) -> Result<u64> {
    let value = output("/bin/ps", &["-p", &pid.to_string(), "-o", "lstart="])?;
    let hash = Sha256::digest(value.as_bytes());
    Ok(u64::from_le_bytes(
        hash[..8]
            .try_into()
            .map_err(|_| "Invalid process identity")?,
    ))
}
fn transaction(dir: &Path, owner: u32) -> Result<MacTransaction> {
    safe_dir(dir, owner)?;
    let tx: MacTransaction =
        serde_json::from_slice(&bounded(&dir.join("transaction.json"), owner, 16384)?)
            .map_err(|e| e.to_string())?;
    if tx.transaction.uid != owner
        || owner == 0
        || tx.transaction.schema != 1
        || !valid_id(&tx.transaction.id)
        || tx.transaction.old_pid < 2
    {
        return Err("Invalid update transaction".into());
    }
    let current = tx.target.join("Contents/MacOS/studyapp");
    if bundle(&current)? != tx.target {
        return Err("Invalid installed target".into());
    }
    Ok(tx)
}
fn pointer_path() -> Result<PathBuf> {
    directories::ProjectDirs::from("io", "StudyApp", "StudyApp")
        .map(|p| p.data_local_dir().join("update-last.txt"))
        .ok_or("No update record location".into())
}
pub fn prepare(package: &VerifiedPackage, data: Option<PathBuf>) -> Result<PathBuf> {
    package.reverify()?;
    let executable = io(std::env::current_exe())?;
    let target = bundle(&executable)?;
    validate_bundle(&target, env!("CARGO_PKG_VERSION"))?;
    let parent = target.parent().ok_or("Missing bundle parent")?;
    let probe = parent.join(format!(".studyapp-write-probe-{}", uuid::Uuid::new_v4()));
    let elevated = match OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(file) => {
            drop(file);
            io(fs::remove_file(&probe))?;
            false
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => true,
        Err(e) => return Err(e.to_string()),
    };
    if elevated {
        protected_bundle(&target)?;
    }
    let owner = uid()?;
    let dir = package
        .directory
        .with_file_name(format!("studyapp-install-{}", uuid::Uuid::new_v4()));
    io(fs::DirBuilder::new().mode(0o700).create(&dir))?;
    write_new(
        &dir.join("manifest.json"),
        &bounded(
            &package.directory.join("manifest.json"),
            owner,
            2 * 1024 * 1024,
        )?,
    )?;
    // ditto preserves any quarantine and other extended attributes on the downloaded image.
    output(
        "/usr/bin/ditto",
        &[
            text(&package.directory.join("package"))?,
            text(&dir.join("package"))?,
        ],
    )?;
    let staged = VerifiedPackage::verify(&dir)?;
    if staged.digest != package.digest || staged.version != package.version {
        return Err("Approved package changed".into());
    }
    // Assess the downloaded distribution before the current application closes.
    with_image(&dir, |point| {
        let candidate = point.join(APP);
        validate_bundle(&candidate, &package.version)?;
        assess(&candidate)
    })?;
    let tx = MacTransaction {
        transaction: InstallTransaction {
            schema: 1,
            id: uuid::Uuid::new_v4().to_string(),
            old_pid: std::process::id(),
            old_start: start_identity(std::process::id())?,
            uid: owner,
            target_version: package.version.clone(),
            data_directory: data,
        },
        target,
        elevated,
    };
    write_new(
        &dir.join("transaction.json"),
        &serde_json::to_vec(&tx).map_err(|e| e.to_string())?,
    )?;
    io(fs::copy(executable, dir.join("supervisor")))?;
    io(fs::set_permissions(
        dir.join("supervisor"),
        fs::Permissions::from_mode(0o700),
    ))?;
    let pointer = pointer_path()?;
    io(fs::create_dir_all(
        pointer.parent().ok_or("Missing pointer parent")?,
    ))?;
    io(fs::write(pointer, text(&dir)?))?;
    Ok(dir)
}
pub fn spawn(dir: &Path) -> Result<()> {
    let tx = transaction(dir, uid()?)?;
    if fs::symlink_metadata(dir.join("ready")).is_ok() {
        return Err("Prepare a fresh update attempt before retrying".into());
    }
    write_new(&dir.join("launch-claimed"), tx.transaction.id.as_bytes())?;
    let mut child = io(Command::new(dir.join("supervisor"))
        .arg("--update-supervisor")
        .arg(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn())?;
    let start = Instant::now();
    loop {
        if io(child.try_wait())?.is_some() {
            return Err("Update supervisor exited before readiness".into());
        }
        if let Ok(value) = bounded(&dir.join("ready"), tx.transaction.uid, 36) {
            if value == tx.transaction.id.as_bytes() {
                return Ok(());
            }
            let _ = child.kill();
            return Err("Readiness identity mismatch".into());
        }
        if start.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            return Err("Update supervisor readiness timeout".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[repr(C)]
struct Kevent {
    ident: usize,
    filter: i16,
    flags: u16,
    fflags: u32,
    data: isize,
    udata: *mut std::ffi::c_void,
}
#[repr(C)]
struct Timespec {
    seconds: i64,
    nanoseconds: i64,
}
unsafe extern "C" {
    fn kqueue() -> i32;
    fn kevent(
        fd: i32,
        changes: *const Kevent,
        nchanges: i32,
        events: *mut Kevent,
        nevents: i32,
        timeout: *const Timespec,
    ) -> i32;
    fn renamex_np(from: *const std::ffi::c_char, to: *const std::ffi::c_char, flags: u32) -> i32;
}
struct ExitWatch(std::os::fd::OwnedFd);
impl ExitWatch {
    fn register(pid: u32) -> Result<Self> {
        use std::os::fd::{AsRawFd, FromRawFd};
        let fd = unsafe { kqueue() };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let owned = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
        let event = Kevent {
            ident: pid as usize,
            filter: -5,
            flags: 0x1 | 0x4 | 0x10,
            fflags: 0x80000000,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        if unsafe {
            kevent(
                owned.as_raw_fd(),
                &event,
                1,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        } < 0
        {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(Self(owned))
    }
    fn wait(self) -> Result<()> {
        use std::os::fd::AsRawFd;
        let mut event = Kevent {
            ident: 0,
            filter: 0,
            flags: 0,
            fflags: 0,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        let timeout = Timespec {
            seconds: 120,
            nanoseconds: 0,
        };
        let count = unsafe {
            kevent(
                self.0.as_raw_fd(),
                std::ptr::null(),
                0,
                &mut event,
                1,
                &timeout,
            )
        };
        if count != 1 || event.flags & 0x4000 != 0 || event.fflags & 0x80000000 == 0 {
            return Err("Application exit observation failed or timed out".into());
        }
        Ok(())
    }
}
fn rename_exclusive(from: &Path, to: &Path) -> Result<()> {
    let from = std::ffi::CString::new(text(from)?).map_err(|e| e.to_string())?;
    let to = std::ffi::CString::new(text(to)?).map_err(|e| e.to_string())?;
    if unsafe { renamex_np(from.as_ptr(), to.as_ptr(), 4) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}
fn backup(tx: &MacTransaction) -> Result<PathBuf> {
    Ok(tx
        .target
        .parent()
        .ok_or("Missing bundle parent")?
        .join(format!(".WW-StudyApp-backup-{}.app", tx.transaction.id)))
}
fn replace_bundle(target: &Path, stage: &Path, old: &Path) -> Result<()> {
    rename_exclusive(target, old)?;
    if let Err(error) = rename_exclusive(stage, target) {
        rename_exclusive(old, target)
            .map_err(|restore| format!("{error}; restoration failed: {restore}"))?;
        return Err(error);
    }
    Ok(())
}
fn assess(app: &Path) -> Result<()> {
    output("/usr/sbin/spctl", &["--assess","--type","execute","--verbose=4",text(app)?]).map_err(|error|format!("macOS distribution assessment rejected the update; the current app remains available. A system-approved distribution is required: {error}"))?;
    Ok(())
}
struct Mount {
    detach: String,
    armed: bool,
}
impl Mount {
    fn finish(self, dir: &Path) -> Result<()> {
        self.finish_with(dir, |device| {
            output("/usr/bin/hdiutil", &["detach", device]).map(|_| ())
        })
    }
    fn finish_with(mut self, dir: &Path, execute: impl FnOnce(&str) -> Result<()>) -> Result<()> {
        self.armed = false;
        if let Err(error) = execute(&self.detach) {
            let message = format!(
                "Disk image {} remains attached; eject this device in Disk Utility: {error}",
                self.detach
            );
            record(dir, "cleanup_failed", &message)?;
            return Err(message);
        }
        Ok(())
    }
}
impl Drop for Mount {
    fn drop(&mut self) {
        if self.armed {
            let _ = Command::new("/usr/bin/hdiutil")
                .args(["detach", &self.detach])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}
fn with_image(dir: &Path, action: impl FnOnce(&Path) -> Result<()>) -> Result<()> {
    with_image_mode(dir, false, action)
}
fn with_image_mode(
    dir: &Path,
    installed_on_success: bool,
    action: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    let (mount, point) = mount(dir)?;
    let result = action(&point);
    let cleanup = mount.finish(dir);
    if installed_on_success {
        return finish_install_cleanup(dir, result, cleanup);
    }
    match (result, cleanup) {
        (Err(error), Err(cleanup)) => Err(format!("{error}; {cleanup}")),
        (Err(error), _) => Err(error),
        (Ok(()), cleanup) => cleanup,
    }
}
fn finish_install_cleanup(dir: &Path, installation: Result<()>, cleanup: Result<()>) -> Result<()> {
    match (installation, cleanup) {
        (Ok(()), Err(warning)) => {
            record(dir, "cleanup_warning", &warning)?;
            Ok(())
        }
        (Err(error), Err(cleanup)) => Err(format!("{error}; {cleanup}")),
        (Err(error), _) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}
fn mount(dir: &Path) -> Result<(Mount, PathBuf)> {
    let point = dir.join(format!("mount-{}", uuid::Uuid::new_v4()));
    io(fs::DirBuilder::new().mode(0o700).create(&point))?;
    let out = io(Command::new("/usr/bin/hdiutil")
        .args(["attach", "-readonly", "-nobrowse", "-plist", "-mountpoint"])
        .arg(&point)
        .arg(dir.join("package"))
        .output())?;
    if !out.status.success() {
        return Err("Cannot mount update image readonly".into());
    }
    let mut guard = Mount {
        detach: text(&point)?.to_owned(),
        armed: true,
    };
    let parsed_device: Result<String> = (|| {
        let mut child = io(Command::new("/usr/bin/plutil")
            .args(["-convert", "json", "-o", "-", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn())?;
        io(child
            .stdin
            .take()
            .ok_or("Cannot parse mount metadata")?
            .write_all(&out.stdout))?;
        let parsed = io(child.wait_with_output())?;
        if !parsed.status.success() {
            return Err("Invalid disk image mount metadata".into());
        }
        let value: serde_json::Value =
            serde_json::from_slice(&parsed.stdout).map_err(|e| e.to_string())?;
        let entries = value["system-entities"]
            .as_array()
            .ok_or("Missing mount entities")?;
        let entry = entries
            .iter()
            .find(|entry| entry["mount-point"].as_str() == point.to_str())
            .ok_or("Unexpected mount destination")?;
        let device = entry["dev-entry"].as_str().ok_or("Missing image device")?;
        if !device.starts_with("/dev/disk")
            || !device[9..].chars().all(|c| c.is_ascii_alphanumeric())
        {
            return Err("Unexpected mount device".into());
        }
        Ok(device.to_owned())
    })();
    match parsed_device {
        Ok(device) => guard.detach = device,
        Err(error) => {
            return match guard.finish(dir) {
                Ok(()) => Err(error),
                Err(cleanup) => Err(format!("{error}; {cleanup}")),
            };
        }
    }
    Ok((guard, point))
}
fn install(dir: &Path, tx: &MacTransaction) -> Result<()> {
    let package = VerifiedPackage::verify(dir)?;
    if package.version != tx.transaction.target_version {
        return Err("Approved target changed".into());
    }
    let current = plist(&tx.target, "CFBundleShortVersionString")?;
    validate_bundle(&tx.target, &current)?;
    if semver::Version::parse(&package.version).map_err(|e| e.to_string())?
        <= semver::Version::parse(&current).map_err(|e| e.to_string())?
    {
        return Err("Refusing non-newer package".into());
    }
    if start_identity(tx.transaction.old_pid).ok() == Some(tx.transaction.old_start) {
        return Err("Original application is still running".into());
    }
    with_image_mode(dir, true, |point| {
        let apps: Vec<_> = io(fs::read_dir(&point))?
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".app"))
            .collect();
        if apps.len() != 1 || apps[0].file_name() != APP {
            return Err("Unexpected app inventory in disk image".into());
        }
        let source = point.join(APP);
        validate_bundle(&source, &package.version)?;
        let stage = tx
            .target
            .parent()
            .ok_or("Missing parent")?
            .join(format!(".WW-StudyApp-stage-{}.app", tx.transaction.id));
        if fs::symlink_metadata(&stage).is_ok() || fs::symlink_metadata(backup(tx)?).is_ok() {
            return Err("Update destination already exists".into());
        }
        output(
            "/usr/bin/ditto",
            &[
                "--rsrc",
                "--extattr",
                "--acl",
                text(&source)?,
                text(&stage)?,
            ],
        )?;
        if let Ok(quarantine) = output(
            "/usr/bin/xattr",
            &["-p", "com.apple.quarantine", text(&dir.join("package"))?],
        ) {
            output(
                "/usr/bin/xattr",
                &["-w", "com.apple.quarantine", &quarantine, text(&stage)?],
            )?;
        }
        validate_bundle(&stage, &package.version)?;
        assess(&stage)?;
        replace_bundle(&tx.target, &stage, &backup(tx)?)
    })
}
fn restore(tx: &MacTransaction) -> Result<()> {
    let old = backup(tx)?;
    let version = plist(&old, "CFBundleShortVersionString")?;
    validate_bundle(&old, &version)?;
    let failed = tx
        .target
        .parent()
        .ok_or("Missing parent")?
        .join(format!(".WW-StudyApp-failed-{}.app", uuid::Uuid::new_v4()));
    replace_bundle(&tx.target, &old, &failed)
}
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace(char::from(39), r"'\''"))
}
fn apple_quote(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace(char::from(92), r"\\")
            .replace(char::from(34), r#"\""#)
            .replace('\n', r"\n")
            .replace('\r', r"\r")
    )
}
fn authorize(dir: &Path, tx: &MacTransaction, recover: bool) -> Result<()> {
    let app = if recover {
        backup(tx)?
    } else {
        tx.target.clone()
    };
    protected_bundle(&app)?;
    let exe = app.join("Contents/MacOS/studyapp");
    let mode = if recover {
        "--update-restore"
    } else {
        "--update-worker"
    };
    let command = format!(
        "{} {} {}",
        shell_quote(text(&exe)?),
        mode,
        shell_quote(text(dir)?)
    );
    output(
        "/usr/bin/osascript",
        &[
            "-e",
            &format!(
                "do shell script {} with administrator privileges",
                apple_quote(&command)
            ),
        ],
    )?;
    Ok(())
}
pub fn worker(dir: &Path, recover: bool) -> Result<()> {
    if uid()? != 0 {
        return Err("Worker requires system authorization".into());
    }
    let exe = io(std::env::current_exe())?;
    let app = exe
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or("Invalid worker bundle")?;
    protected_bundle(app)?;
    let owner = io(fs::symlink_metadata(dir))?.uid();
    let tx = transaction(dir, owner)?;
    let derived = app.parent().ok_or("Missing installed parent")?.join(APP);
    if derived != tx.target {
        return Err("Worker destination does not match installed bundle".into());
    }
    if recover {
        if app != backup(&tx)? {
            return Err("Recovery requires protected backup worker".into());
        }
        return restore(&tx);
    }
    if app != tx.target {
        return Err("Installation requires current protected worker".into());
    }
    let root =
        Path::new("/private/var/tmp").join(format!("studyapp-root-{}", uuid::Uuid::new_v4()));
    io(fs::DirBuilder::new().mode(0o700).create(&root))?;
    write_new(
        &root.join("manifest.json"),
        &bounded(&dir.join("manifest.json"), owner, 2 * 1024 * 1024)?,
    )?;
    // Snapshot bytes before verification. Preserve quarantine separately, never remove it.
    write_new(
        &root.join("package"),
        &bounded(&dir.join("package"), owner, 256 * 1024 * 1024)?,
    )?;
    if let Ok(q) = output(
        "/usr/bin/xattr",
        &["-p", "com.apple.quarantine", text(&dir.join("package"))?],
    ) {
        output(
            "/usr/bin/xattr",
            &[
                "-w",
                "com.apple.quarantine",
                &q,
                text(&root.join("package"))?,
            ],
        )?;
    }
    install(&root, &tx)
}
fn launch(tx: &MacTransaction, health: Option<&Path>) -> Result<std::process::Child> {
    if uid()? != tx.transaction.uid || uid()? == 0 {
        return Err("Refusing elevated GUI launch".into());
    }
    let mut command = Command::new(tx.target.join("Contents/MacOS/studyapp"));
    command.env_remove("STUDYAPP_UPDATE_TRANSACTION");
    if let Some(dir) = health {
        command.env("STUDYAPP_UPDATE_TRANSACTION", dir);
    }
    if let Some(data) = &tx.transaction.data_directory {
        command.arg("--data-dir").arg(data);
    }
    io(command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn())
}
pub fn supervise(dir: &Path) -> Result<()> {
    safe_dir(dir, uid()?)?;
    let result = supervise_inner(dir);
    if let Err(error) = &result {
        record(dir, "failed", error)?;
    }
    result
}
fn supervise_inner(dir: &Path) -> Result<()> {
    let tx = transaction(dir, uid()?)?;
    let package = VerifiedPackage::verify(dir)?;
    if package.version != tx.transaction.target_version {
        return Err("Approved package changed".into());
    }
    let watch = ExitWatch::register(tx.transaction.old_pid)?;
    write_new(&dir.join("ready"), tx.transaction.id.as_bytes())?;
    record(dir, "waiting", "Waiting for application exit")?;
    watch.wait()?;
    record(dir, "installing", "Installing verified bundle")?;
    let result = if tx.elevated {
        authorize(dir, &tx, false)
    } else {
        install(dir, &tx)
    };
    if let Err(error) = result {
        if plist(&tx.target, "CFBundleShortVersionString")
            .ok()
            .is_some_and(|v| v != tx.transaction.target_version)
        {
            let _ = launch(&tx, None);
        }
        return Err(error);
    }
    record(dir, "installed", &package.version)?;
    let health = wait_health(dir, &tx);
    if let Err(error) = health {
        record(dir, "recovering", &error)?;
        if tx.elevated {
            authorize(dir, &tx, true)?;
        } else {
            restore(&tx)?;
        }
        launch(&tx, None)?;
        record(
            dir,
            "recovered",
            "Previous app restored; failed candidate retained",
        )?;
        return Err(format!("{error}; previous app restored"));
    }
    record(dir, "healthy", "Updated app and library startup confirmed")
}
fn wait_health(dir: &Path, tx: &MacTransaction) -> Result<()> {
    let mut child = launch(tx, Some(dir))?;
    let start = Instant::now();
    let result = (|| loop {
        if io(child.try_wait())?.is_some() {
            return Err("Updated application exited before startup confirmation".into());
        }
        if let Ok(bytes) = bounded(&dir.join("health.json"), tx.transaction.uid, 4096) {
            let value: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            if health_matches(
                &value,
                &tx.transaction.id,
                &tx.transaction.target_version,
                child.id(),
            ) {
                return Ok(());
            }
            return Err("Health identity mismatch".into());
        }
        if start.elapsed() > Duration::from_secs(60) {
            return Err("Updated application startup timeout".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    })();
    // Never restore/relaunch the previous app while the failed candidate remains alive.
    if result.is_err() && io(child.try_wait())?.is_none() {
        io(child.kill())?;
        io(child.wait())?;
    }
    result
}
pub fn acknowledge_health() -> Result<()> {
    let Some(dir) = std::env::var_os("STUDYAPP_UPDATE_TRANSACTION") else {
        return Ok(());
    };
    let dir = PathBuf::from(dir);
    let tx = transaction(&dir, uid()?)?;
    if bundle(&io(std::env::current_exe())?)? != tx.target
        || tx.transaction.target_version != env!("CARGO_PKG_VERSION")
    {
        return Err("Health target mismatch".into());
    }
    let temp = dir.join(format!("health-{}.tmp", uuid::Uuid::new_v4()));
    write_new(&temp,&serde_json::to_vec(&serde_json::json!({"id":tx.transaction.id,"version":env!("CARGO_PKG_VERSION"),"pid":std::process::id()})).map_err(|e|e.to_string())?)?;
    rename_exclusive(&temp, &dir.join("health.json"))
}
pub fn previous_failure() -> Result<Option<String>> {
    let pointer = pointer_path()?;
    if !pointer.exists() {
        return Ok(None);
    }
    let owner = uid()?;
    let dir = PathBuf::from(
        String::from_utf8(bounded(&pointer, owner, 4096)?).map_err(|e| e.to_string())?,
    );
    safe_dir(&dir, owner)?;
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
            serde_json::from_slice(&bounded(path, owner, 16384)?).map_err(|e| e.to_string())?;
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
    fn dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!("studyapp-mac-test-{}", uuid::Uuid::new_v4()));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        fs::canonicalize(path).unwrap()
    }
    #[test]
    fn update_install_macos_writable_ancestor_blocks_authorization_path() {
        if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true") {
            return;
        }
        let root = dir();
        let fixed = root.join("root-owned");
        let app = fixed.join(APP);
        fs::create_dir(&fixed).unwrap();
        fs::create_dir(&app).unwrap();
        fs::set_permissions(&fixed, fs::Permissions::from_mode(0o555)).unwrap();
        fs::set_permissions(&app, fs::Permissions::from_mode(0o555)).unwrap();
        assert!(
            Command::new("/usr/bin/sudo")
                .args(["-n", "/usr/sbin/chown", "root:wheel"])
                .arg(&fixed)
                .arg(&app)
                .status()
                .unwrap()
                .success()
        );
        assert!(protected_bundle(&app).unwrap_err().contains("replaceable"));
        // The caller can actually replace the path above both root-owned directories.
        let moved = root.with_file_name(format!("moved-mac-ancestor-{}", uuid::Uuid::new_v4()));
        fs::rename(&root, &moved).unwrap();
        fs::create_dir(&root).unwrap();
        fs::create_dir(&fixed).unwrap();
        assert!(fs::metadata(&fixed).unwrap().uid() != 0);
    }
    #[test]
    fn update_install_macos_successful_replacement_continues_despite_detach_warning() {
        let root = dir();
        assert!(
            finish_install_cleanup(&root, Ok(()), Err("/dev/disk987 device busy".into())).is_ok()
        );
        let record = fs::read_dir(root).unwrap().next().unwrap().unwrap().path();
        assert!(
            fs::read_to_string(record)
                .unwrap()
                .contains("cleanup_warning")
        );
        assert!(
            finish_install_cleanup(&dir(), Err("rename failed".into()), Err("busy".into()))
                .is_err()
        );
    }
    #[test]
    fn update_install_macos_detach_failure_records_device_and_recovery() {
        let root = dir();
        let mount = Mount {
            detach: "/dev/disk987".into(),
            armed: true,
        };
        let error = mount
            .finish_with(&root, |_| Err("device busy".into()))
            .unwrap_err();
        assert!(error.contains("/dev/disk987") && error.contains("Disk Utility"));
        let record = fs::read_dir(root).unwrap().next().unwrap().unwrap().path();
        assert!(
            fs::read_to_string(record)
                .unwrap()
                .contains("cleanup_failed")
        );
    }
    #[test]
    fn update_install_macos_quoted_paths_remain_single_argument() {
        let value = "/Applications/中文 app's $HOME `literal`";
        let command = format!("printf %s {}", shell_quote(value));
        let out = Command::new("/bin/sh")
            .args(["-c", &command])
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(String::from_utf8(out.stdout).unwrap(), value);
        let script = format!("return {}", apple_quote("a\"b\\c\n中文"));
        let out = Command::new("/usr/bin/osascript")
            .args(["-e", &script])
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(
            String::from_utf8(out.stdout).unwrap().trim_end(),
            "a\"b\\c\n中文"
        );
    }
    #[test]
    fn update_install_macos_failed_second_rename_restores_original() {
        let root = dir();
        let target = root.join("original.app");
        let backup = root.join("backup.app");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("identity"), "old").unwrap();
        assert!(replace_bundle(&target, &root.join("missing-stage.app"), &backup).is_err());
        assert_eq!(fs::read_to_string(target.join("identity")).unwrap(), "old");
        assert!(!backup.exists());
    }
    #[test]
    fn update_install_macos_exclusive_rename_preserves_existing_destination() {
        let root = dir();
        let old = root.join("old");
        let dest = root.join("dest");
        fs::write(&old, "old").unwrap();
        fs::write(&dest, "keep").unwrap();
        assert!(rename_exclusive(&old, &dest).is_err());
        assert_eq!(fs::read_to_string(dest).unwrap(), "keep");
        assert!(old.exists());
    }
    #[test]
    fn update_install_macos_kqueue_observes_registered_child_exit() {
        let mut child = Command::new("/bin/sleep").arg("1").spawn().unwrap();
        let watcher = ExitWatch::register(child.id()).unwrap();
        watcher.wait().unwrap();
        assert!(child.wait().unwrap().success());
    }
    #[test]
    fn update_install_macos_rejects_source_and_symlink_bundle_locations() {
        assert!(bundle(Path::new("/tmp/studyapp")).is_err());
        let root = dir();
        let real = root.join("real");
        fs::create_dir(&real).unwrap();
        let alias = root.join("alias");
        std::os::unix::fs::symlink(real, &alias).unwrap();
        assert!(safe_ancestors(&alias).is_err());
    }
}
