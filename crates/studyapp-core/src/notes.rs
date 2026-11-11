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
        crate::documents::supported_import(path),
        "不支持此文件格式；支持 Markdown、PDF、DOCX 和常见文本文件"
    );
    if crate::documents::is_original_document(path) {
        return crate::documents::read_document(path);
    }
    let mut file =
        std::fs::File::open(path).with_context(|| format!("无法读取 {}", path.display()))?;
    let mut bytes = vec![];
    let markdown = path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| {
            value.eq_ignore_ascii_case("md") || value.eq_ignore_ascii_case("markdown")
        });
    if markdown {
        file.read_to_end(&mut bytes)?;
    } else {
        let limit = crate::documents::MAX_DOCUMENT_BYTES as u64;
        ensure!(
            file.metadata()?.len() <= limit,
            "文本文件超过 32 MiB，请拆分后导入"
        );
        file.take(limit + 1).read_to_end(&mut bytes)?;
        ensure!(bytes.len() as u64 <= limit, "文本文件超过 32 MiB");
    }
    let text =
        if !markdown && (bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff])) {
            ensure!(bytes.len() % 2 == 0, "UTF-16 文本字节数不完整");
            let little = bytes[0] == 0xff;
            let units = bytes[2..]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|part| {
                    if little {
                        u16::from_le_bytes([part[0], part[1]])
                    } else {
                        u16::from_be_bytes([part[0], part[1]])
                    }
                })
                .collect::<Vec<_>>();
            String::from_utf16(&units).context("文本不是有效的 UTF-16 编码")?
        } else {
            String::from_utf8(bytes).context("笔记不是有效的 UTF-8 文本")?
        };
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
    let (diff, summary) = if content.starts_with(crate::documents::DOCUMENT_PREFIX) {
        (
            String::new(),
            if baseline {
                String::new()
            } else {
                "原文档已更新；在阅读页查看完整页面。".into()
            },
        )
    } else if baseline {
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
    Arc::make_mut(&mut state.saved_bodies).remove(&version.note_id);
    Arc::make_mut(&mut state.versions).push(version);
    if !baseline
        && !offline
        && !state.notes[index].detached
        && !state.timer.paused
        && state.timer.kind == crate::TimerKind::Study
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
    scan_filtered(
        state,
        offline,
        now,
        false,
        false,
        state.settings.snapshots.mode == crate::SnapshotMode::Automatic,
    )
}
pub(crate) fn scan_external(state: &mut State, offline: bool, now: i64) -> String {
    scan_filtered(
        state,
        offline,
        now,
        true,
        false,
        state.settings.snapshots.mode == crate::SnapshotMode::Automatic,
    )
}
pub(crate) fn capture(state: &mut State, now: i64) -> String {
    let message = scan_filtered(state, false, now, false, true, true);
    if state
        .notes
        .iter()
        .filter(|note| !note.detached)
        .all(|note| note.error.is_none())
    {
        state.snapshot_warning = None;
    }
    message
}
fn scan_filtered(
    state: &mut State,
    offline: bool,
    now: i64,
    external_only: bool,
    explicit: bool,
    save: bool,
) -> String {
    if !explicit && !state.settings.snapshots.scheduled() {
        return "手动模式：未扫描".into();
    }
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
        let stamp = crate::documents::is_original_document(&path)
            .then(|| crate::documents::ScanStamp::read(&path, now))
            .flatten();
        if !explicit
            && !state.notes[index].missing
            && state.notes[index].error.is_none()
            && let Some(current) = stamp.as_ref()
            && state
                .document_stamps
                .get(&note_id)
                .is_some_and(|previous| previous.unchanged(current))
        {
            state.candidates.remove(&note_id);
            state.unattributed_notes.remove(&note_id);
            state.offline_unreconciled_notes.remove(&note_id);
            continue;
        }
        let content = if explicit {
            read_consistent(&state.notes[index])
        } else if state.notes[index].managed {
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
                    if let Some(stamp) = stamp {
                        state.document_stamps.insert(note_id.clone(), stamp);
                    }
                    Arc::make_mut(&mut state.saved_bodies).remove(&note_id);
                    state.candidates.remove(&note_id);
                    state.unattributed_notes.remove(&note_id);
                    state.offline_unreconciled_notes.remove(&note_id);
                    continue;
                }
                if save
                    && (explicit
                        || capture_offline
                        || state.unattributed_notes.contains(&note_id)
                        || state
                            .candidates
                            .get(&note_id)
                            .is_some_and(|(candidate, first)| {
                                *candidate == digest
                                    && now.saturating_sub(*first)
                                        >= i64::from(state.settings.snapshots.stable_seconds)
                            }))
                {
                    let content = if explicit {
                        content
                    } else {
                        match read_consistent(&state.notes[index]) {
                            Ok(checked) if hash(&checked) == digest => checked,
                            Ok(checked) => {
                                state.candidates.insert(note_id, (hash(&checked), now));
                                continue;
                            }
                            Err(error) => {
                                state.notes[index].error = Some(format!("{error:#}"));
                                state.candidates.remove(&note_id);
                                errors += 1;
                                continue;
                            }
                        }
                    };
                    append_version(state, index, content, now, capture_offline, false);
                    if let Some(stamp) = stamp {
                        state.document_stamps.insert(note_id.clone(), stamp);
                    }
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
                state.document_stamps.remove(&note_id);
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

/// Check the same bytes twice without writing source files. A racing writer is
/// reported rather than promoted to a snapshot during preview/final capture.
pub(crate) fn read_consistent(note: &Note) -> Result<String> {
    let path = Path::new(&note.path);
    let before = std::fs::metadata(path)?;
    let load = || {
        if note.managed {
            crate::managed::read_content(path)
        } else {
            read(path)
        }
    };
    let content = load()?;
    let check = load()?;
    let after = std::fs::metadata(path)?;
    ensure!(
        before.len() == after.len()
            && before.modified().ok() == after.modified().ok()
            && content == check,
        "文件正在变化，未保存不一致的快照，请稍后重试"
    );
    Ok(content)
}

pub(crate) fn preview(state: &mut State, now: i64) -> Result<String> {
    let message = scan_filtered(state, false, now, false, true, false);
    let mut changes = Vec::new();
    for note in &state.notes {
        if note.detached || !state.candidates.contains_key(&note.id) {
            continue;
        }
        let current = read_consistent(note)?;
        let expected_hash = hash(&current);
        let previous = state
            .versions
            .iter()
            .rev()
            .find(|v| v.note_id == note.id)
            .map(|v| v.content.as_str())
            .unwrap_or("");
        if hash(previous) == expected_hash {
            continue;
        }
        let diff = if current.starts_with(crate::documents::DOCUMENT_PREFIX) {
            "原文档内容已变化；保存只新增历史，不改写原文件。".into()
        } else {
            TextDiff::from_lines(previous, &current)
                .unified_diff()
                .context_radius(3)
                .header("上次快照", "待保存内容")
                .to_string()
        };
        changes.push(crate::SnapshotChange {
            note_id: note.id.clone(),
            title: note.title.clone(),
            expected_hash,
            previous: previous.into(),
            current,
            diff,
        });
    }
    state.snapshot_changes = Arc::new(changes);
    Ok(format!(
        "{message}；待保存 {} 份",
        state.snapshot_changes.len()
    ))
}

pub(crate) fn capture_selected(
    state: &mut State,
    selected: &[crate::SnapshotSelection],
    now: i64,
) -> Result<String> {
    ensure!(!selected.is_empty(), "请先勾选需要保存的笔记");
    let mut ids = std::collections::HashSet::new();
    let mut ready = Vec::new();
    // Validate the whole selection before changing any state/history.
    for selection in selected {
        ensure!(
            ids.insert(selection.note_id.clone()),
            "勾选清单包含重复笔记"
        );
        let index = state
            .notes
            .iter()
            .position(|n| n.id == selection.note_id && !n.detached)
            .context("待保存笔记已不存在或取消管理，请重新扫描")?;
        let content = read_consistent(&state.notes[index])?;
        ensure!(
            hash(&content) == selection.expected_hash,
            "{} 的内容已变化，请重新扫描和比较后保存",
            state.notes[index].title
        );
        ready.push((index, content));
    }
    let mut captured = 0;
    for (index, content) in ready {
        let note_id = state.notes[index].id.clone();
        if state
            .versions
            .iter()
            .rev()
            .find(|v| v.note_id == note_id)
            .is_none_or(|v| v.hash != hash(&content))
        {
            let offline = state.offline_unreconciled_notes.contains(&note_id);
            append_version(state, index, content, now, offline, false);
            captured += 1;
        }
        state.candidates.remove(&note_id);
        state.offline_unreconciled_notes.remove(&note_id);
        state.unattributed_notes.remove(&note_id);
    }
    Arc::make_mut(&mut state.snapshot_changes).retain(|change| !ids.contains(&change.note_id));
    Ok(format!(
        "已为勾选笔记新增 {captured} 个快照；未勾选内容保持待保存"
    ))
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
