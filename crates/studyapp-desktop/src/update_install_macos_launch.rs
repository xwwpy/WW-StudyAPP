//! LaunchServices policy approval and ordinary-user process receipts.
use super::*;
use crate::update_install::launch_receipt::LaunchReceipt;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    nonce: String,
    version: String,
    sha256: String,
    health: Option<PathBuf>,
    created: u64,
    #[serde(default)]
    quarantine: String,
    #[cfg(feature = "update-acceptance")]
    #[serde(default)]
    acceptance_directory: Option<PathBuf>,
}
fn now() -> Result<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|v| v.as_secs())
        .map_err(|e| e.to_string())
}
fn binary_hash(app: &Path) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(read_bounded(
            &app.join("Contents/MacOS/studyapp"),
            512 * 1024 * 1024
        )?)
    ))
}
fn request(dir: &Path) -> Result<Request> {
    safe_dir(dir, uid()?)?;
    serde_json::from_slice(&bounded(&dir.join("request.json"), uid()?, 16384)?)
        .map_err(|e| e.to_string())
}
fn receipt(dir: &Path, request: &Request) -> Result<LaunchReceipt> {
    let receipt: LaunchReceipt =
        serde_json::from_slice(&bounded(&dir.join("receipt.json"), uid()?, 16384)?)
            .map_err(|e| e.to_string())?;
    if !receipt.matches(&request.nonce, &request.version, &request.sha256) {
        return Err("Candidate launch receipt does not match this approval attempt".into());
    }
    Ok(receipt)
}
fn publish(dir: &Path, request: &Request) -> Result<()> {
    if now()?
        .checked_sub(request.created)
        .is_none_or(|age| age > 3600)
    {
        return Err("Approval attempt expired; retry from the update card".into());
    }
    let current = io(std::env::current_exe())?;
    let digest = format!(
        "{:x}",
        Sha256::digest(read_bounded(&current, 512 * 1024 * 1024)?)
    );
    if request.sha256 != digest
        || request.version != env!("CARGO_PKG_VERSION")
        || !valid_id(&request.nonce)
    {
        return Err("Candidate approval identity mismatch".into());
    }
    let value = LaunchReceipt {
        nonce: request.nonce.clone(),
        version: request.version.clone(),
        sha256: digest,
        pid: std::process::id(),
    };
    let temp = dir.join(format!("receipt-{}.tmp", uuid::Uuid::new_v4()));
    write_new(
        &temp,
        &serde_json::to_vec(&value).map_err(|e| e.to_string())?,
    )?;
    // A system retry may open the same harmless probe more than once.
    io(fs::rename(temp, dir.join("receipt.json")))
}
fn probe_pointer() -> Result<PathBuf> {
    Ok(pointer_path()?.with_file_name("update-policy-probe.txt"))
}

/// Runs before any GUI, database, automatic checks or failure dialogs.
pub(crate) fn dispatch_probe(args: &[std::ffi::OsString]) -> Option<i32> {
    let explicit = args.first().is_some_and(|a| a == "--update-policy-probe");
    let directory = if explicit {
        if args.len() != 2 {
            return Some(1);
        }
        PathBuf::from(&args[1])
    } else {
        // The system's Open Anyway action can reopen without the original arguments.
        if !args.is_empty() {
            return None;
        }
        let path = probe_pointer().ok()?;
        let value = bounded(&path, uid().ok()?, 4096).ok()?;
        let dir = PathBuf::from(String::from_utf8(value).ok()?);
        let req = request(&dir).ok()?;
        let current = std::env::current_exe().ok()?;
        let digest = format!(
            "{:x}",
            Sha256::digest(read_bounded(&current, 512 * 1024 * 1024).ok()?)
        );
        if req.sha256 != digest {
            return None;
        }
        if !current.starts_with(&dir) && !current.to_string_lossy().contains("/AppTranslocation/") {
            return None;
        }
        if dir.join("approved").exists() {
            return Some(0);
        }
        dir
    };
    Some(
        match request(&directory).and_then(|req| publish(&directory, &req)) {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("Update policy probe: {error}");
                1
            }
        },
    )
}

