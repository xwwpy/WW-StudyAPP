use crate::*;
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::Arc,
};

pub(crate) const APP_ID: i32 = 0x53545544;
pub(crate) const SCHEMA: i32 = 3;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Undo {
    pub event_id: String,
    pub review: Review,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct State {
    pub settings: Settings,
    pub notes: Vec<Note>,
    #[serde(default)]
    pub folders: Vec<NoteFolder>,
    pub sessions: Vec<Session>,
    pub reviews: Vec<Review>,
    pub review_events: Vec<ReviewEvent>,
    pub timer: TimerState,
    pub undo: Vec<Undo>,
    pub active_versions: Vec<String>,
    pub last_tick: i64,
    // A source unreadable at resume remains ineligible until its first recovered
    // baseline is captured, including across a restart.
    #[serde(default)]
    pub unattributed_notes: HashSet<String>,
    // Startup/restore reconciliation can fail while a source is missing or
    // unreadable. Its first recovered snapshot must keep offline provenance.
    #[serde(default)]
    pub offline_unreconciled_notes: HashSet<String>,
    #[serde(skip)]
    pub versions: Arc<Vec<NoteVersion>>,
    #[serde(skip)]
    pub candidates: HashMap<String, (String, i64)>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            interval_minutes: 60,
            answer_minutes: 5,
            review_days: vec![1, 3, 7, 14, 30],
            show_file_paths: false,
            layout: Default::default(),
            appearance: Default::default(),
            images: Default::default(),
        }
    }
}
impl Settings {
    pub fn validate(&self) -> Result<()> {
        self.layout.validate()?;
        self.appearance.validate()?;
        crate::media_settings::validate_images(&self.images)?;
        ensure!(
            (5..=240).contains(&self.interval_minutes),
            "询问间隔应为 5–240 分钟"
        );
        ensure!(
            (1..=30).contains(&self.answer_minutes),
            "回答期限应为 1–30 分钟"
        );
        ensure!(
            (1..=20).contains(&self.review_days.len())
                && self.review_days.iter().all(|d| (1..=3650).contains(d)),
            "复习间隔须为 1–3650 天，共 1–20 阶段"
        );
        Ok(())
    }
}
impl crate::LayoutSettings {
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            (128..=320).contains(&self.navigation_width)
                && (112..=360).contains(&self.folders_width)
                && (160..=440).contains(&self.notes_width),
            "面板宽度超出允许范围"
        );
        Ok(())
    }
}
impl State {
    pub fn new(now: i64) -> Self {
        Self {
            settings: Settings::default(),
            notes: vec![],
            folders: vec![],
            sessions: vec![],
            reviews: vec![],
            review_events: vec![],
            timer: TimerState {
                paused: false,
                active_seconds: 0,
                target_seconds: 3600,
                started_at: now,
                spans: vec![],
            },
            undo: vec![],
            active_versions: vec![],
            last_tick: now,
            unattributed_notes: HashSet::new(),
            offline_unreconciled_notes: HashSet::new(),
            versions: Arc::new(vec![]),
            candidates: HashMap::new(),
        }
    }
    pub fn snapshot(&self, path: String) -> Snapshot {
        Snapshot {
            settings: self.settings.clone(),
            notes: self.notes.clone(),
            folders: self.folders.clone(),
            versions: self.versions.clone(),
            sessions: self.sessions.clone(),
            reviews: self.reviews.clone(),
            review_events: self.review_events.clone(),
            timer: self.timer.clone(),
            data_path: path,
        }
    }
    pub fn expire(&mut self, now: i64) {
        for s in &mut self.sessions {
            if s.status == SessionStatus::Pending && now >= s.deadline {
                s.status = SessionStatus::Missed;
                s.updated_at = now;
                s.revision += 1;
            }
        }
    }
    pub fn validate(&self) -> Result<()> {
        self.settings.validate()?;
        crate::folders::validate(self)?;
        ensure!(
            (300..=14400).contains(&self.timer.target_seconds)
                && self.timer.active_seconds < self.timer.target_seconds,
            "备份计时状态无效"
        );
        ensure!(
            span_seconds(&self.timer.spans)? == self.timer.active_seconds
                && self
                    .timer
                    .spans
                    .last()
                    .is_none_or(|s| s.end <= self.last_tick),
            "备份活动时长与区间不一致"
        );
        let mut ids = std::collections::HashSet::new();
        for n in &self.notes {
            ensure!(
                !n.id.is_empty() && n.revision > 0 && ids.insert(&n.id),
                "笔记 ID 或修订号无效"
            );
            if n.managed {
                crate::managed::validate_id(&n.id)?;
                ensure!(n.title.chars().count() <= 200, "托管笔记标题超过 200 字符");
            }
        }
        let notes = ids;
        let mut version_ids = std::collections::HashSet::new();
        for v in self.versions.iter() {
            use sha2::{Digest, Sha256};
            ensure!(
                !v.id.is_empty() && notes.contains(&v.note_id) && version_ids.insert(&v.id),
                "版本关联无效"
            );
            ensure!(
                v.content.len() <= 10 * 1024 * 1024
                    && v.hash == format!("{:x}", Sha256::digest(v.content.as_bytes())),
                "笔记版本内容或哈希损坏"
            );
        }
        ensure!(
            self.active_versions
                .iter()
                .all(|id| version_ids.contains(id)),
            "活动版本关联无效"
        );
        let mut sessions = std::collections::HashSet::new();
        for s in &self.sessions {
            ensure!(
                sessions.insert(&s.id)
                    && s.started_at <= s.ended_at
                    && s.spans.iter().all(|p| p.start <= p.end)
                    && s.note_ids.iter().all(|n| notes.contains(n))
                    && s.version_ids.iter().all(|id| version_ids.contains(id)
                        && self
                            .versions
                            .iter()
                            .any(|v| v.id == *id && s.note_ids.contains(&v.note_id))),
                "学习记录无效"
            );
            ensure!(
                s.active_seconds > 0
                    && span_seconds(&s.spans)? == s.active_seconds
                    && s.spans.first().is_some_and(|p| p.start >= s.started_at)
                    && s.spans.last().is_some_and(|p| p.end <= s.ended_at)
                    && s.deadline >= s.ended_at
                    && s.revision > 0,
                "学习活动区间无效"
            );
            ensure!(
                s.status != SessionStatus::Learned || !s.summary.trim().is_empty(),
                "已学习记录缺少摘要"
            );
        }
        let mut review_ids = std::collections::HashSet::new();
        let mut reviewed_sessions = std::collections::HashSet::new();
        for r in &self.reviews {
            validate_review_model(r, &notes)?;
            ensure!(
                review_ids.insert(&r.id)
                    && reviewed_sessions.insert(&r.session_id)
                    && sessions.contains(&r.session_id)
                    && !r.intervals.is_empty()
                    && r.intervals.len() <= 20
                    && r.intervals.iter().all(|d| (1..=3650).contains(d))
                    && r.stage <= r.intervals.len(),
                "复习记录无效"
            );
            ensure!(
                (r.stage < r.intervals.len()) == r.next_review_at.is_some()
                    && (r.stage > 0) == r.last_review_at.is_some()
                    && r.note_ids.iter().all(|n| notes.contains(n)),
                "复习时间或关联无效"
            );
            let session = self
                .sessions
                .iter()
                .find(|s| s.id == r.session_id)
                .context("复习所属记录不存在")?;
            ensure!(
                session.status == SessionStatus::Learned
                    && session.summary == r.title
                    && session.note_ids == r.note_ids
                    && session.ended_at == r.learned_at,
                "复习与学习记录不一致"
            );
            let events: Vec<_> = self
                .review_events
                .iter()
                .filter(|e| e.review_id == r.id)
                .collect();
            ensure!(
                events.len() == r.stage
                    && events.iter().enumerate().all(|(index, e)| e.stage == index),
                "复习事件阶段无效"
            );
            ensure!(
                r.last_review_at == events.last().map(|e| e.completed_at),
                "复习时间与事件历史不一致"
            );
        }
        let mut event_ids = std::collections::HashSet::new();
        for e in &self.review_events {
            ensure!(
                !e.id.is_empty() && review_ids.contains(&e.review_id) && event_ids.insert(&e.id),
                "复习事件关联无效"
            );
        }
        ensure!(
            self.undo.len() == self.review_events.len(),
            "复习撤销记录数量无效"
        );
        let mut undo_events = HashSet::new();
        let mut next_undo_stage = HashMap::<&str, usize>::new();
        for undo in &self.undo {
            let prior = &undo.review;
            validate_review_model(prior, &notes)?;
            ensure!(undo_events.insert(&undo.event_id), "复习撤销事件重复");
            let event = self
                .review_events
                .iter()
                .find(|e| e.id == undo.event_id)
                .context("撤销所属事件不存在")?;
            let current = self
                .reviews
                .iter()
                .find(|r| r.id == prior.id)
                .context("撤销所属复习不存在")?;
            ensure!(
                event.review_id == prior.id && event.stage == prior.stage,
                "撤销事件与复习阶段不匹配"
            );
            ensure!(
                prior.session_id == current.session_id
                    && prior.learned_at == current.learned_at
                    && prior.intervals == current.intervals
                    && prior.revision < current.revision,
                "撤销记录的学习来源或计划配置无效"
            );
            ensure!(
                prior
                    .next_review_at
                    .is_some_and(|due| due <= event.completed_at),
                "撤销记录未达到复习期限"
            );
            let previous_event = self
                .review_events
                .iter()
                .find(|e| e.review_id == prior.id && e.stage.checked_add(1) == Some(prior.stage));
            ensure!(
                prior.last_review_at == previous_event.map(|e| e.completed_at),
                "撤销时间与此前事件不一致"
            );
            let expected = next_undo_stage.entry(&prior.id).or_default();
            ensure!(prior.stage == *expected, "同一复习计划的撤销记录顺序无效");
            *expected += 1;
        }
        ensure!(
            self.unattributed_notes.iter().all(|id| notes.contains(id)),
            "暂停基线笔记关联无效"
        );
        ensure!(
            self.offline_unreconciled_notes
                .iter()
                .all(|id| notes.contains(id)),
            "离线基线笔记关联无效"
        );
        Ok(())
    }
}
fn validate_review_model(review: &Review, notes: &HashSet<&String>) -> Result<()> {
    let unique_notes: HashSet<_> = review.note_ids.iter().collect();
    ensure!(
        !review.id.is_empty()
            && !review.session_id.is_empty()
            && review.revision > 0
            && !review.title.trim().is_empty()
            && review.title.len() <= 100_000
            && (1..=20).contains(&review.intervals.len())
            && review.intervals.iter().all(|d| (1..=3650).contains(d))
            && review.stage <= review.intervals.len()
            && review.note_ids.iter().all(|id| notes.contains(id))
            && unique_notes.len() == review.note_ids.len(),
        "复习模型字段无效"
    );
    ensure!(
        (review.stage > 0) == review.last_review_at.is_some()
            && review
                .last_review_at
                .is_none_or(|last| last >= review.learned_at),
        "复习模型上次完成时间无效"
    );
    let expected_due = review
        .intervals
        .get(review.stage)
        .map(|days| {
            review
                .last_review_at
                .unwrap_or(review.learned_at)
                .checked_add(i64::from(*days) * 86400)
                .context("复习时间超出范围")
        })
        .transpose()?;
    ensure!(
        review.next_review_at == expected_due,
        "复习模型下次期限与阶段不一致"
    );
    Ok(())
}
fn span_seconds(spans: &[ActiveSpan]) -> Result<u64> {
    let mut sum = 0_u64;
    let mut previous_end = None;
    for span in spans {
        ensure!(
            span.start < span.end && previous_end.is_none_or(|end| end <= span.start),
            "活动区间重叠或无效"
        );
        let duration = span
            .end
            .checked_sub(span.start)
            .context("活动区间超出范围")?;
        sum = sum
            .checked_add(duration as u64)
            .context("活动时长超出范围")?;
        previous_end = Some(span.end);
    }
    Ok(sum)
}
pub(crate) fn configure(conn: &Connection) -> Result<()> {
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute_batch(
        "PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;",
    )?;
    Ok(())
}
pub(crate) fn initialize(conn: &mut Connection) -> Result<()> {
    let app_id: i32 = conn.query_row("PRAGMA application_id", [], |r| r.get(0))?;
    let tables: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
        [],
        |r| r.get(0),
    )?;
    ensure!(
        app_id == APP_ID || (app_id == 0 && tables == 0),
        "此文件不是 StudyApp 资料库"
    );
    if app_id == 0 {
        let tx = conn.transaction()?;
        tx.execute_batch("CREATE TABLE app_state (id INTEGER PRIMARY KEY CHECK(id=1), data TEXT NOT NULL); CREATE TABLE note_versions (sequence INTEGER PRIMARY KEY, id TEXT NOT NULL UNIQUE, data TEXT NOT NULL); CREATE TABLE write_intents (id INTEGER PRIMARY KEY CHECK(id=1), data TEXT NOT NULL);")?;
        tx.pragma_update(None, "application_id", APP_ID)?;
        tx.pragma_update(None, "user_version", SCHEMA)?;
        tx.commit()?;
    }
    let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version == 1 || version == 2 {
        let tx = conn.transaction()?;
        if version == 1 {
            tx.execute_batch(
                "CREATE TABLE write_intents (id INTEGER PRIMARY KEY CHECK(id=1), data TEXT NOT NULL);",
            )?;
        }
        tx.pragma_update(None, "user_version", SCHEMA)?;
        tx.commit()?;
        return Ok(());
    }
    ensure!(version == SCHEMA, "资料库版本不受支持：{version}");
    Ok(())
}
pub(crate) fn pending_write_intent_count(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT count(*) FROM write_intents", [], |row| row.get(0))?)
}

