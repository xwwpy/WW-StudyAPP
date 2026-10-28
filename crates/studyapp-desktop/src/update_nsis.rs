//! NSIS update path: signed `latest.json` -> verified setup -> silent install.
//!
//! The installer waits for this process to exit (`/S`), installs, and restarts
//! the application because the launcher passes `/UPDATED`.
#![allow(dead_code)] // consumed by the panel rewrite in the same change series.
use crate::{
    update_backend::{
        UpdateErrorKind, UpdateResult, download_setup_to, failure, fetch_latest_manifest_text,
    },
    update_manifest::{UpdateManifest, parse_latest_json},
    update_signature::verify_manifest_signature,
};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

/// Uploaded NSIS setups stay well below this; larger downloads are refused.
const SETUP_LIMIT: u64 = 256 * 1024 * 1024;

/// Returns the offered release only when it is newer than the running version.
pub fn check(current_version: &str) -> UpdateResult<Option<UpdateManifest>> {
    let text = fetch_latest_manifest_text()?;
    let manifest =
        parse_latest_json(&text).map_err(|_| failure(UpdateErrorKind::InvalidRelease))?;
    Ok(manifest.is_newer_than(current_version).then_some(manifest))
}

/// Downloads the advertised setup and refuses to return it unless the signature
/// over the exact bytes validates against the compiled-in release key.
pub fn download(
    manifest: &UpdateManifest,
    directory: &Path,
    progress: impl FnMut(u64, u64),
) -> UpdateResult<PathBuf> {
    std::fs::create_dir_all(directory).map_err(|_| failure(UpdateErrorKind::Preparation))?;
    let setup = directory.join("WW-StudyApp-Setup.exe");
    download_setup_to(&manifest.setup_url, &setup, SETUP_LIMIT, progress)?;
    let payload = std::fs::read(&setup).map_err(|_| failure(UpdateErrorKind::Preparation))?;
    verify_manifest_signature(&payload, &manifest.signature)
        .map_err(|_| failure(UpdateErrorKind::Verification))?;
    Ok(setup)
}

/// Starts the installer in silent mode. The caller exits the application after
/// this returns so the installer can replace the executable.
pub fn launch(setup: &Path) -> UpdateResult<()> {
    Command::new(setup)
        .arg("/S")
        .arg("/UPDATED")
        .spawn()
        .map_err(|_| failure(UpdateErrorKind::Launch))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn missing_installer_is_reported_as_a_launch_failure() {
        let missing = std::env::temp_dir().join("ww-studyapp-missing-setup.exe");
        let _ = std::fs::remove_file(&missing);
        let error = launch(&missing).unwrap_err();
        assert!(matches!(error.kind, UpdateErrorKind::Launch));
    }
}
