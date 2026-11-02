// The fixed journal retains the original authority until the verified database opens.
use crate::storage_backend::{guard_directories, plain_handle};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
use studyapp_core::{Command, Engine};
use studyapp_desktop::app_paths::{AppPathConfig, AppPaths, validate_config, validate_path};
use studyapp_ui::transfer_progress::TransferProgress;

static STARTUP_PENDING: AtomicBool = AtomicBool::new(false);
static CANCEL: AtomicBool = AtomicBool::new(false);
pub(crate) fn reset_cancel() {
    CANCEL.store(false, Ordering::Release)
}
pub(crate) fn cancel() {
    CANCEL.store(true, Ordering::Release)
}
fn check_cancel() -> Result<()> {
    ensure!(!CANCEL.load(Ordering::Acquire), "已取消迁移，原位置仍有效");
    Ok(())
}
pub fn startup_pending() -> bool {
    STARTUP_PENDING.load(Ordering::Acquire)
}
pub fn set_startup_pending(value: bool) {
    STARTUP_PENDING.store(value, Ordering::Release);
}

fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let folder = path.parent().context("配置目录无效")?;
    let _guards = guard_directories(folder, true)?;
    validate_path(path)?;
    let next = folder.join(format!(".migration-{}.next", uuid::Uuid::new_v4()));
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&next)?;
    serde_json::to_writer(&mut f, value)?;
    f.sync_all()?;
    drop(f);
    // std::fs::rename uses overwrite semantics on Windows (MoveFileExW).
    fs::rename(next, path)?;
    #[cfg(unix)]
    File::open(folder)?.sync_all()?;
    Ok(())
}
fn publish_new(source: &Path, target: &Path) -> Result<()> {
    #[cfg(windows)]
    let success = {
        use std::os::windows::ffi::OsStrExt;
        let src: Vec<_> = source.as_os_str().encode_wide().chain(Some(0)).collect();
        let dst: Vec<_> = target.as_os_str().encode_wide().chain(Some(0)).collect();
        // No replace flag: another directory entry must survive a publication race.
        unsafe {
            windows_sys::Win32::Storage::FileSystem::MoveFileExW(
                src.as_ptr(),
                dst.as_ptr(),
                windows_sys::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH,
            ) != 0
        }
    };
    #[cfg(unix)]
    let success = {
        use std::os::unix::ffi::OsStrExt;
        let src = std::ffi::CString::new(source.as_os_str().as_bytes())?;
        let dst = std::ffi::CString::new(target.as_os_str().as_bytes())?;
        #[cfg(target_os = "linux")]
        let rc = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                src.as_ptr(),
                libc::AT_FDCWD,
                dst.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        #[cfg(target_os = "macos")]
        let rc = unsafe { libc::renamex_np(src.as_ptr(), dst.as_ptr(), libc::RENAME_EXCL) };
        rc == 0
    };
    if !success {
        return Err(std::io::Error::last_os_error().into());
    }
    #[cfg(unix)]
    File::open(target.parent().context("目标目录无效")?)?.sync_all()?;
    Ok(())
}
fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    validate_path(path)?;
    if !path.try_exists()? {
        return Ok(None);
    }
    let _guards = guard_directories(path.parent().context("配置目录无效")?, false)?;
    let mut bytes = Vec::new();
    plain_handle(path, false, true)?
        .take(65537)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 65536, "迁移配置过大");
    Ok(Some(bytes))
}
fn publish_config(locator: &Path, config: &AppPathConfig) -> Result<()> {
    atomic_json(locator, &validate_config(config)?)
}
fn lease(locator: &Path) -> Result<File> {
    let parent = locator.parent().context("位置配置目录无效")?;
    let _guards = guard_directories(parent, true)?;
    let path = parent.join("app-migration.lock");
    validate_path(&path)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    lock.try_lock().context("另一实例正在更改应用位置")?;
    Ok(lock)
}
pub fn remember_library(legacy: &Path, directory: &Path) -> Result<()> {
    let locator = legacy.with_file_name("storage-location.json");
    let _lease = lease(&locator)?;
    if let Some(bytes) = read_optional(&locator)? {
        let mut config: AppPathConfig = serde_json::from_slice(&bytes)?;
        config.library = Some(directory.into());
        return publish_config(&locator, &config);
    }
    crate::storage_backend::persist_location(legacy, directory)
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct Journal {
    schema: u32,
    id: String,
    source: PathBuf,
    destination: PathBuf,
    previous: Option<Vec<u8>>,
    phase: Phase,
    #[serde(default)]
    backup_identity: Option<(u64, u128)>,
    #[serde(default)]
    backup_digest: Option<[u8; 32]>,
    #[serde(default)]
    stage_identity: Option<(u64, u128)>,
    #[serde(default)]
    cleanup_digest: Option<[u8; 32]>,
    #[serde(default)]
    profile_copies: Vec<ProfileCopy>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileCopy {
    target: PathBuf,
    identity: (u64, u128),
    digest: [u8; 32],
}
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq)]
enum Phase {
    Copying,
    AwaitingRestart,
    Publishing,
    Complete,
    Aborted,
}
fn journal_file(paths: &AppPaths) -> PathBuf {
    paths.locator.with_file_name("app-migration.json")
}
fn journal(paths: &AppPaths) -> Result<Option<Journal>> {
    let Some(bytes) = read_optional(&journal_file(paths))? else {
        return Ok(None);
    };
    let j: Journal = serde_json::from_slice(&bytes)?;
    ensure!(
        j.schema == 1 && uuid::Uuid::parse_str(&j.id).is_ok(),
        "迁移日志无效"
    );
    validate_path(&j.source)?;
    validate_path(&j.destination)?;
    Ok(Some(j))
}
pub(crate) fn ensure_no_pending() -> Result<()> {
    ensure!(
        pending(&crate::app_usage::runtime_paths()?)?.is_none(),
        "应用目录迁移尚未完成，请先启动应用完成恢复"
    );
    Ok(())
}
pub fn pending(paths: &AppPaths) -> Result<Option<PathBuf>> {
    let Some(j) = journal(paths)? else {
        return Ok(None);
    };
    if j.phase == Phase::Aborted {
        return Ok(None);
    }
    let target = j.destination.join("library");
    if paths.configured_library() == Some(&target)
        && paths.custom_root.as_ref() == Some(&j.destination)
    {
        ensure!(
            plain_handle(&target.join("studyapp.sqlite3"), false, false).is_ok(),
            "迁移后的资料库不可用，请重新连接目标磁盘"
        );
        return Ok(None);
    }
    if j.phase == Phase::Complete {
        return Ok(None);
    }
    ensure!(
        read_optional(&paths.locator)? == j.previous,
        "位置配置已变化，不能恢复迁移"
    );
    ensure!(
        fs::canonicalize(paths.resolve_library(None)?)? == j.source,
        "原资料库已变化，不能恢复迁移"
    );
    Ok(Some(j.destination))
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    source: PathBuf,
    size: u64,
    identity: (u64, u128),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyRecord {
    source: PathBuf,
    target: PathBuf,
    identity: (u64, u128),
    size: u64,
    digest: [u8; 32],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    target_digest: Option<[u8; 32]>,
    old_library: PathBuf,
}
impl CopyRecord {
    pub fn source(&self) -> &Path {
        &self.source
    }
    pub fn size(&self) -> u64 {
        self.size
    }
}
// A saved receipt never grants permission: the user sees the remaining exact
// paths again, and deletion revalidates each open file after confirmation.
pub fn remaining_cleanup(
    paths: &AppPaths,
    snapshot: &studyapp_core::Snapshot,
) -> Result<Vec<CopyRecord>> {
    let (_, records) = recorded_inventory(paths, snapshot)?;
    let mut checked_parents = std::collections::BTreeSet::new();
    let mut remaining = Vec::new();
    for r in records {
        if let Some(parent) = r.source.parent()
            && checked_parents.insert(parent.to_owned())
        {
            recover_removal_parent(parent)?;
        }
        if r.source.try_exists()? {
            remaining.push(r)
        }
    }
    Ok(remaining)
}
// Statistics must not recover claims, repair pointers or traverse old roots.
pub fn inventory_paths(
    paths: &AppPaths,
    snapshot: &studyapp_core::Snapshot,
) -> Result<(Option<PathBuf>, Vec<PathBuf>)> {
    let (stage, records) = recorded_inventory(paths, snapshot)?;
    let mut files = std::collections::BTreeSet::new();
    for r in records {
        let parent = r.source.parent().context("旧副本父目录无效")?;
        for p in [&r.source, &parent.join(".studyapp-removal-v1/claimed")] {
            validate_path(p)?;
            if p.try_exists()? {
                let _guards = guard_directories(p.parent().context("旧副本文件位置无效")?, false)?;
                if crate::app_usage::identity(&plain_handle(p, false, false)?)? == r.identity {
                    files.insert(p.to_owned());
                }
            }
        }
    }
    Ok((stage, files.into_iter().collect()))
}
fn recorded_inventory(
    paths: &AppPaths,
    snapshot: &studyapp_core::Snapshot,
) -> Result<(Option<PathBuf>, Vec<CopyRecord>)> {
    let Some(j) = journal(paths)? else {
        return Ok((None, vec![]));
    };
    let active = Path::new(&snapshot.data_path)
        .parent()
        .context("资料库目录无效")?;
    if paths.custom_root.as_ref() != Some(&j.destination) && canon(active)? != j.source {
        return Ok((None, vec![]));
    }
    let stage = j.destination.join(format!(".studyapp-migration-{}", j.id));
    let _guards = guard_directories(&stage, false)?;
    ensure!(
        Some(crate::app_usage::identity(&plain_handle(
            &stage, true, false
        )?)?)
            == j.stage_identity,
        "迁移记录目录已变化"
    );
    let Some(expected) = j.cleanup_digest else {
        return Ok((Some(stage), vec![]));
    };
    let next_library = j.destination.join("library");
    if paths.custom_root.as_ref() != Some(&j.destination)
        || paths.configured_library() != Some(next_library.as_path())
        || Path::new(&snapshot.data_path) != next_library.join("studyapp.sqlite3")
        || !matches!(j.phase, Phase::Publishing | Phase::Complete)
    {
        return Ok((Some(stage), vec![]));
    }
    let receipt = stage.join("cleanup.json");
    let file = plain_handle(&receipt, false, true)?;
    ensure!(file.metadata()?.len() <= 64 * 1024 * 1024, "旧副本清单过大");
    let mut bytes = Vec::new();
    file.take(64 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= 64 * 1024 * 1024 && <[u8; 32]>::from(Sha256::digest(&bytes)) == expected,
        "旧副本清单摘要已变化"
    );
    let records: Vec<CopyRecord> = serde_json::from_slice(&bytes)?;
    ensure!(records.len() <= 100_001, "旧副本清单条目过多");
    let old_cache = match j.previous {
        None => paths.defaults.cache.clone(),
        Some(raw) => {
            let config = validate_config(&serde_json::from_slice(&raw)?)?;
            config.root.join("cache")
        }
    };
    let old_profile = canon(&old_cache)?.join("editor");
    let owned_notes: std::collections::BTreeSet<_> = snapshot
        .notes
        .iter()
        .filter(|n| n.managed)
        .map(|n| Path::new("notes").join(format!("{}.md", n.id)))
        .collect();
    let mut remaining = Vec::new();
    for r in records {
        validate_path(&r.source)?;
        validate_path(&r.target)?;
        ensure!(
            r.old_library == j.source && !nested(&r.source, &j.destination),
            "旧副本目录无效"
        );
        let expected = if let Ok(rel) = r.source.strip_prefix(&old_profile) {
            j.destination.join("cache/editor").join(rel)
        } else if let Ok(rel) = r.source.strip_prefix(&j.source) {
            if !(rel == Path::new("studyapp.sqlite3")
                || owned_notes.contains(rel)
                || owned_library_payload(rel))
            {
                continue;
            }
            next_library.join(rel)
        } else {
            anyhow::bail!("旧副本不在已登记的源目录中")
        };
        ensure!(r.target == expected, "旧副本目标已变化");
        remaining.push(r);
    }
    Ok((Some(stage), remaining))
}
pub(crate) fn recover_removal_parent(parent: &Path) -> Result<()> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if parent.try_exists()? {
        let guards = guard_directories(parent, false)?;
        studyapp_sync::recover_removal_dir(guards.last().context("文件目录无效")?, parent)?;
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let _ = parent;
    Ok(())
}
fn canon(path: &Path) -> Result<PathBuf> {
    validate_path(path)?;
    if path.exists() {
        return Ok(fs::canonicalize(path)?);
    }
    Ok(canon(path.parent().context("路径没有现有父目录")?)?
        .join(path.file_name().context("目录名无效")?))
}
fn nested(path: &Path, root: &Path) -> bool {
    #[cfg(windows)]
    {
        PathBuf::from(path.to_string_lossy().to_lowercase())
            .starts_with(PathBuf::from(root.to_string_lossy().to_lowercase()))
    }
    #[cfg(not(windows))]
    {
        path.starts_with(root)
    }
}
fn path_key(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(
            studyapp_desktop::presentation::display_path(&path.to_string_lossy()).to_lowercase(),
        )
    }
    #[cfg(not(windows))]
    {
        path.to_owned()
    }
}
fn held(error: &anyhow::Error) -> bool {
    error
        .chain()
        .filter_map(|e| e.downcast_ref::<std::io::Error>())
        .any(|e| {
            (cfg!(windows) && matches!(e.raw_os_error(), Some(32 | 33)))
                || e.kind() == std::io::ErrorKind::WouldBlock
        })
}
// A destroyed WebView can checkpoint SQLite from its remaining WebKit process.
// Wait before publishing any copy; compare bytes as well as the complete file set.
// Only the closed editor profile may treat a disappearing file as transient.
fn profile_busy() -> anyhow::Error {
    std::io::Error::new(
        std::io::ErrorKind::WouldBlock,
        "编辑器缓存仍在变化，请退出并重新打开应用以继续迁移",
    )
    .into()
}
fn profile_transient(error: anyhow::Error) -> anyhow::Error {
    if error
        .chain()
        .filter_map(|e| e.downcast_ref::<std::io::Error>())
        .any(|e| e.kind() == std::io::ErrorKind::NotFound)
    {
        std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            format!("编辑器缓存正在关闭：{error:#}"),
        )
        .into()
    } else {
        error
    }
}
fn profile_revision(profile: &Path) -> Result<BTreeMap<PathBuf, (Entry, [u8; 32])>> {
    let mut revision = BTreeMap::new();
    if !profile.try_exists()? {
        return Ok(revision);
    }
    crate::storage_backend::visit_migration_tree(
        profile,
        &|_| false,
        &mut 0,
        &mut |path, rel, file| {
            let entry = Entry {
                source: path.into(),
                size: file.metadata()?.len(),
                identity: crate::app_usage::identity(&file)?,
            };
            revision.insert(rel.into(), (entry, digest(file, |_| {})?));
            check_cancel()
        },
    )
    .map_err(profile_transient)?;
    Ok(revision)
}
fn wait_profile_stable(profile: &Path, attempts: usize, mut settle: impl FnMut()) -> Result<()> {
    for _ in 0..attempts {
        check_cancel()?;
        let before = match profile_revision(profile) {
            Ok(revision) => revision,
            Err(error) if held(&error) => {
                settle();
                continue;
            }
            Err(error) => return Err(error),
        };
        settle();
        match profile_revision(profile) {
            Ok(after) if before == after => return Ok(()),
            Ok(_) => {}
            Err(error) if held(&error) => {}
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::WouldBlock,
        "编辑器缓存仍在变化，请退出并重新打开应用以继续迁移",
    )
    .into())
}
fn manifest(
    paths: &AppPaths,
    source: &Path,
    dest: &Path,
    external: &std::collections::BTreeSet<PathBuf>,
) -> Result<BTreeMap<PathBuf, Entry>> {
    let mut roots = vec![
        (
            canon(&paths.effective.cache)?.join("editor"),
            "cache/editor",
        ),
        (canon(&paths.effective.cache)?, "cache"),
        (source.into(), "library"),
        (canon(&paths.effective.config)?, "config"),
        (canon(&paths.effective.local_data)?, "local"),
    ];
    roots.sort_by_key(|(p, _)| std::cmp::Reverse(p.components().count()));
    let mut result = BTreeMap::new();
    let excludes = [
        canon(&paths.locator)?,
        canon(&paths.legacy_location())?,
        canon(&journal_file(paths))?,
        canon(&paths.locator.with_file_name("app-migration.lock"))?,
        canon(&paths.effective.updates)?,
        canon(&paths.defaults.updates)?,
    ];
    let historical_data = canon(&paths.effective.data)?;
    let mut seen = Vec::new();
    let mut count = 0;
    for (root, kind) in &roots {
        ensure!(
            !nested(dest, root) && !nested(root, dest),
            "目标不能位于原目录内或包含原目录：{}",
            root.display()
        );
        if !root.exists() || seen.contains(root) || root == &historical_data && root != source {
            continue;
        }
        seen.push(root.clone());
        let _guards = guard_directories(root, false)?;
        crate::storage_backend::visit_migration_tree(
            root,
            &|rel| {
                let p = root.join(rel);
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                roots.iter().any(|(r, _)| r != root && p == *r)
                    || excludes.contains(&p)
                    || external.contains(&path_key(&p))
                    || name.starts_with(".migration-")
                    || matches!(name, "update-last.txt" | "update-status.json")
                    || name.starts_with("status-") && name.ends_with(".json")
                    || (*kind == "library"
                        && (crate::storage_backend::live_database_file(
                            rel,
                            &source.join("studyapp.sqlite3"),
                        ) || crate::storage_backend::runtime_lock_file(
                            rel,
                            &source.join("studyapp.sqlite3"),
                        )))
            },
            &mut count,
            &mut |path, rel, file| {
                let target = Path::new(kind).join(rel);
                let entry = Entry {
                    source: path.into(),
                    size: file.metadata()?.len(),
                    identity: crate::app_usage::identity(&file)?,
                };
                ensure!(result.insert(target, entry).is_none(), "迁移目标路径冲突");
                Ok(())
            },
        )
        .map_err(|error| {
            if *kind == "cache/editor" {
                profile_transient(error)
            } else {
                error
            }
        })?;
    }
    // Preserve the absolute pointer to the old transaction across successive
    // custom-root moves; the signed transaction itself never changes location.
    let pointer = paths.update_pointer()?;
    if pointer.try_exists()? && pointer != paths.defaults.local_data.join("update-last.txt") {
        let f = plain_handle(&pointer, false, true)?;
        result.insert(
            PathBuf::from("local/update-last.txt"),
            Entry {
                source: pointer,
                size: f.metadata()?.len(),
                identity: crate::app_usage::identity(&f)?,
            },
        );
    }
    Ok(result)
}
struct MovedIndex {
    source_digest: [u8; 32],
    bytes: Vec<u8>,
}
// The committed index and its writer's hidden alias share an inode. Prepare
// the replacement bytes once, independent of which name is enumerated first.
fn moved_indexes(
    files: &BTreeMap<PathBuf, Entry>,
    source: &Path,
    target: &Path,
    default_images: bool,
) -> Result<BTreeMap<(u64, u128), MovedIndex>> {
    let mut result = BTreeMap::new();
    for (rel, entry) in files {
        let Some(tail) = rel
            .strip_prefix("library/image-index")
            .ok()
            .and_then(Path::to_str)
            .and_then(|s| s.strip_suffix(".json"))
        else {
            continue;
        };
        let tail = tail.replace('\\', "/");
        if studyapp_sync::validate_asset_key(&format!("assets/{tail}")).is_err() {
            continue;
        }
        let mut bytes = Vec::new();
        plain_handle(&entry.source, false, true)?
            .take(32769)
            .read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 32768, "图片索引过大");
        let stored: PathBuf = serde_json::from_slice(&bytes)?;
        ensure!(
            stored.is_absolute()
                && !stored
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
                && stored.ends_with(Path::new(&tail)),
            "图片索引与资产身份不匹配"
        );
        if path_key(&stored) == path_key(&source.join("images").join(&tail))
            || (default_images && files.contains_key(&Path::new("library/images").join(&tail)))
        {
            let replacement = serde_json::to_vec(&target.join("library/images").join(tail))?;
            result.insert(
                entry.identity,
                MovedIndex {
                    source_digest: Sha256::digest(bytes).into(),
                    bytes: replacement,
                },
            );
        }
    }
    Ok(result)
}
fn digest(mut file: File, mut progress: impl FnMut(u64)) -> Result<[u8; 32]> {
    hash_stream(&mut file, |n| {
        progress(n);
        Ok(())
    })
}
fn hash_stream(file: &mut File, mut progress: impl FnMut(u64) -> Result<()>) -> Result<[u8; 32]> {
    let mut hash = Sha256::new();
    let mut buf = [0; 65536];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
        progress(n as u64)?
    }
    Ok(hash.finalize().into())
}
pub struct Prepared {
    paths: AppPaths,
    journal: Journal,
    _lock: File,
    pub database: PathBuf,
    pub records: Vec<CopyRecord>,
    pub target_lock: File,
}
impl Prepared {
    pub fn publish(mut self) -> Result<(PathBuf, File, Vec<CopyRecord>)> {
        self.commit()?;
        Ok((self.database, self.target_lock, self.records))
    }
    fn commit(&mut self) -> Result<()> {
        if let Err(e) = check_cancel() {
            self.journal.phase = Phase::Aborted;
            atomic_json(&journal_file(&self.paths), &self.journal)?;
            return Err(e);
        }
        self.paths.verify_relocation_allowed()?;
        ensure!(
            read_optional(&self.paths.locator)? == self.journal.previous,
            "位置配置已变化，请重新迁移"
        );
        // Nothing opens the final DB for writing until this locator is committed.
        let final_db = plain_handle(&self.database, false, true)?;
        ensure!(
            Some(crate::app_usage::identity(&final_db)?) == self.journal.backup_identity
                && Some(digest(final_db, |_| {})?) == self.journal.backup_digest,
            "目标数据库已变化，已保留原文件"
        );
        for suffix in ["-wal", "-shm", "-journal"] {
            ensure!(
                !self
                    .database
                    .with_file_name(format!("studyapp.sqlite3{suffix}"))
                    .try_exists()?,
                "目标出现未知数据库文件"
            );
        }
        let receipt = self
            .journal
            .destination
            .join(format!(".studyapp-migration-{}", self.journal.id))
            .join("cleanup.json");
        atomic_json(&receipt, &self.records)?;
        self.journal.cleanup_digest = Some(digest(plain_handle(&receipt, false, true)?, |_| {})?);
        self.journal.phase = Phase::Publishing;
        atomic_json(&journal_file(&self.paths), &self.journal)?;
        publish_config(
            &self.paths.locator,
            &AppPathConfig {
                schema: 1,
                root: self.journal.destination.clone(),
                library: self.database.parent().map(Path::to_owned),
            },
        )?;
        self.journal.phase = Phase::Complete;
        // Locator is authoritative even if the final acknowledgement fails.
        let _ = atomic_json(&journal_file(&self.paths), &self.journal);
        Ok(())
    }
}
pub fn prepare(
    engine: &mut Engine,
    paths: AppPaths,
    target: &Path,
    now: i64,
    progress: impl FnMut(TransferProgress),
) -> Result<Prepared> {
    prepare_checked(
        engine,
        paths,
        target,
        now,
        progress,
        studyapp_core::verify_backup_file,
    )
}
fn prepare_checked(
    engine: &mut Engine,
    paths: AppPaths,
    target: &Path,
    now: i64,
    mut progress: impl FnMut(TransferProgress),
    probe: impl FnOnce(&Path) -> Result<()>,
) -> Result<Prepared> {
    let source = canon(
        Path::new(&engine.snapshot().data_path)
            .parent()
            .context("资料库目录无效")?,
    )?;
    ensure!(
        canon(&paths.resolve_library(None)?)? == source,
        "当前资料库与应用位置配置不一致"
    );
    let dest = canon(target)?;
    let _lock = lease(&paths.locator)?;
    paths.verify_relocation_allowed()?;
    if !cfg!(test) {
        let exe = std::env::current_exe()?;
        ensure!(
            crate::maintenance_execute::discover_recovery(&exe)?.is_none(),
            "维护恢复尚未完成"
        );
        if let Some(status) = crate::maintenance_execute::read_last_maintenance_status(&exe) {
            use crate::maintenance_execute::MaintenanceState;
            ensure!(
                matches!(
                    status.state,
                    MaintenanceState::Succeeded | MaintenanceState::RolledBack
                ),
                "维护事务正在执行或需要恢复"
            );
        }
    }
    let mut j = if let Some(j) = journal(&paths)?
        && pending(&paths)?.is_some()
    {
        ensure!(
            j.destination == dest && j.source == source,
            "请先在原目标继续上一次迁移：{}",
            j.destination.display()
        );
        j
    } else {
        for p in [
            &source,
            &paths.effective.cache,
            &paths.effective.config,
            &paths.effective.local_data,
            &paths.effective.data,
            &paths.effective.updates,
        ] {
            let p = canon(p)?;
            ensure!(
                !nested(&dest, &p) && !nested(&p, &dest),
                "目标不能与原目录重叠"
            )
        }
        let _guard = guard_directories(&dest, true)?;
        ensure!(
            fs::read_dir(&dest)?.next().is_none(),
            "应用迁移目标必须为空"
        );
        let id = uuid::Uuid::new_v4().to_string();
        let stage = dest.join(format!(".studyapp-migration-{id}"));
        fs::create_dir(&stage)?;
        let stage_id = crate::app_usage::identity(&plain_handle(&stage, true, false)?)?;
        let j = Journal {
            schema: 1,
            id,
            source: source.clone(),
            destination: dest.clone(),
            previous: read_optional(&paths.locator)?,
            phase: Phase::Copying,
            backup_identity: None,
            backup_digest: None,
            stage_identity: Some(stage_id),
            cleanup_digest: None,
            profile_copies: Vec::new(),
        };
        atomic_json(&journal_file(&paths), &j)?;
        j
    };
    let execute = (|| -> Result<_> {
        let scratch = dest.join(format!(".studyapp-migration-{}", j.id));
        let _scratch = guard_directories(&scratch, false)?;
        ensure!(
            Some(crate::app_usage::identity(&plain_handle(
                &scratch, true, false
            )?)?)
                == j.stage_identity,
            "目标暂存目录已被替换"
        );
        // Only verified copies recorded by this transaction may be refreshed.
        // Exact identity and digest checks preserve any replacement/unknown file.
        for copy in &j.profile_copies {
            ensure!(
                copy.target
                    .strip_prefix(dest.join("cache/editor"))
                    .is_ok_and(|rel| rel
                        .components()
                        .all(|c| matches!(c, std::path::Component::Normal(_)))),
                "编辑器副本记录无效"
            );
            if copy.target.try_exists()? {
                unlink_exact(&copy.target, copy.identity, &copy.digest)?;
            }
        }
        j.profile_copies.clear();
        atomic_json(&journal_file(&paths), &j)?;
        progress("正在等待编辑器释放缓存文件…".into());
        wait_profile_stable(&paths.effective.cache.join("editor"), 16, || {
            std::thread::sleep(std::time::Duration::from_millis(125));
        })?;
        progress("正在统计可迁移文件…".into());
        let snapshot = engine.snapshot();
        let mut external: std::collections::BTreeSet<_> = snapshot
            .notes
            .iter()
            .filter(|n| !n.managed)
            .map(|n| path_key(Path::new(&n.path)))
            .collect();
        if let Some(dir) = &snapshot.settings.images.root {
            external.insert(path_key(Path::new(dir)));
        }
        let start = std::time::Instant::now();
        let files = loop {
            match manifest(&paths, &source, &dest, &external) {
                Err(e) if held(&e) && start.elapsed() < std::time::Duration::from_secs(2) => {
                    progress("正在等待编辑器释放缓存文件…".into());
                    check_cancel()?;
                    std::thread::sleep(std::time::Duration::from_millis(125));
                }
                result => break result?,
            }
        };
        crate::storage_backend::visit_migration_tree(
            &dest,
            &|r| dest.join(r) == scratch || r == Path::new("library/studyapp.lock"),
            &mut 0,
            &mut |_, rel, _| {
                ensure!(
                    files.contains_key(rel)
                        || (rel == Path::new("library/studyapp.sqlite3")
                            && j.backup_identity.is_some()),
                    "目标出现不属于本次迁移的文件：{}",
                    rel.display()
                );
                Ok(())
            },
        )?;
        let total = files
            .values()
            .try_fold(0u64, |n, e| n.checked_add(e.size).context("迁移总大小溢出"))?;
        let db_size = fs::metadata(source.join("studyapp.sqlite3"))?.len();
        ensure!(
            available_space(&dest)? >= total.saturating_add(db_size).saturating_add(1024 * 1024),
            "目标磁盘空间不足"
        );
        let database = dest.join("library/studyapp.sqlite3");
        let _guard = guard_directories(database.parent().unwrap(), true)?;
        let lock_path = database.with_extension("lock");
        validate_path(&lock_path)?;
        let target_lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)?;
        target_lock.try_lock().context("目标资料库正在使用")?;
        // Partial bytes never shadow a final file; each restart independently
        // verifies already-published files against the newly quiesced source.

        let indexes = moved_indexes(
            &files,
            &source,
            &dest,
            snapshot.settings.images.root.is_none(),
        )?;
        let mut representatives: BTreeMap<(u64, u128), PathBuf> = BTreeMap::new();
        let mut destination_groups = BTreeMap::new();
        let verify_total = files
            .values()
            .map(|f| {
                indexes
                    .get(&f.identity)
                    .map_or(f.size, |i| i.bytes.len() as u64)
            })
            .sum();
        let mut copied = 0;
        let mut verified = 0;
        let mut records = Vec::new();
        let profile = canon(&paths.effective.cache)?.join("editor");
        let mut throttle = std::time::Instant::now();
        for (rel, entry) in &files {
            check_cancel()?;
            let dst = dest.join(rel);
            if nested(&entry.source, &profile) {
                ensure!(
                    !dst.try_exists()?,
                    "目标出现未登记的编辑器缓存文件：{}",
                    dst.display()
                );
            }
            let _srcguards =
                guard_directories(entry.source.parent().unwrap(), false).map_err(|error| {
                    if nested(&entry.source, &profile) {
                        profile_transient(error)
                    } else {
                        error
                    }
                })?;
            let _guards = guard_directories(dst.parent().unwrap(), true)?;
            let input = plain_handle(&entry.source, false, true).map_err(|error| {
                if nested(&entry.source, &profile) {
                    profile_transient(error)
                } else {
                    error
                }
            })?;
            let rewrite = indexes.get(&entry.identity);
            let first = representatives.get(&entry.identity);
            if let Some(first) = first
                && !dst.try_exists()?
            {
                fs::hard_link(first, &dst)
                    .with_context(|| format!("目标文件系统未能保留硬链接：{}", dst.display()))?;
            }
            let expected = if let Some(rewrite) = rewrite {
                ensure!(
                    digest(input, |_| {})? == rewrite.source_digest,
                    "图片索引已变化"
                );
                if !dst.try_exists()? {
                    let temp = scratch.join(uuid::Uuid::new_v4().to_string());
                    let mut output = OpenOptions::new()
                        .create_new(true)
                        .write(true)
                        .open(&temp)?;
                    output.write_all(&rewrite.bytes)?;
                    output.sync_all()?;
                    drop(output);
                    publish_new(&temp, &dst)?;
                }
                copied += entry.size;
                Sha256::digest(&rewrite.bytes).into()
            } else if dst.try_exists()? {
                copied += entry.size;
                digest(input, |_| {})?
            } else {
                let temp = scratch.join(uuid::Uuid::new_v4().to_string());
                let mut options = OpenOptions::new();
                options.create_new(true).write(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                let mut output = options.open(&temp)?;
                let mut reader = input;
                let mut hash = Sha256::new();
                let mut buf = [0; 65536];
                loop {
                    check_cancel()?;
                    let n = reader.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    output.write_all(&buf[..n])?;
                    hash.update(&buf[..n]);
                    copied += n as u64;
                    if throttle.elapsed().as_millis() >= 100 {
                        progress(TransferProgress::bytes("正在复制", copied, total));
                        throttle = std::time::Instant::now();
                    }
                }
                output.sync_all()?;
                drop(output);
                ensure!(!dst.try_exists()?, "目标出现其他文件");
                publish_new(&temp, &dst)?;
                hash.finalize().into()
            };
            let mut dest_file = plain_handle(&dst, false, true)?;
            let dest_id = crate::app_usage::identity(&dest_file)?;
            ensure!(
                destination_groups
                    .insert(dest_id, entry.identity)
                    .is_none_or(|old| old == entry.identity),
                "目标文件存在未登记的硬链接"
            );
            if let Some(first) = first {
                ensure!(
                    crate::app_usage::identity(&plain_handle(first, false, true)?)? == dest_id,
                    "目标硬链接拓扑已变化"
                );
            } else {
                representatives.insert(entry.identity, dst.clone());
            }
            let actual = hash_stream(&mut dest_file, |n| {
                verified += n;
                if throttle.elapsed().as_millis() >= 100 {
                    progress(TransferProgress::bytes("正在校验", verified, verify_total));
                    throttle = std::time::Instant::now()
                }
                check_cancel()
            })?;
            if expected != actual && nested(&entry.source, &profile) {
                return Err(profile_busy());
            }
            ensure!(expected == actual, "文件校验失败：{}", dst.display());
            if nested(&entry.source, &profile) {
                j.profile_copies.push(ProfileCopy {
                    target: dst.clone(),
                    identity: dest_id,
                    digest: actual,
                });
                atomic_json(&journal_file(&paths), &j)?;
            }
            records.push(CopyRecord {
                source: entry.source.clone(),
                target: dst,
                identity: entry.identity,
                size: entry.size,
                digest: rewrite.map_or(actual, |r| r.source_digest),
                target_digest: rewrite.map(|_| actual),
                old_library: source.clone(),
            });
        }
        progress(TransferProgress::bytes(
            "已校验，正在备份资料库",
            verified,
            verify_total,
        ));
        let backup = scratch.join(format!("{}.sqlite3", uuid::Uuid::new_v4()));
        engine.apply(Command::Backup(backup.clone()), now)?;
        progress("正在验证暂存数据库…".into());
        probe(&backup)?;
        OpenOptions::new().write(true).open(&backup)?.sync_all()?;
        validate_path(&database)?;
        if database.try_exists()? {
            let existing = plain_handle(&database, false, true)?;
            ensure!(
                Some(crate::app_usage::identity(&existing)?) == j.backup_identity
                    && Some(digest(existing, |_| {})?) == j.backup_digest,
                "目标数据库已变化，已保留原文件"
            );
            let prior = scratch.join(format!("prior-{}.sqlite3", uuid::Uuid::new_v4()));
            publish_new(&database, &prior)?;
            let saved = plain_handle(&prior, false, true)?;
            if Some(crate::app_usage::identity(&saved)?) != j.backup_identity
                || Some(digest(saved, |_| {})?) != j.backup_digest
            {
                publish_new(&prior, &database).with_context(|| {
                    format!("目标文件发生冲突，原条目保存在 {}", prior.display())
                })?;
                anyhow::bail!("目标数据库已变化，已保留原文件");
            }
        }
        let fresh = plain_handle(&backup, false, true)?;
        j.backup_identity = Some(crate::app_usage::identity(&fresh)?);
        j.backup_digest = Some(digest(fresh, |_| {})?);
        atomic_json(&journal_file(&paths), &j)?;
        publish_new(&backup, &database)?;
        let after = manifest(&paths, &source, &dest, &external)?;
        if after != files {
            let outside_profile = |entries: &BTreeMap<PathBuf, Entry>| {
                entries
                    .iter()
                    .filter(|(rel, _)| !rel.starts_with("cache/editor"))
                    .map(|(rel, entry)| (rel.clone(), entry.clone()))
                    .collect::<BTreeMap<_, _>>()
            };
            ensure!(
                outside_profile(&after) == outside_profile(&files),
                "迁移期间源文件发生变化，原位置仍有效"
            );
            return Err(profile_busy());
        }
        for record in &records {
            let copy = plain_handle(&record.target, false, true)?;
            ensure!(
                destination_groups.get(&crate::app_usage::identity(&copy)?)
                    == Some(&record.identity)
                    && digest(copy, |_| {})? == record.target_digest.unwrap_or(record.digest),
                "目标副本或硬链接发生变化：{}",
                record.target.display()
            );
            let mut file = plain_handle(&record.source, false, true).map_err(|error| {
                if nested(&record.source, &profile) {
                    profile_transient(error)
                } else {
                    error
                }
            })?;
            let current = hash_stream(&mut file, |_| check_cancel())?;
            if current != record.digest && nested(&record.source, &profile) {
                return Err(profile_busy());
            }
            ensure!(
                current == record.digest,
                "源文件已变化：{}",
                record.source.display()
            );
        }
        // Limit cleanup to material with positive evidence of app ownership.
        // Unrecognized files are copied, but are never proposed for deletion.
        let known_notes: std::collections::BTreeSet<_> = engine
            .snapshot()
            .notes
            .iter()
            .filter(|n| n.managed)
            .map(|n| source.join("notes").join(format!("{}.md", n.id)))
            .collect();
        let profile = canon(&paths.effective.cache)?.join("editor");
        records.retain(|r| {
            known_notes.contains(&r.source)
                || nested(&r.source, &profile)
                || r.source
                    .strip_prefix(&source)
                    .is_ok_and(owned_library_payload)
        });
        let old_database = source.join("studyapp.sqlite3");
        let old_file = plain_handle(&old_database, false, false)?;
        records.push(CopyRecord {
            source: old_database,
            target: database.clone(),
            old_library: source.clone(),
            identity: crate::app_usage::identity(&old_file)?,
            size: old_file.metadata()?.len(),
            digest: digest(old_file, |_| {})?,
            target_digest: None,
        });
        Ok((database, records, target_lock))
    })();
    match execute {
        Ok((database, records, target_lock)) => Ok(Prepared {
            paths,
            journal: j,
            _lock,
            database,
            records,
            target_lock,
        }),
        Err(e) => {
            let held = held(&e);
            j.phase = if held {
                Phase::AwaitingRestart
            } else {
                Phase::Aborted
            };
            atomic_json(&journal_file(&paths), &j)?;
            if held {
                Err(e.context("编辑器缓存仍被占用，请退出并重新打开应用以继续迁移"))
            } else {
                Err(e)
            }
        }
    }
}
#[cfg(windows)]
fn available_space(path: &Path) -> Result<u64> {
    use std::os::windows::ffi::OsStrExt;
    let path: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut free = 0;
    // SAFETY: nul-terminated path and a valid writable u64 output.
    if unsafe {
        windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
            path.as_ptr(),
            &mut free,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(free)
}
#[cfg(unix)]
fn available_space(path: &Path) -> Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: the path is nul-terminated and the result is read only on success.
    if unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let stat = unsafe { stat.assume_init() };
    Ok((stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64))
}

