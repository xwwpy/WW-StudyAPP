//! Durable package-install transactions; helper entry runs before GUI startup.
#![allow(dead_code)]
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallTransaction {
    pub schema: u32,
    pub id: String,
    pub old_pid: u32,
    pub old_start: u64,
    pub uid: u32,
    pub target_version: String,
    pub data_directory: Option<PathBuf>,
}

/// Cannot be constructed without validating the signed envelope and exact bytes.
#[derive(Clone, Debug)]
pub struct VerifiedPackage {
    pub(crate) directory: PathBuf,
    pub(crate) version: String,
    digest: String,
}
impl VerifiedPackage {
    pub(crate) fn identity(&self) -> String {
        format!("{}:{}", self.version, self.digest)
    }
    pub fn download(
        envelope: &str,
        base: &Path,
        progress: impl FnMut(u64, u64),
    ) -> Result<Self, String> {
        let manifest = crate::update_manifest::parse_latest_json(envelope)?;
        let directory = base.join(format!("studyapp-update-{}", uuid::Uuid::new_v4()));
        #[allow(unused_mut)]
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&directory).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        let directory = std::fs::canonicalize(directory).map_err(|e| e.to_string())?;
        std::fs::write(directory.join("manifest.json"), envelope).map_err(|e| e.to_string())?;
        crate::update_backend::download_setup_to(
            &manifest.setup_url,
            &directory.join(package_file()),
            manifest.size,
            progress,
        )
        .map_err(|e| e.to_string())?;
        Self::verify(&directory)
    }
    pub fn reverify(&self) -> Result<(), String> {
        let current = Self::verify(&self.directory)?;
        if current.version != self.version || current.digest != self.digest {
            return Err("Approved update package changed".into());
        }
        Ok(())
    }
    pub fn verify(directory: &Path) -> Result<Self, String> {
        let envelope = String::from_utf8(read_bounded(
            &directory.join("manifest.json"),
            2 * 1024 * 1024,
        )?)
        .map_err(|e| e.to_string())?;
        let manifest = crate::update_manifest::parse_latest_json(&envelope)?;
        let bytes = read_bounded(&directory.join(package_file()), manifest.size)?;
        if bytes.len() as u64 != manifest.size
            || format!("{:x}", Sha256::digest(&bytes)) != manifest.sha256
        {
            return Err("Package size or digest mismatch".into());
        }
        crate::update_signature::verify_manifest_signature(&bytes, &manifest.signature)?;
        Ok(Self {
            directory: directory.to_owned(),
            version: manifest.version.to_string(),
            digest: manifest.sha256,
        })
    }
}

fn package_file() -> &'static str {
    if cfg!(windows) {
        "package.exe"
    } else {
        "package"
    }
}

#[derive(Clone, Debug)]
pub enum PreparedPackage {
    Windows(PathBuf),
    Linux(PathBuf),
    Macos(PathBuf),
}
pub fn prepare(
    package: &VerifiedPackage,
    data: Option<PathBuf>,
) -> Result<PreparedPackage, String> {
    package.reverify()?;
    #[cfg(windows)]
    {
        windows::prepare(package, data).map(PreparedPackage::Windows)
    }
    #[cfg(target_os = "linux")]
    {
        linux::prepare(package, data).map(PreparedPackage::Linux)
    }
    #[cfg(target_os = "macos")]
    {
        macos::prepare(package, data).map(PreparedPackage::Macos)
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = data;
        Err("Automatic installation is not available for this platform yet".into())
    }
}
pub fn launch(package: &PreparedPackage) -> Result<(), String> {
    match package {
        PreparedPackage::Windows(directory) => {
            #[cfg(windows)]
            {
                windows::spawn(directory)
            }
            #[cfg(not(windows))]
            {
                let _ = directory;
                Err("Windows helper unsupported".into())
            }
        }
        PreparedPackage::Macos(directory) => {
            #[cfg(target_os = "macos")]
            {
                macos::spawn(directory)
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = directory;
                Err("macOS helper unsupported".into())
            }
        }
        PreparedPackage::Linux(directory) => {
            #[cfg(target_os = "linux")]
            {
                linux::spawn(directory)
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = directory;
                Err("Linux helper unsupported".into())
            }
        }
    }
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err("Unsafe or oversized package input".into());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("Oversized package input".into());
    }
    Ok(bytes)
}

