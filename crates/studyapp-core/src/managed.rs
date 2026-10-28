//! SQLite write intents bridge file publication and the snapshot transaction.
//! Staging/original files are deliberately retained; no user file is deleted.
use crate::{
    Note, NoteVersion, notes,
    storage::{self, State},
};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write as _},
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Write {
    note_id: String,
    expected_hash: Option<String>,
    content: String,
}

#[derive(Serialize, Deserialize)]
struct Intent {
    id: String,
    state: State,
    versions: Vec<NoteVersion>,
    previous_count: usize,
    replace: bool,
    writes: Vec<Write>,
}

#[cfg(test)]
thread_local! {
    pub(crate) static FAIL_LINK_PROBE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(crate) fn check_filesystem(database: &Path) -> Result<()> {
    let directory = database.parent().context("资料库目录不存在")?;
    let probe = directory.join(format!(".studyapp-link-probe-{}", uuid::Uuid::new_v4()));
    let linked = probe.with_extension("link");
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)?
        .sync_all()?;
    #[cfg(test)]
    if FAIL_LINK_PROBE.with(|fail| fail.replace(false)) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "hard-link probe failure fixture",
        )
        .into());
    }
    std::fs::hard_link(&probe, &linked).context(
        "资料库所在文件系统不支持安全托管写入；未创建写入意图，请使用支持硬链接的本地 NTFS 资料库",
    )?;
    Ok(())
}

pub(crate) fn validate_id(id: &str) -> Result<()> {
    let uuid = uuid::Uuid::parse_str(id).context("笔记 ID 必须是 UUID")?;
    ensure!(uuid.to_string() == id, "笔记 UUID 必须使用小写连字符格式");
    Ok(())
}

pub(crate) fn path(database: &Path, id: &str) -> Result<PathBuf> {
    validate_id(id)?;
    Ok(database
        .parent()
        .context("资料库目录不存在")?
        .join("notes")
        .join(format!("{id}.md")))
}

pub(crate) fn rebase(state: &mut State, database: &Path) -> Result<()> {
    for note in state
        .notes
        .iter_mut()
        .filter(|note| note.managed && !note.detached)
    {
        note.path = path(database, &note.id)?.to_string_lossy().into_owned();
    }
    Ok(())
}

fn validate_content(content: &str) -> Result<()> {
    ensure!(
        content.len() as u64 <= notes::MAX_BYTES,
        "笔记超过 10 MiB 限制"
    );
    Ok(())
}

fn read_handle(file: &mut File) -> Result<String> {
    ensure!(
        file.metadata()?.len() <= notes::MAX_BYTES,
        "笔记超过 10 MiB 限制"
    );
    let mut bytes = Vec::new();
    file.take(notes::MAX_BYTES + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= notes::MAX_BYTES,
        "笔记超过 10 MiB 限制"
    );
    String::from_utf8(bytes).context("笔记不是有效的 UTF-8 文本")
}

fn plain_file(path: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "托管笔记不能是目录或链接"
    );
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(
            metadata.file_attributes() & 0x400 == 0,
            "托管笔记不能是重解析链接"
        );
    }
    Ok(())
}

pub(crate) fn read_content(path: &Path) -> Result<String> {
    plain_file(path)?;
    read_handle(&mut File::open(path)?)
}

fn current(path: &Path) -> Result<Option<String>> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => read_content(path).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn create(
    state: &mut State,
    database: &Path,
    id: String,
    title: String,
    content: String,
    now: i64,
    offline: bool,
) -> Result<Write> {
    validate_id(&id)?;
    validate_content(&content)?;
    ensure!(title.chars().count() <= 200, "笔记标题不能超过 200 字符");
    ensure!(
        !state.notes.iter().any(|note| note.id == id),
        "笔记 ID 已存在"
    );
    let path = path(database, &id)?;
    // create-only is checked again after the durable intent is recorded.
    ensure!(
        !path.try_exists()?,
        "托管路径已存在，请先导入或检查已有文件"
    );
    state.notes.push(Note {
        id: id.clone(),
        path: path.to_string_lossy().into_owned(),
        title: if title.trim().is_empty() {
            notes::title(&content, Path::new("未命名笔记.md"))
        } else {
            title.trim().into()
        },
        title_override: false,
        managed: true,
        folder_id: None,
        detached: false,
        missing: false,
        error: None,
        created_at: now,
        updated_at: now,
        revision: 0,
    });
    notes::append_version(
        state,
        state.notes.len() - 1,
        content.clone(),
        now,
        offline,
        offline,
    );
    if !title.trim().is_empty() {
        state.notes.last_mut().expect("created note").title = title.trim().into();
    }
    Ok(Write {
        note_id: id,
        expected_hash: None,
        content,
    })
}

