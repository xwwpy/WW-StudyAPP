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
    pub stable_seconds: u32,
}
impl Default for SnapshotSettings {
    fn default() -> Self {
        Self {
            mode: SnapshotMode::Automatic,
            interval_seconds: 60,
            stable_seconds: 3,
        }
    }
}
impl SnapshotSettings {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (3..=86400).contains(&self.interval_seconds),
            "快照扫描间隔须为3–86400秒"
        );
        ensure!(
            (1..=86400).contains(&self.stable_seconds),
            "内容稳定等待须为1–86400秒"
        );
        Ok(())
    }
    pub fn scheduled(self) -> bool {
        self.mode != SnapshotMode::Manual
    }
}

/// A hash-bound selection prevents an old preview from saving newer unseen text.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotSelection {
    pub note_id: String,
    pub expected_hash: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnapshotChange {
    pub note_id: String,
    pub title: String,
    pub expected_hash: String,
    pub previous: String,
    pub current: String,
    pub diff: String,
}
/// Durably saved text which has not yet been promoted to a history snapshot.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedNoteBody {
    pub content: String,
    pub hash: String,
}
