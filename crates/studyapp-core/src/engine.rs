use crate::{
    storage::{self, State, Undo},
    *,
};
use anyhow::{Context, Result, bail, ensure};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

pub struct Engine {
    conn: Connection,
    path: PathBuf,
    state: State,
    deferred_settings_baseline: Option<SettingsMirrorBaseline>,
    managed_storage_checked: bool,
}
struct SettingsMirrorBaseline {
    settings: Settings,
    target_seconds: u64,
}
fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn add_days(now: i64, days: u32) -> Result<i64> {
    now.checked_add(i64::from(days) * 86400)
        .context("时间超出范围")
}
impl Engine {
    pub fn open(database: &Path, now: i64) -> Result<Self> {
        if let Some(parent) = database.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let mut conn = Connection::open(database).context("无法打开学习资料库")?;
        storage::initialize(&mut conn)?;
        storage::configure(&conn)?;
        let path = std::fs::canonicalize(database)?;
        let mut state = storage::load(&conn)?.unwrap_or_else(|| State::new(now));
        crate::managed::rebase(&mut state, &path)?;
        let recovered_conflict = crate::managed::recover(&mut conn, &path, &mut state)
            .context("托管笔记恢复未完成，原文和写入草稿仍保留；请排除文件占用或目录错误后重试")?
            .unwrap_or(false);
        state.last_tick = state.last_tick.max(now);
        state.expire(now);
        let mut engine = Self {
            conn,
            path,
            state,
            deferred_settings_baseline: None,
            managed_storage_checked: false,
        };
        let recovery_errors: Vec<_> = if recovered_conflict {
            engine
                .state
                .notes
                .iter()
                .filter_map(|note| note.error.clone().map(|error| (note.id.clone(), error)))
                .collect()
        } else {
            Vec::new()
        };
        engine.apply(Command::Scan { offline: true }, now)?;
        if !recovery_errors.is_empty() {
            for (id, error) in recovery_errors {
                if let Some(note) = engine.state.notes.iter_mut().find(|note| note.id == id) {
                    note.error = Some(error);
                }
            }
            storage::save(
                &mut engine.conn,
                &engine.state,
                engine.state.versions.len(),
                false,
            )?;
        }
        Ok(engine)
    }
    pub fn snapshot(&self) -> Snapshot {
        self.state
            .snapshot(self.path.to_string_lossy().into_owned())
    }

    /// Reset only the legacy configuration mirror. The caller must hold the
    /// library's exclusive lock; this never opens a normal scanning engine.
    pub fn reset_existing_preferences_for_uninstall(database: &Path) -> Result<()> {
        let mut conn =
            Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
                .context("无法打开已有资料库，配置已保留")?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let application: i32 = tx.query_row("PRAGMA application_id", [], |row| row.get(0))?;
        let schema: i32 = tx.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        ensure!(
            application == storage::APP_ID && schema == storage::SCHEMA,
            "资料库版本不受支持，配置已保留"
        );
        ensure!(
            storage::pending_write_intent_count(&tx)? == 0,
            "笔记写入尚未恢复完成，配置已保留"
        );
        let mut state = storage::load(&tx)?.context("资料库没有可重置的配置")?;
        Self::save_settings(&mut state, Settings::default())?;
        storage::write_state(&tx, &state, state.versions.len(), false)?;
        tx.commit()?;
        Ok(())
    }

    pub fn prepare_settings(&self, settings: Settings) -> Result<PreparedSettings> {
        settings.validate()?;
        let pending = storage::pending_write_intent_count(&self.conn)?;
        ensure!(
            pending == 0,
            "笔记写入尚未恢复完成，请先排除文件占用后再保存设置"
        );
        Ok(PreparedSettings { settings })
    }

    /// The file-backed configuration has already committed. No fallible I/O is
    /// permitted here; subsequent checkpoints refresh SQLite's legacy mirror.
    pub fn install_prepared_settings(&mut self, prepared: PreparedSettings) {
        // Independent configuration publication does not update app_state. Keep
        // its original mirror fields until a later SQLite commit, even across
        // multiple preference installations before that commit.
        self.deferred_settings_baseline
            .get_or_insert_with(|| SettingsMirrorBaseline {
                settings: self.state.settings.clone(),
                target_seconds: self.state.timer.target_seconds,
            });
        if self.state.timer.active_seconds == 0 {
            self.state.timer.target_seconds = u64::from(prepared.settings.interval_minutes) * 60;
        }
        self.state.settings = prepared.settings;
    }

