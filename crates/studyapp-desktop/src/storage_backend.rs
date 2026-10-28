//! Library storage inspection and explicit, single-file maintenance.
#[path = "storage_viewmodel.rs"]
pub mod viewmodel;

use anyhow::{Context, Result, ensure};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{File, OpenOptions},
    io::{Read, Seek, Write},
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use studyapp_core::{Command, Engine, Note, Snapshot};

#[derive(Clone, Debug, Default)]
pub struct StorageReport {
    pub data_path: String,
    pub database_bytes: u64,
    pub managed_bytes: u64,
    pub retained_bytes: u64,
    pub auxiliary_bytes: u64,
    pub backup_bytes: u64,
    pub image_bytes: u64,
    pub draft_bytes: u64,
    pub scanned_at: i64,
    pub incomplete: bool,
    pub issues: Vec<StorageIssue>,
    pub image_locations: Vec<String>,
    pub cleanup: Option<CopyCleanupOutcome>,
    pub copies: Vec<ManagedCopy>,
    pub presentation: Arc<viewmodel::StoragePresentation>,
}

impl StorageReport {
    /// Build display data on the worker that produced the immutable report.
    pub fn prepare_presentation(&mut self) {
        self.presentation = Arc::new(viewmodel::StoragePresentation::prepare(self));
    }

    /// Logical file lengths, counted once per path. Hard links can share disk blocks.
    pub fn logical_bytes(&self) -> u64 {
        [
            self.database_bytes,
            self.managed_bytes,
            self.retained_bytes,
            self.auxiliary_bytes,
            self.backup_bytes,
            self.image_bytes,
            self.draft_bytes,
        ]
        .into_iter()
        .fold(0, u64::saturating_add)
    }
}

#[derive(Clone, Debug)]
pub struct StorageIssue {
    pub path: String,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyState {
    Present,
    DetachedPresent,
    Cleaned,
    Missing,
    Unavailable,
    Unsafe,
}

impl CopyState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Present => "正在管理",
            Self::DetachedPresent => "已停止管理，副本仍在",
            Self::Cleaned => "副本已清理",
            Self::Missing => "副本缺失",
            Self::Unavailable => "暂时无法访问",
            Self::Unsafe => "受保护的位置",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CopyCleanupOutcome {
    Removed,
    AlreadyAbsent,
    Retained { message: String },
}

impl CopyCleanupOutcome {
    pub fn notice(&self) -> std::result::Result<String, String> {
        match self {
            Self::Removed => Ok("已清理这一份托管副本；历史与学习记录仍保留。".into()),
            Self::AlreadyAbsent => Ok("这份副本已经不存在；历史与学习记录仍保留。".into()),
            Self::Retained { message } => Err(message.clone()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ManagedCopy {
    pub note_id: String,
    pub title: String,
    pub path: String,
    pub bytes: u64,
    pub state: CopyState,
    pub deletable: bool,
    pub reason: Option<String>,
}

#[derive(Clone, Debug)]
pub enum StorageEvent {
    Finished(std::result::Result<StorageReport, String>),
}

/// Preserve the failing filesystem boundary without exposing file contents.
#[derive(Debug)]
pub(crate) struct StorageAccessPath(pub PathBuf);

impl std::fmt::Display for StorageAccessPath {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "无法访问文件或目录：{}", self.0.display())
    }
}

impl std::error::Error for StorageAccessPath {}

fn reparse(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

/// Windows handles deny directory renames while traversing, and explicitly open
/// the reparse point itself. No inspected/copied file is opened through a link.
pub(crate) fn plain_handle(path: &Path, directory: bool, exclusive_read: bool) -> Result<File> {
    let before =
        std::fs::symlink_metadata(path).with_context(|| StorageAccessPath(path.to_owned()))?;
    ensure!(
        !reparse(&before),
        "拒绝访问链接或重解析点：{}",
        path.display()
    );
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000 | 0x0200_0000);
        options.share_mode(if directory {
            3
        } else if exclusive_read {
            1
        } else {
            7
        });
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0x20000); // O_NOFOLLOW
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0x100); // O_NOFOLLOW
    }
    #[cfg(not(windows))]
    let _ = exclusive_read;
    let file = options
        .open(path)
        .with_context(|| StorageAccessPath(path.to_owned()))?;
    let metadata = file
        .metadata()
        .with_context(|| StorageAccessPath(path.to_owned()))?;
    ensure!(
        !reparse(&metadata),
        "拒绝访问链接或重解析点：{}",
        path.display()
    );
    ensure!(
        if directory {
            metadata.is_dir()
        } else {
            metadata.is_file()
        },
        "路径类型不正确：{}",
        path.display()
    );
    Ok(file)
}

pub(crate) fn guard_directories(path: &Path, create: bool) -> Result<Vec<File>> {
    ensure!(path.is_absolute(), "需要绝对目录路径");
    ensure!(
        !path.components().any(|c| matches!(c, Component::ParentDir)),
        "目录不能包含上级路径"
    );
    let mut guards = Vec::new();
    let mut ancestors: Vec<_> = path.ancestors().collect();
    ancestors.reverse();
    for directory in ancestors {
        if create
            && !directory
                .try_exists()
                .with_context(|| StorageAccessPath(directory.to_owned()))?
        {
            match std::fs::create_dir(directory) {
                Ok(()) => {}
                // Another asset worker can create this ancestor after the
                // existence check. The handle below still validates its type
                // and rejects links/reparse points before any child is used.
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(
                        anyhow::Error::from(error).context(StorageAccessPath(directory.to_owned()))
                    );
                }
            }
        }
        guards.push(plain_handle(directory, true, false)?);
    }
    Ok(guards)
}

