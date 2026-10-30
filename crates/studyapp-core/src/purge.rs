use crate::storage::{self, State};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use std::{collections::HashSet, path::Path, sync::Arc};

pub(crate) fn matches_persisted_state(left: &State, right: &State) -> Result<bool> {
    let mut left_json = serde_json::to_value(left)?;
    let mut right_json = serde_json::to_value(right)?;
    // HashSet serialization order changes on load; compare those fields as sets.
    for field in ["unattributed_notes", "offline_unreconciled_notes"] {
        left_json.as_object_mut().unwrap().remove(field);
        right_json.as_object_mut().unwrap().remove(field);
    }
    Ok(left_json == right_json
        && left.unattributed_notes == right.unattributed_notes
        && left.offline_unreconciled_notes == right.offline_unreconciled_notes
        && serde_json::to_value(&left.versions)? == serde_json::to_value(&right.versions)?)
}

pub(crate) fn verify(conn: &Connection) -> Result<()> {
    let application: i32 = conn.query_row("PRAGMA application_id", [], |r| r.get(0))?;
    let schema: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    ensure!(
        application == storage::APP_ID && schema == storage::SCHEMA,
        "资料库不是当前版本的 StudyApp 资料库"
    );
    ensure!(
        storage::pending_write_intent_count(conn)? == 0,
        "笔记写入尚未恢复完成，不能永久删除"
    );
    Ok(())
}

pub(crate) fn transform(
    state: &mut State,
    note_id: &str,
    expected_revision: u64,
    now: i64,
) -> Result<()> {
    state.validate()?;
    crate::managed::validate_id(note_id)?;
    let note = state
        .notes
        .iter()
        .find(|n| n.id == note_id)
        .context("笔记不存在")?;
    ensure!(note.managed, "只能永久删除应用托管笔记");
    ensure!(
        note.revision == expected_revision,
        "笔记已改变，请重新确认删除"
    );
    let removed: HashSet<_> = state
        .versions
        .iter()
        .filter(|v| v.note_id == note_id)
        .map(|v| v.id.clone())
        .collect();
    // Suggestions can outlive later changes to a session's note selection, so
    // clear all generated suggestions. Authored summaries remain untouched.
    for session in &mut state.sessions {
        let affected = session.note_ids.iter().any(|id| id == note_id)
            || session.version_ids.iter().any(|id| removed.contains(id))
            || !session.suggested_summary.is_empty();
        if affected {
            session.note_ids.retain(|id| id != note_id);
            session.version_ids.retain(|id| !removed.contains(id));
            session.suggested_summary.clear();
            session.revision = session
                .revision
                .checked_add(1)
                .context("学习记录修订号超出范围")?;
            session.updated_at = session.updated_at.max(now);
        }
    }
    for review in &mut state.reviews {
        if review.note_ids.iter().any(|id| id == note_id) {
            review.note_ids.retain(|id| id != note_id);
            review.revision = review
                .revision
                .checked_add(1)
                .context("复习记录修订号超出范围")?;
        }
    }
    for undo in &mut state.undo {
        undo.review.note_ids.retain(|id| id != note_id);
    }
    state.notes.retain(|note| note.id != note_id);
    Arc::make_mut(&mut state.versions).retain(|v| v.note_id != note_id);
    state.active_versions.retain(|id| !removed.contains(id));
    state.candidates.remove(note_id);
    state.unattributed_notes.remove(note_id);
    state.offline_unreconciled_notes.remove(note_id);
    state.validate()
}

fn truncate_wal(conn: &Connection) -> Result<()> {
    let (busy, _, _): (i32, i32, i32) =
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
    ensure!(busy == 0, "资料库正被其他连接读取，无法清除 WAL 残留");
    Ok(())
}

pub(crate) fn cleanup(conn: &Connection) -> Result<()> {
    truncate_wal(conn)?;
    conn.execute_batch("VACUUM;")?;
    truncate_wal(conn)
}

/// Scrub one caller-verified, app-owned SQLite backup after explicit confirmation.
/// Never migrates or makes a recovery copy. Requires exclusive access to the file.
/// Returns false if the note is already absent. An error may follow a committed
/// deletion when SQLite cannot finish reclaiming its old pages or WAL.
pub fn purge_managed_note_from_backup(path: &Path, note_id: &str) -> Result<bool> {
    crate::managed::validate_id(note_id)?;
    let mut conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    // Verify before changing persistent pragmas or opening a write transaction.
    verify(&conn)?;
    let check: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    ensure!(check == "ok", "备份完整性检查失败：{check}");
    conn.execute_batch("PRAGMA secure_delete=ON; PRAGMA synchronous=FULL;")?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    verify(&tx)?;
    let mut state = storage::load(&tx)?.context("备份中没有资料")?;
    let Some(note) = state.notes.iter().find(|n| n.id == note_id) else {
        tx.rollback()?;
        // Also completes residual cleanup after an earlier committed scrub.
        cleanup(&conn)?;
        return Ok(false);
    };
    let revision = note.revision;
    let now = state.last_tick;
    transform(&mut state, note_id, revision, now)?;
    storage::write_state(&tx, &state, 0, true)?;
    tx.commit()?;
    cleanup(&conn).context("备份记录已删除，但数据库残留清理未完成")?;
    Ok(true)
}
