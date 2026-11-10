//! Persistent learning rules, independent of the desktop interface.
mod appearance;
mod color;
pub mod documents;
mod engine;
mod folders;
mod hover_content;
pub mod integration;
mod list_style;
mod logging;
mod managed;
pub mod media_settings;
mod models;
mod notes;
mod notice_surface;
pub use hover_content::*;
mod notifications;
pub use notice_surface::*;
mod purge;
mod safe_html;
mod safe_html_document;
mod selection_style;
mod snapshots;
mod stats;
pub use selection_style::*;
pub use snapshots::*;
mod storage;
mod table_layout;
mod timer;
pub use appearance::*;
pub use color::*;
pub use engine::Engine;
pub use list_style::*;
pub use logging::*;
pub use media_settings::{ImageSettings, ImageStorage};
pub use models::*;
pub use notifications::*;
pub use purge::{
    finish_purge_cleanup, inspect_purge_backup, purge_backup_revision,
    purge_managed_note_from_backup, scrub_confirmed_backup,
};
pub use safe_html::{html_image_sources, sanitize_html, sanitize_html_style, sanitize_inline_html};
pub use stats::daily_stats;
pub use table_layout::*;
pub use timer::*;
/// Check SQLite integrity, application identity and stored state without modifying the backup.
pub fn verify_backup_file(path: &std::path::Path) -> anyhow::Result<()> {
    storage::verified_backup(path).map(drop)
}

pub use safe_html_document::{SafeHtmlNode, safe_html_document};