pub(crate) fn valid_id(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|parsed| parsed.to_string() == id)
}
pub(crate) fn process_start(stat: &str) -> Option<u64> {
    stat.rsplit_once(") ")?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

/// Always headless; no helper failure may reach a native error dialog.
pub fn dispatch() -> Option<i32> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    #[cfg(target_os = "macos")]
    {
        if args.first().is_some_and(|a| a == "--update-launch") {
            return match macos::early_launch(&args) {
                Ok(()) => None,
                Err(error) => {
                    eprintln!("Update launch: {error}");
                    Some(1)
                }
            };
        }
        if let Some(code) = macos::dispatch_probe(&args) {
            return Some(code);
        }
    }
    let mode = args.first()?.to_str()?;
    if !matches!(
        mode,
        "--update-supervisor" | "--update-worker" | "--update-restore" | "--update-verify-package"
    ) {
        return None;
    }
    if mode == "--update-verify-package" {
        return Some(
            if args.len() == 2 && VerifiedPackage::verify(Path::new(&args[1])).is_ok() {
                0
            } else {
                1
            },
        );
    }
    #[cfg(target_os = "linux")]
    let result = if args.len() == 2 {
        let directory = PathBuf::from(&args[1]);
        match mode {
            "--update-worker" => linux::worker(&directory),
            "--update-supervisor" => linux::supervise(&directory),
            _ => Err("Recovery mode is unsupported on Linux".into()),
        }
    } else {
        Err("Invalid updater arguments".into())
    };
    #[cfg(target_os = "macos")]
    let result = if args.len() == 2 {
        let directory = PathBuf::from(&args[1]);
        match mode {
            "--update-supervisor" => macos::supervise(&directory),
            "--update-worker" => macos::worker(&directory, false),
            _ => macos::worker(&directory, true),
        }
    } else {
        Err("Invalid updater arguments".into())
    };
    #[cfg(windows)]
    let result = if args.len() == 2 && mode == "--update-supervisor" {
        windows::supervise(Path::new(&args[1]))
    } else {
        Err("Unsupported Windows helper arguments".into())
    };
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    let result: Result<(), String> = Err("Updater helper unsupported on this platform".into());
    Some(match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("Update: {error}");
            1
        }
    })
}

#[cfg(target_os = "linux")]
#[path = "update_install_linux.rs"]
pub mod linux;
#[cfg(target_os = "macos")]
#[path = "update_install_macos.rs"]
pub mod macos;
#[cfg(windows)]
#[path = "update_install_windows.rs"]
pub mod windows;

#[cfg(any(target_os = "macos", test))]
#[path = "update_launch_receipt.rs"]
mod launch_receipt;
#[path = "update_install_process.rs"]
mod process_snapshot;
#[cfg(any(target_os = "macos", test))]
#[path = "update_install_quarantine.rs"]
pub(crate) mod quarantine;
#[path = "update_install_status.rs"]
mod status;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn process_identity_handles_spaces_and_parentheses() {
        assert_eq!(
            process_start("42 (name ) here) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 987 0"),
            Some(987)
        );
        assert_eq!(process_start("42 malformed"), None);
    }
    #[test]
    fn transaction_ids_cannot_escape_staging() {
        assert!(valid_id("d2dc4b3c-6389-4bcc-901e-e045f9d61e44"));
        for value in [
            "../escape",
            "",
            "/tmp/a",
            "abc",
            "AAAAAAAA-6389-4bcc-901e-e045f9d61e44",
        ] {
            assert!(!valid_id(value));
        }
    }
}