fn owned_library_payload(rel: &Path) -> bool {
    let name = rel.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if rel == Path::new("note-draft.json") {
        return true;
    }
    if rel.parent() == Some(Path::new("drafts")) {
        return name
            .strip_suffix(".json")
            .is_some_and(|s| uuid::Uuid::parse_str(s).is_ok());
    }
    if rel.starts_with("image-index") && name.ends_with(".json") {
        return true;
    }
    if rel.starts_with("images") {
        return rel
            .iter()
            .nth(1)
            .and_then(|s| s.to_str())
            .is_some_and(|s| uuid::Uuid::parse_str(s).is_ok());
    }
    rel.starts_with("settings")
        && matches!(
            name,
            "manifest.json"
                | "shortcuts.json"
                | "images.json"
                | "appearance.json"
                | "learning.json"
                | "workspace.json"
                | "sync.json"
        )
}
pub fn cleanup(records: &[CopyRecord], snapshot: &studyapp_core::Snapshot) -> Result<String> {
    let current_database = &snapshot.data_path;
    let external: std::collections::BTreeSet<_> = snapshot
        .notes
        .iter()
        .filter(|n| !n.managed)
        .map(|n| path_key(Path::new(&n.path)))
        .collect();
    let first = records.first().context("没有待删除的旧副本")?;
    let paths = crate::app_usage::runtime_paths()?;
    let active = canon(
        Path::new(current_database)
            .parent()
            .context("资料库目录无效")?,
    )?;
    ensure!(
        canon(&paths.resolve_library(None)?)? == active,
        "应用位置已经变化，旧副本已保留"
    );
    let root = canon(paths.custom_root.as_deref().context("未配置应用位置")?)?;
    ensure!(
        active == root.join("library"),
        "当前资料库已切换，旧文件保留"
    );
    let old = canon(&first.old_library)?;
    ensure!(
        !nested(&old, &root) && !nested(&root, &old),
        "旧位置与当前目录重叠"
    );
    let _lease = lease(&paths.locator)?;
    let _old_guard = guard_directories(&old, false)?;
    let old_lock_path = old.join("studyapp.lock");
    validate_path(&old_lock_path)?;
    let old_lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(old_lock_path)?;
    old_lock
        .try_lock()
        .context("旧资料库正在使用，未删除旧文件")?;
    let mut removed = 0;
    let mut retained = Vec::new();
    for record in records {
        let result = (|| -> Result<()> {
            ensure!(
                record.old_library == old
                    && !nested(&record.source, &root)
                    && nested(&record.target, &root),
                "源或目标目录已变化"
            );
            let source_key = path_key(&record.source);
            ensure!(
                !external.contains(&source_key)
                    && !snapshot
                        .settings
                        .images
                        .root
                        .as_ref()
                        .is_some_and(|r| source_key.starts_with(path_key(Path::new(r)))),
                "旧文件仍被当前资料库作为外部文件引用"
            );
            let _src = guard_directories(record.source.parent().context("源目录无效")?, false)?;
            let _dst = guard_directories(record.target.parent().context("目标目录无效")?, false)?;
            if record.source == old.join("studyapp.sqlite3") {
                ensure!(retained.is_empty(), "部分旧文件仍在原位，保留旧数据库");
                ensure!(
                    canon(Path::new(current_database))? == canon(&record.target)?,
                    "目标数据库已经改变"
                );
                for suffix in ["-wal", "-journal"] {
                    let p = old.join(format!("studyapp.sqlite3{suffix}"));
                    ensure!(
                        !p.try_exists()? || plain_handle(&p, false, true)?.metadata()?.len() == 0,
                        "旧资料库尚有未合并数据"
                    );
                }
            } else {
                ensure!(
                    digest(plain_handle(&record.target, false, true)?, |_| {})?
                        == record.target_digest.unwrap_or(record.digest),
                    "新副本已变化"
                );
            }
            unlink_exact(&record.source, record.identity, &record.digest)
        })();
        match result {
            Ok(()) => removed += 1,
            Err(e) => retained.push(format!("{}（{e}）", record.source.display())),
        }
    }
    Ok(format!(
        "已删除 {removed} 个旧副本，保留 {} 个。{}",
        retained.len(),
        retained.first().map(String::as_str).unwrap_or("")
    ))
}
pub(crate) fn unlink_exact(path: &Path, identity: (u64, u128), expected: &[u8; 32]) -> Result<()> {
    validate_path(path)?;
    let _guards = guard_directories(path.parent().context("文件目录无效")?, false)?;
    unlink_os(path, identity, expected)
}
#[cfg(windows)]
fn unlink_os(path: &Path, identity: (u64, u128), expected: &[u8; 32]) -> Result<()> {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_DISPOSITION_INFO, FileDispositionInfo, SetFileInformationByHandle,
    };
    let file = OpenOptions::new()
        .read(true)
        .access_mode(0x8000_0000 | 0x0001_0000)
        .share_mode(1)
        .custom_flags(0x0020_0000)
        .open(path)?;
    ensure!(
        crate::app_usage::identity(&file)? == identity
            && digest(file.try_clone()?, |_| {})? == *expected,
        "旧文件已变化"
    );
    let info = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: the handle was opened with DELETE and pinned to the exact confirmed file.
    if unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfo,
            (&info as *const FILE_DISPOSITION_INFO).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}