fn tree_digest(root: &Path) -> Result<String> {
    fn walk(root: &Path, path: &Path, hash: &mut Sha256) -> Result<()> {
        let mut entries = io(fs::read_dir(path))?
            .collect::<std::io::Result<Vec<_>>>()
            .map_err(|e| e.to_string())?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let relative = text(path.strip_prefix(root).map_err(|e| e.to_string())?)?.as_bytes();
            hash.update((relative.len() as u64).to_le_bytes());
            hash.update(relative);
            let meta = io(fs::symlink_metadata(&path))?;
            if meta.file_type().is_symlink() {
                hash.update(b"link");
                let link = io(fs::read_link(&path))?;
                let value = text(&link)?.as_bytes();
                hash.update((value.len() as u64).to_le_bytes());
                hash.update(value);
            } else if meta.is_dir() {
                hash.update(b"directory");
                walk(root, &path, hash)?;
            } else if meta.is_file() {
                hash.update(b"file");
                hash.update(Sha256::digest(read_bounded(&path, 512 * 1024 * 1024)?));
            } else {
                return Err("Unsupported bundle entry".into());
            }
            hash.update([0]);
        }
        Ok(())
    }
    let mut hash = Sha256::new();
    walk(root, root, &mut hash)?;
    Ok(format!("{:x}", hash.finalize()))
}