    pub fn restore_preserving_settings(&mut self, path: &Path, now: i64) -> Result<String> {
        self.recover_writes()?;
        self.restore_with_settings(path, now, Some(self.state.settings.clone()))
    }
    pub fn apply(&mut self, command: Command, now: i64) -> Result<String> {
        self.recover_writes()?;
        if let Command::Backup(path) = &command {
            self.backup(path)?;
            return Ok(format!("备份已保存：{}", path.display()));
        }
        if let Command::Restore(path) = &command {
            return self.restore(path, now);
        }
        let mut next = self.state.clone();
        next.expire(now);
        match &command {
            Command::CreateNote { title, content }
            | Command::CreateNoteInFolder { title, content, .. }
            | Command::CreateNoteWithId { title, content, .. } => {
                let folder_id = match &command {
                    Command::CreateNoteInFolder { folder_id, .. }
                    | Command::CreateNoteWithId { folder_id, .. } => folder_id.clone(),
                    _ => None,
                };
                let note_id = match &command {
                    Command::CreateNoteWithId { note_id, .. } => note_id.clone(),
                    _ => id(),
                };
                crate::folders::require_target(&next, folder_id.as_deref())?;
                let write = crate::managed::create(
                    &mut next,
                    &self.path,
                    note_id,
                    title.clone(),
                    content.clone(),
                    now,
                    false,
                )?;
                next.notes
                    .last_mut()
                    .expect("created managed note")
                    .folder_id = folder_id;
                self.write_managed(next, vec![write], false)?;
                return Ok("笔记已创建".into());
            }
            Command::SaveNote {
                note_id,
                expected_hash,
                content,
            } => {
                if let Some(write) = crate::managed::edit(
                    &mut next,
                    &self.path,
                    note_id,
                    expected_hash,
                    content.clone(),
                    now,
                    false,
                )? {
                    self.write_managed(next, vec![write], false)?;
                    return Ok("笔记已保存".into());
                }
                // Empty success is a deliberate no-op: keep the completion
                // acknowledgement but do not announce a new saved version.
                return Ok(String::new());
            }
            Command::ReceiveSyncedNote {
                note_id,
                expected_hash,
                content,
            } => {
                crate::managed::validate_id(note_id)?;
                let write = if let Some(hash) = expected_hash {
                    crate::managed::edit(
                        &mut next,
                        &self.path,
                        note_id,
                        hash,
                        content.clone(),
                        now,
                        true,
                    )?
                } else {
                    Some(crate::managed::create(
                        &mut next,
                        &self.path,
                        note_id.clone(),
                        String::new(),
                        content.clone(),
                        now,
                        true,
                    )?)
                };
                if let Some(write) = write {
                    self.write_managed(next, vec![write], false)?;
                }
                return Ok("同步笔记已接收（离线版本）".into());
            }
            Command::ReceiveSyncedCopy {
                note_id,
                title,
                content,
            } => {
                let write = crate::managed::create(
                    &mut next,
                    &self.path,
                    note_id.clone(),
                    title.clone(),
                    content.clone(),
                    now,
                    true,
                )?;
                self.write_managed(next, vec![write], false)?;
                return Ok("同步副本已接收（离线版本）".into());
            }
            Command::ExportNote {
                note_id,
                version_id,
                path,
            } => {
                crate::managed::export(&next, note_id, version_id.as_deref(), path)?;
                return Ok(format!("Markdown 已导出：{}", path.display()));
            }
            _ => {}
        }
        let message = match command {
            Command::CreateFolder { name, parent_id } => {
                crate::folders::create(&mut next, name, parent_id)?;
                "目录已创建".into()
            }
            Command::RenameFolder { folder_id, name } => {
                crate::folders::rename(&mut next, &folder_id, name)?;
                "目录已重命名".into()
            }
            Command::MoveFolder {
                folder_id,
                parent_id,
            } => {
                crate::folders::move_folder(&mut next, &folder_id, parent_id)?;
                "目录已移动".into()
            }
            Command::DeleteFolder { folder_id } => {
                crate::folders::delete(&mut next, &folder_id)?;
                "空目录已删除；笔记文件未改动".into()
            }
            Command::MoveNote { note_id, folder_id } => {
                crate::folders::move_note(&mut next, &note_id, folder_id, now)?;
                "笔记所属目录已更新".into()
            }
            Command::RenameNote {
                note_id,
                title,
                expected_revision,
            } => {
                if !crate::notes::rename(&mut next, &note_id, &title, expected_revision, now)? {
                    return Ok(String::new());
                }
                "笔记已重命名".into()
            }
            Command::SaveSettings(settings) => {
                Self::save_settings(&mut next, settings)?;
                "设置已保存".into()
            }
            Command::SaveLearningSettings {
                interval_minutes,
                answer_minutes,
                review_days,
            } => {
                let settings = Settings {
                    interval_minutes,
                    answer_minutes,
                    review_days,
                    ..next.settings.clone()
                };
                Self::save_settings(&mut next, settings)?;
                "学习设置已保存".into()
            }
            Command::SetShowFilePaths(visible) => {
                next.settings.show_file_paths = visible;
                "显示设置已保存".into()
            }
            Command::SaveLayout(layout) => {
                layout.validate()?;
                next.settings.layout = layout;
                "布局已保存".into()
            }
            Command::SetPaused(paused) => {
                if next.timer.paused && !paused {
                    // Reconcile every source before admitting new activity. Candidates may
                    // have been observed while paused, or not observed at all.
                    next.unattributed_notes.extend(
                        next.notes
                            .iter()
                            .filter(|n| !n.detached)
                            .map(|n| n.id.clone()),
                    );
                    crate::notes::scan(&mut next, false, now);
                }
                next.timer.paused = paused;
                next.last_tick = next.last_tick.max(now);
                if paused {
                    next.candidates.clear();
                }
                if paused {
                    "计时已暂停"
                } else {
                    "计时已继续"
                }
                .into()
            }
            Command::Import(paths) => crate::notes::import(&mut next, &self.path, paths, now),
            Command::UnmanageNote { note_id } => {
                crate::notes::unmanage(&mut next, &note_id, now)?;
                "已取消管理；原文件、版本和学习复习记录均保留，可重新导入恢复管理".into()
            }
            Command::Relink { note_id, path } => {
                crate::notes::relink(&mut next, &note_id, path, now)?;
                "笔记已重新关联".into()
            }
            Command::Scan { offline } => crate::notes::scan(&mut next, offline, now),
            Command::Submit {
                session_id,
                summary,
                note_ids,
                version_ids,
            } => {
                Self::notes_exist(&next, &note_ids)?;
                Self::summary_valid(&summary)?;
                let note_ids = unique(note_ids);
                let index = next
                    .sessions
                    .iter()
                    .position(|s| s.id == session_id)
                    .context("找不到学习记录")?;
                let session = &next.sessions[index];
                ensure!(
                    session.status == SessionStatus::Pending && now < session.deadline,
                    "该记录已处理或已超过回答期限，请使用更正功能"
                );
                let version_ids = Self::reconcile_versions(
                    &next,
                    &note_ids,
                    &session.version_ids,
                    version_ids,
                    session.ended_at,
                )?;
                let session = &mut next.sessions[index];
                session.status = SessionStatus::Learned;
                session.summary = summary.trim().into();
                session.note_ids = note_ids;
                session.version_ids = version_ids;
                session.updated_at = now;
                session.revision += 1;
                Self::ensure_review(&mut next, &session_id)?;
                "学习记录已确认".into()
            }
            Command::Skip { session_id } => {
                let session = next
                    .sessions
                    .iter_mut()
                    .find(|s| s.id == session_id)
                    .context("找不到学习记录")?;
                ensure!(session.status == SessionStatus::Pending, "该记录已处理");
                session.status = SessionStatus::Missed;
                session.updated_at = now;
                session.revision += 1;
                "已标记为未学习".into()
            }
            Command::Manual {
                started_at,
                minutes,
                summary,
                note_ids,
                version_ids,
            } => {
                Self::summary_valid(&summary)?;
                Self::notes_exist(&next, &note_ids)?;
                ensure!((1..=1440).contains(&minutes), "补记时长应为 1–1440 分钟");
                let seconds = u64::from(minutes) * 60;
                let ended_at = started_at
                    .checked_add(seconds as i64)
                    .context("时间超出范围")?;
                ensure!(ended_at <= now, "不能补记尚未结束的学习时段");
                let note_ids = unique(note_ids);
                let version_ids =
                    Self::reconcile_versions(&next, &note_ids, &[], version_ids, ended_at)?;
                let session_id = id();
                next.sessions.push(Session {
                    id: session_id.clone(),
                    started_at,
                    ended_at,
                    active_seconds: seconds,
                    deadline: ended_at,
                    status: SessionStatus::Learned,
                    summary: summary.trim().into(),
                    suggested_summary: String::new(),
                    note_ids,
                    version_ids,
                    spans: vec![ActiveSpan {
                        start: started_at,
                        end: ended_at,
                    }],
                    updated_at: now,
                    revision: 1,
                });
                Self::ensure_review(&mut next, &session_id)?;
                "学习记录已补记".into()
            }
            Command::EditSession {
                session_id,
                summary,
                learned,
                note_ids,
                version_ids,
            } => {
                Self::notes_exist(&next, &note_ids)?;
                if learned {
                    Self::summary_valid(&summary)?;
                }
                let note_ids = unique(note_ids);
                let index = next
                    .sessions
                    .iter()
                    .position(|s| s.id == session_id)
                    .context("找不到学习记录")?;
                let session = &next.sessions[index];
                let version_ids = Self::reconcile_versions(
                    &next,
                    &note_ids,
                    &session.version_ids,
                    version_ids,
                    session.ended_at,
                )?;
                let session = &mut next.sessions[index];
                session.summary = summary.trim().into();
                session.note_ids = note_ids;
                session.version_ids = version_ids;
                session.status = if learned {
                    SessionStatus::Learned
                } else {
                    SessionStatus::Missed
                };
                session.updated_at = now;
                session.revision += 1;
                if learned {
                    Self::ensure_review(&mut next, &session_id)?;
                } else {
                    let removed: Vec<String> = next
                        .reviews
                        .iter()
                        .filter(|r| r.session_id == session_id)
                        .map(|r| r.id.clone())
                        .collect();
                    next.reviews.retain(|r| r.session_id != session_id);
                    next.review_events
                        .retain(|e| !removed.contains(&e.review_id));
                    next.undo.retain(|u| !removed.contains(&u.review.id));
                }
                "学习记录已更正".into()
            }
            Command::CompleteReview {
                review_id,
                expected_stage,
            } => {
                let review = next
                    .reviews
                    .iter_mut()
                    .find(|r| r.id == review_id)
                    .context("找不到复习计划")?;
                ensure!(
                    review.stage == expected_stage,
                    "复习状态已变化，请刷新后重试"
                );
                ensure!(
                    review.next_review_at.is_some_and(|due| due <= now),
                    "复习尚未到期或本轮已完成"
                );
                let event_id = id();
                next.undo.push(Undo {
                    event_id: event_id.clone(),
                    review: review.clone(),
                });
                next.review_events.push(ReviewEvent {
                    id: event_id,
                    review_id,
                    completed_at: now,
                    stage: review.stage,
                });
                review.stage += 1;
                review.last_review_at = Some(now);
                review.next_review_at = review
                    .intervals
                    .get(review.stage)
                    .map(|d| add_days(now, *d))
                    .transpose()?;
                review.revision += 1;
                "复习已完成".into()
            }
            Command::UndoReview { review_id } => {
                let index = next
                    .undo
                    .iter()
                    .rposition(|u| u.review.id == review_id)
                    .context("没有可撤销的复习")?;
                let undo = next.undo.remove(index);
                let review = next
                    .reviews
                    .iter_mut()
                    .find(|r| r.id == review_id)
                    .context("找不到复习计划")?;
                // Restore only scheduling state: later edits to title/links must survive undo.
                review.stage = undo.review.stage;
                review.last_review_at = undo.review.last_review_at;
                review.next_review_at = undo.review.next_review_at;
                review.revision += 1;
                next.review_events.retain(|e| e.id != undo.event_id);
                // Validate the resulting history before persisting scheduling values
                // copied from a stored prior state. Normal timer writes remain cheap.
                next.validate()?;
                "已撤销上次复习".into()
            }
            Command::Backup(_)
            | Command::Restore(_)
            | Command::CreateNote { .. }
            | Command::CreateNoteInFolder { .. }
            | Command::CreateNoteWithId { .. }
            | Command::SaveNote { .. }
            | Command::ReceiveSyncedNote { .. }
            | Command::ReceiveSyncedCopy { .. }
            | Command::ExportNote { .. } => unreachable!(),
        };
        storage::save(&mut self.conn, &next, self.state.versions.len(), false)?;
        self.state = next;
        self.deferred_settings_baseline = None;
        Ok(message)
    }
    fn save_settings(state: &mut State, settings: Settings) -> Result<()> {
        settings.validate()?;
        if state.timer.active_seconds == 0 {
            state.timer.target_seconds = u64::from(settings.interval_minutes) * 60;
        }
        state.settings = settings;
        Ok(())
    }