pub(crate) fn load(conn: &Connection) -> Result<Option<State>> {
    let raw: Option<String> = conn
        .query_row("SELECT data FROM app_state WHERE id=1", [], |r| r.get(0))
        .optional()?;
    let Some(raw) = raw else {
        return Ok(None);
    };
    let mut state: State = serde_json::from_str(&raw).context("资料库状态损坏")?;
    let mut statement = conn.prepare("SELECT data FROM note_versions ORDER BY sequence")?;
    let rows = statement.query_map([], |r| r.get::<_, String>(0))?;
    let versions = rows
        .map(|r| Ok(serde_json::from_str(&r?)?))
        .collect::<Result<Vec<NoteVersion>>>()?;
    state.versions = Arc::new(versions);
    state.validate()?;
    Ok(Some(state))
}
pub(crate) fn save(
    conn: &mut Connection,
    state: &State,
    previous_count: usize,
    replace: bool,
) -> Result<()> {
    let tx = conn.transaction()?;
    write_state(&tx, state, previous_count, replace)?;
    tx.commit()?;
    Ok(())
}
pub(crate) fn write_state(
    tx: &rusqlite::Transaction<'_>,
    state: &State,
    previous_count: usize,
    replace: bool,
) -> Result<()> {
    let raw = serde_json::to_string(state)?;
    if replace {
        tx.execute("DELETE FROM note_versions", [])?;
    }
    for v in state
        .versions
        .iter()
        .skip(if replace { 0 } else { previous_count })
    {
        tx.execute(
            "INSERT INTO note_versions(id,data) VALUES (?1,?2)",
            rusqlite::params![v.id, serde_json::to_string(v)?],
        )?;
    }
    tx.execute("INSERT INTO app_state(id,data) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET data=excluded.data", [raw])?;
    Ok(())
}
pub(crate) fn verified_backup(path: &Path) -> Result<State> {
    let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let app_id: i32 = conn.query_row("PRAGMA application_id", [], |r| r.get(0))?;
    let schema: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    ensure!(
        app_id == APP_ID && (1..=SCHEMA).contains(&schema),
        "备份不是受支持的 StudyApp 资料库"
    );
    let check: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    ensure!(check == "ok", "备份完整性检查失败：{check}");
    if schema >= 2 {
        let pending: i64 =
            conn.query_row("SELECT count(*) FROM write_intents", [], |r| r.get(0))?;
        ensure!(
            pending == 0,
            "备份含有未完成的托管笔记写入，请先在原资料库恢复写入"
        );
    }
    load(&conn)?.context("备份中没有资料")
}
