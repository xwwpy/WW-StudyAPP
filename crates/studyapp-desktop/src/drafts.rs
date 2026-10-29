use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct NoteDraft {
    pub note_id: Option<String>,
    pub expected_hash: Option<String>,
    #[serde(default)]
    pub folder_id: Option<String>,
    #[serde(default)]
    pub asset_group: String,
    pub title: String,
    pub content: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DraftStatus {
    Active,
    Saved { note_id: String, hash: String },
    Deleted,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftRecord {
    pub schema: u32,
    pub id: String,
    pub revision: u64,
    pub created_at: i64,
    pub updated_at: i64,
    pub status: DraftStatus,
    pub draft: NoteDraft,
    #[serde(default)]
    pub source_path: Option<PathBuf>,
    #[serde(default)]
    pub media_bindings: BTreeMap<String, String>,
    #[serde(default)]
    pub legacy_digest: Option<String>,
    #[serde(default)]
    pub save_target: Option<String>,
}
impl DraftRecord {
    pub fn new(mut draft: NoteDraft, now: i64) -> Self {
        if draft.asset_group.is_empty() {
            draft.asset_group = crate::media::asset_group(draft.note_id.as_deref(), &draft.content);
        }
        Self {
            schema: 2,
            id: uuid::Uuid::new_v4().to_string(),
            revision: 1,
            created_at: now,
            updated_at: now,
            status: DraftStatus::Active,
            draft,
            source_path: None,
            media_bindings: BTreeMap::new(),
            legacy_digest: None,
            save_target: None,
        }
    }
    pub fn active(&self) -> bool {
        self.status == DraftStatus::Active
    }
    pub fn update(&mut self, draft: NoteDraft, now: i64) {
        if self.draft != draft {
            self.draft = draft;
            self.touch(now);
        }
    }
    pub fn touch(&mut self, now: i64) {
        self.revision = self
            .revision
            .checked_add(1)
            .expect("draft revision overflow");
        self.updated_at = now;
    }
    pub fn retire(&mut self, status: DraftStatus, now: i64) {
        self.status = status;
        self.touch(now);
    }
}

const MAX_JSON_BYTES: u64 = 64 * 1024 * 1024;

/// A derived protection set for storage views. Rebuild at draft lifecycle
/// boundaries, never once per rendered file. Destructive actions still consult
/// the authoritative records before acting.
pub fn protected_note_ids<'a>(
    records: impl IntoIterator<Item = &'a DraftRecord>,
) -> std::collections::HashSet<String> {
    records
        .into_iter()
        .filter(|record| record.active())
        .flat_map(|record| record.draft.note_id.iter().chain(record.save_target.iter()))
        .cloned()
        .collect()
}
fn ordinary(path: &Path, directory: bool) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    ensure!(
        !metadata.file_type().is_symlink()
            && if directory {
                metadata.is_dir()
            } else {
                metadata.is_file()
            },
        "草稿路径必须是普通文件或目录"
    );
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(
            metadata.file_attributes() & 0x400 == 0,
            "草稿路径不能是重解析链接"
        );
    }
    Ok(())
}
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    ordinary(path, false)?;
    let file = std::fs::File::open(path)?;
    ensure!(file.metadata()?.len() <= MAX_JSON_BYTES, "草稿超过大小限制");
    let mut bytes = Vec::new();
    file.take(MAX_JSON_BYTES + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= MAX_JSON_BYTES, "草稿超过大小限制");
    Ok(serde_json::from_slice(&bytes)?)
}
fn validate(record: &DraftRecord) -> Result<()> {
    ensure!(
        record.schema == 2 && record.revision > 0,
        "不支持的草稿格式"
    );
    ensure!(
        uuid::Uuid::parse_str(&record.id).is_ok_and(|id| id.to_string() == record.id),
        "草稿身份无效"
    );
    ensure!(
        record.draft.content.len() <= 10 * 1024 * 1024,
        "Markdown 超过 10 MiB"
    );
    ensure!(record.media_bindings.len() <= 200, "草稿图片绑定过多");
    Ok(())
}
fn read_record(path: &Path) -> Result<DraftRecord> {
    let record: DraftRecord = read_json(path)?;
    validate(&record)?;
    ensure!(
        path.file_stem().and_then(|s| s.to_str()) == Some(record.id.as_str()),
        "草稿文件身份不一致"
    );
    Ok(record)
}
fn write_record(library: &str, record: &DraftRecord) -> Result<()> {
    validate(record)?;
    let path = DraftStore::record_path(library, &record.id)?;
    let parent = path.parent().context("草稿目录无效")?;
    std::fs::create_dir_all(parent)?;
    ordinary(parent, true)?;
    match std::fs::symlink_metadata(&path) {
        Ok(_) => {
            let previous = read_record(&path)?;
            ensure!(record.revision >= previous.revision, "忽略过期草稿写入");
            if record.revision == previous.revision {
                ensure!(previous == *record, "相同草稿版本的内容不一致");
                return Ok(());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let bytes = serde_json::to_vec(record)?;
    ensure!(bytes.len() as u64 <= MAX_JSON_BYTES, "草稿超过大小限制");
    let next = parent.join(format!("{}.{}.next", record.id, uuid::Uuid::new_v4()));
    let mut output = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&next)?;
    output.write_all(&bytes)?;
    output.sync_all()?;
    drop(output);
    std::fs::rename(next, &path)?;
    #[cfg(unix)]
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}
fn catalog(library: &str) -> (Vec<DraftRecord>, Vec<String>) {
    let mut records = Vec::new();
    let mut errors = Vec::new();
    let directory = DraftStore::directory(library);
    let scan = || -> Result<Vec<PathBuf>> {
        match ordinary(&directory, true) {
            Ok(()) => {}
            Err(_) if !directory.try_exists().unwrap_or(true) => return Ok(Vec::new()),
            Err(error) => return Err(error),
        }
        let mut paths = Vec::new();
        for entry in std::fs::read_dir(&directory)? {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "json") {
                paths.push(path);
            }
        }
        Ok(paths)
    };
    match scan() {
        Ok(paths) => {
            for path in paths {
                match read_record(&path) {
                    Ok(record) => records.push(record),
                    Err(error) => errors.push(format!("{}：{error:#}", path.display())),
                }
            }
        }
        Err(error) => errors.push(format!("草稿目录读取失败，原文件保留：{error:#}")),
    }
    let legacy_path = DraftStore::path(library);
    if legacy_path.try_exists().unwrap_or(true) {
        let migrate = || -> Result<Option<DraftRecord>> {
            ordinary(&legacy_path, false)?;
            let bytes = std::fs::read(&legacy_path)?;
            ensure!(bytes.len() as u64 <= MAX_JSON_BYTES, "旧草稿超过大小限制");
            let draft: Option<NoteDraft> = serde_json::from_slice(&bytes)?;
            let Some(draft) = draft else {
                return Ok(None);
            };
            let digest = Sha256::digest(&bytes);
            let id = uuid::Uuid::from_bytes(digest[..16].try_into().unwrap()).to_string();
            let path = DraftStore::record_path(library, &id)?;
            if path.try_exists()? {
                return Ok(None);
            }
            let mut draft = draft;
            if draft.asset_group.is_empty() {
                draft.asset_group = draft
                    .note_id
                    .as_deref()
                    .and_then(|note_id| uuid::Uuid::parse_str(note_id).ok())
                    .map_or_else(|| id.clone(), |note_id| note_id.to_string());
            }
            let mut record = DraftRecord::new(draft, 0);
            record.id = id;
            record.legacy_digest = Some(format!("{digest:x}"));
            write_record(library, &record)?;
            Ok(Some(record))
        };
        match migrate() {
            Ok(Some(record)) => records.push(record),
            Ok(None) => {}
            Err(error) => errors.push(format!("旧草稿未修改，可重试恢复：{error:#}")),
        }
    }
    records.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    (records, errors)
}

enum Request {
    Load(String),
    Save(String, Box<DraftRecord>, Option<u64>),
    Drain(String, u64),
    Stop,
}
pub enum DraftUpdate {
    Catalog {
        library: String,
        records: Vec<DraftRecord>,
        errors: Vec<String>,
    },
    Written {
        library: String,
        id: String,
        revision: u64,
        token: Option<u64>,
    },
    Error {
        library: String,
        id: Option<String>,
        revision: u64,
        token: Option<u64>,
        message: String,
    },
    Drained {
        library: String,
        token: u64,
        result: Result<(), String>,
    },
}
struct Pending {
    record: Box<DraftRecord>,
    due: Instant,
}
type Key = (String, String);
fn publish(
    library: &str,
    record: Box<DraftRecord>,
    token: Option<u64>,
    updates: &mpsc::Sender<DraftUpdate>,
    failed: &mut BTreeMap<Key, Box<DraftRecord>>,
) -> Result<(), String> {
    let key = (library.to_owned(), record.id.clone());
    if let Ok(path) = DraftStore::record_path(library, &record.id)
        && let Ok(previous) = read_record(&path)
        && (previous.revision > record.revision
            || (!previous.active() && previous != *record && previous.revision >= record.revision))
    {
        if token.is_none() {
            return Ok(());
        }
        let message = "草稿已由更新的操作完成，请重新选择。".to_owned();
        let _ = updates.send(DraftUpdate::Error {
            library: library.into(),
            id: Some(record.id),
            revision: record.revision,
            token,
            message: message.clone(),
        });
        return Err(message);
    }
    match write_record(library, &record) {
        Ok(()) => {
            failed.remove(&key);
            let _ = updates.send(DraftUpdate::Written {
                library: library.into(),
                id: record.id,
                revision: record.revision,
                token,
            });
            Ok(())
        }
        Err(error) => {
            let message = format!("草稿保存失败，原文件和编辑内容仍保留：{error:#}");
            // A terminal record remains authoritative even if an old queued save follows it.
            if failed
                .get(&key)
                .is_none_or(|old| old.revision <= record.revision)
            {
                failed.insert(key, record.clone());
            }
            let _ = updates.send(DraftUpdate::Error {
                library: library.into(),
                id: Some(record.id),
                revision: record.revision,
                token,
                message: message.clone(),
            });
            Err(message)
        }
    }
}
fn flush(
    library: Option<&str>,
    pending: &mut BTreeMap<Key, Pending>,
    failed: &mut BTreeMap<Key, Box<DraftRecord>>,
    updates: &mpsc::Sender<DraftUpdate>,
    due_only: bool,
) {
    let keys = pending
        .iter()
        .filter(|((root, _), p)| {
            library.is_none_or(|l| l == root) && (!due_only || p.due <= Instant::now())
        })
        .map(|(key, _)| key.clone())
        .collect::<Vec<_>>();
    for key in keys {
        let pending = pending.remove(&key).unwrap();
        let _ = publish(&key.0, pending.record, None, updates, failed);
    }
}
/// One serial writer, independently coalesced records, bounded first-write deadlines.
/// Terminal records are durable tombstones; no source, asset or draft file is removed.
pub struct DraftStore {
    sender: mpsc::Sender<Request>,
    pub receiver: mpsc::Receiver<DraftUpdate>,
    thread: Option<thread::JoinHandle<()>>,
}
impl DraftStore {
    pub fn new() -> Self {
        let (sender, requests) = mpsc::channel();
        let (updates, receiver) = mpsc::channel();
        let thread = thread::spawn(move || {
            let mut pending: BTreeMap<Key, Pending> = BTreeMap::new();
            let mut failed = BTreeMap::new();
            loop {
                #[cfg(feature = "update-acceptance")]
                let held = crate::update_acceptance::hold_autosave();
                #[cfg(not(feature = "update-acceptance"))]
                let held = false;
                if !held {
                    flush(None, &mut pending, &mut failed, &updates, true);
                }
                let delay = pending
                    .values()
                    .map(|p| p.due.saturating_duration_since(Instant::now()))
                    .min()
                    .unwrap_or(Duration::from_secs(60));
                let delay = if held { Duration::from_secs(60) } else { delay };
                match requests.recv_timeout(delay) {
                    Ok(Request::Save(library, record, token)) => {
                        let key = (library.clone(), record.id.clone());
                        if token.is_some() || !record.active() {
                            pending.remove(&key);
                            let _ = publish(&library, record, token, &updates, &mut failed);
                        } else if failed
                            .get(&key)
                            .is_none_or(|old| old.revision < record.revision)
                        {
                            let due = pending
                                .get(&key)
                                .map(|p| p.due)
                                .unwrap_or_else(|| Instant::now() + Duration::from_millis(650));
                            if pending
                                .get(&key)
                                .is_none_or(|old| old.record.revision <= record.revision)
                            {
                                pending.insert(key, Pending { record, due });
                            }
                        }
                    }
                    Ok(Request::Load(library)) => {
                        flush(Some(&library), &mut pending, &mut failed, &updates, false);
                        let (records, errors) = catalog(&library);
                        let _ = updates.send(DraftUpdate::Catalog {
                            library,
                            records,
                            errors,
                        });
                    }
                    Ok(Request::Drain(library, token)) => {
                        flush(Some(&library), &mut pending, &mut failed, &updates, false);
                        let retry = failed
                            .iter()
                            .filter(|((root, _), _)| root == &library)
                            .map(|(_, r)| r.clone())
                            .collect::<Vec<_>>();
                        let mut failures = Vec::new();
                        for record in retry {
                            if let Err(error) =
                                publish(&library, record, None, &updates, &mut failed)
                            {
                                failures.push(error);
                            }
                        }
                        let result = if failures.is_empty() {
                            Ok(())
                        } else {
                            Err(failures.join("；"))
                        };
                        let _ = updates.send(DraftUpdate::Drained {
                            library,
                            token,
                            result,
                        });
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Ok(Request::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                        flush(None, &mut pending, &mut failed, &updates, false);
                        break;
                    }
                }
            }
        });
        Self {
            sender,
            receiver,
            thread: Some(thread),
        }
    }
    /// Read-only legacy location. New writes never mutate this file.
    pub fn path(library: &str) -> PathBuf {
        Path::new(library)
            .parent()
            .unwrap_or(Path::new("."))
            .join("note-draft.json")
    }
    pub fn directory(library: &str) -> PathBuf {
        Path::new(library)
            .parent()
            .unwrap_or(Path::new("."))
            .join("drafts")
    }
    pub fn record_path(library: &str, id: &str) -> Result<PathBuf> {
        ensure!(
            !library.is_empty()
                && uuid::Uuid::parse_str(id).is_ok_and(|value| value.to_string() == id),
            "草稿身份无效"
        );
        Ok(Self::directory(library).join(format!("{id}.json")))
    }
    pub fn load(&self, library: &str) {
        let _ = self.sender.send(Request::Load(library.into()));
    }
    pub fn save_record(&self, library: &str, record: DraftRecord) {
        let _ = self
            .sender
            .send(Request::Save(library.into(), Box::new(record), None));
    }
    pub fn persist_record(&self, library: &str, record: DraftRecord, token: u64) {
        let _ = self
            .sender
            .send(Request::Save(library.into(), Box::new(record), Some(token)));
    }
    pub fn drain(&self, library: &str, token: u64) {
        let _ = self.sender.send(Request::Drain(library.into(), token));
    }
}
impl Drop for DraftStore {
    fn drop(&mut self) {
        let _ = self.sender.send(Request::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
#[path = "drafts_tests.rs"]
mod tests;