    pub fn tick(&mut self, now: i64, elapsed_seconds: u64) -> Result<()> {
        self.recover_writes()?;
        let mut next = self.state.clone();
        next.expire(now);
        if !next.timer.paused && (1..=10).contains(&elapsed_seconds) && now > next.last_tick {
            // A monotonic elapsed duration is bounded by wall time to prevent overlap or clock rollback.
            let available = now.saturating_sub(next.last_tick) as u64;
            let elapsed = elapsed_seconds.min(available);
            let mut cursor = now.saturating_sub(elapsed as i64);
            let mut remaining = elapsed;
            while remaining > 0 {
                let part = remaining.min(next.timer.target_seconds - next.timer.active_seconds);
                let end = cursor + part as i64;
                if let Some(span) = next.timer.spans.last_mut().filter(|s| s.end == cursor) {
                    span.end = end;
                } else {
                    next.timer.spans.push(ActiveSpan { start: cursor, end });
                }
                next.timer.active_seconds += part;
                cursor = end;
                remaining -= part;
                if next.timer.active_seconds == next.timer.target_seconds {
                    Self::finish_segment(&mut next, end)?;
                }
            }
        }
        next.last_tick = next.last_tick.max(now);
        storage::save(&mut self.conn, &next, self.state.versions.len(), false)?;
        self.state = next;
        self.deferred_settings_baseline = None;
        Ok(())
    }
    pub fn checkpoint(&mut self) -> Result<()> {
        self.recover_writes()?;
        storage::save(
            &mut self.conn,
            &self.state,
            self.state.versions.len(),
            false,
        )?;
        self.deferred_settings_baseline = None;
        self.conn.execute_batch("PRAGMA wal_checkpoint(FULL);")?;
        Ok(())
    }
    fn notes_exist(state: &State, ids: &[String]) -> Result<()> {
        ensure!(
            ids.iter().all(|id| state.notes.iter().any(|n| n.id == *id)),
            "关联笔记不存在"
        );
        Ok(())
    }
    fn summary_valid(summary: &str) -> Result<()> {
        ensure!(
            !summary.trim().is_empty() && summary.len() <= 100_000,
            "请填写学习摘要（最多 100000 字节）"
        );
        Ok(())
    }
    fn reconcile_versions(
        state: &State,
        notes: &[String],
        existing: &[String],
        explicit: Option<Vec<String>>,
        ended_at: i64,
    ) -> Result<Vec<String>> {
        let versions: std::collections::HashMap<&str, &NoteVersion> =
            state.versions.iter().map(|v| (v.id.as_str(), v)).collect();
        let explicit_selection = explicit.is_some();
        let mut chosen = if let Some(ids) = explicit {
            let ids = unique(ids);
            for id in &ids {
                let version = versions.get(id.as_str()).context("选择的笔记版本不存在")?;
                ensure!(notes.contains(&version.note_id), "选择的版本不属于关联笔记");
            }
            ids
        } else {
            unique(
                existing
                    .iter()
                    .filter(|id| {
                        versions
                            .get(id.as_str())
                            .is_some_and(|v| notes.contains(&v.note_id))
                    })
                    .cloned()
                    .collect(),
            )
        };
        for note_id in notes {
            if chosen.iter().any(|id| {
                versions
                    .get(id.as_str())
                    .is_some_and(|v| v.note_id == *note_id)
            }) {
                continue;
            }
            ensure!(!explicit_selection, "每份关联笔记至少需要选择一个版本");
            let matching = || state.versions.iter().filter(|v| v.note_id == *note_id);
            let version = matching()
                .filter(|v| v.captured_at <= ended_at)
                .max_by_key(|v| v.captured_at)
                .or_else(|| matching().max_by_key(|v| v.captured_at))
                .context("关联笔记没有可用的内容快照")?;
            chosen.push(version.id.clone());
        }
        Ok(chosen)
    }
    fn ensure_review(state: &mut State, session_id: &str) -> Result<()> {
        let s = state
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .context("找不到学习记录")?;
        if let Some(r) = state
            .reviews
            .iter_mut()
            .find(|r| r.session_id == session_id)
        {
            r.title = s.summary.clone();
            r.note_ids = s.note_ids.clone();
            r.revision += 1;
        } else {
            state.reviews.push(Review {
                id: id(),
                session_id: session_id.into(),
                title: s.summary.clone(),
                note_ids: s.note_ids.clone(),
                learned_at: s.ended_at,
                last_review_at: None,
                next_review_at: Some(add_days(s.ended_at, state.settings.review_days[0])?),
                stage: 0,
                intervals: state.settings.review_days.clone(),
                revision: 1,
            });
        }
        Ok(())
    }
    fn finish_segment(state: &mut State, end: i64) -> Result<()> {
        let versions: Vec<&NoteVersion> = state
            .versions
            .iter()
            .filter(|v| {
                state.active_versions.contains(&v.id)
                    && !v.offline
                    && state
                        .timer
                        .spans
                        .iter()
                        .any(|p| p.start <= v.captured_at && v.captured_at <= p.end)
            })
            .collect();
        let version_ids = versions.iter().map(|v| v.id.clone()).collect();
        let note_ids = unique(versions.iter().map(|v| v.note_id.clone()).collect());
        let suggested_summary = versions
            .iter()
            .map(|v| v.summary.as_str())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        let deadline = end
            .checked_add(i64::from(state.settings.answer_minutes) * 60)
            .context("时间超出范围")?;
        state.sessions.push(Session {
            id: id(),
            started_at: state
                .timer
                .spans
                .first()
                .map_or(state.timer.started_at, |p| p.start),
            ended_at: end,
            active_seconds: state.timer.active_seconds,
            deadline,
            status: SessionStatus::Pending,
            summary: String::new(),
            suggested_summary,
            note_ids,
            version_ids,
            spans: std::mem::take(&mut state.timer.spans),
            updated_at: end,
            revision: 1,
        });
        state.active_versions.retain(|id| {
            state
                .versions
                .iter()
                .any(|v| v.id == *id && v.captured_at > end)
        });
        state.timer.active_seconds = 0;
        state.timer.target_seconds = u64::from(state.settings.interval_minutes) * 60;
        state.timer.started_at = end;
        Ok(())
    }
    fn safe_destination(&self, path: &Path) -> Result<PathBuf> {
        let absolute = if path.exists() {
            std::fs::canonicalize(path)?
        } else {
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            std::fs::canonicalize(parent)?.join(path.file_name().context("请选择备份文件")?)
        };
        let live = self.path.to_string_lossy();
        let destination = absolute.to_string_lossy();
        let equal = |a: &str, b: &str| {
            if cfg!(windows) {
                a.eq_ignore_ascii_case(b)
            } else {
                a == b
            }
        };
        ensure!(
            !equal(&destination, &live)
                && !equal(&destination, &format!("{live}-wal"))
                && !equal(&destination, &format!("{live}-shm")),
            "备份或恢复路径不能是正在使用的资料库及其 WAL/SHM 文件"
        );
        Ok(absolute)
    }
    fn backup(&mut self, path: &Path) -> Result<()> {
        let destination = self.safe_destination(path)?;
        ensure!(
            !destination.exists(),
            "备份文件已存在，请选择新文件名以保留原备份"
        );
        self.checkpoint()?;
        let mut to = Connection::open(&destination)?;
        let backup = rusqlite::backup::Backup::new(&self.conn, &mut to)?;
        backup.run_to_completion(128, std::time::Duration::from_millis(5), None)?;
        Ok(())
    }
    fn restore(&mut self, path: &Path, now: i64) -> Result<String> {
        self.restore_with_settings(path, now, None)
    }

