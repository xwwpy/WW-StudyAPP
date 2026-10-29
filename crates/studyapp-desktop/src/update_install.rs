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
    pub data_directory: Option<PathBuf>,
}

/// Cannot be constructed without validating the signed envelope and exact bytes.
#[derive(Clone, Debug)]
pub struct VerifiedPackage {
    pub(crate) directory: PathBuf,
    pub(crate) version: String,
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
            &directory.join("package"),
            manifest.size,
            progress,
        )
        .map_err(|e| e.to_string())?;
        Self::verify(&directory)
    }
    pub fn verify(directory: &Path) -> Result<Self, String> {
        let envelope = String::from_utf8(read_bounded(
            &directory.join("manifest.json"),
            2 * 1024 * 1024,
        )?)
        .map_err(|e| e.to_string())?;
        let manifest = crate::update_manifest::parse_latest_json(&envelope)?;
        let bytes = read_bounded(&directory.join("package"), manifest.size)?;
        if bytes.len() as u64 != manifest.size
            || format!("{:x}", Sha256::digest(&bytes)) != manifest.sha256
        {
            return Err("Package size or digest mismatch".into());
        }
        crate::update_signature::verify_manifest_signature(&bytes, &manifest.signature)?;
        Ok(Self {
            directory: directory.to_owned(),
            version: manifest.version.to_string(),
        })
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