#[cfg(unix)]
fn unlink_os(path: &Path, identity: (u64, u128), expected: &[u8; 32]) -> Result<()> {
    let dir = plain_handle(path.parent().context("源目录无效")?, true, false)?;
    let file = plain_handle(path, false, true)?;
    ensure!(
        crate::app_usage::identity(&file)? == identity,
        "旧文件身份已变化"
    );
    let bytes = file.metadata()?.len();
    let hash = expected
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let revision = format!("{}:{}:{hash}", identity.0, identity.1);
    studyapp_sync::claim_unlink(&dir, path, &revision, &hash, bytes, None)?;
    Ok(())
}
#[cfg(all(test, unix))]
pub(crate) fn simulate_interrupted_claim(path: &Path) -> PathBuf {
    use std::os::unix::{ffi::OsStrExt, fs::PermissionsExt};
    let identity = |f: &File| {
        let (dev, ino) = crate::app_usage::identity(f).unwrap();
        format!("{dev}:{ino}")
    };
    let parent = path.parent().unwrap();
    let slot = parent.join(".studyapp-removal-v1");
    fs::create_dir(&slot).unwrap();
    fs::set_permissions(&slot, fs::Permissions::from_mode(0o700)).unwrap();
    let file = File::open(path).unwrap();
    let hash = format!("{:x}", Sha256::digest(fs::read(path).unwrap()));
    let receipt = serde_json::json!({"name":path.file_name().unwrap().as_bytes(),"parent":identity(&File::open(parent).unwrap()),"transaction":identity(&File::open(&slot).unwrap()),"revision":format!("{}:{hash}",identity(&file)),"hash":hash,"bytes":file.metadata().unwrap().len()});
    fs::write(slot.join("receipt.json"), receipt.to_string()).unwrap();
    let claimed = slot.join("claimed");
    fs::rename(path, &claimed).unwrap();
    claimed
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (PathBuf, AppPaths, Engine) {
        let base = tempfile::tempdir().unwrap().keep();
        let dirs = studyapp_desktop::app_paths::AppDirectories {
            data: base.join("source/library"),
            config: base.join("source/config"),
            local_data: base.join("source/local"),
            cache: base.join("source/cache"),
            updates: base.join("source/updates"),
        };
        let paths =
            AppPaths::from_defaults(dirs.clone(), dirs.config.join("storage-location.json"))
                .unwrap();
        let engine = Engine::open(&dirs.data.join("studyapp.sqlite3"), 1000).unwrap();
        (base, paths, engine)
    }
    #[test]
    fn checkpoint_after_copy_is_restartable_and_refreshes_only_owned_copies() {
        let (base, paths, mut engine) = fixture();
        let profile = paths.effective.cache.join("editor");
        fs::create_dir_all(&profile).unwrap();
        let database = profile.join("localstorage.db");
        let wal = profile.join("localstorage.db-wal");
        fs::write(&database, b"old bytes").unwrap();
        fs::write(&wal, b"pending").unwrap();
        let target = base.join("dest");
        let mut changed = false;
        let error = prepare(&mut engine, paths.clone(), &target, 1001, |p| {
            if !changed && p.label.starts_with("已校验，正在备份资料库") {
                changed = true;
                fs::write(&database, b"new bytes").unwrap();
                fs::remove_file(&wal).unwrap();
            }
        })
        .err()
        .unwrap();
        assert!(held(&error), "{error:#}");
        assert!(changed);
        assert_eq!(pending(&paths).unwrap(), Some(canon(&target).unwrap()));
        assert!(!paths.locator.exists());
        let mut next = prepare(&mut engine, paths.clone(), &target, 1002, |_| {}).unwrap();
        next.commit().unwrap();
        assert_eq!(
            fs::read(target.join("cache/editor/localstorage.db")).unwrap(),
            b"new bytes"
        );
        assert!(!target.join("cache/editor/localstorage.db-wal").exists());
        assert_eq!(fs::read(database).unwrap(), b"new bytes");
    }
    #[test]
    fn replaced_profile_copy_is_never_deleted_during_resume() {
        let (base, paths, mut engine) = fixture();
        let profile = paths.effective.cache.join("editor");
        fs::create_dir_all(&profile).unwrap();
        let database = profile.join("localstorage.db");
        fs::write(&database, b"old bytes").unwrap();
        let target = base.join("dest");
        let error = prepare(&mut engine, paths.clone(), &target, 1001, |p| {
            if p.label.starts_with("已校验，正在备份资料库") {
                fs::write(&database, b"new bytes").unwrap();
            }
        })
        .err()
        .unwrap();
        assert!(held(&error), "{error:#}");
        let copy = target.join("cache/editor/localstorage.db");
        fs::write(&copy, b"user replacement").unwrap();
        assert!(prepare(&mut engine, paths, &target, 1002, |_| {}).is_err());
        assert_eq!(fs::read(copy).unwrap(), b"user replacement");
    }
    #[test]
    fn closed_profile_waits_for_wal_checkpoint_and_hashes_main_database() {
        let (base, paths, _) = fixture();
        let profile = paths.effective.cache.join("editor");
        fs::create_dir_all(&profile).unwrap();
        let database = profile.join("localstorage.db");
        let wal = profile.join("localstorage.db-wal");
        fs::write(&database, b"old bytes").unwrap();
        fs::write(&wal, b"pending bytes").unwrap();
        let mut rounds = 0;
        wait_profile_stable(&profile, 4, || {
            rounds += 1;
            if rounds == 1 {
                fs::write(&database, b"new bytes").unwrap();
                fs::remove_file(&wal).unwrap();
            } else if rounds == 2 {
                // Same inode and length: metadata comparison alone is insufficient.
                fs::write(&database, b"end bytes").unwrap();
            }
        })
        .unwrap();
        assert_eq!(rounds, 3);
        assert_eq!(fs::read(database).unwrap(), b"end bytes");
        assert!(!wal.exists());
        assert!(base.exists());
    }
    #[test]
    fn continuously_changing_profile_is_held_without_publishing() {
        let (base, paths, _) = fixture();
        let profile = paths.effective.cache.join("editor");
        fs::create_dir_all(&profile).unwrap();
        let database = profile.join("localstorage.db");
        fs::write(&database, b"0").unwrap();
        let mut rounds = 0;
        let error = wait_profile_stable(&profile, 3, || {
            rounds += 1;
            fs::write(&database, rounds.to_string()).unwrap();
        })
        .unwrap_err();
        assert!(held(&error));
        assert_eq!(rounds, 3);
        assert!(!base.join("dest").exists());
    }
    #[test]
    fn interrupted_live_database_probe_resumes_in_same_target() {
        let (base, paths, mut engine) = fixture();
        let target = base.join("dest");
        let mut live_probe = None;
        let crash = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = prepare_checked(
                &mut engine,
                paths.clone(),
                &target,
                1010,
                |_| {},
                |path| {
                    live_probe = Some(Engine::open(path, 1010)?);
                    assert!(
                        path.with_file_name(format!(
                            "{}-wal",
                            path.file_name().unwrap().to_string_lossy()
                        ))
                        .exists()
                    );
                    panic!("interrupt while SQLite has live sidecars")
                },
            );
        }));
        assert!(crash.is_err() && live_probe.is_some());
        assert!(!paths.locator.exists() && !target.join("library/studyapp.sqlite3").exists());
        assert_eq!(pending(&paths).unwrap(), Some(canon(&target).unwrap()));
        let prepared = prepare(&mut engine, paths, &target, 1011, |_| {}).unwrap();
        let (db, _lock, _) = prepared.publish().unwrap();
        Engine::open(&db, 1012).unwrap();
    }
    #[test]
    fn resume_refuses_a_split_target_alias_with_identical_bytes() {
        let (base, paths, mut engine) = fixture();
        let src = &paths.effective.data;
        fs::write(src.join("a"), b"same").unwrap();
        fs::hard_link(src.join("a"), src.join("b")).unwrap();
        let dest = base.join("new");
        drop(prepare(&mut engine, paths.clone(), &dest, 1010, |_| {}).unwrap());
        let b = dest.join("library/b");
        fs::rename(&b, base.join("saved-link")).unwrap();
        fs::write(&b, b"same").unwrap();
        assert!(prepare(&mut engine, paths.clone(), &dest, 1011, |_| {}).is_err());
        assert_eq!(fs::read(&b).unwrap(), b"same");
        assert!(!paths.locator.exists());
    }
    #[test]
    fn migrated_image_and_index_hardlinks_can_be_purged_without_touching_source() {
        let (base, paths, mut engine) = fixture();
        let id = uuid::Uuid::new_v4().to_string();
        engine
            .apply(
                Command::CreateNoteWithId {
                    note_id: id.clone(),
                    title: "image".into(),
                    content: format!("![asset](studyapp-media:{id}/one.gif)"),
                    folder_id: None,
                },
                1010,
            )
            .unwrap();
        let image = Path::new("images").join(&id).join("one.gif");
        let index = Path::new("image-index").join(&id).join("one.gif.json");
        let source = &paths.effective.data;
        let pending = format!(".studyapp-image-{}.pending", uuid::Uuid::new_v4());
        for rel in [&image, &index] {
            let p = source.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            let content = if *rel == image {
                b"gif".to_vec()
            } else {
                serde_json::to_vec(&source.join(&image)).unwrap()
            };
            fs::write(&p, content).unwrap();
            fs::hard_link(&p, p.with_file_name(&pending)).unwrap();
        }
        let middle = base.join("middle");
        let db =
            crate::storage_backend::migrate_library(&mut engine, &middle, 1011, |_| {}).unwrap();
        let mut middle_engine = Engine::open(&db, 1011).unwrap();
        remember_library(&paths.legacy_location(), &middle).unwrap();
        let before =
            crate::storage_backend::purge::prepare(&middle_engine.snapshot(), &id).unwrap();
        assert!(
            !before
                .files
                .keys()
                .any(|p| p.starts_with(fs::canonicalize(source).unwrap()))
        );
        let target = base.join("new");
        let mut migration =
            prepare(&mut middle_engine, paths.clone(), &target, 1011, |_| {}).unwrap();
        migration.commit().unwrap();
        let mut next = Engine::open(&migration.database, 1012).unwrap();
        for rel in [&image, &index] {
            let p = target.join("library").join(rel);
            let identity = |p: &Path| crate::app_usage::identity(&File::open(p).unwrap()).unwrap();
            assert_eq!(identity(&p), identity(&p.with_file_name(&pending)));
            assert_ne!(identity(&p), identity(&source.join(rel)));
        }
        let plan = crate::storage_backend::purge::prepare(&next.snapshot(), &id).unwrap();
        assert_eq!(
            serde_json::from_slice::<PathBuf>(
                &fs::read(target.join("library").join(&index)).unwrap()
            )
            .unwrap(),
            fs::canonicalize(target.join("library").join(&image)).unwrap()
        );
        crate::storage_backend::purge::execute(&mut next, &plan, 1013).unwrap();
        let before =
            crate::storage_backend::purge::prepare(&middle_engine.snapshot(), &id).unwrap();
        crate::storage_backend::purge::execute(&mut middle_engine, &before, 1014).unwrap();
        for rel in [&image, &index] {
            let p = target.join("library").join(rel);
            assert!(!p.exists() && !p.with_file_name(&pending).exists());
            assert!(
                source.join(rel).exists() && source.join(rel).with_file_name(&pending).exists()
            );
        }
    }
    #[test]
    fn staged_copy_and_publication_preserve_old_data_and_exclude_external_inputs() {
        let (base, paths, mut engine) = fixture();
        let profile = paths.effective.cache.join("editor/profile");
        fs::create_dir_all(&profile).unwrap();
        fs::write(profile.join("Preferences"), b"cache bytes").unwrap();
        fs::create_dir_all(&paths.effective.updates).unwrap();
        fs::write(paths.effective.updates.join("transaction.json"), b"keep").unwrap();
        let target = base.join("新 位置");
        let mut events = Vec::new();
        let mut prepared = prepare(&mut engine, paths.clone(), &target, 1001, |s| {
            events.push(s)
        })
        .unwrap();
        assert!(!paths.locator.exists());
        assert_eq!(
            fs::read(target.join("cache/editor/profile/Preferences")).unwrap(),
            b"cache bytes"
        );
        assert!(profile.join("Preferences").exists());
        assert!(!target.join("updates/transaction.json").exists());
        assert!(events.iter().any(|s| s.label.contains("校验")));
        assert_eq!(
            prepared
                .records
                .iter()
                .filter(|r| r.source.ends_with("Preferences"))
                .count(),
            1
        );
        prepared.commit().unwrap();
        let _next = Engine::open(&prepared.database, 1002).unwrap();
        let after = AppPaths::from_defaults(paths.defaults, paths.locator).unwrap();
        assert_eq!(
            after.resolve_library(None).unwrap(),
            canon(&target).unwrap().join("library")
        );
        assert!(pending(&after).unwrap().is_none());
        #[cfg(unix)]
        let claim = simulate_interrupted_claim(&profile.join("Preferences"));
        let (recorded_stage, old) = inventory_paths(&after, &_next.snapshot()).unwrap();
        assert!(recorded_stage.unwrap().join("cleanup.json").exists());
        #[cfg(unix)]
        assert!(claim.exists() && !profile.join("Preferences").exists() && old.contains(&claim));
        #[cfg(windows)]
        assert!(old.iter().any(|f| f.ends_with("Preferences")));
        let restored = remaining_cleanup(&after, &_next.snapshot()).unwrap();
        #[cfg(unix)]
        assert!(!claim.exists() && profile.join("Preferences").exists());
        assert_eq!(restored.len(), prepared.records.len());
        let j = journal(&after).unwrap().unwrap();
        let receipt = j
            .destination
            .join(format!(".studyapp-migration-{}", j.id))
            .join("cleanup.json");
        fs::write(receipt, b"[]").unwrap();
        assert!(remaining_cleanup(&after, &_next.snapshot()).is_err());
    }
    #[test]
    fn unrelated_and_overlapping_targets_preserve_locator() {
        let (base, paths, mut engine) = fixture();
        let nonempty = base.join("nonempty");
        fs::create_dir(&nonempty).unwrap();
        fs::write(nonempty.join("unrelated"), b"preserve").unwrap();
        for dest in [&nonempty, &paths.effective.data.join("inside"), &base] {
            assert!(prepare(&mut engine, paths.clone(), dest, 1001, |_| {}).is_err());
            assert!(!paths.locator.exists());
        }
        assert_eq!(fs::read(nonempty.join("unrelated")).unwrap(), b"preserve");
    }
    #[cfg(windows)]
    #[test]
    fn held_profile_resumes_at_recorded_destination() {
        use std::os::windows::fs::OpenOptionsExt;
        let (base, paths, mut engine) = fixture();
        let profile = paths.effective.cache.join("editor");
        fs::create_dir_all(&profile).unwrap();
        let path = profile.join("locked");
        fs::write(&path, b"held").unwrap();
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(path)
            .unwrap();
        let target = base.join("next");
        let error = prepare(&mut engine, paths.clone(), &target, 1001, |_| {})
            .err()
            .unwrap();
        assert!(format!("{error:#}").contains("重新打开"), "{error:#}");
        assert_eq!(pending(&paths).unwrap(), Some(canon(&target).unwrap()));
        drop(lock);
        let mut next = prepare(&mut engine, paths, &target, 1001, |_| {}).unwrap();
        next.commit().unwrap();
        assert_eq!(
            fs::read(target.join("cache/editor/locked")).unwrap(),
            b"held"
        );
    }
    #[test]
    fn changed_locator_prevents_commit_and_unlisted_files_are_preserved() {
        let (base, paths, mut engine) = fixture();
        let target = base.join("new");
        let mut candidate = prepare(&mut engine, paths.clone(), &target, 1001, |_| {}).unwrap();
        let other = AppPathConfig {
            schema: 1,
            root: base.join("changed"),
            library: None,
        };
        publish_config(&paths.locator, &other).unwrap();
        assert!(candidate.commit().is_err());
        let stored: AppPathConfig =
            serde_json::from_slice(&fs::read(paths.locator).unwrap()).unwrap();
        assert_eq!(stored.root, other.root);
    }
    #[test]
    fn exact_unlink_rechecks_identity_content_and_path_for_each_confirmation() {
        let base = tempfile::tempdir().unwrap().keep();
        let source = base.join("confirmed");
        let kept = base.join("unselected");
        fs::write(&source, b"old").unwrap();
        fs::write(&kept, b"old").unwrap();
        let file = plain_handle(&source, false, false).unwrap();
        let id = crate::app_usage::identity(&file).unwrap();
        let checksum = digest(file, |_| {}).unwrap();
        assert!(unlink_exact(&kept, id, &checksum).is_err());
        fs::write(&source, b"changed").unwrap();
        assert!(unlink_exact(&source, id, &checksum).is_err());
        fs::write(&source, b"old").unwrap();
        unlink_exact(&source, id, &checksum).unwrap();
        assert!(!source.exists());
        assert!(kept.exists());
    }
    #[test]
    fn committed_offline_target_never_creates_replacement_database() {
        let (base, paths, mut engine) = fixture();
        let target = base.join("new");
        let mut migration = prepare(&mut engine, paths.clone(), &target, 1001, |_| {}).unwrap();
        migration.commit().unwrap();
        drop(migration);
        let db = target.join("library/studyapp.sqlite3");
        fs::rename(&db, db.with_extension("offline")).unwrap();
        let active = AppPaths::from_defaults(paths.defaults, paths.locator).unwrap();
        assert!(pending(&active).is_err());
        assert!(!db.exists());
    }
    #[test]
    fn published_library_and_cache_remain_consistent_after_library_switch() {
        let base = tempfile::tempdir().unwrap().keep();
        let locator = base.join("config/storage-location.json");
        let root = base.join("新位置");
        let config = AppPathConfig {
            schema: 1,
            root: root.clone(),
            library: Some(root.join("library")),
        };
        publish_config(&locator, &config).unwrap();
        remember_library(&base.join("config/location.json"), &base.join("other")).unwrap();
        let stored: AppPathConfig =
            serde_json::from_slice(&std::fs::read(locator).unwrap()).unwrap();
        assert_eq!(stored.root, root);
        assert_eq!(stored.library, Some(base.join("other")));
    }
}