fn same_path(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        a.to_string_lossy()
            .eq_ignore_ascii_case(&b.to_string_lossy())
    } else {
        a == b
    }
}

fn root(snapshot: &Snapshot) -> Result<&Path> {
    Path::new(&snapshot.data_path)
        .parent()
        .context("资料库目录不存在")
}

fn owned_path(snapshot: &Snapshot, note: &Note) -> Result<PathBuf> {
    ensure!(note.managed, "外部源文件不能删除");
    let id = note.id.as_bytes();
    ensure!(
        id.len() == 36
            && id.iter().enumerate().all(|(i, byte)| {
                if [8, 13, 18, 23].contains(&i) {
                    *byte == b'-'
                } else {
                    byte.is_ascii_digit() || (b'a'..=b'f').contains(byte)
                }
            }),
        "笔记身份不是规范 UUID"
    );
    let expected = root(snapshot)?
        .join("notes")
        .join(format!("{}.md", note.id));
    ensure!(
        same_path(Path::new(&note.path), &expected),
        "副本不属于当前资料库，原位置已保留"
    );
    Ok(expected)
}

fn visit_tree(
    root: &Path,
    relative: &Path,
    reject_links: bool,
    skip: &impl Fn(&Path) -> bool,
    count: &mut usize,
    visitor: &mut impl FnMut(&Path, &Path, File) -> Result<()>,
) -> Result<()> {
    ensure!(relative.components().count() <= 64, "资料库目录层数过深");
    let directory = root.join(relative);
    let _guard = plain_handle(&directory, true, false)?;
    for entry in std::fs::read_dir(&directory)? {
        let entry = entry?;
        *count += 1;
        ensure!(*count <= 100_000, "资料库文件数量超过统计限制");
        let path = entry.path();
        let relative = relative.join(entry.file_name());
        if skip(&relative) {
            continue;
        }
        let metadata = std::fs::symlink_metadata(&path)?;
        if reparse(&metadata) {
            ensure!(
                !reject_links,
                "资料库含有链接或重解析点，未迁移：{}",
                path.display()
            );
            continue;
        }
        if metadata.is_dir() {
            visit_tree(root, &relative, reject_links, skip, count, visitor)?;
        } else if metadata.is_file() {
            let file = plain_handle(&path, false, reject_links)?;
            visitor(&path, &relative, file)?;
        } else {
            ensure!(!reject_links, "资料库含有不支持的特殊文件");
        }
    }
    Ok(())
}

fn live_database_file(relative: &Path, database: &Path) -> bool {
    let Some(name) = database.file_name() else {
        return false;
    };
    if relative.components().count() != 1 {
        return false;
    }
    let name = name.to_string_lossy();
    let relative = relative.to_string_lossy();
    relative == name
        || ["-wal", "-shm", "-journal"]
            .iter()
            .any(|suffix| relative == format!("{name}{suffix}"))
}

fn runtime_lock_file(relative: &Path, database: &Path) -> bool {
    relative.components().count() == 1
        && relative.file_name() == database.with_extension("lock").file_name()
}

fn io_error(error: &anyhow::Error) -> Option<&std::io::Error> {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<std::io::Error>())
}

/// Keep operating-system details in the error chain, never in product messages.
pub fn user_message(error: &anyhow::Error) -> String {
    if let Some(io) = io_error(error) {
        return match io.kind() {
            std::io::ErrorKind::NotFound => {
                "文件或目录已不存在，请重新统计；笔记内容可从历史恢复。"
            }
            std::io::ErrorKind::PermissionDenied => {
                "无法访问文件：请关闭占用它的程序，并检查该目录的访问权限后重试。"
            }
            std::io::ErrorKind::AlreadyExists => "目标已有文件，请选择新的空目录，原文件仍保留。",
            std::io::ErrorKind::InvalidInput | std::io::ErrorKind::InvalidData => {
                "文件或路径格式无法读取，请检查目录设置后重试。"
            }
            _ => "暂时无法读取或处理该文件，请检查目录是否可用、关闭占用它的程序后重试。",
        }
        .into();
    }
    let message = error.to_string();
    if message.contains("重解析点") || message.contains("链接") {
        "此位置是链接或重解析点，已跳过以保护原文件；请选择普通目录。".into()
    } else if message.contains("不属于当前资料库") {
        "副本不属于当前资料库，原位置已保留；可查看历史或定位原文件。".into()
    } else if message.contains("外部源文件") {
        "这是外部源文件，空间管理不会删除它；原文件已保留。".into()
    } else if message.contains("UUID") || message.contains("路径类型") {
        "无法确认这是当前资料库的普通托管文件，已保留原位置；请查看历史或检查路径。".into()
    } else if message.contains("目录层数") || message.contains("统计限制") {
        "目录规模超过本次统计上限，已显示能够安全统计的部分。".into()
    } else if message
        .chars()
        .any(|character| ('\u{4e00}'..='\u{9fff}').contains(&character))
    {
        message
    } else {
        "无法完成这次存储操作，原文件已保留；请检查目录设置后重试。".into()
    }
}

fn path_key(path: &Path) -> String {
    let value = path.to_string_lossy();
    if cfg!(windows) {
        value.to_lowercase()
    } else {
        value.into_owned()
    }
}

fn issue(issues: &mut Vec<StorageIssue>, path: &Path, error: anyhow::Error) {
    // The existence of any entry means the totals are incomplete. Keep UI output bounded.
    if issues.len() < 100 {
        issues.push(StorageIssue {
            path: path.to_string_lossy().into_owned(),
            message: user_message(&error),
        });
    }
}

