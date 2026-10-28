use serde::{Deserialize, Serialize};
use std::path::PathBuf;
/// Maximum number of logical folders from a root folder to a leaf.
pub const MAX_FOLDER_DEPTH: usize = 8;
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LibrarySort {
    #[default]
    NameAscending,
    NameDescending,
    Newest,
    Oldest,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct LayoutSettings {
    pub navigation_width: u32,
    pub folders_width: u32,
    pub notes_width: u32,
    pub navigation_visible: bool,
    pub folders_visible: bool,
    pub library_sort: LibrarySort,
}

impl Default for LayoutSettings {
    fn default() -> Self {
        Self {
            navigation_width: 160,
            folders_width: 144,
            notes_width: 280,
            navigation_visible: true,
            folders_visible: true,
            library_sort: LibrarySort::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Settings {
    pub interval_minutes: u32,
    pub answer_minutes: u32,
    pub review_days: Vec<u32>,
    #[serde(default)]
    pub show_file_paths: bool,
    #[serde(default)]
    pub layout: LayoutSettings,
    #[serde(default)]
    pub appearance: crate::AppearanceSettings,
    #[serde(default)]
    pub images: crate::media_settings::ImageSettings,
}
/// Validated configuration, ready to install without another storage operation.
pub struct PreparedSettings {
    pub(crate) settings: Settings,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SessionStatus {
    Pending,
    Learned,
    Missed,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub path: String,
    pub title: String,
    /// Explicit display names survive subsequent Markdown heading changes.
    #[serde(default)]
    pub title_override: bool,
    #[serde(default)]
    pub folder_id: Option<String>,
    #[serde(default)]
    pub managed: bool,
    #[serde(default)]
    pub detached: bool,
    pub missing: bool,
    pub error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub revision: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct NoteFolder {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub parent_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NoteVersion {
    pub id: String,
    pub note_id: String,
    pub content: String,
    pub hash: String,
    pub captured_at: i64,
    pub offline: bool,
    pub summary: String,
    pub diff: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActiveSpan {
    pub start: i64,
    pub end: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub started_at: i64,
    pub ended_at: i64,
    pub active_seconds: u64,
    pub deadline: i64,
    pub status: SessionStatus,
    pub summary: String,
    pub suggested_summary: String,
    pub note_ids: Vec<String>,
    pub version_ids: Vec<String>,
    pub spans: Vec<ActiveSpan>,
    pub updated_at: i64,
    pub revision: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Review {
    pub id: String,
    pub session_id: String,
    pub title: String,
    pub note_ids: Vec<String>,
    pub learned_at: i64,
    pub last_review_at: Option<i64>,
    pub next_review_at: Option<i64>,
    pub stage: usize,
    pub intervals: Vec<u32>,
    pub revision: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReviewEvent {
    pub id: String,
    pub review_id: String,
    pub completed_at: i64,
    pub stage: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TimerState {
    pub paused: bool,
    pub active_seconds: u64,
    pub target_seconds: u64,
    pub started_at: i64,
    pub spans: Vec<ActiveSpan>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub settings: Settings,
    pub notes: Vec<Note>,
    #[serde(default)]
    pub folders: Vec<NoteFolder>,
    pub versions: std::sync::Arc<Vec<NoteVersion>>,
    pub sessions: Vec<Session>,
    pub reviews: Vec<Review>,
    pub review_events: Vec<ReviewEvent>,
    pub timer: TimerState,
    pub data_path: String,
}
#[derive(Clone, Debug)]
pub enum Command {
    CreateNote {
        title: String,
        content: String,
    },
    CreateNoteInFolder {
        title: String,
        content: String,
        folder_id: Option<String>,
    },
    /// Commit a retained draft without changing the UUID used by its assets.
    CreateNoteWithId {
        note_id: String,
        title: String,
        content: String,
        folder_id: Option<String>,
    },
    CreateFolder {
        name: String,
        parent_id: Option<String>,
    },
    RenameFolder {
        folder_id: String,
        name: String,
    },
    MoveFolder {
        folder_id: String,
        parent_id: Option<String>,
    },
    DeleteFolder {
        folder_id: String,
    },
    MoveNote {
        note_id: String,
        folder_id: Option<String>,
    },
    RenameNote {
        note_id: String,
        title: String,
        expected_revision: u64,
    },
    SaveNote {
        note_id: String,
        expected_hash: String,
        content: String,
    },
    ReceiveSyncedNote {
        note_id: String,
        expected_hash: Option<String>,
        content: String,
    },
    ReceiveSyncedCopy {
        note_id: String,
        title: String,
        content: String,
    },
    ExportNote {
        note_id: String,
        version_id: Option<String>,
        path: PathBuf,
    },
    Import(Vec<PathBuf>),
    UnmanageNote {
        note_id: String,
    },
    Relink {
        note_id: String,
        path: PathBuf,
    },
    SaveSettings(Settings),
    SaveLearningSettings {
        interval_minutes: u32,
        answer_minutes: u32,
        review_days: Vec<u32>,
    },
    SetShowFilePaths(bool),
    SaveLayout(LayoutSettings),
    SetPaused(bool),
    Submit {
        session_id: String,
        summary: String,
        note_ids: Vec<String>,
        version_ids: Option<Vec<String>>,
    },
    Skip {
        session_id: String,
    },
    Manual {
        started_at: i64,
        minutes: u32,
        summary: String,
        note_ids: Vec<String>,
        version_ids: Option<Vec<String>>,
    },
    EditSession {
        session_id: String,
        summary: String,
        learned: bool,
        note_ids: Vec<String>,
        version_ids: Option<Vec<String>>,
    },
    CompleteReview {
        review_id: String,
        expected_stage: usize,
    },
    UndoReview {
        review_id: String,
    },
    Backup(PathBuf),
    Restore(PathBuf),
    Scan {
        offline: bool,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DayStat {
    pub date: String,
    pub learned_seconds: u64,
    pub learned_count: usize,
    pub missed_count: usize,
    pub review_count: usize,
}
