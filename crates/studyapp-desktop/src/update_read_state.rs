//! Device-local read marker. Reading release notes never changes installed version.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{io::Read, path::Path};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UpdateReadState {
    schema: u32,
    version: Option<String>,
}
impl Default for UpdateReadState {
    fn default() -> Self {
        Self {
            schema: 1,
            version: None,
        }
    }
}
impl UpdateReadState {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.try_exists()? {
            return Ok(Self::default());
        }
        let _guards = crate::storage_backend::guard_directories(
            path.parent()
                .ok_or_else(|| anyhow::anyhow!("阅读状态目录无效"))?,
            false,
        )?;
        let file = crate::storage_backend::plain_handle(path, false, true)?;
        ensure!(file.metadata()?.len() <= 8192, "更新阅读状态文件过大");
        let mut bytes = Vec::new();
        file.take(8193).read_to_end(&mut bytes)?;
        let state: Self = serde_json::from_slice(&bytes)?;
        ensure!(state.schema == 1, "更新阅读状态版本不受支持");
        ensure!(
            state
                .version
                .as_deref()
                .is_none_or(|v| semver::Version::parse(v).is_ok()),
            "更新阅读版本无效"
        );
        Ok(state)
    }
    pub fn unread(&self, version: &str) -> bool {
        self.version.as_deref() != Some(version)
    }
    pub fn mark_read(&mut self, version: &str) {
        self.version = Some(version.to_owned());
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        crate::app_migration::atomic_json(path, self)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn read_marker_is_version_specific_and_survives_restart_without_changing_installation() {
        let directory = tempfile::tempdir().unwrap().keep();
        let path = directory.join("update-read.json");
        let mut state = UpdateReadState::load(&path).unwrap();
        assert!(state.unread("0.3.0"));
        state.mark_read("0.3.0");
        state.save(&path).unwrap();
        let restored = UpdateReadState::load(&path).unwrap();
        assert!(!restored.unread("0.3.0"));
        assert!(restored.unread("0.3.1"));
        for contents in ["{broken", "{\"schema\":2}", "{\"version\":\"invalid\"}"] {
            std::fs::write(&path, contents).unwrap();
            assert!(UpdateReadState::load(&path).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), contents);
        }
    }
}
