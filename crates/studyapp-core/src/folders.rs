//! Logical notebook folders. No operation in this module touches source files.
use crate::{MAX_FOLDER_DEPTH, NoteFolder, storage::State};
use anyhow::{Context, Result, ensure};
use std::collections::{HashMap, HashSet};

fn normalized_name(name: &str) -> Result<String> {
    ensure!(
        !name
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '/' | '\\')),
        "目录名称不能包含路径分隔符或控制字符"
    );
    let name = name.trim();
    ensure!(!name.is_empty(), "目录名称不能为空");
    ensure!(name != "." && name != "..", "目录名称不能是 . 或 ..");
    ensure!(name.chars().count() <= 200, "目录名称不能超过 200 字符");
    Ok(name.to_owned())
}

pub(crate) fn require_target(state: &State, folder_id: Option<&str>) -> Result<()> {
    if let Some(id) = folder_id {
        ensure!(
            state.folders.iter().any(|folder| folder.id == id),
            "目标目录不存在"
        );
    }
    Ok(())
}

/// Shared by commands and database/backup validation. Walking each ancestry is
/// bounded by MAX_FOLDER_DEPTH, including for an untrusted imported hierarchy.
pub(crate) fn validate(state: &State) -> Result<()> {
    let mut folders = HashMap::with_capacity(state.folders.len());
    let mut names = HashSet::with_capacity(state.folders.len());
    for folder in &state.folders {
        ensure!(
            !folder.id.is_empty() && folders.insert(folder.id.as_str(), folder).is_none(),
            "目录 ID 为空或重复"
        );
        ensure!(
            normalized_name(&folder.name)? == folder.name,
            "目录名称不能包含首尾空白"
        );
        ensure!(
            names.insert((folder.parent_id.as_deref(), folder.name.to_lowercase())),
            "同一父目录下的目录名称不能重复"
        );
    }
    for folder in &state.folders {
        let mut current = Some(folder.id.as_str());
        let mut visited = HashSet::with_capacity(MAX_FOLDER_DEPTH);
        while let Some(id) = current {
            ensure!(visited.insert(id), "目录不能移动到自身或其子目录中");
            ensure!(
                visited.len() <= MAX_FOLDER_DEPTH,
                "目录最多支持 {MAX_FOLDER_DEPTH} 层"
            );
            current = folders
                .get(id)
                .context("目录的父目录不存在")?
                .parent_id
                .as_deref();
        }
    }
    ensure!(
        state.notes.iter().all(|note| note
            .folder_id
            .as_deref()
            .is_none_or(|id| folders.contains_key(id))),
        "笔记关联的目录不存在"
    );
    Ok(())
}

pub(crate) fn create(state: &mut State, name: String, parent_id: Option<String>) -> Result<()> {
    require_target(state, parent_id.as_deref())?;
    state.folders.push(NoteFolder {
        id: uuid::Uuid::new_v4().to_string(),
        name: normalized_name(&name)?,
        parent_id,
    });
    validate(state)
}

pub(crate) fn rename(state: &mut State, folder_id: &str, name: String) -> Result<()> {
    let name = normalized_name(&name)?;
    state
        .folders
        .iter_mut()
        .find(|folder| folder.id == folder_id)
        .context("找不到目录")?
        .name = name;
    validate(state)
}

pub(crate) fn move_folder(
    state: &mut State,
    folder_id: &str,
    parent_id: Option<String>,
) -> Result<()> {
    require_target(state, parent_id.as_deref())?;
    state
        .folders
        .iter_mut()
        .find(|folder| folder.id == folder_id)
        .context("找不到目录")?
        .parent_id = parent_id;
    // Includes every descendant: moving a short root can make its subtree too deep.
    validate(state)
}

pub(crate) fn delete(state: &mut State, folder_id: &str) -> Result<()> {
    let index = state
        .folders
        .iter()
        .position(|folder| folder.id == folder_id)
        .context("找不到目录")?;
    ensure!(
        !state
            .folders
            .iter()
            .any(|folder| folder.parent_id.as_deref() == Some(folder_id)),
        "目录中仍有子目录，请先移出或删除空的子目录"
    );
    ensure!(
        !state
            .notes
            .iter()
            .any(|note| note.folder_id.as_deref() == Some(folder_id)),
        "目录中仍有笔记，请先移出笔记；已取消管理的笔记也需要移出"
    );
    state.folders.remove(index);
    Ok(())
}

pub(crate) fn move_note(
    state: &mut State,
    note_id: &str,
    folder_id: Option<String>,
    now: i64,
) -> Result<()> {
    require_target(state, folder_id.as_deref())?;
    let note = state
        .notes
        .iter_mut()
        .find(|note| note.id == note_id)
        .context("找不到笔记")?;
    if note.folder_id != folder_id {
        let revision = note.revision.checked_add(1).context("笔记修订号超出范围")?;
        note.folder_id = folder_id;
        note.updated_at = now;
        note.revision = revision;
    }
    Ok(())
}
