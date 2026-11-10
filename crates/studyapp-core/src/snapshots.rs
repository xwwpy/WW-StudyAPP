//! Snapshot policy is independent of durable drafts and explicit editor saves.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotMode {
    #[default]
    Automatic,
    ScanOnly,
    Manual,
}
impl SnapshotMode {
    pub const ALL: [Self; 3] = [Self::Automatic, Self::ScanOnly, Self::Manual];
    pub const fn label(self) -> &'static str {
        match self {
            Self::Automatic => "定时扫描并保存",
            Self::ScanOnly => "定时扫描，手动保存",
            Self::Manual => "手动扫描并保存",
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SnapshotSettings {
    pub mode: SnapshotMode,
    pub interval_seconds: u32,
}
impl Default for SnapshotSettings {
    fn default() -> Self {
        Self {
            mode: SnapshotMode::Automatic,
            interval_seconds: 60,
        }
    }
}
impl SnapshotSettings {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (3..=86400).contains(&self.interval_seconds),
            "快照扫描间隔须为3–86400秒"
        );
        Ok(())
    }
    pub fn scheduled(self) -> bool {
        self.mode != SnapshotMode::Manual
    }
}
