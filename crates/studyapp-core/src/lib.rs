//! Persistent learning rules, independent of the desktop interface.
mod appearance;
mod color;
mod engine;
mod folders;
pub mod integration;
mod managed;
pub mod media_settings;
mod models;
mod notes;
mod notifications;
mod purge;
mod stats;
mod storage;
mod timer;
pub use appearance::*;
pub use color::*;
pub use engine::Engine;
pub use media_settings::{ImageSettings, ImageStorage};
pub use models::*;
pub use notifications::*;
pub use purge::{
    finish_purge_cleanup, inspect_purge_backup, purge_backup_revision,
    purge_managed_note_from_backup, scrub_confirmed_backup,
};
pub use stats::daily_stats;
pub use timer::*;
/// Check SQLite integrity, application identity and stored state without modifying the backup.
pub fn verify_backup_file(path: &std::path::Path) -> anyhow::Result<()> {
    storage::verified_backup(path).map(drop)
}