/// Read-only reporting tolerates individual failures. Migration intentionally uses
/// the separate fail-fast traversal above, so an incomplete copy is never accepted.
fn inspect_tree(
    root: &Path,
    relative: &Path,
    count: &mut usize,
    issues: &mut Vec<StorageIssue>,
    visitor: &mut impl FnMut(&Path, &Path, File) -> Result<()>,
) {
    let directory = root.join(relative);
    let opened = (|| -> Result<(File, std::fs::ReadDir)> {
        ensure!(
            relative.components().count() <= 64 && *count < 100_000,
            "资料库文件数量或目录层数超过统计限制"
        );
        Ok((
            plain_handle(&directory, true, false)?,
            std::fs::read_dir(&directory)?,
        ))
    })();
    let (_guard, entries) = match opened {
        Ok(value) => value,
        Err(error) => {
            issue(issues, &directory, error);
            return;
        }
    };
    for entry in entries {
        *count += 1;
        if *count > 100_000 {
            issue(
                issues,
                &directory,
                anyhow::anyhow!("资料库文件数量超过统计限制"),
            );
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                issue(issues, &directory, error.into());
                continue;
            }
        };
        let path = entry.path();
        let relative = relative.join(entry.file_name());
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                issue(issues, &path, error.into());
                continue;
            }
        };
        if reparse(&metadata) {
            issue(issues, &path, anyhow::anyhow!("拒绝访问链接或重解析点"));
        } else if metadata.is_dir() {
            inspect_tree(root, &relative, count, issues, visitor);
        } else if metadata.is_file() {
            if let Err(error) =
                plain_handle(&path, false, false).and_then(|file| visitor(&path, &relative, file))
            {
                issue(issues, &path, error);
            }
        } else {
            issue(
                issues,
                &path,
                anyhow::anyhow!("无法统计特殊文件，已保留原位置"),
            );
        }
    }
}

#[derive(Clone, Copy)]
enum Category {
    Database,
    Managed,
    Retained,
    Auxiliary,
    Backup,
    Image,
    Draft,
}

fn category(relative: &Path, snapshot: &Snapshot, owned: &BTreeSet<String>) -> Category {
    if live_database_file(relative, Path::new(&snapshot.data_path)) {
        Category::Database
    } else if owned.contains(&path_key(&root(snapshot).unwrap().join(relative))) {
        Category::Managed
    } else if relative.starts_with("notes")
        || relative.starts_with("retained")
        || relative
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with(".studyapp-"))
    {
        Category::Retained
    } else if relative.starts_with("images") {
        Category::Image
    } else if relative.starts_with("drafts") || relative == Path::new("note-draft.json") {
        Category::Draft
    } else if relative
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            ["sqlite", "sqlite3", "db"]
                .iter()
                .any(|expected| extension.eq_ignore_ascii_case(expected))
        })
    {
        Category::Backup
    } else {
        Category::Auxiliary
    }
}

fn indexed_images(snapshot: &Snapshot, relative: &Path, file: File) -> Result<Vec<PathBuf>> {
    let Some(tail) = relative
        .strip_prefix("image-index")
        .ok()
        .and_then(|path| path.to_str())
        .and_then(|path| path.strip_suffix(".json"))
    else {
        return Ok(Vec::new());
    };
    let tail = tail.replace('\\', "/");
    // Pending retained index files are not committed asset records.
    if tail.split('/').any(|component| component.starts_with('.')) {
        return Ok(Vec::new());
    }
    studyapp_sync::validate_asset_key(&format!("assets/{tail}"))
        .context("图片索引身份无效，已跳过")?;
    ensure!(file.metadata()?.len() <= 32_768, "图片索引过大，已跳过");
    let saved: PathBuf =
        serde_json::from_reader(file.take(32_769)).context("图片索引无法读取，已保留原文件")?;
    ensure!(
        saved.is_absolute()
            && !saved
                .components()
                .any(|component| matches!(component, Component::ParentDir))
            && saved.ends_with(Path::new(&tail)),
        "图片索引位置与资产身份不一致，已跳过"
    );
    let current = snapshot
        .settings
        .images
        .root
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or(root(snapshot)?.join("images"))
        .join(&tail);
    Ok(vec![saved, current])
}

