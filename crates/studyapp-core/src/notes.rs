use crate::{storage::State, *};
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use similar::{ChangeTag, TextDiff};
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};
pub(crate) fn read(path: &Path) -> Result<String> {
    ensure!(
        path.extension()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("md") || s.eq_ignore_ascii_case("markdown")),
        "仅支持 .md 或 .markdown 文件"
    );
    let mut file =
        std::fs::File::open(path).with_context(|| format!("无法读取 {}", path.display()))?;
    let mut bytes = vec![];
    file.read_to_end(&mut bytes)?;
    let text = String::from_utf8(bytes).context("笔记不是有效的 UTF-8 文本")?;
    Ok(text.strip_prefix('\u{feff}').unwrap_or(&text).into())
}
pub(crate) fn hash(content: &str) -> String {
    format!("{:x}", Sha256::digest(content.as_bytes()))
}
pub(crate) fn title(content: &str, path: &Path) -> String {
    content
        .lines()
        .find_map(|line| {
            line.strip_prefix("# ")
                .map(str::trim)
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("未命名笔记")
        })
        .chars()
        .take(200)
        .collect()
}
pub(crate) fn append_version(
    state: &mut State,
    index: usize,
    content: String,
    now: i64,
    offline: bool,
    baseline: bool,
) {
    let note = &state.notes[index];
    let previous = state
        .versions
        .iter()
        .rev()
        .find(|v| v.note_id == note.id)
        .map(|v| v.content.as_str())
        .unwrap_or("");
    let (diff, summary) = if baseline {
        (String::new(), String::new())
    } else {
        let delta = TextDiff::from_lines(previous, &content);
        let diff = delta
            .unified_diff()
            .context_radius(3)
            .header("上一版本", "当前版本")
            .to_string();
        let mut pieces: Vec<String> = vec![];
        let mut heading = String::new();
        for change in delta.iter_all_changes() {
            let line = change.value().trim();
            if line.starts_with('#') && change.tag() != ChangeTag::Delete {
                heading = line.into();
            }
            if change.tag() == ChangeTag::Insert && !line.is_empty() {
                if !heading.is_empty() && !pieces.contains(&heading) {
                    pieces.push(heading.clone());
                }
                if pieces.len() < 16 {
                    pieces.push(line.chars().take(160).collect());
                }
            }
        }
        if pieces.is_empty() && previous != content {
            pieces.push("删除或调整了笔记内容".into());
        }
        (diff, pieces.join("\n"))
    };
    let version_id = uuid::Uuid::new_v4().to_string();
    let version = NoteVersion {
        id: version_id.clone(),
        note_id: note.id.clone(),
        hash: hash(&content),
        content,
        captured_at: now,
        offline,
        summary,
        diff,
    };
    if !state.notes[index].managed
        || (!state.notes[index].title_override
            && version.content.lines().any(|line| line.starts_with("# ")))
    {
        state.notes[index].title = title(&version.content, Path::new(&state.notes[index].path));
    }
    state.notes[index].updated_at = now;
    state.notes[index].revision += 1;
    Arc::make_mut(&mut state.versions).push(version);
    if !baseline
        && !offline
        && !state.notes[index].detached
        && !state.timer.paused
        && !state.unattributed_notes.contains(&state.notes[index].id)
    {
        state.active_versions.push(version_id);
    }
}
pub(crate) fn import(state: &mut State, database: &Path, paths: Vec<PathBuf>, now: i64) -> String {
    let mut success = 0;
    let mut duplicate = 0;
    let mut failures = vec![];
    for path in paths {
        let result = (|| -> Result<()> {
            let canonical = std::fs::canonicalize(&path)
                .with_context(|| format!("找不到 {}", path.display()))?;
            let key = canonical.to_string_lossy().into_owned();
            if let Some(index) = state.notes.iter().position(|n| same_path(&n.path, &key)) {
                if !state.notes[index].detached {
                    duplicate += 1;
                    return Ok(());
                }
                let note_id = state.notes[index].id.clone();
                // A detached managed file keeps its original path across library
                // migration. Only resume writes if it still belongs to this library.
                let managed = state.notes[index].managed
                    && same_path(
                        &key,
                        &crate::managed::path(database, &note_id)?.to_string_lossy(),
                    );
                let content = if managed {
                    crate::managed::read_content(&canonical)?
                } else {
                    read(&canonical)?
                };
                let changed = state
                    .versions
                    .iter()
                    .rev()
                    .find(|v| v.note_id == note_id)
                    .is_none_or(|v| v.hash != hash(&content));
                let note = &mut state.notes[index];
                note.detached = false;
                note.managed = managed;
                note.path = key;
                note.missing = false;
                note.error = None;
                note.updated_at = now;
                note.revision += 1;
                state.candidates.remove(&note_id);
                state.unattributed_notes.remove(&note_id);
                state.offline_unreconciled_notes.remove(&note_id);
                if changed {
                    append_version(state, index, content, now, true, false);
                }
                success += 1;
                return Ok(());
            }
            let content = read(&canonical)?;
            state.notes.push(Note {
                id: uuid::Uuid::new_v4().to_string(),
                path: key,
                title: title(&content, &canonical),
                title_override: false,
                managed: false,
                folder_id: None,
                detached: false,
                missing: false,
                error: None,
                created_at: now,
                updated_at: now,
                revision: 0,
            });
            append_version(state, state.notes.len() - 1, content, now, false, true);
            success += 1;
            Ok(())
        })();
        if let Err(e) = result {
            failures.push(format!("{}：{e:#}", path.display()));
        }
    }
    format!(
        "导入成功 {success} 份，已存在 {duplicate} 份，失败 {} 份{}",
        failures.len(),
        if failures.is_empty() {
            String::new()
        } else {
            format!("\n{}", failures.join("\n"))
        }
    )
}
fn same_path(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}
/// Rename display metadata only. Return false for a normalized same-title no-op.
pub(crate) fn rename(
    state: &mut State,
    note_id: &str,
    title: &str,
    expected_revision: u64,
    now: i64,
) -> Result<bool> {
    let note = state
        .notes
        .iter_mut()
        .find(|note| note.id == note_id)
        .context("找不到笔记")?;
    ensure!(
        note.managed && !note.detached,
        "仅支持重命名正在管理的托管笔记"
    );
    ensure!(
        note.revision == expected_revision,
        "笔记已更新，请重新打开重命名对话框"
    );
    ensure!(
        !title
            .chars()
            .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}')),
        "笔记名称不能包含换行或控制字符"
    );
    let title = title.trim();
    ensure!(
        !title.is_empty() && title.chars().count() <= 200,
        "笔记名称须为 1 至 200 个字符"
    );
    if note.title == title {
        return Ok(false);
    }
    note.title = title.into();
    note.title_override = true;
    note.updated_at = now;
    note.revision += 1;
    Ok(true)
}
pub(crate) fn unmanage(state: &mut State, note_id: &str, now: i64) -> Result<()> {
    let note = state
        .notes
        .iter_mut()
        .find(|note| note.id == note_id)
        .context("找不到笔记")?;
    if !note.detached {
        note.detached = true;
        note.updated_at = now;
        note.revision += 1;
    }
    state.candidates.remove(note_id);
    state.unattributed_notes.remove(note_id);
    state.offline_unreconciled_notes.remove(note_id);
    state.active_versions.retain(|id| {
        state
            .versions
            .iter()
            .any(|v| v.id == *id && v.note_id != note_id)
    });
    Ok(())
}
pub(crate) fn relink(state: &mut State, note_id: &str, path: PathBuf, now: i64) -> Result<()> {
    let index = state
        .notes
        .iter()
        .position(|n| n.id == note_id)
        .context("找不到笔记")?;
    ensure!(!state.notes[index].detached, "笔记已取消管理，请先重新导入");
    ensure!(!state.notes[index].managed, "托管笔记不能关联外部路径");
    let path = std::fs::canonicalize(path)?;
    let key = path.to_string_lossy().into_owned();
    ensure!(
        !state
            .notes
            .iter()
            .any(|n| n.id != note_id && same_path(&n.path, &key)),
        "该路径已属于另一份笔记"
    );
    let content = read(&path)?;
    state.notes[index].path = key;
    state.notes[index].missing = false;
    state.notes[index].error = None;
    state.notes[index].updated_at = now;
    state.notes[index].revision += 1;
    state.candidates.remove(note_id);
    state.unattributed_notes.remove(note_id);
    state.offline_unreconciled_notes.remove(note_id);
    if state
        .versions
        .iter()
        .rev()
        .find(|v| v.note_id == note_id)
        .is_none_or(|v| v.hash != hash(&content))
    {
        append_version(state, index, content, now, true, false);
    }
    Ok(())
}
pub(crate) fn scan(state: &mut State, offline: bool, now: i64) -> String {
    scan_filtered(state, offline, now, false)
}
pub(crate) fn scan_external(state: &mut State, offline: bool, now: i64) -> String {
    scan_filtered(state, offline, now, true)
}
fn scan_filtered(state: &mut State, offline: bool, now: i64, external_only: bool) -> String {
    if offline {
        state.offline_unreconciled_notes.extend(
            state
                .notes
                .iter()
                .filter(|note| !note.detached && (!external_only || !note.managed))
                .map(|note| note.id.clone()),
        );
    }
    let mut captured = 0;
    let mut errors = 0;
    for index in 0..state.notes.len() {
        if state.notes[index].detached || (external_only && state.notes[index].managed) {
            continue;
        }
        let path = PathBuf::from(&state.notes[index].path);
        let note_id = state.notes[index].id.clone();
        let capture_offline = offline || state.offline_unreconciled_notes.contains(&note_id);
        let content = if state.notes[index].managed {
            crate::managed::read_content(&path)
        } else {
            read(&path)
        };
        match content {
            Ok(content) => {
                if state.notes[index].missing || state.notes[index].error.is_some() {
                    state.notes[index].updated_at = now;
                    state.notes[index].revision += 1;
                }
                state.notes[index].missing = false;
                state.notes[index].error = None;
                let digest = hash(&content);
                if state
                    .versions
                    .iter()
                    .rev()
                    .find(|v| v.note_id == note_id)
                    .is_some_and(|v| v.hash == digest)
                {
                    state.candidates.remove(&note_id);
                    state.unattributed_notes.remove(&note_id);
                    state.offline_unreconciled_notes.remove(&note_id);
                    continue;
                }
                if capture_offline
                    || state.unattributed_notes.contains(&note_id)
                    || state
                        .candidates
                        .get(&note_id)
                        .is_some_and(|(candidate, first)| {
                            *candidate == digest && now.saturating_sub(*first) >= 3
                        })
                {
                    append_version(state, index, content, now, capture_offline, false);
                    state.candidates.remove(&note_id);
                    state.unattributed_notes.remove(&note_id);
                    state.offline_unreconciled_notes.remove(&note_id);
                    captured += 1;
                } else if state
                    .candidates
                    .get(&note_id)
                    .is_none_or(|(candidate, _)| *candidate != digest)
                {
                    state.candidates.insert(note_id, (digest, now));
                }
            }
            Err(error) => {
                let missing = !path.exists();
                let error = format!("{error:#}");
                if state.notes[index].missing != missing
                    || state.notes[index].error.as_ref() != Some(&error)
                {
                    state.notes[index].updated_at = now;
                    state.notes[index].revision += 1;
                }
                state.notes[index].missing = missing;
                state.notes[index].error = Some(error);
                state.candidates.remove(&note_id);
                errors += 1;
            }
        }
    }
    format!("扫描完成，新增 {captured} 个版本，读取失败 {errors} 份")
}

#[cfg(test)]
mod rename_tests {
    use super::*;

    #[test]
    fn explicit_managed_title_survives_heading_edit() {
        let mut state = State::new(1000);
        state.notes.push(
            serde_json::from_value(serde_json::json!({
                "id": "note", "path": "notes/note.md", "title": "Display title",
                "title_override": true, "managed": true, "detached": false,
                "missing": false, "error": null, "created_at": 1000,
                "updated_at": 1000, "revision": 1
            }))
            .unwrap(),
        );
        append_version(
            &mut state,
            0,
            "# Changed heading\nbody".into(),
            1001,
            false,
            true,
        );
        assert_eq!(state.notes[0].title, "Display title");
    }
}