pub(crate) fn edit(
    state: &mut State,
    database: &Path,
    id: &str,
    expected: &str,
    content: String,
    now: i64,
    offline: bool,
) -> Result<Option<Write>> {
    validate_content(&content)?;
    let index = state
        .notes
        .iter()
        .position(|note| note.id == id)
        .context("找不到笔记")?;
    ensure!(!state.notes[index].detached, "笔记已取消管理，请先重新导入");
    ensure!(
        state.notes[index].managed,
        "导入的外部笔记为只读，请创建托管副本后编辑"
    );
    let previous = state
        .versions
        .iter()
        .rev()
        .find(|version| version.note_id == id)
        .context("笔记版本不存在")?;
    ensure!(
        previous.hash == expected,
        "笔记版本已变化，草稿未覆盖，请重新比较后保存"
    );
    let disk = read_content(&path(database, id)?).context("无法读取托管原文，草稿未覆盖")?;
    ensure!(
        notes::hash(&disk) == expected,
        "笔记文件已被外部修改，草稿未覆盖，请重新比较后保存"
    );
    if notes::hash(&content) == expected {
        return Ok(None);
    }
    notes::append_version(state, index, content.clone(), now, offline, false);
    state.notes[index].missing = false;
    state.notes[index].error = None;
    state.candidates.remove(id);
    state.offline_unreconciled_notes.remove(id);
    Ok(Some(Write {
        note_id: id.into(),
        expected_hash: Some(expected.into()),
        content,
    }))
}

pub(crate) fn export(
    state: &State,
    id: &str,
    version: Option<&str>,
    destination: &Path,
) -> Result<()> {
    ensure!(state.notes.iter().any(|note| note.id == id), "找不到笔记");
    let version = state
        .versions
        .iter()
        .rev()
        .find(|v| v.note_id == id && version.is_none_or(|id| id == v.id))
        .context("找不到指定笔记版本")?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .context("导出目标已存在或无法创建，请选择新文件名")?;
    file.write_all(version.content.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

pub(crate) fn restore_writes(state: &mut State, database: &Path, now: i64) -> Result<Vec<Write>> {
    rebase(state, database)?;
    let mut writes = Vec::new();
    for index in 0..state.notes.len() {
        if !state.notes[index].managed || state.notes[index].detached {
            continue;
        }
        let note_id = state.notes[index].id.clone();
        state.notes[index].missing = false;
        state.notes[index].error = None;
        state.offline_unreconciled_notes.remove(&note_id);
        let content = state
            .versions
            .iter()
            .rev()
            .find(|v| v.note_id == note_id)
            .context("托管笔记缺少版本")?
            .content
            .clone();
        let disk = current(&path(database, &note_id)?)?;
        if disk.as_deref() == Some(&content) {
            continue;
        }
        if let Some(local) = &disk {
            notes::append_version(state, index, local.clone(), now, true, false);
            notes::append_version(state, index, content.clone(), now, true, false);
        }
        writes.push(Write {
            note_id,
            expected_hash: disk.as_deref().map(notes::hash),
            content,
        });
    }
    Ok(writes)
}

pub(crate) fn prepare(
    conn: &mut Connection,
    previous: &State,
    next: State,
    writes: Vec<Write>,
    replace: bool,
) -> Result<()> {
    let intent = Intent {
        id: uuid::Uuid::new_v4().to_string(),
        versions: next
            .versions
            .iter()
            .skip(if replace { 0 } else { previous.versions.len() })
            .cloned()
            .collect(),
        previous_count: previous.versions.len(),
        state: next,
        replace,
        writes,
    };
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let saved: String = tx.query_row("SELECT data FROM app_state WHERE id=1", [], |row| {
        row.get(0)
    })?;
    ensure!(
        saved == serde_json::to_string(previous)?,
        "资料库已被另一进程修改，请重新打开后保存"
    );
    tx.execute(
        "INSERT INTO write_intents(id,data) VALUES(1,?1)",
        [serde_json::to_string(&intent)?],
    )?;
    tx.commit()?;
    Ok(())
}

fn directory(database: &Path) -> Result<PathBuf> {
    let parent = database.parent().context("资料库目录不存在")?;
    let directory = parent.join("notes");
    match std::fs::create_dir(&directory) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let metadata = std::fs::symlink_metadata(&directory)?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "notes 必须是资料库内的普通目录"
    );
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(
            metadata.file_attributes() & 0x400 == 0,
            "notes 目录不能是重解析链接"
        );
    }
    ensure!(
        std::fs::canonicalize(&directory)?.parent() == Some(parent),
        "托管目录不能指向资料库外部"
    );
    Ok(directory)
}

