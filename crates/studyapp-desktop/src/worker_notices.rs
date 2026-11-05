//! Explicit operation contexts; raw worker errors are never notification summaries.
use super::Work;
use studyapp_core::{
    Command, NoticeAction as Action, NoticeCategory as Category, NoticeEvent,
    NoticeSeverity as Severity,
};

#[derive(Clone, Copy)]
pub(crate) enum Context {
    Save,
    Sync,
    SyncConflict,
    Import,
    ImportProgress,
    Export,
    Migration,
    Image,
    Recovery,
    Operation,
}
impl Context {
    pub(crate) fn for_work(work: &Work) -> Self {
        match work {
            Work::Scoped { work, .. } => Self::for_work(work),
            Work::Apply(command) => match command {
                Command::CreateNote { .. }
                | Command::CreateNoteInFolder { .. }
                | Command::CreateNoteWithId { .. }
                | Command::SaveNote { .. }
                | Command::SaveSettings(_)
                | Command::SaveLearningSettings { .. }
                | Command::SetShowFilePaths(_)
                | Command::SaveLayout(_)
                | Command::Submit { .. }
                | Command::Manual { .. }
                | Command::EditSession { .. } => Self::Save,
                Command::Backup(_) | Command::ExportNote { .. } => Self::Export,
                Command::Import(_) | Command::Restore(_) => Self::Import,
                _ => Self::Operation,
            },
            Work::SavePreferences(_) => Self::Save,
            Work::StartSync { .. } | Work::SyncFinished { .. } => Self::Sync,
            Work::ImportPaths(_) => Self::ImportProgress,
            Work::PreparedImport { .. } => Self::Import,
            Work::MigrateLibrary(_) | Work::MigrateApplication(_) | Work::CleanMigration(_) => {
                Self::Migration
            }
            Work::Media(_)
            | Work::TestImageCloud
            | Work::RetryImageCloud
            | Work::ImageCloudFinished { .. } => Self::Image,
            _ => Self::Operation,
        }
    }
    pub(crate) fn event(self, outcome: &Result<String, String>) -> NoticeEvent {
        let failed = outcome.is_err();
        let (success, failure, key, action, failure_summary) = match self {
            Self::Save => (
                Category::SaveSuccess,
                Category::SaveFailure,
                "save",
                Action::OpenSave,
                "内容未保存，当前修改仍保留。请检查存储位置后重试。",
            ),
            Self::Sync | Self::SyncConflict => (
                if matches!(self, Self::SyncConflict) {
                    Category::SyncConflict
                } else {
                    Category::SyncComplete
                },
                Category::SyncFailure,
                "sync",
                Action::OpenSync,
                "同步未全部完成，本机内容仍保留。请检查连接与同步设置后重试。",
            ),
            Self::Import | Self::ImportProgress => (
                Category::ImportResult,
                Category::ImportResult,
                "import",
                Action::OpenLibrary,
                "导入未完成，请检查文件格式、访问权限和资料库位置后重试。",
            ),
            Self::Export => (
                Category::ExportResult,
                Category::ExportResult,
                "export",
                Action::OpenLibrary,
                "导出未完成，请检查目标位置及其写入权限后重试。",
            ),
            Self::Migration => (
                Category::MigrationResult,
                Category::MigrationResult,
                "migration",
                Action::OpenStorage,
                "位置迁移未完成。请查看存储设置；现有恢复与原文件保护仍保留。",
            ),
            Self::Image => (
                Category::OperationResult,
                Category::ImageFailure,
                "image",
                Action::OpenStorage,
                "图片操作未完成，请检查图片位置、网络与云端设置。",
            ),
            Self::Recovery => (
                Category::StartupRecovery,
                Category::StartupRecovery,
                "recovery",
                Action::OpenStorage,
                "启动恢复需要检查，应用已保留可恢复的资料。请查看存储位置与恢复选项。",
            ),
            Self::Operation => (
                Category::OperationResult,
                Category::OperationFailure,
                "operation",
                Action::OpenLibrary,
                "操作未完成，请检查相关文件、目录或设置后重试。",
            ),
        };
        let severity = if failed {
            Severity::Error
        } else if matches!(self, Self::SyncConflict) {
            Severity::Warning
        } else if matches!(self, Self::ImportProgress) {
            Severity::Information
        } else {
            Severity::Success
        };
        let summary = if failed {
            failure_summary.to_owned()
        } else {
            outcome.as_ref().unwrap().clone()
        };
        let mut event = NoticeEvent::new(if failed { failure } else { success }, severity, summary);
        event.dedupe_key = Some(format!(
            "worker-{key}-{}",
            if failed { "failure" } else { "complete" }
        ));
        event.action = Some(action);
        event
    }
}