    fn restore_with_settings(
        &mut self,
        path: &Path,
        now: i64,
        settings: Option<Settings>,
    ) -> Result<String> {
        let source = self.safe_destination(path)?;
        if !source.is_file() {
            bail!("找不到备份文件");
        }
        let mut restored = storage::verified_backup(&source)?;
        let preserving_settings = settings.is_some();
        if let Some(settings) = settings {
            Self::save_settings(&mut restored, settings)?;
            restored.validate()?;
        }
        let pre = self
            .path
            .with_file_name(format!("studyapp-before-restore-{now}-{}.sqlite", id()));
        self.backup(&pre)?;
        restored.last_tick = restored.last_tick.max(now);
        restored.candidates.clear();
        restored.expire(now);
        let writes = crate::managed::restore_writes(&mut restored, &self.path, now)?;
        crate::notes::scan_external(&mut restored, true, now);
        self.write_managed(restored, writes, true)?;
        Ok(format!(
            "{}；恢复前备份：{}",
            if preserving_settings {
                "学习资料已恢复，当前设置已保留"
            } else {
                "资料库已恢复"
            },
            pre.display()
        ))
    }
    fn write_managed(
        &mut self,
        next: State,
        writes: Vec<crate::managed::Write>,
        replace: bool,
    ) -> Result<()> {
        if !self.managed_storage_checked && !writes.is_empty() {
            crate::managed::check_filesystem(&self.path)?;
            self.managed_storage_checked = true;
        }
        let mut previous;
        let previous = if let Some(baseline) = &self.deferred_settings_baseline {
            previous = self.state.clone();
            previous.settings = baseline.settings.clone();
            previous.timer.target_seconds = baseline.target_seconds;
            &previous
        } else {
            &self.state
        };
        crate::managed::prepare(&mut self.conn, previous, next, writes, replace)?;
        self.recover_writes()
    }
    fn recover_writes(&mut self) -> Result<()> {
        let recovered = crate::managed::recover(&mut self.conn, &self.path, &mut self.state)
            .context("托管写入未完成，原文和草稿仍保留；排除文件占用或目录错误后重试")?;
        if recovered.is_some() {
            self.deferred_settings_baseline = None;
        }
        ensure!(
            !recovered.unwrap_or(false),
            "发现外部修改：外部文件未被覆盖，待保存草稿已保留在版本历史"
        );
        Ok(())
    }
}
fn unique(ids: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    ids.into_iter()
        .filter(|id| seen.insert(id.clone()))
        .collect()
}

