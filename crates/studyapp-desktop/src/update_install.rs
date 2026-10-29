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
    Windows(VerifiedPackage),
    Linux(PathBuf),
}
pub fn prepare(
    package: &VerifiedPackage,
    data: Option<PathBuf>,
) -> Result<PreparedPackage, String> {
    package.reverify()?;
    #[cfg(windows)]
    {
        let _ = data;
        Ok(PreparedPackage::Windows(package.clone()))
    }
    #[cfg(target_os = "linux")]
    {
        linux::prepare(package, data).map(PreparedPackage::Linux)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = data;
        Err("Automatic installation is not available for this platform yet".into())
    }
}
pub fn launch(package: &PreparedPackage) -> Result<(), String> {
    match package {
        PreparedPackage::Windows(package) => {
            package.reverify()?;
            crate::update_nsis::launch(&package.directory.join(package_file()))
                .map_err(|e| e.to_string())
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
    let mode = args.first()?.to_str()?;
    if !matches!(mode, "--update-supervisor" | "--update-worker") {
        return None;
    }
    #[cfg(target_os = "linux")]
    let result = if args.len() == 2 {
        let directory = PathBuf::from(&args[1]);
        if mode == "--update-worker" {
            linux::worker(&directory)
        } else {
            linux::supervise(&directory)
        }
    } else {
        Err("Invalid updater arguments".into())
    };
    #[cfg(not(target_os = "linux"))]
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
    #[cfg(target_os = "linux")]
    {
        static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !DONE.swap(true, std::sync::atomic::Ordering::Relaxed) {
            let _ = linux::acknowledge_health();
        }
    }
}
pub fn previous_failure() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        linux::previous_failure().ok().flatten()
    }
    #[cfg(not(target_os = "linux"))]
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