/// Returns divergent content instead of overwriting a file changed after CAS.
fn publish(database: &Path, intent_id: &str, write: &Write) -> Result<Option<String>> {
    let directory = directory(database)?;
    let destination = path(database, &write.note_id)?;
    let existing = current(&destination)?;
    if existing.as_deref() == Some(&write.content) {
        return Ok(None);
    }
    if let Some(existing) = &existing
        && Some(notes::hash(existing)) != write.expected_hash
    {
        return Ok(Some(existing.clone()));
    }
    let backup = directory.join(format!(".original-{intent_id}-{}.md", write.note_id));
    ensure!(
        existing.is_some() || write.expected_hash.is_none() || backup.exists(),
        "原文已移走，写入意图仍保留，请恢复原文后重试"
    );
    let staged = directory.join(format!(".pending-{}.md", uuid::Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged)?;
    file.write_all(write.content.as_bytes())?;
    file.sync_all()?;
    drop(file);
    if existing.is_some() {
        plain_file(&destination)?;
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // Deny concurrent writers, but allow our rename to the retained original.
            options.share_mode(4);
        }
        let mut original = options
            .open(&destination)
            .context("笔记正被其他程序占用，写入意图已保留")?;
        let actual = read_handle(&mut original)?;
        if Some(notes::hash(&actual)) != write.expected_hash {
            return Ok(Some(actual));
        }
        let backup = if backup.try_exists()? {
            directory.join(format!(".original-{}.md", uuid::Uuid::new_v4()))
        } else {
            backup
        };
        std::fs::rename(&destination, &backup).context("无法保留原文，未覆盖笔记")?;
        // The open handle protects the original from Windows writers until publication.
        std::fs::hard_link(&staged, &destination)
            .context("无法发布笔记；原文及草稿已保留，可重试恢复")?;
        drop(original);
    } else {
        std::fs::hard_link(&staged, &destination)
            .context("托管目标已出现或无法发布；草稿已保留")?;
    }
    OpenOptions::new()
        .write(true)
        .open(&destination)?
        .sync_all()?;
    #[cfg(unix)]
    File::open(&directory)?.sync_all()?;
    Ok(None)
}

/// None means no intent; Some reports whether a committed recovery found a conflict.
pub(crate) fn recover(
    conn: &mut Connection,
    database: &Path,
    state: &mut State,
) -> Result<Option<bool>> {
    let raw: Option<String> = conn
        .query_row("SELECT data FROM write_intents WHERE id=1", [], |row| {
            row.get(0)
        })
        .optional()?;
    let Some(raw) = raw else {
        return Ok(None);
    };
    let mut intent: Intent = serde_json::from_str(&raw).context("托管写入恢复记录损坏")?;
    validate_id(&intent.id)?;
    let persisted = storage::load(conn)?.context("资料库状态不存在")?;
    let mut versions = if intent.replace {
        Vec::new()
    } else {
        ensure!(
            persisted.versions.len() == intent.previous_count,
            "恢复记录与当前版本数量不符"
        );
        persisted.versions.as_ref().clone()
    };
    versions.extend(intent.versions);
    intent.state.versions = Arc::new(versions);
    rebase(&mut intent.state, database)?;
    intent.state.validate()?;
    let mut conflict = false;
    for write in &intent.writes {
        validate_content(&write.content)?;
        let index = intent
            .state
            .notes
            .iter()
            .position(|note| note.managed && note.id == write.note_id)
            .context("恢复写入未关联托管笔记")?;
        ensure!(
            intent
                .state
                .versions
                .iter()
                .rev()
                .find(|v| v.note_id == write.note_id)
                .is_some_and(|v| v.content == write.content),
            "恢复写入与版本快照不一致"
        );
        if let Some(external) = publish(database, &intent.id, write)? {
            let now = intent.state.notes[index].updated_at;
            notes::append_version(&mut intent.state, index, external, now, true, false);
            intent.state.notes[index].error = Some(
                "写入期间发现外部修改：保留了外部文件，草稿已保存在版本历史，请比较后重新保存"
                    .into(),
            );
            conflict = true;
        }
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    storage::write_state(&tx, &intent.state, intent.previous_count, intent.replace)?;
    tx.execute("DELETE FROM write_intents WHERE id=1", [])?;
    tx.commit()?;
    *state = intent.state;
    Ok(Some(conflict))
}