#[cfg(test)]
#[path = "engine_reset_tests.rs"]
mod reset_tests;

#[cfg(test)]
mod managed_storage_tests {
    use super::*;

    fn install_changed_preferences(engine: &mut Engine) -> Settings {
        let mut settings = engine.snapshot().settings;
        settings.interval_minutes = 30;
        settings.appearance.mode = ThemeMode::Dark;
        let prepared = engine.prepare_settings(settings.clone()).unwrap();
        engine.install_prepared_settings(prepared);
        settings
    }

    fn interrupt_managed_create_after_publication(engine: &mut Engine) -> PathBuf {
        let before: String = engine
            .conn
            .query_row("SELECT data FROM app_state", [], |row| row.get(0))
            .unwrap();
        // Blocking the notes directory is not a portable post-intent failure:
        // Unix try_exists returns ENOTDIR before prepare can journal the write.
        // Fail the final state commit instead, after the intent and publication.
        engine.conn.execute_batch(
            "CREATE TRIGGER fail_final_state_commit BEFORE UPDATE ON app_state BEGIN SELECT RAISE(ABORT, 'final state commit failure fixture'); END;",
        ).unwrap();
        let error = engine
            .apply(
                Command::CreateNote {
                    title: "interrupted draft".into(),
                    content: "durable draft".into(),
                },
                1001,
            )
            .unwrap_err();
        assert!(format!("{error:#}").contains("final state commit failure fixture"));
        let pending: i64 = engine
            .conn
            .query_row("SELECT count(*) FROM write_intents", [], |row| row.get(0))
            .unwrap();
        assert_eq!(pending, 1);
        let after: String = engine
            .conn
            .query_row("SELECT data FROM app_state", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            after, before,
            "failed commit must preserve the SQLite mirror"
        );
        assert!(engine.snapshot().notes.is_empty());
        assert!(engine.snapshot().versions.is_empty());
        let raw: String = engine
            .conn
            .query_row("SELECT data FROM write_intents", [], |row| row.get(0))
            .unwrap();
        let intent: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let note_id = intent["state"]["notes"][0]["id"].as_str().unwrap();
        let published = crate::managed::path(&engine.path, note_id).unwrap();
        assert_eq!(
            std::fs::read_to_string(&published).unwrap(),
            "durable draft"
        );
        engine
            .conn
            .execute_batch("DROP TRIGGER fail_final_state_commit;")
            .unwrap();
        published
    }