fn quarantine_fingerprint(
    package: &VerifiedPackage,
    source: &Path,
    candidate: &Path,
) -> Result<String> {
    fn walk(path: &Path, hash: &mut Sha256) -> Result<()> {
        let value = serde_json::to_vec(&quarantine::read(path)?).map_err(|e| e.to_string())?;
        hash.update((value.len() as u64).to_le_bytes());
        hash.update(value);
        if io(fs::symlink_metadata(path))?.is_dir() {
            let mut entries = io(fs::read_dir(path))?
                .collect::<std::io::Result<Vec<_>>>()
                .map_err(|e| e.to_string())?;
            entries.sort_by_key(|entry| entry.file_name());
            for entry in entries {
                walk(&entry.path(), hash)?;
            }
        }
        Ok(())
    }
    let mut hash = Sha256::new();
    for path in [&package.directory.join("package"), source, candidate] {
        walk(path, &mut hash)?;
        hash.update([0]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn prepare_approval_candidate(package: &VerifiedPackage, source: &Path) -> Result<Request> {
    let dir = package.directory.join("approval");
    let candidate = dir.join(APP);
    if dir.exists() {
        safe_dir(&dir, uid()?)?;
        let reusable = match request(&dir) {
            Ok(previous) => {
                previous.quarantine == quarantine_fingerprint(package, source, &candidate)?
            }
            // A failed copy/attribute/request write can leave an incomplete attempt.
            // Keep it for diagnostics, but reconstruct from the verified source.
            Err(_) => false,
        };
        if !reusable {
            // Keep every old attribute and receipt intact, but never reuse this attempt.
            let archived = package
                .directory
                .join(format!("approval-superseded-{}", uuid::Uuid::new_v4()));
            io(fs::rename(&dir, archived))?;
        }
    }
    if !dir.exists() {
        io(fs::DirBuilder::new().mode(0o700).create(&dir))?;
        output("/usr/bin/ditto", &[text(source)?, text(&candidate)?])?;
        quarantine::preserve_tree(source, &candidate)?;
        quarantine::propagate(&package.directory.join("package"), &candidate)?;
        let req = Request {
            nonce: uuid::Uuid::new_v4().to_string(),
            version: package.version.clone(),
            sha256: binary_hash(&candidate)?,
            health: None,
            created: now()?,
            quarantine: quarantine_fingerprint(package, source, &candidate)?,
            #[cfg(feature = "update-acceptance")]
            acceptance_directory: None,
        };
        write_new(
            &dir.join("request.json"),
            &serde_json::to_vec(&req).map_err(|e| e.to_string())?,
        )?;
    }
    safe_dir(&dir, uid()?)?;
    if tree_digest(source)? != tree_digest(&candidate)? {
        return Err("Approval candidate differs from signed package".into());
    }
    quarantine::preserve_tree(source, &candidate)?;
    let mut req = request(&dir)?;
    if req.version != package.version || req.sha256 != binary_hash(source)? {
        return Err("Approval request identity mismatch".into());
    }
    if now()?.checked_sub(req.created).is_none_or(|age| age > 3600) {
        req.nonce = uuid::Uuid::new_v4().to_string();
        req.created = now()?;
        let temp = dir.join(format!("request-{}.tmp", uuid::Uuid::new_v4()));
        write_new(&temp, &serde_json::to_vec(&req).map_err(|e| e.to_string())?)?;
        io(fs::rename(temp, dir.join("request.json")))?;
    }
    Ok(req)
}

pub(super) fn approve(package: &VerifiedPackage, source: &Path) -> Result<()> {
    let req = prepare_approval_candidate(package, source)?;
    let dir = package.directory.join("approval");
    let candidate = dir.join(APP);
    validate_bundle(&candidate, &package.version)?;
    if receipt(&dir, &req).is_ok()
        && quarantine_fingerprint(package, source, &candidate)? == req.quarantine
    {
        return Ok(());
    }
    let pointer = probe_pointer()?;
    io(fs::create_dir_all(
        pointer.parent().ok_or("Missing probe pointer parent")?,
    ))?;
    io(fs::write(pointer, text(&dir)?))?;
    let assessment = assess(&candidate)
        .err()
        .unwrap_or_else(|| "Assessment accepted; awaiting normal system launch".into());
    write_new(
        &dir.join(format!("assessment-{}.txt", uuid::Uuid::new_v4())),
        assessment.as_bytes(),
    )?;
    // open invokes normal LaunchServices/Gatekeeper; never execute the candidate directly.
    let mut child = io(Command::new("/usr/bin/open")
        .args(["-n", "-g", "-a"])
        .arg(&candidate)
        .arg("--args")
        .arg("--update-policy-probe")
        .arg(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn())?;
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(3) {
        if receipt(&dir, &req).is_ok()
            && quarantine_fingerprint(package, source, &candidate)? == req.quarantine
        {
            return Ok(());
        }
        if io(child.try_wait())?.is_some_and(|status| !status.success()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err("macOS 阻止了新版本启动，当前应用和未保存内容已保留。请确认下载来源后退出应用，手动安装新版，并按系统提示完成首次打开。".into())
}

pub(super) fn finish_approval(package: &VerifiedPackage) -> Result<()> {
    let dir = package.directory.join("approval");
    if !dir.join("approved").exists() {
        write_new(&dir.join("approved"), b"approved")?;
    }
    Ok(())
}

static HEALTH_DIRECTORY: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
pub(super) fn health_directory() -> Option<PathBuf> {
    HEALTH_DIRECTORY
        .get()
        .cloned()
        .or_else(|| std::env::var_os("STUDYAPP_UPDATE_TRANSACTION").map(PathBuf::from))
}
pub(crate) fn early_launch(args: &[std::ffi::OsString]) -> Result<()> {
    let dir = PathBuf::from(args.get(1).ok_or("Missing launch receipt directory")?);
    let req = request(&dir)?;
    if let Some(health) = &req.health {
        if dir.parent() != Some(health.as_path()) {
            return Err("Unexpected launch health directory".into());
        }
        let tx = transaction(health, uid()?)?;
        if tx.transaction.target_version != req.version
            || bundle(&io(std::env::current_exe())?)? != tx.target
        {
            return Err("Launch target identity mismatch".into());
        }
        HEALTH_DIRECTORY
            .set(health.clone())
            .map_err(|_| "Launch context already set")?;
    }
    publish(&dir, &req)?;
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(30) {
        if bounded(&dir.join("continue"), uid()?, 36).ok().as_deref() == Some(req.nonce.as_bytes())
        {
            #[cfg(feature = "update-acceptance")]
            if let Some(path) = req.acceptance_directory {
                crate::update_acceptance::accept_launch_directory(path)?;
            }
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err("Supervisor did not accept this launch; no library was opened".into())
}
#[link(name = "proc")]
unsafe extern "C" {
    fn proc_pidpath(pid: i32, buffer: *mut std::ffi::c_void, size: u32) -> i32;
}
unsafe extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
}
fn process_path(pid: u32) -> Result<PathBuf> {
    let mut bytes = vec![0u8; 4096];
    let size = unsafe { proc_pidpath(pid as i32, bytes.as_mut_ptr().cast(), bytes.len() as u32) };
    if size <= 0 {
        return Err("Candidate process is no longer available".into());
    }
    bytes.truncate(size as usize);
    while bytes.last() == Some(&0) {
        bytes.pop();
    }
    use std::os::unix::ffi::OsStringExt;
    Ok(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}
pub(super) struct Candidate {
    pub pid: u32,
    start: u64,
    executable: PathBuf,
    watch: ExitWatch,
    exited: std::cell::Cell<bool>,
}
impl Candidate {
    pub fn alive(&self) -> Result<bool> {
        if self.exited.get() || self.watch.has_exited()? {
            self.exited.set(true);
            return Ok(false);
        }
        if start_identity(self.pid)? != self.start || process_path(self.pid)? != self.executable {
            return Err(
                "Candidate identity could not be confirmed; refusing recovery while it may run"
                    .into(),
            );
        }
        Ok(true)
    }
    pub fn stop(self) -> Result<()> {
        let observed = self.alive();
        crate::update_install::launch_receipt::confirm_stopped(
            observed,
            || {
                if unsafe { kill(self.pid as i32, 9) } != 0 {
                    return Err(std::io::Error::last_os_error().to_string());
                }
                Ok(())
            },
            || self.watch.wait(),
        )
    }
}
pub(super) fn launch(tx: &MacTransaction, health: Option<&Path>) -> Result<Candidate> {
    if uid()? != tx.transaction.uid || uid()? == 0 {
        return Err("Refusing elevated GUI launch".into());
    }
    let parent = health
        .map(Path::to_owned)
        .unwrap_or_else(std::env::temp_dir);
    let dir = parent.join(format!("studyapp-launch-{}", uuid::Uuid::new_v4()));
    io(fs::DirBuilder::new().mode(0o700).create(&dir))?;
    let req = Request {
        nonce: uuid::Uuid::new_v4().to_string(),
        version: plist(&tx.target, "CFBundleShortVersionString")?,
        sha256: binary_hash(&tx.target)?,
        health: health.map(Path::to_owned),
        created: now()?,
        quarantine: String::new(),
        #[cfg(feature = "update-acceptance")]
        acceptance_directory: tx.acceptance_directory.clone(),
    };
    write_new(
        &dir.join("request.json"),
        &serde_json::to_vec(&req).map_err(|e| e.to_string())?,
    )?;
    let mut command = Command::new("/usr/bin/open");
    command
        .args(["-n", "-g", "-a"])
        .arg(&tx.target)
        .arg("--args")
        .arg("--update-launch")
        .arg(&dir);
    if let Some(data) = &tx.transaction.data_directory {
        command.arg("--data-dir").arg(data);
    }
    let mut opener = io(command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn())?;
    let started = Instant::now();
    loop {
        if let Ok(receipt) = receipt(&dir, &req) {
            let executable = io(fs::canonicalize(tx.target.join("Contents/MacOS/studyapp")))?;
            let start = start_identity(receipt.pid)?;
            let watch = ExitWatch::register(receipt.pid)?;
            let candidate = Candidate {
                pid: receipt.pid,
                start,
                executable,
                watch,
                exited: std::cell::Cell::new(false),
            };
            if !candidate.alive()? {
                return Err("LaunchServices candidate process/path mismatch".into());
            }
            write_new(&dir.join("continue"), req.nonce.as_bytes())?;
            return Ok(candidate);
        }
        if io(opener.try_wait())?.is_some_and(|status| !status.success()) {
            return Err(
                "macOS refused to open installed app; use the normal system approval prompt".into(),
            );
        }
        if started.elapsed() > Duration::from_secs(30) {
            return Err("macOS application launch receipt timed out".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn update_install_macos_incomplete_approval_can_retry_without_deleting_evidence() {
        let root =
            std::env::temp_dir().join(format!("studyapp-incomplete-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let source = root.join("source");
        fs::create_dir(&source).unwrap();
        fs::write(root.join("package"), b"image").unwrap();
        let package = VerifiedPackage {
            directory: root.clone(),
            version: "0.1.4".into(),
            digest: "unchanged".into(),
        };
        assert!(prepare_approval_candidate(&package, &source).is_err());
        assert!(root.join("approval").is_dir());
        assert!(!root.join("approval/request.json").exists());
        fs::create_dir_all(source.join("Contents/MacOS")).unwrap();
        fs::write(
            source.join("Contents/MacOS/studyapp"),
            b"complete executable",
        )
        .unwrap();
        prepare_approval_candidate(&package, &source).unwrap();
        assert!(root.join("approval/request.json").exists());
        assert_eq!(
            fs::read_dir(&root)
                .unwrap()
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("approval-superseded-"))
                .count(),
            1
        );
    }
    #[test]
    fn update_install_macos_refresh_rebuilds_and_retains_superseded_quarantine() {
        let root = std::env::temp_dir().join(format!("studyapp-refresh-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let source = root.join("source");
        fs::create_dir_all(source.join("Contents/MacOS")).unwrap();
        fs::write(source.join("Contents/MacOS/studyapp"), b"signed executable").unwrap();
        fs::write(source.join("Contents/resource"), b"signed resource").unwrap();
        fs::write(root.join("package"), b"signed package").unwrap();
        let package = VerifiedPackage {
            directory: root.clone(),
            version: "0.1.4".into(),
            digest: "unchanged".into(),
        };
        let first = prepare_approval_candidate(&package, &source).unwrap();
        write_new(
            &root.join("approval/receipt.json"),
            &serde_json::to_vec(&LaunchReceipt {
                nonce: first.nonce.clone(),
                version: first.version.clone(),
                sha256: first.sha256.clone(),
                pid: 42,
            })
            .unwrap(),
        )
        .unwrap();
        let provenance = b"0083;1234;Source;existing";
        quarantine::write(&source.join("Contents/resource"), provenance).unwrap();
        let second = prepare_approval_candidate(&package, &source).unwrap();
        assert_ne!(first.nonce, second.nonce);
        assert!(receipt(&root.join("approval"), &second).is_err());
        assert_eq!(
            quarantine::read(&root.join("approval").join(APP).join("Contents/resource"))
                .unwrap()
                .as_deref(),
            Some(provenance.as_slice())
        );
        let changed = b"0081;1234;ChangedCandidate;existing";
        quarantine::write(&root.join("approval").join(APP), changed).unwrap();
        let third = prepare_approval_candidate(&package, &source).unwrap();
        assert_ne!(second.nonce, third.nonce);
        assert_eq!(
            quarantine::read(&root.join("approval").join(APP)).unwrap(),
            None
        );
        let archives: Vec<_> = fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("approval-superseded-")
            })
            .collect();
        assert_eq!(archives.len(), 2);
        assert!(archives.iter().any(
            |e| quarantine::read(&e.path().join(APP)).unwrap().as_deref()
                == Some(changed.as_slice())
        ));
        assert!(
            archives
                .iter()
                .any(|e| e.path().join("receipt.json").exists())
        );
    }
    #[test]
    fn update_install_macos_approval_cache_detects_quarantine_without_byte_changes() {
        let root =
            std::env::temp_dir().join(format!("studyapp-provenance-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let source = root.join("source");
        let candidate = root.join("candidate");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&candidate).unwrap();
        fs::write(root.join("package"), b"signed image bytes").unwrap();
        fs::write(source.join("resource"), b"same").unwrap();
        fs::write(candidate.join("resource"), b"same").unwrap();
        let package = VerifiedPackage {
            directory: root.clone(),
            version: "0.1.4".into(),
            digest: "unchanged".into(),
        };
        let before = quarantine_fingerprint(&package, &source, &candidate).unwrap();
        quarantine::write(&root.join("package"), b"0083;1234;Browser;existing").unwrap();
        let marked = quarantine_fingerprint(&package, &source, &candidate).unwrap();
        assert_ne!(before, marked);
        assert_eq!(
            fs::read(root.join("package")).unwrap(),
            b"signed image bytes"
        );
        quarantine::write(&source.join("resource"), b"0083;1234;Source;existing").unwrap();
        assert_ne!(
            marked,
            quarantine_fingerprint(&package, &source, &candidate).unwrap()
        );
        assert_eq!(
            tree_digest(&source).unwrap(),
            tree_digest(&candidate).unwrap()
        );
        quarantine::preserve_tree(&source, &candidate).unwrap();
        assert_eq!(
            quarantine::read(&source.join("resource")).unwrap(),
            quarantine::read(&candidate.join("resource")).unwrap()
        );
        quarantine::write(
            &source.join("resource"),
            b"0083;1234;ChangedSource;existing",
        )
        .unwrap();
        assert!(quarantine::preserve_tree(&source, &candidate).is_err());
        assert_eq!(
            quarantine::read(&candidate.join("resource"))
                .unwrap()
                .as_deref(),
            Some(b"0083;1234;Source;existing".as_slice())
        );
    }
    #[test]
    fn update_install_macos_approval_tree_detects_resource_changes() {
        let root = std::env::temp_dir().join(format!("studyapp-tree-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("resource"), b"original").unwrap();
        let before = tree_digest(&root).unwrap();
        fs::write(root.join("resource"), b"changed").unwrap();
        assert_ne!(tree_digest(&root).unwrap(), before);
    }
}