pub fn inspect(snapshot: &Snapshot) -> Result<StorageReport> {
    let directory = root(snapshot)?;
    let mut report = StorageReport {
        data_path: snapshot.data_path.clone(),
        scanned_at: chrono::Utc::now().timestamp(),
        ..Default::default()
    };
    let mut issues = Vec::new();
    let root_guards = guard_directories(directory, false);
    let root_error = root_guards.as_ref().err();
    if let Some(error) = root_error {
        issue(&mut issues, directory, anyhow::anyhow!(user_message(error)));
    }
    let mut owned = BTreeSet::new();
    for note in snapshot.notes.iter().filter(|note| note.managed) {
        let inspected = (|| -> Result<(PathBuf, u64)> {
            let path = owned_path(snapshot, note)?;
            if let Some(error) = root_error {
                anyhow::bail!(user_message(error));
            }
            let _guard = plain_handle(&directory.join("notes"), true, false)?;
            let file = plain_handle(&path, false, false)?;
            Ok((path, file.metadata()?.len()))
        })();
        let (bytes, state, reason) = match inspected {
            Ok((path, bytes)) => {
                owned.insert(path_key(&path));
                (
                    bytes,
                    if note.detached {
                        CopyState::DetachedPresent
                    } else {
                        CopyState::Present
                    },
                    None,
                )
            }
            Err(error)
                if io_error(&error).is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound) =>
            {
                if note.detached {
                    (
                        0,
                        CopyState::Cleaned,
                        Some("副本已清理，历史与学习记录仍保留。".into()),
                    )
                } else {
                    (
                        0,
                        CopyState::Missing,
                        Some(
                            "本地副本缺失，可通过“历史与恢复”找回内容；请先确认文件是否被移动。"
                                .into(),
                        ),
                    )
                }
            }
            Err(error) => {
                let state = if io_error(&error).is_some() || root_error.is_some() {
                    CopyState::Unavailable
                } else {
                    CopyState::Unsafe
                };
                (0, state, Some(user_message(&error)))
            }
        };
        report.copies.push(ManagedCopy {
            note_id: note.id.clone(),
            title: note.title.clone(),
            path: note.path.clone(),
            bytes,
            state,
            deletable: matches!(state, CopyState::Present | CopyState::DetachedPresent),
            reason,
        });
    }
    let mut files = BTreeMap::new();
    let mut image_paths = BTreeSet::new();
    let mut image_records = Vec::new();
    if root_error.is_none() {
        inspect_tree(
            directory,
            Path::new(""),
            &mut 0,
            &mut issues,
            &mut |path, relative, file| {
                if runtime_lock_file(relative, Path::new(&snapshot.data_path)) {
                    return Ok(());
                }
                files.insert(
                    path_key(path),
                    (file.metadata()?.len(), category(relative, snapshot, &owned)),
                );
                if relative.starts_with("image-index") {
                    let paths = indexed_images(snapshot, relative, file)?;
                    if !paths.is_empty() {
                        image_paths.extend(paths.iter().cloned());
                        image_records.push(paths);
                    }
                }
                Ok(())
            },
        );
        let mut locations = BTreeSet::new();
        for path in image_paths {
            let key = path_key(&path);
            if let Some((_, category)) = files.get_mut(&key) {
                *category = Category::Image;
                if let Some(location) = path.parent().and_then(Path::parent) {
                    locations.insert(location.to_string_lossy().into_owned());
                }
                continue;
            }
            let inspected = (|| -> Result<u64> {
                let _guards = guard_directories(path.parent().context("图片目录不存在")?, false)?;
                Ok(plain_handle(&path, false, false)?.metadata()?.len())
            })();
            match inspected {
                Ok(bytes) => {
                    files.insert(key, (bytes, Category::Image));
                    if let Some(location) = path.parent().and_then(Path::parent) {
                        locations.insert(location.to_string_lossy().into_owned());
                    }
                }
                // An index may retain a previous location or precede cloud-only caching.
                Err(error)
                    if io_error(&error)
                        .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound) => {}
                Err(error) => issue(&mut issues, &path, error),
            }
        }
        for paths in image_records {
            if !paths.iter().any(|path| files.contains_key(&path_key(path))) {
                issue(
                    &mut issues,
                    &paths[0],
                    anyhow::anyhow!(
                        "已登记的图片在当前和历史位置均无法读取；请检查图片目录或云端配置。"
                    ),
                );
            }
        }
        if files
            .values()
            .any(|(_, category)| matches!(category, Category::Image))
            && std::fs::symlink_metadata(directory.join("images"))
                .is_ok_and(|metadata| metadata.is_dir() && !reparse(&metadata))
        {
            locations.insert(directory.join("images").to_string_lossy().into_owned());
        }
        report.image_locations = locations.into_iter().collect();
    }
    for (bytes, category) in files.into_values() {
        let total = match category {
            Category::Database => &mut report.database_bytes,
            Category::Managed => &mut report.managed_bytes,
            Category::Retained => &mut report.retained_bytes,
            Category::Auxiliary => &mut report.auxiliary_bytes,
            Category::Backup => &mut report.backup_bytes,
            Category::Image => &mut report.image_bytes,
            Category::Draft => &mut report.draft_bytes,
        };
        *total = total.saturating_add(bytes);
    }
    report.incomplete = !issues.is_empty();
    report.issues = issues;
    report.prepare_presentation();
    Ok(report)
}

pub fn delete_managed_copy(engine: &mut Engine, note_id: &str, now: i64) -> Result<StorageReport> {
    let snapshot = engine.snapshot();
    let note = snapshot
        .notes
        .iter()
        .find(|note| note.id == note_id)
        .context("找不到笔记")?;
    // Check ownership before considering absence: an absent foreign path is never ours.
    let path = owned_path(&snapshot, note)?;
    let _root_guards = guard_directories(root(&snapshot)?, false)?;
    let parent = plain_handle(path.parent().context("副本目录不存在")?, true, false);
    let inspected = match &parent {
        Ok(_) => plain_handle(&path, false, false).map(Some),
        Err(error)
            if io_error(error).is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound) =>
        {
            Ok(None)
        }
        Err(error) => anyhow::bail!(user_message(error)),
    };
    let file = match inspected {
        Ok(file) => file,
        Err(error)
            if io_error(&error).is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound) =>
        {
            None
        }
        Err(error) => return Err(error.context("副本不可删除，原文件已保留")),
    };
    let existed = file.is_some();
    engine.apply(
        Command::UnmanageNote {
            note_id: note_id.into(),
        },
        now,
    )?;
    drop(file);
    // Exactly one validated owned file; parent handles remain pinned on Windows.
    let cleanup = if existed {
        match std::fs::remove_file(&path) {
            Ok(()) => CopyCleanupOutcome::Removed,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                CopyCleanupOutcome::AlreadyAbsent
            }
            Err(error) => CopyCleanupOutcome::Retained {
                message: format!(
                    "已停止管理，但副本尚未清理。{} 历史与学习记录仍保留，可稍后重试。",
                    user_message(&error.into())
                ),
            },
        }
    } else {
        CopyCleanupOutcome::AlreadyAbsent
    };
    let mut report = inspect(&engine.snapshot())?;
    report.cleanup = Some(cleanup);
    Ok(report)
}