    #[test]
    fn installed_preferences_allow_immediate_managed_create_without_checkpoint() {
        let dir = tempfile::tempdir().unwrap().keep();
        let database = dir.join("preferences.sqlite");
        let mut engine = Engine::open(&database, 1000).unwrap();
        let original = engine.snapshot().settings;
        let settings = install_changed_preferences(&mut engine);
        assert_eq!(
            storage::load(&engine.conn).unwrap().unwrap().settings,
            original
        );
        engine
            .apply(
                Command::CreateNote {
                    title: "immediate".into(),
                    content: "after preferences".into(),
                },
                1001,
            )
            .unwrap();
        assert_eq!(engine.snapshot().settings, settings);
        assert_eq!(engine.snapshot().timer.target_seconds, 1800);
        assert_eq!(
            storage::load(&engine.conn).unwrap().unwrap().settings,
            settings
        );
        drop(engine);
        let reopened = Engine::open(&database, 1002).unwrap();
        assert_eq!(reopened.snapshot().settings, settings);
        assert_eq!(reopened.snapshot().versions[0].content, "after preferences");
    }

    #[test]
    fn installed_preferences_allow_immediate_managed_save_during_active_cycle() {
        let dir = tempfile::tempdir().unwrap().keep();
        let mut engine = Engine::open(&dir.join("preferences.sqlite"), 1000).unwrap();
        engine
            .apply(
                Command::CreateNote {
                    title: "existing".into(),
                    content: "before".into(),
                },
                1000,
            )
            .unwrap();
        engine.tick(1001, 1).unwrap();
        let before = engine.snapshot();
        let settings = install_changed_preferences(&mut engine);
        engine
            .apply(
                Command::SaveNote {
                    note_id: before.notes[0].id.clone(),
                    expected_hash: before.versions[0].hash.clone(),
                    content: "after".into(),
                },
                1002,
            )
            .unwrap();
        assert_eq!(engine.snapshot().settings, settings);
        assert_eq!(engine.snapshot().timer.target_seconds, 3600);
        assert_eq!(engine.snapshot().timer.active_seconds, 1);
        assert_eq!(engine.snapshot().versions.last().unwrap().content, "after");
    }

