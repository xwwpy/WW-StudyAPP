//! Explicit operation contexts; raw worker errors are never notification summaries.
use super::Work;
use studyapp_core::{
    Command, NoticeAction as Action, NoticeCategory as Category, NoticeEvent,
    NoticeSeverity as Severity,
};

#[derive(Clone, Copy)]
pub(crate) enum Context {
    Save,
    Table,
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
                Command::SetTableView { .. } => Self::Table,
                Command::CreateNote { .. }
                | Command::CreateNoteInFolder { .. }
                | Command::CreateNoteWithId { .. }
                | Command::SaveNote { .. }
                | Command::SaveSettings(_)
                | Command::SaveLearningSettings { .. }
                | Command::SetShowFilePaths(_)
                | Command::SetTimerWidgetPinned(_)
                | Command::SaveLayout(_)
                | Command::Submit { .. }
                | Command::Manual { .. }
                | Command::EditSession { .. } => Self::Save,
                Command::Backup(_)
                | Command::ExportNote { .. }
                | Command::ExportPreviewedNote { .. } => Self::Export,
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
            Self::Table => (
                Category::SaveSuccess,
                Category::SaveFailure,
                "table-view",
                Action::OpenStorage,
                "表格显示设置未保存，笔记源文未改变。当前显示仍保留，请重试调整或检查资料库。",
            ),
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
            format!(
                "{failure_summary}
原因：{}",
                failure_reason(outcome.as_ref().unwrap_err())
            )
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
                    Command::SetTableView {
                        document, table, ..
                    } => {
                        document.hash(hash);
                        table.hash(hash);
                    }
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
                    | Command::ExportPreviewedNote { note_id, path, .. }
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
                Work::Apply(Command::SetTimerWidgetPinned(false)),
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

/// Translate known failure boundaries without displaying credentials or raw stacks.
fn failure_reason(error: &str) -> &'static str {
    let text = error.to_lowercase();
    if [
        "permission",
        "access is denied",
        "os error 5",
        "拒绝访问",
        "权限",
        "read-only",
        "readonly",
        "只读",
    ]
    .iter()
    .any(|key| text.contains(key))
    {
        "没有访问或写入权限。请选择有权限的目录，或检查文件是否为只读。"
    } else if [
        "os error 32",
        "os error 33",
        "locked",
        "being used",
        "占用",
        "被锁",
    ]
    .iter()
    .any(|key| text.contains(key))
    {
        "文件或资料库正被其他进程占用。关闭另一实例或占用该文件的程序后重试。"
    } else if ["no space", "disk full", "os error 112", "空间不足"]
        .iter()
        .any(|key| text.contains(key))
    {
        "目标磁盘空间不足。释放空间或更换存储位置后重试。"
    } else if [
        "not found",
        "no such file",
        "os error 2",
        "os error 3",
        "不存在",
        "找不到",
    ]
    .iter()
    .any(|key| text.contains(key))
    {
        "所需文件或目录不存在。检查文件是否已移动，以及外置磁盘或网络目录是否已连接。"
    } else if ["timeout", "timed out", "超时"]
        .iter()
        .any(|key| text.contains(key))
    {
        "连接或处理超时。检查网络和服务状态后重试。"
    } else if [
        "http 401",
        "http 403",
        "unauthorized",
        "forbidden",
        "凭据",
        "认证",
    ]
    .iter()
    .any(|key| text.contains(key))
    {
        "服务拒绝访问。检查账号、访问令牌与服务权限，不要在公开反馈中粘贴凭据。"
    } else if ["network", "connect", "dns", "网络", "连接"]
        .iter()
        .any(|key| text.contains(key))
    {
        "无法连接目标服务。检查网络、地址和服务是否可用。"
    } else if ["超过", "too large", "过大", "限额"]
        .iter()
        .any(|key| text.contains(key))
    {
        "内容或文件超过此操作允许的大小或数量，请缩小范围后分批处理。"
    } else if ["冲突", "stale", "revision", "已更改"]
        .iter()
        .any(|key| text.contains(key))
    {
        "内容已被另一操作修改。保留当前修改，刷新状态后再重试。"
    } else if ["格式", "utf-8", "invalid", "损坏", "校验", "仅支持"]
        .iter()
        .any(|key| text.contains(key))
    {
        "文件格式、编码或完整性检查未通过，请确认文件可正常打开并使用支持的格式。"
    } else {
        "无法确认相关操作已完成。请检查目标状态；需要进一步排查时，开启诊断日志后重试。"
    }
}

#[cfg(test)]
mod readable_error_tests {
    use super::*;
    #[test]
    fn failure_reasons_are_actionable_and_never_copy_raw_paths_tokens_or_identifiers() {
        for (error, expected) in [
            ("permission denied (os error 5)", "权限"),
            ("being used (os error 32)", "占用"),
            ("disk full (os error 112)", "空间不足"),
            ("file not found (os error 2)", "不存在"),
            ("connection timed out", "超时"),
            ("HTTP 401", "服务拒绝访问"),
            ("HTTP status client error (403 Forbidden)", "服务拒绝访问"),
            ("read-only filesystem", "权限"),
            ("unknown failure in folder-401", "无法确认"),
            ("invalid UTF-8", "编码"),
            ("stale revision", "另一操作"),
        ] {
            let raw =
                format!("{error}\nC:/private/person.txt https://service/?token=secret EV-private");
            let event = Context::Save.event(&Err(raw));
            assert!(event.summary.contains(expected), "{}", event.summary);
            for secret in ["person.txt", "token=", "EV-private", "secret"] {
                assert!(!event.summary.contains(secret));
            }
        }
    }
}