pub fn prepare_migration(engine: &mut Engine, target: &Path, now: i64) -> Result<PathBuf> {
    let snapshot = engine.snapshot();
    let source = root(&snapshot)?;
    let _source_guards = guard_directories(source, false)?;
    ensure!(target.is_absolute(), "迁移目标需要绝对路径");
    ensure!(
        !target
            .components()
            .any(|c| matches!(c, Component::ParentDir)),
        "迁移路径不能包含上级路径"
    );
    let mut existing = target;
    let mut tail = Vec::new();
    while !existing.try_exists()? {
        tail.push(existing.file_name().context("迁移目录无效")?.to_owned());
        existing = existing.parent().context("迁移目录无效")?;
    }
    let _existing_guards = guard_directories(existing, false)?;
    let mut resolved = std::fs::canonicalize(existing)?;
    for component in tail.iter().rev() {
        resolved.push(component);
    }
    ensure!(
        !resolved.starts_with(source),
        "迁移目标不能位于当前资料库内"
    );
    let _target_guards = guard_directories(&resolved, true)?;
    ensure!(
        std::fs::read_dir(&resolved)?.next().is_none(),
        "迁移目标必须是空目录"
    );
    let database = resolved.join("studyapp.sqlite3");
    engine.checkpoint()?;
    engine.apply(Command::Backup(database.clone()), now)?;
    let skip = |relative: &Path| {
        live_database_file(relative, Path::new(&snapshot.data_path))
            || runtime_lock_file(relative, Path::new(&snapshot.data_path))
    };
    visit_tree(
        source,
        Path::new(""),
        true,
        &skip,
        &mut 0,
        &mut |_, relative, mut file| {
            let destination = resolved.join(relative);
            let _guards = guard_directories(destination.parent().context("迁移目录无效")?, true)?;
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&destination)?;
            file.rewind()?;
            std::io::copy(&mut file, &mut output)?;
            output.sync_all()?;
            drop(output);
            let mut copied = plain_handle(&destination, false, true)?;
            file.rewind()?;
            let mut source_bytes = [0; 64 * 1024];
            let mut copied_bytes = [0; 64 * 1024];
            loop {
                let count = file.read(&mut source_bytes)?;
                if count == 0 {
                    ensure!(
                        copied.read(&mut copied_bytes[..1])? == 0,
                        "迁移文件校验失败"
                    );
                    break;
                }
                copied.read_exact(&mut copied_bytes[..count])?;
                ensure!(
                    source_bytes[..count] == copied_bytes[..count],
                    "迁移文件校验失败"
                );
            }
            Ok(())
        },
    )?;
    Ok(database)
}