    #[test]
    fn repeated_preference_installs_keep_the_original_managed_write_baseline() {
        let dir = tempfile::tempdir().unwrap().keep();
        let mut engine = Engine::open(&dir.join("preferences.sqlite"), 1000).unwrap();
        let original = engine.snapshot().settings;
        for mode in [ThemeMode::Dark, ThemeMode::Light, original.appearance.mode] {
            let mut settings = original.clone();
            settings.appearance.mode = mode;
            settings.interval_minutes = 30;
            let prepared = engine.prepare_settings(settings).unwrap();
            engine.install_prepared_settings(prepared);
        }
        for now in [1001, 1002] {
            engine
                .apply(
                    Command::CreateNote {
                        title: "repeat".into(),
                        content: format!("note {now}"),
                    },
                    now,
                )
                .unwrap();
        }
        assert_eq!(engine.snapshot().notes.len(), 2);
        assert_eq!(engine.snapshot().timer.target_seconds, 1800);
    }

    #[test]
    fn installed_preferences_still_reject_a_genuine_second_engine_write() {
        let dir = tempfile::tempdir().unwrap().keep();
        let database = dir.join("preferences.sqlite");
        let mut engine = Engine::open(&database, 1000).unwrap();
        let mut other = Engine::open(&database, 1000).unwrap();
        install_changed_preferences(&mut engine);
        other.apply(Command::SetPaused(true), 1001).unwrap();
        let error = engine
            .apply(
                Command::CreateNote {
                    title: "stale".into(),
                    content: "must not publish".into(),
                },
                1002,
            )
            .unwrap_err();
        assert!(error.to_string().contains("资料库已被另一进程修改"));
        assert!(engine.snapshot().notes.is_empty());
        let stored = storage::load(&engine.conn).unwrap().unwrap();
        assert!(stored.timer.paused);
        assert!(stored.notes.is_empty());
        assert_eq!(stored.settings, other.snapshot().settings);
        let pending: i64 = engine
            .conn
            .query_row("SELECT count(*) FROM write_intents", [], |row| row.get(0))
            .unwrap();
        assert_eq!(pending, 0);
    }

    #[test]
    fn preference_mirror_baseline_advances_after_each_ordinary_persistence_path() {
        for path in ["command", "tick", "checkpoint", "backup", "restore"] {
            let dir = tempfile::tempdir().unwrap().keep();
            let mut engine = Engine::open(&dir.join("preferences.sqlite"), 1000).unwrap();
            let backup = dir.join("backup.sqlite");
            engine.apply(Command::Backup(backup.clone()), 1000).unwrap();
            install_changed_preferences(&mut engine);
            match path {
                "command" => {
                    engine.apply(Command::SetPaused(true), 1001).unwrap();
                }
                "tick" => engine.tick(1001, 1).unwrap(),
                "checkpoint" => engine.checkpoint().unwrap(),
                "backup" => {
                    engine
                        .apply(Command::Backup(dir.join("updated.sqlite")), 1001)
                        .unwrap();
                }
                "restore" => {
                    engine.restore_preserving_settings(&backup, 1001).unwrap();
                }
                _ => unreachable!(),
            }
            engine
                .apply(
                    Command::CreateNote {
                        title: path.into(),
                        content: "after persistence".into(),
                    },
                    1002,
                )
                .unwrap();
            assert_eq!(engine.snapshot().notes.len(), 1, "{path}");
        }
    }

    #[test]
    fn interrupted_write_with_installed_preferences_recovers_before_the_next_write() {
        for reopen in [false, true] {
            let dir = tempfile::tempdir().unwrap().keep();
            let database = dir.join("preferences.sqlite");
            let mut engine = Engine::open(&database, 1000).unwrap();
            let settings = install_changed_preferences(&mut engine);
            let published = interrupt_managed_create_after_publication(&mut engine);
            if reopen {
                drop(engine);
                engine = Engine::open(&database, 1002).unwrap();
            }
            engine
                .apply(
                    Command::CreateNote {
                        title: "next".into(),
                        content: "after recovery".into(),
                    },
                    1003,
                )
                .unwrap();
            assert_eq!(engine.snapshot().notes.len(), 2);
            assert_eq!(engine.snapshot().versions.len(), 2);
            assert_eq!(std::fs::read_to_string(published).unwrap(), "durable draft");
            let pending: i64 = engine
                .conn
                .query_row("SELECT count(*) FROM write_intents", [], |row| row.get(0))
                .unwrap();
            assert_eq!(pending, 0);
            assert_eq!(engine.snapshot().settings, settings);
            assert_eq!(
                storage::load(&engine.conn).unwrap().unwrap().settings,
                settings
            );
        }
    }

    #[test]
    fn failed_ordinary_save_keeps_the_deferred_preference_baseline() {
        let dir = tempfile::tempdir().unwrap().keep();
        let mut engine = Engine::open(&dir.join("preferences.sqlite"), 1000).unwrap();
        let original = engine.snapshot().settings;
        let settings = install_changed_preferences(&mut engine);
        engine.conn.execute_batch(
            "CREATE TRIGGER fail_state_save BEFORE INSERT ON app_state BEGIN SELECT RAISE(ABORT, 'fixture save failure'); END;",
        ).unwrap();
        assert!(engine.apply(Command::SetPaused(true), 1001).is_err());
        assert!(!engine.snapshot().timer.paused);
        assert_eq!(
            storage::load(&engine.conn).unwrap().unwrap().settings,
            original
        );
        engine
            .conn
            .execute_batch("DROP TRIGGER fail_state_save;")
            .unwrap();
        engine
            .apply(
                Command::CreateNote {
                    title: "retry".into(),
                    content: "after failed state save".into(),
                },
                1002,
            )
            .unwrap();
        assert_eq!(engine.snapshot().settings, settings);
        assert_eq!(engine.snapshot().notes.len(), 1);
    }