/// In-memory grouping uses operation + opaque target identity, never user prose/content.
pub(crate) fn work_key(work: &Work, generation: u64) -> String {
    use std::hash::{Hash, Hasher};
    fn target(work: &Work, hash: &mut impl Hasher) {
        std::mem::discriminant(work).hash(hash);
        match work {
            Work::Scoped { library, work } => {
                library.hash(hash);
                target(work, hash);
            }
            Work::Apply(command) => {
                std::mem::discriminant(command).hash(hash);
                match command {
                    Command::SaveNote { note_id, .. }
                    | Command::RenameNote { note_id, .. }
                    | Command::MoveNote { note_id, .. }
                    | Command::UnmanageNote { note_id }
                    | Command::ReceiveSyncedNote { note_id, .. }
                    | Command::ReceiveSyncedCopy { note_id, .. }
                    | Command::CreateNoteWithId { note_id, .. } => note_id.hash(hash),
                    Command::RenameFolder { folder_id, .. }
                    | Command::MoveFolder { folder_id, .. }
                    | Command::DeleteFolder { folder_id } => folder_id.hash(hash),
                    Command::ExportNote { note_id, path, .. }
                    | Command::Relink { note_id, path } => {
                        note_id.hash(hash);
                        path.hash(hash);
                    }
                    Command::Submit { session_id, .. }
                    | Command::EditSession { session_id, .. }
                    | Command::Skip { session_id } => session_id.hash(hash),
                    Command::CompleteReview { review_id, .. }
                    | Command::UndoReview { review_id } => review_id.hash(hash),
                    Command::Backup(path) | Command::Restore(path) => path.hash(hash),
                    Command::Import(paths) => paths.hash(hash),
                    _ => {}
                }
            }
            Work::ImportPaths(paths) => paths.hash(hash),
            Work::SwitchLibrary(path)
            | Work::MigrateLibrary(path)
            | Work::MigrateApplication(path) => path.hash(hash),
            Work::DeleteManagedCopy { note_id } | Work::PreviewPurge(note_id, _) => {
                note_id.hash(hash)
            }
            Work::Media(request) => request.request().hash(hash),
            _ => {}
        }
    }
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    generation.hash(&mut hash);
    target(work, &mut hash);
    format!("worker-{:016x}", hash.finish())
}
pub(crate) fn set_work_key(event: &mut NoticeEvent, key: &str) {
    event.dedupe_key = Some(format!(
        "{key}-{}",
        if event.severity >= Severity::Error {
            "failure"
        } else {
            "complete"
        }
    ));
}

#[cfg(test)]
mod tests {
    use super::{Category, Context, Severity, Work};
    use studyapp_core::Command;
    #[test]
    fn operation_types_select_categories_without_inspecting_error_prose() {
        let cases = [
            (
                Work::Apply(Command::SaveNote {
                    note_id: "a".into(),
                    expected_hash: "hash".into(),
                    content: "private content".into(),
                }),
                Category::SaveFailure,
            ),
            (
                Work::Apply(Command::Backup("private-export-location".into())),
                Category::ExportResult,
            ),
            (Work::Apply(Command::Import(vec![])), Category::ImportResult),
            (Work::StartSync { test_only: false }, Category::SyncFailure),
            (
                Work::MigrateLibrary("private-library-location".into()),
                Category::MigrationResult,
            ),
            (Work::RetryImageCloud, Category::ImageFailure),
            (
                Work::DeleteManagedCopy {
                    note_id: "a".into(),
                },
                Category::OperationFailure,
            ),
        ];
        for (work, expected) in cases {
            let event = Context::for_work(&work)
                .event(&Err("private-path?token=credential\nsecret-stack".into()));
            assert_eq!(event.category, expected);
            assert_eq!(event.severity, Severity::Error);
            assert!(!event.summary.contains("private"));
            assert!(!event.summary.contains("secret"));
            assert!(!event.summary.contains("credential"));
            event.validate().unwrap();
        }
        assert_eq!(
            Context::SyncConflict
                .event(&Ok("已保留冲突副本".into()))
                .category,
            Category::SyncConflict
        );
        let save = |id: &str, content: &str| {
            Work::Apply(Command::SaveNote {
                note_id: id.into(),
                expected_hash: "revision".into(),
                content: content.into(),
            })
        };
        assert_eq!(
            super::work_key(&save("a", "private one"), 0),
            super::work_key(&save("a", "different body"), 0)
        );
        assert_ne!(
            super::work_key(&save("a", "private one"), 0),
            super::work_key(&save("b", "private one"), 0)
        );
        assert_ne!(
            super::work_key(&save("a", "private one"), 0),
            super::work_key(&save("a", "private one"), 1)
        );
        assert_eq!(
            Context::Save.event(&Ok("已保存".into())).category,
            Category::SaveSuccess
        );
    }
}