pub fn persist_location(config: &Path, directory: &Path) -> Result<()> {
    let parent = config.parent().context("启动配置目录不存在")?;
    let _guards = guard_directories(parent, true)?;
    if let Ok(metadata) = std::fs::symlink_metadata(config) {
        ensure!(
            !reparse(&metadata) && metadata.is_file(),
            "启动配置必须是普通文件"
        );
    }
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let temporary = parent.join(format!(
        ".location-{}-{}-{}.next",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    output.write_all(&serde_json::to_vec(directory)?)?;
    output.sync_all()?;
    drop(output);
    std::fs::rename(&temporary, config).context("无法更新启动位置；当前资料库未切换")?;
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use studyapp_core::Command;

    fn library() -> (PathBuf, Engine) {
        let root = tempfile::tempdir().unwrap().keep();
        let engine = Engine::open(&root.join("studyapp.sqlite3"), 1000).unwrap();
        (root, engine)
    }

    fn create(engine: &mut Engine) -> studyapp_core::Note {
        engine
            .apply(
                Command::CreateNote {
                    title: "owned copy".into(),
                    content: "managed content".into(),
                },
                1001,
            )
            .unwrap();
        engine.snapshot().notes.last().unwrap().clone()
    }

    #[test]
    fn migration_copies_categorized_preferences_and_counts_them_as_auxiliary_data() {
        let (root, mut engine) = library();
        let database = root.join("studyapp.sqlite3");
        let (mut store, mut runtime) =
            crate::preferences::PreferencesStore::open(&database, &mut engine).unwrap();
        let before = store.snapshot();
        let mut settings = before.settings.clone();
        settings.show_file_paths = true;
        store
            .save(
                crate::preferences::SavePreferences {
                    expected_revision: before.revision,
                    settings,
                    shortcuts: before.shortcuts,
                    sync: before.sync,
                    password: None,
                    image_secret: Default::default(),
                },
                &mut engine,
                &mut runtime,
            )
            .unwrap();
        let manifest = std::fs::read(root.join("settings/manifest.json")).unwrap();
        let report = inspect(&engine.snapshot()).unwrap();
        assert!(report.auxiliary_bytes > manifest.len() as u64);
        let destination = tempfile::tempdir().unwrap().keep().join("migrated");
        let migrated_database = prepare_migration(&mut engine, &destination, 1001).unwrap();
        let mut migrated = Engine::open(&migrated_database, 1002).unwrap();
        let (copied, _) =
            crate::preferences::PreferencesStore::open(&migrated_database, &mut migrated).unwrap();
        assert_eq!(copied.snapshot().revision, store.snapshot().revision);
        assert!(migrated.snapshot().settings.show_file_paths);
        assert_eq!(
            std::fs::read(destination.join("settings/manifest.json")).unwrap(),
            manifest
        );
        assert_eq!(
            std::fs::read(root.join("settings/manifest.json")).unwrap(),
            manifest
        );
    }

    #[test]
    fn report_counts_owned_copies_backups_and_retained_data_without_external_sources() {
        let (root, mut engine) = library();
        let note = create(&mut engine);
        let external_root = tempfile::tempdir().unwrap().keep();
        let external = external_root.join("external.md");
        std::fs::write(&external, "never count or remove external source").unwrap();
        engine.apply(Command::Import(vec![external]), 1002).unwrap();
        std::fs::write(root.join("note-draft.json"), "null").unwrap();
        std::fs::write(root.join("notes/unknown-orphan.md"), "retained").unwrap();
        let backup = root.join("backup.sqlite3");
        engine.apply(Command::Backup(backup.clone()), 1003).unwrap();
        let report = inspect(&engine.snapshot()).unwrap();
        assert_eq!(report.data_path, engine.snapshot().data_path);
        assert!(report.database_bytes > 0);
        assert_eq!(report.managed_bytes, "managed content".len() as u64);
        assert!(report.retained_bytes >= "retained".len() as u64);
        assert_eq!(
            report.backup_bytes,
            std::fs::metadata(backup).unwrap().len()
        );
        assert_eq!(report.draft_bytes, 4);
        assert_eq!(report.auxiliary_bytes, 0);
        assert_eq!(report.copies.len(), 1);
        assert_eq!(report.copies[0].note_id, note.id);
        assert!(report.copies[0].deletable);
        assert_eq!(report.presentation.copies.len(), 1);
        assert!(report.presentation.copy_by_id(&note.id).unwrap().deletable);
        assert!(
            report
                .presentation
                .logical_label
                .starts_with("已统计逻辑大小 ")
        );
    }

    #[test]
    fn delete_owned_copy_detaches_and_preserves_history_and_external_files() {
        let (root, mut engine) = library();
        let note = create(&mut engine);
        let external = root.join("external.md");
        std::fs::write(&external, "external").unwrap();
        engine
            .apply(Command::Import(vec![external.clone()]), 1002)
            .unwrap();
        let external_id = engine.snapshot().notes[1].id.clone();
        assert!(delete_managed_copy(&mut engine, &external_id, 1003).is_err());
        assert_eq!(std::fs::read_to_string(&external).unwrap(), "external");
        assert!(!engine.snapshot().notes[1].detached);
        engine
            .apply(
                Command::Manual {
                    started_at: 900,
                    minutes: 1,
                    summary: "Keep this learning record".into(),
                    note_ids: vec![note.id.clone()],
                    version_ids: None,
                },
                1003,
            )
            .unwrap();
        let before = engine.snapshot();
        assert!(!before.sessions.is_empty());
        assert!(!before.reviews.is_empty());
        let report = delete_managed_copy(&mut engine, &note.id, 1004).unwrap();
        assert!(!Path::new(&note.path).exists());
        assert!(engine.snapshot().notes[0].detached);
        assert_eq!(engine.snapshot().versions.len(), 2);
        let after = engine.snapshot();
        assert_eq!(
            serde_json::to_value(&after.versions).unwrap(),
            serde_json::to_value(&before.versions).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&after.sessions).unwrap(),
            serde_json::to_value(&before.sessions).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&after.reviews).unwrap(),
            serde_json::to_value(&before.reviews).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&after.review_events).unwrap(),
            serde_json::to_value(&before.review_events).unwrap()
        );
        assert!(!report.copies[0].deletable);
        assert_eq!(report.copies[0].state, CopyState::Cleaned);
        let displayed_copy = report.presentation.copy_by_id(&note.id).unwrap();
        assert_eq!(displayed_copy.state, CopyState::Cleaned);
        assert!(!displayed_copy.deletable);
        assert_eq!(report.cleanup, Some(CopyCleanupOutcome::Removed));
        assert_eq!(
            report.copies[0].reason.as_deref(),
            Some("副本已清理，历史与学习记录仍保留。")
        );
        let refreshed = inspect(&engine.snapshot()).unwrap();
        assert_eq!(refreshed.copies[0].reason, report.copies[0].reason);
        assert_eq!(std::fs::read_to_string(external).unwrap(), "external");
    }

    #[test]
    fn repeated_cleanup_of_verified_owned_copy_is_safe() {
        let (_, mut engine) = library();
        let note = create(&mut engine);
        delete_managed_copy(&mut engine, &note.id, 1002).unwrap();
        let history = engine.snapshot().versions;
        let report = delete_managed_copy(&mut engine, &note.id, 1003).unwrap();
        assert_eq!(report.cleanup, Some(CopyCleanupOutcome::AlreadyAbsent));
        assert_eq!(
            report.copies[0].reason.as_deref(),
            Some("副本已清理，历史与学习记录仍保留。")
        );
        assert_eq!(engine.snapshot().versions.len(), history.len());
        assert_eq!(engine.snapshot().versions[0].content, history[0].content);
    }

    #[test]
    fn missing_owned_directory_allows_safe_already_absent_result() {
        let (root, mut engine) = library();
        let note = create(&mut engine);
        let source = root.join("notes");
        let retained = root.join("retained-notes");
        assert!(source.starts_with(&root) && retained.starts_with(&root));
        std::fs::rename(&source, &retained).unwrap();
        let report = delete_managed_copy(&mut engine, &note.id, 1003).unwrap();
        assert_eq!(report.cleanup, Some(CopyCleanupOutcome::AlreadyAbsent));
        assert_eq!(report.copies[0].state, CopyState::Cleaned);
        assert_eq!(
            std::fs::read_to_string(retained.join(format!("{}.md", note.id))).unwrap(),
            "managed content"
        );
    }

    #[test]
    fn missing_active_copy_offers_history_recovery_without_os_error() {
        let (root, mut engine) = library();
        let note = create(&mut engine);
        std::fs::rename(&note.path, root.join("retained-copy.md")).unwrap();
        let report = inspect(&engine.snapshot()).unwrap();
        assert!(!report.copies[0].deletable);
        assert_eq!(report.copies[0].state, CopyState::Missing);
        assert_eq!(
            report.copies[0].reason.as_deref(),
            Some("本地副本缺失，可通过“历史与恢复”找回内容；请先确认文件是否被移动。")
        );
        assert!(!engine.snapshot().notes[0].detached);
        assert_eq!(engine.snapshot().versions[0].content, "managed content");
    }

    #[cfg(windows)]
    #[test]
    fn failed_single_copy_deletion_keeps_file_detached_and_can_retry() {
        use std::os::windows::fs::OpenOptionsExt;
        let (_, mut engine) = library();
        let note = create(&mut engine);
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&note.path)
            .unwrap();
        let report = delete_managed_copy(&mut engine, &note.id, 1002).unwrap();
        assert!(matches!(
            report.cleanup,
            Some(CopyCleanupOutcome::Retained { .. })
        ));
        assert_eq!(report.copies[0].state, CopyState::DetachedPresent);
        let notice = report.cleanup.unwrap().notice().unwrap_err();
        assert!(notice.contains("尚未清理"));
        assert!(!notice.contains("os error"));
        assert!(engine.snapshot().notes[0].detached);
        assert_eq!(
            std::fs::read_to_string(&note.path).unwrap(),
            "managed content"
        );
        drop(held);
        delete_managed_copy(&mut engine, &note.id, 1003).unwrap();
        assert!(!Path::new(&note.path).exists());
    }

    #[test]
    fn custom_indexed_images_count_once_without_scanning_unrelated_files() {
        let (root, engine) = library();
        let images = tempfile::tempdir().unwrap().keep();
        let group = "11111111-1111-4111-8111-111111111111";
        let folder = images.join(group);
        std::fs::create_dir(&folder).unwrap();
        let asset = folder.join("known.png");
        std::fs::write(&asset, b"image").unwrap();
        std::fs::write(images.join("unrelated.bin"), [0; 200]).unwrap();
        let indexes = root.join("image-index").join(group);
        std::fs::create_dir_all(&indexes).unwrap();
        std::fs::write(
            indexes.join("known.png.json"),
            serde_json::to_vec(&asset).unwrap(),
        )
        .unwrap();
        let mut snapshot = engine.snapshot();
        snapshot.settings.images.root = Some(images.to_string_lossy().into_owned());
        let report = inspect(&snapshot).unwrap();
        assert_eq!(report.image_bytes, 5);
        assert_eq!(
            report.image_locations,
            vec![images.to_string_lossy().into_owned()]
        );
        assert!(!report.incomplete);
        assert_eq!(std::fs::read(&asset).unwrap(), b"image");
        assert_eq!(
            std::fs::read(images.join("unrelated.bin")).unwrap(),
            [0; 200]
        );
        // Changing the configured root retains the indexed historical location.
        snapshot.settings.images.root =
            Some(root.join("not-created").to_string_lossy().into_owned());
        assert_eq!(inspect(&snapshot).unwrap().image_bytes, 5);
    }

    #[test]
    fn invalid_image_index_preserves_partial_totals_and_foreign_file() {
        let (root, engine) = library();
        let outside = tempfile::tempdir().unwrap().keep().join("foreign.png");
        std::fs::write(&outside, b"keep foreign data").unwrap();
        let index = root.join("image-index/11111111-1111-4111-8111-111111111111");
        std::fs::create_dir_all(&index).unwrap();
        std::fs::write(
            index.join("known.png.json"),
            serde_json::to_vec(&outside).unwrap(),
        )
        .unwrap();
        let report = inspect(&engine.snapshot()).unwrap();
        assert!(report.database_bytes > 0);
        assert!(report.incomplete);
        assert_eq!(report.image_bytes, 0);
        assert!(report.issues[0].message.contains("不一致"));
        assert_eq!(std::fs::read(&outside).unwrap(), b"keep foreign data");
    }

    #[test]
    fn missing_registered_image_marks_scan_incomplete_without_os_error() {
        let (root, engine) = library();
        let group = "11111111-1111-4111-8111-111111111111";
        let asset = root.join("images").join(group).join("missing.png");
        let indexes = root.join("image-index").join(group);
        std::fs::create_dir_all(&indexes).unwrap();
        std::fs::write(
            indexes.join("missing.png.json"),
            serde_json::to_vec(&asset).unwrap(),
        )
        .unwrap();
        let report = inspect(&engine.snapshot()).unwrap();
        assert!(report.incomplete);
        assert!(report.database_bytes > 0);
        assert_eq!(report.image_bytes, 0);
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.message.contains("图片") && issue.message.contains("无法读取"))
        );
        assert!(
            report
                .issues
                .iter()
                .all(|issue| !issue.message.contains("os error"))
        );
    }

    #[test]
    fn permission_errors_have_actionable_diagnostic_without_os_details() {
        let error = anyhow::Error::from(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "private raw OS detail",
        ));
        let message = user_message(&error);
        assert!(message.contains("权限"));
        assert!(message.contains("重试"));
        assert!(!message.contains("private raw"));
        let message = user_message(&anyhow::anyhow!("拒绝访问链接或重解析点"));
        assert!(message.contains("普通目录"));
    }

    #[test]
    fn multiple_drafts_are_counted_and_absent_folder_is_normal() {
        let (root, engine) = library();
        let initial = inspect(&engine.snapshot()).unwrap();
        assert_eq!(initial.draft_bytes, 0);
        assert!(!initial.incomplete);
        std::fs::create_dir(root.join("drafts")).unwrap();
        std::fs::write(
            root.join("drafts/11111111-1111-4111-8111-111111111111.json"),
            b"draft one",
        )
        .unwrap();
        std::fs::write(
            root.join("drafts/22222222-2222-4222-8222-222222222222.json"),
            b"draft two",
        )
        .unwrap();
        assert_eq!(inspect(&engine.snapshot()).unwrap().draft_bytes, 18);
    }

    #[test]
    fn migration_copies_consistent_library_and_auxiliary_files_without_changing_source() {
        let (root, mut engine) = library();
        let note = create(&mut engine);
        let draft = r#"{"note_id":null,"expected_hash":null,"title":"draft","content":"latest"}"#;
        std::fs::write(root.join("note-draft.json"), draft).unwrap();
        std::fs::write(root.join("sync-config.json"), "{}").unwrap();
        std::fs::create_dir(root.join("retained")).unwrap();
        std::fs::write(root.join("retained/user.lock"), "user content").unwrap();
        let target_root = tempfile::tempdir().unwrap().keep();
        let target = target_root.join("migrated");
        let database = prepare_migration(&mut engine, &target, 1002).unwrap();
        let migrated = Engine::open(&database, 1003).unwrap();
        let snapshot = migrated.snapshot();
        assert_eq!(snapshot.notes[0].id, note.id);
        assert_eq!(snapshot.versions[0].content, "managed content");
        assert!(
            Path::new(&snapshot.notes[0].path).starts_with(std::fs::canonicalize(&target).unwrap())
        );
        assert_eq!(
            std::fs::read_to_string(target.join("note-draft.json")).unwrap(),
            draft
        );
        assert_eq!(
            std::fs::read_to_string(target.join("sync-config.json")).unwrap(),
            "{}"
        );
        assert_eq!(
            std::fs::read_to_string(target.join("retained/user.lock")).unwrap(),
            "user content"
        );
        assert_eq!(
            std::fs::read_to_string(&note.path).unwrap(),
            "managed content"
        );
        assert!(root.join("studyapp.sqlite3").is_file());
        let config = target_root.join("config/location.json");
        persist_location(&config, &target).unwrap();
        let remembered: PathBuf = serde_json::from_slice(&std::fs::read(config).unwrap()).unwrap();
        assert_eq!(remembered, target);
    }

    #[test]
    fn migration_refuses_nonempty_or_nested_destination_and_preserves_source() {
        let (root, mut engine) = library();
        let note = create(&mut engine);
        let target = tempfile::tempdir().unwrap().keep();
        std::fs::write(target.join("user.txt"), "keep").unwrap();
        assert!(prepare_migration(&mut engine, &target, 1002).is_err());
        assert_eq!(
            std::fs::read_to_string(target.join("user.txt")).unwrap(),
            "keep"
        );
        assert!(prepare_migration(&mut engine, &root.join("nested"), 1003).is_err());
        assert_eq!(
            std::fs::read_to_string(&note.path).unwrap(),
            "managed content"
        );
        assert!(!engine.snapshot().notes[0].detached);
    }

    #[test]
    fn detached_copy_from_another_library_cannot_be_deleted() {
        let (source, mut original) = library();
        let note = create(&mut original);
        original
            .apply(
                Command::UnmanageNote {
                    note_id: note.id.clone(),
                },
                1002,
            )
            .unwrap();
        let backup = source.join("detached.sqlite3");
        original
            .apply(Command::Backup(backup.clone()), 1003)
            .unwrap();
        let (_, mut restored) = library();
        restored.apply(Command::Restore(backup), 1004).unwrap();
        assert!(delete_managed_copy(&mut restored, &note.id, 1005).is_err());
        assert_eq!(
            std::fs::read_to_string(&note.path).unwrap(),
            "managed content"
        );
        let report = inspect(&restored.snapshot()).unwrap();
        assert!(!report.copies[0].deletable);
        assert!(
            report.copies[0]
                .reason
                .as_ref()
                .unwrap()
                .contains("不属于当前资料库")
        );
    }

    #[cfg(windows)]
    #[test]
    fn junctions_are_not_counted_copied_or_used_to_delete_outside_files() {
        let (root, mut engine) = library();
        let note = create(&mut engine);
        let external = tempfile::tempdir().unwrap().keep();
        let victim = external.join(format!("{}.md", note.id));
        std::fs::write(&victim, "external victim").unwrap();
        let original_notes = root.join("notes");
        let retained_notes = root.join("retained-notes");
        assert!(original_notes.starts_with(&root) && retained_notes.starts_with(&root));
        std::fs::rename(&original_notes, &retained_notes).unwrap();
        let output = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&original_notes)
            .arg(&external)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report = inspect(&engine.snapshot()).unwrap();
        assert_eq!(report.managed_bytes, 0);
        assert!(!report.copies[0].deletable);
        assert!(delete_managed_copy(&mut engine, &note.id, 1002).is_err());
        assert!(!engine.snapshot().notes[0].detached);
        let target_root = tempfile::tempdir().unwrap().keep();
        let error = prepare_migration(&mut engine, &target_root.join("target"), 1003).unwrap_err();
        assert!(error.to_string().contains("重解析点"), "{error:#}");
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "external victim");
        assert!(!target_root.join("target/notes").exists());
    }

    #[cfg(windows)]
    #[test]
    fn location_publish_failure_preserves_previous_location_bytes() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = tempfile::tempdir().unwrap().keep();
        let config = root.join("location.json");
        let previous = serde_json::to_vec(&root.join("original")).unwrap();
        std::fs::write(&config, &previous).unwrap();
        let held = OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&config)
            .unwrap();
        assert!(persist_location(&config, &root.join("new")).is_err());
        assert_eq!(std::fs::read(&config).unwrap(), previous);
        drop(held);
        persist_location(&config, &root.join("new")).unwrap();
        let current: PathBuf = serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
        assert_eq!(current, root.join("new"));
    }
}