    #[test]
    fn recovered_file_conflict_advances_the_deferred_preference_baseline() {
        let dir = tempfile::tempdir().unwrap().keep();
        let mut engine = Engine::open(&dir.join("preferences.sqlite"), 1000).unwrap();
        let settings = install_changed_preferences(&mut engine);
        let external_path = interrupt_managed_create_after_publication(&mut engine);
        std::fs::write(&external_path, "external writer").unwrap();
        let error = engine.apply(Command::SetPaused(true), 1002).unwrap_err();
        assert!(error.to_string().contains("发现外部修改"));
        assert_eq!(
            std::fs::read_to_string(external_path).unwrap(),
            "external writer"
        );
        assert_eq!(engine.snapshot().settings, settings);
        assert_eq!(engine.snapshot().versions.len(), 2);
        assert_eq!(engine.snapshot().versions[0].content, "durable draft");
        assert_eq!(engine.snapshot().versions[1].content, "external writer");
        assert!(engine.snapshot().versions[1].offline);
        let pending: i64 = engine
            .conn
            .query_row("SELECT count(*) FROM write_intents", [], |row| row.get(0))
            .unwrap();
        assert_eq!(pending, 0);
        engine
            .apply(
                Command::CreateNote {
                    title: "next".into(),
                    content: "after recovered conflict".into(),
                },
                1003,
            )
            .unwrap();
        assert_eq!(engine.snapshot().notes.len(), 2);
        assert_eq!(
            storage::load(&engine.conn).unwrap().unwrap().settings,
            settings
        );
    }

    #[test]
    fn preferences_cannot_commit_while_an_old_managed_write_can_replace_live_state() {
        let dir = tempfile::tempdir().unwrap().keep();
        let engine = Engine::open(&dir.join("pending.sqlite"), 1000).unwrap();
        let original = engine.snapshot().settings;
        // Recovery of a pending intent may install its entire older state.
        // Refuse a new independent preference publication until it is recovered.
        engine
            .conn
            .execute("INSERT INTO write_intents(id,data) VALUES(1,'{}')", [])
            .unwrap();
        assert!(engine.prepare_settings(original.clone()).is_err());
        assert_eq!(engine.snapshot().settings, original);
    }

    #[test]
    fn prepared_preferences_do_not_write_and_restore_can_preserve_current_settings() {
        let dir = tempfile::tempdir().unwrap().keep();
        let database = dir.join("preferences.sqlite");
        let mut engine = Engine::open(&database, 1000).unwrap();
        let backup = dir.join("learning-backup.sqlite");
        engine.apply(Command::Backup(backup.clone()), 1000).unwrap();
        let original = engine.snapshot().settings;
        let mut settings = original.clone();
        settings.interval_minutes = 30;
        settings.layout.folders_visible = false;
        let prepared = engine.prepare_settings(settings.clone()).unwrap();
        assert_eq!(engine.snapshot().settings, original);
        engine.install_prepared_settings(prepared);
        assert_eq!(engine.snapshot().settings, settings);
        assert_eq!(engine.snapshot().timer.target_seconds, 1800);
        let stored = storage::load(&engine.conn).unwrap().unwrap();
        assert_eq!(
            stored.settings, original,
            "installation must not write SQLite"
        );
        engine.restore_preserving_settings(&backup, 1001).unwrap();
        assert_eq!(engine.snapshot().settings, settings);
        assert_eq!(engine.snapshot().timer.target_seconds, 1800);
        assert_eq!(
            storage::load(&engine.conn).unwrap().unwrap().settings,
            settings
        );
        engine.apply(Command::Restore(backup), 1002).unwrap();
        assert_eq!(
            engine.snapshot().settings,
            original,
            "standalone core restore stays compatible"
        );
    }

    #[test]
    fn prepared_preferences_preserve_an_active_timer_cycle() {
        let dir = tempfile::tempdir().unwrap().keep();
        let mut engine = Engine::open(&dir.join("preferences.sqlite"), 1000).unwrap();
        let mut settings = engine.snapshot().settings;
        settings.interval_minutes = 5;
        let prepared = engine.prepare_settings(settings).unwrap();
        engine.tick(1001, 1).unwrap();
        engine.install_prepared_settings(prepared);
        assert_eq!(engine.snapshot().timer.active_seconds, 1);
        assert_eq!(engine.snapshot().timer.target_seconds, 3600);
    }

    #[test]
    fn unsupported_hard_links_never_leave_a_blocking_write_intent() {
        let dir = tempfile::tempdir().unwrap().keep();
        let mut engine = Engine::open(&dir.join("library.sqlite"), 1000).unwrap();
        crate::managed::FAIL_LINK_PROBE.with(|fail| fail.set(true));
        assert!(
            engine
                .apply(
                    Command::CreateNote {
                        title: "draft".into(),
                        content: "draft remains in editor".into()
                    },
                    1001
                )
                .is_err()
        );
        let count: i64 = engine
            .conn
            .query_row("SELECT count(*) FROM write_intents", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
        assert!(engine.snapshot().notes.is_empty());
        engine.apply(Command::SetPaused(true), 1002).unwrap();
        engine.checkpoint().unwrap();
        assert!(engine.snapshot().timer.paused);
        engine
            .apply(
                Command::CreateNote {
                    title: "retry".into(),
                    content: "saved after retry".into(),
                },
                1003,
            )
            .unwrap();
        assert_eq!(engine.snapshot().notes.len(), 1);
    }
}

#[cfg(test)]
#[path = "engine_rename_tests.rs"]
mod rename_tests;
