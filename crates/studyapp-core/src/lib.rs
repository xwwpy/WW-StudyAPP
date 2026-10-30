//! Persistent learning rules, independent of the desktop interface.
mod appearance;
mod engine;
mod folders;
pub mod integration;
mod managed;
pub mod media_settings;
mod models;
mod notes;
mod purge;
mod stats;
mod storage;
pub use appearance::*;
pub use engine::Engine;
pub use media_settings::{ImageSettings, ImageStorage};
pub use models::*;
pub use purge::purge_managed_note_from_backup;
pub use stats::daily_stats;