pub fn acknowledge_health() {
    #[cfg(feature = "update-acceptance")]
    if crate::update_acceptance::withhold_health() {
        return;
    }
    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    {
        static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !DONE.swap(true, std::sync::atomic::Ordering::Relaxed) {
            #[cfg(windows)]
            let _ = windows::acknowledge_health();
            #[cfg(target_os = "linux")]
            let _ = linux::acknowledge_health();
            #[cfg(target_os = "macos")]
            let _ = macos::acknowledge_health();
        }
    }
}
pub fn previous_failure() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        linux::previous_failure().ok().flatten()
    }
    #[cfg(target_os = "macos")]
    {
        macos::previous_failure().ok().flatten()
    }
    #[cfg(windows)]
    {
        windows::previous_failure().ok().flatten()
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

pub fn startup_ready(startup_busy: bool, draft_loading: bool, has_snapshot: bool) -> bool {
    !startup_busy && !draft_loading && has_snapshot
}
pub(crate) fn health_matches(value: &serde_json::Value, id: &str, version: &str, pid: u32) -> bool {
    value["id"] == id && value["version"] == version && value["pid"] == pid
}
#[cfg(test)]
mod health_tests {
    use super::*;
    #[test]
    fn startup_health_requires_loaded_library_and_restored_drafts() {
        assert!(startup_ready(false, false, true));
        assert!(!startup_ready(true, false, true));
        assert!(!startup_ready(false, true, true));
        assert!(!startup_ready(false, false, false));
    }
    #[test]
    fn health_is_bound_to_attempt_build_and_child_process() {
        let health = serde_json::json!({"id":"attempt", "version":"0.9.0", "pid":42});
        assert!(health_matches(&health, "attempt", "0.9.0", 42));
        assert!(!health_matches(&health, "old-attempt", "0.9.0", 42));
        assert!(!health_matches(&health, "attempt", "0.8.0", 42));
        assert!(!health_matches(&health, "attempt", "0.9.0", 43));
    }
}

#[derive(Default)]
pub struct StartupHealth {
    catalog_restored: bool,
}
impl StartupHealth {
    pub fn catalog_result(&mut self, errors: &[String]) {
        self.catalog_restored = errors.is_empty();
    }
    pub fn reset_catalog(&mut self) {
        self.catalog_restored = false;
    }
    pub fn ready(&self, busy: bool, loading: bool, snapshot: bool, draft_failed: bool) -> bool {
        self.catalog_restored && !draft_failed && startup_ready(busy, loading, snapshot)
    }
}
pub fn clear_pending<A, B>(legacy: &mut Option<A>, package: &mut Option<B>) {
    *legacy = None;
    *package = None;
}
#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    #[test]
    fn failed_catalog_event_never_acknowledges_even_after_loading_clears() {
        let mut state = StartupHealth::default();
        state.catalog_result(&["draft record unreadable".into()]);
        assert!(!state.ready(false, false, true, false));
        state.catalog_result(&[]);
        assert!(state.ready(false, false, true, false));
        state.reset_catalog();
        assert!(!state.ready(false, false, true, false));
    }
    #[test]
    fn failed_close_disarms_package_before_subsequent_ordinary_close() {
        let mut old = Some("legacy");
        let mut package = Some("approved package");
        clear_pending(&mut old, &mut package);
        assert!(old.take().is_none());
        assert!(package.take().is_none());
        package = Some("fresh explicit retry");
        assert_eq!(package.take(), Some("fresh explicit retry"));
    }
}

#[derive(Default)]
pub(crate) struct PreparationGate {
    epoch: u64,
    running: Option<u64>,
}
impl PreparationGate {
    pub fn begin(&mut self) -> Option<u64> {
        if self.running.is_some() {
            return None;
        }
        self.epoch += 1;
        self.running = Some(self.epoch);
        Some(self.epoch)
    }
    pub fn invalidate(&mut self) {
        self.epoch += 1;
    }
    pub fn finish(&mut self, token: u64) -> bool {
        if self.running != Some(token) {
            return false;
        }
        self.running = None;
        token == self.epoch
    }
}
#[cfg(test)]
mod preparation_tests {
    use super::*;
    #[test]
    fn update_preparation_deduplicates_and_rejects_switch_away_then_back() {
        let mut gate = PreparationGate::default();
        let first = gate.begin().unwrap();
        assert!(gate.begin().is_none());
        gate.invalidate();
        gate.invalidate();
        assert!(!gate.finish(first));
        let next = gate.begin().unwrap();
        assert!(gate.finish(next));
    }
}
