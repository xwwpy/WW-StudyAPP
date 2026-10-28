//! Immutable, snapshot-aligned presentation data. Build on the worker, never in render.
use chrono::{NaiveDate, TimeZone};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use studyapp_core::{DayStat, SessionStatus, Snapshot};

#[derive(Default)]
pub struct BuildWork {
    pub sessions_indexed: usize,
    pub span_day_intersections: usize,
}

#[derive(Default)]
pub struct SnapshotReadModels {
    pub library: Arc<crate::library_tree::LibraryIndex>,
    pub date: NaiveDate,
    pub learned_seconds: u64,
    pub learned_count: usize,
    pub managed_note_count: usize,
    pub unrecorded_note_count: usize,
    pub pending_sessions: Vec<usize>,
    pub work: BuildWork,
    calendar: Vec<DayStat>,
    records: [Vec<usize>; 4],
    records_by_date: HashMap<String, [Vec<usize>; 4]>,
    reviews: Vec<usize>,
    due_reviews: Vec<usize>,
    due_times: Vec<i64>,
    completed_reviews: Vec<usize>,
    completed_today: HashSet<String>,
    notes: HashMap<String, usize>,
    versions: HashMap<String, Vec<usize>>,
}

impl SnapshotReadModels {
    pub fn calendar(&self) -> &[DayStat] {
        &self.calendar
    }
    pub fn recent_days(&self, count: usize) -> &[DayStat] {
        &self.calendar[self.calendar.len().saturating_sub(count)..]
    }
    pub fn today_learned_seconds(&self) -> u64 {
        self.calendar.last().map_or(0, |day| day.learned_seconds)
    }
    pub fn due_count(&self, now: i64) -> usize {
        self.due_times.partition_point(|&time| time <= now)
    }
    pub fn review_positions(&self, filter: usize, now: i64) -> &[usize] {
        match filter {
            0 => &self.due_reviews[..self.due_count(now)],
            1 => &self.completed_reviews,
            _ => &self.reviews,
        }
    }
    pub fn review_completed_today(&self, id: &str) -> bool {
        self.completed_today.contains(id)
    }
    pub fn record_positions(&self, date: &str, filter: usize) -> &[usize] {
        let filter = if filter < 4 { filter } else { 0 };
        if date.is_empty() {
            &self.records[filter]
        } else {
            self.records_by_date
                .get(date)
                .map_or(&[], |rows| rows[filter].as_slice())
        }
    }
    pub fn note_position(&self, id: &str) -> Option<usize> {
        self.notes.get(id).copied()
    }
    pub fn version_positions(&self, note: &str) -> &[usize] {
        self.versions.get(note).map_or(&[], Vec::as_slice)
    }
    /// Preserve global reverse-capture order without scanning unrelated histories.
    pub fn selected_version_positions(&self, notes: &HashSet<String>) -> Vec<usize> {
        let mut positions: Vec<_> = notes
            .iter()
            .flat_map(|id| self.version_positions(id).iter().copied())
            .collect();
        positions.sort_unstable_by(|a, b| b.cmp(a));
        positions
    }
}

#[derive(PartialEq, Eq)]
struct SessionKey {
    id: String,
    revision: u64,
    ended_at: i64,
    active_seconds: u64,
    status: SessionStatus,
    spans: Vec<(i64, i64)>,
}
#[derive(PartialEq, Eq)]
struct InputKey {
    library: String,
    notes: Vec<(String, bool)>,
    sessions: Vec<SessionKey>,
    reviews: Vec<(String, Option<i64>)>,
    events: Vec<(String, String, i64)>,
    date: NaiveDate,
    boundaries: Vec<(String, Option<(i64, i64)>)>,
    // These exact local dates also detect timezone changes affecting older
    // records outside the graph window, even when today's UTC offset is equal.
    session_dates: Vec<String>,
    event_dates: Vec<String>,
}

fn date_of<Tz: TimeZone>(timezone: &Tz, timestamp: i64) -> String {
    timezone
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|time| time.date_naive().to_string())
        .unwrap_or_default()
}
fn start_of<Tz: TimeZone>(timezone: &Tz, date: NaiveDate) -> Option<i64> {
    (0..24).find_map(|hour| {
        timezone
            .from_local_datetime(&date.and_hms_opt(hour, 0, 0)?)
            .earliest()
            .map(|time| time.timestamp())
    })
}
impl InputKey {
    fn new<Tz: TimeZone>(snapshot: &Snapshot, timezone: &Tz, now: i64) -> Self {
        let date = timezone
            .timestamp_opt(now, 0)
            .single()
            .map(|time| time.date_naive())
            .unwrap_or_default();
        let boundaries = (0..91)
            .rev()
            .filter_map(|ago| {
                let day = date.checked_sub_signed(chrono::Duration::days(ago))?;
                let interval = start_of(timezone, day)
                    .zip(day.succ_opt().and_then(|next| start_of(timezone, next)));
                Some((day.to_string(), interval))
            })
            .collect();
        Self {
            library: snapshot.data_path.clone(),
            notes: snapshot
                .notes
                .iter()
                .map(|note| (note.id.clone(), note.detached))
                .collect(),
            sessions: snapshot
                .sessions
                .iter()
                .map(|session| SessionKey {
                    id: session.id.clone(),
                    revision: session.revision,
                    ended_at: session.ended_at,
                    active_seconds: session.active_seconds,
                    status: session.status.clone(),
                    spans: session
                        .spans
                        .iter()
                        .map(|span| (span.start, span.end))
                        .collect(),
                })
                .collect(),
            reviews: snapshot
                .reviews
                .iter()
                .map(|review| (review.id.clone(), review.next_review_at))
                .collect(),
            events: snapshot
                .review_events
                .iter()
                .map(|event| {
                    (
                        event.id.clone(),
                        event.review_id.clone(),
                        event.completed_at,
                    )
                })
                .collect(),
            date,
            boundaries,
            session_dates: snapshot
                .sessions
                .iter()
                .map(|session| date_of(timezone, session.ended_at))
                .collect(),
            event_dates: snapshot
                .review_events
                .iter()
                .map(|event| date_of(timezone, event.completed_at))
                .collect(),
        }
    }
}

#[derive(Default)]
pub struct ReadModelCache {
    key: Option<InputKey>,
    versions: Option<Arc<Vec<studyapp_core::NoteVersion>>>,
    model: Arc<SnapshotReadModels>,
}
impl ReadModelCache {
    pub fn refresh<Tz: TimeZone>(
        &mut self,
        snapshot: &Snapshot,
        timezone: &Tz,
        now: i64,
    ) -> Arc<SnapshotReadModels> {
        let key = InputKey::new(snapshot, timezone, now);
        let library_changed = self.model.library.needs_update(snapshot);
        if !library_changed
            && self.key.as_ref() == Some(&key)
            && self
                .versions
                .as_ref()
                .is_some_and(|versions| Arc::ptr_eq(versions, &snapshot.versions))
        {
            return self.model.clone();
        }
        let library = if library_changed {
            Arc::new(crate::library_tree::LibraryIndex::build(snapshot))
        } else {
            self.model.library.clone()
        };
        self.model = Arc::new(build(snapshot, &key, library));
        self.key = Some(key);
        self.versions = Some(snapshot.versions.clone());
        self.model.clone()
    }
}

fn status_slot(status: &SessionStatus) -> usize {
    match status {
        SessionStatus::Learned => 1,
        SessionStatus::Missed => 2,
        SessionStatus::Pending => 3,
    }
}
fn build(
    snapshot: &Snapshot,
    key: &InputKey,
    library: Arc<crate::library_tree::LibraryIndex>,
) -> SnapshotReadModels {
    let mut model = SnapshotReadModels {
        library,
        date: key.date,
        ..Default::default()
    };
    model.calendar = key
        .boundaries
        .iter()
        .map(|(date, _)| DayStat {
            date: date.clone(),
            learned_seconds: 0,
            learned_count: 0,
            missed_count: 0,
            review_count: 0,
        })
        .collect();
    let intervals: Vec<_> = key
        .boundaries
        .iter()
        .enumerate()
        .filter_map(|(index, (_, bounds))| bounds.map(|(start, end)| (index, start, end)))
        .collect();
    let locate = |time| {
        let index = intervals.partition_point(|&(_, _, end)| end <= time);
        intervals
            .get(index)
            .filter(|&&(_, start, end)| start <= time && time < end)
            .map(|&(index, _, _)| index)
    };
    for (index, session) in snapshot.sessions.iter().enumerate().rev() {
        model.work.sessions_indexed += 1;
        let slot = status_slot(&session.status);
        model.records[0].push(index);
        model.records[slot].push(index);
        let rows = model
            .records_by_date
            .entry(key.session_dates[index].clone())
            .or_default();
        rows[0].push(index);
        rows[slot].push(index);
        if session.status == SessionStatus::Pending {
            model.pending_sessions.push(index);
        }
        if let Some(day) = locate(session.ended_at) {
            match session.status {
                SessionStatus::Learned => model.calendar[day].learned_count += 1,
                SessionStatus::Missed => model.calendar[day].missed_count += 1,
                _ => {}
            }
        }
        if session.status != SessionStatus::Learned {
            continue;
        }
        model.learned_seconds += session.active_seconds;
        model.learned_count += 1;
        for span in &session.spans {
            let first = intervals.partition_point(|&(_, _, end)| end <= span.start);
            for &(day, start, end) in intervals[first..]
                .iter()
                .take_while(|&&(_, start, _)| start < span.end)
            {
                model.work.span_day_intersections += 1;
                model.calendar[day].learned_seconds +=
                    (span.end.min(end) - span.start.max(start)).max(0) as u64;
            }
        }
    }
    model.pending_sessions.reverse();
    let today = key.date.to_string();
    for (index, event) in snapshot.review_events.iter().enumerate() {
        if let Some(day) = locate(event.completed_at) {
            model.calendar[day].review_count += 1;
        }
        if key.event_dates[index] == today {
            model.completed_today.insert(event.review_id.clone());
        }
    }
    model.reviews = (0..snapshot.reviews.len()).collect();
    model
        .reviews
        .sort_by_key(|&index| snapshot.reviews[index].next_review_at.unwrap_or(i64::MAX));
    for &index in &model.reviews {
        let review = &snapshot.reviews[index];
        if let Some(time) = review.next_review_at {
            model.due_reviews.push(index);
            model.due_times.push(time);
        }
        if model.completed_today.contains(&review.id) {
            model.completed_reviews.push(index);
        }
    }
    for (index, note) in snapshot.notes.iter().enumerate() {
        model.notes.insert(note.id.clone(), index);
        model.managed_note_count += usize::from(!note.detached);
        model.unrecorded_note_count += usize::from(
            !note.detached
                && model.library.note(&note.id).is_some_and(|metadata| {
                    metadata.status != crate::library_tree::StudyStatus::Recorded
                }),
        );
    }
    for (index, version) in snapshot.versions.iter().enumerate() {
        model
            .versions
            .entry(version.note_id.clone())
            .or_default()
            .push(index);
    }
    model
}

#[cfg(test)]
mod today_checklist_tests {
    use super::ReadModelCache;
    use chrono::Utc;
    use studyapp_core::{Command, Engine};

    #[test]
    fn unrecorded_count_tracks_confirmation_and_excludes_detached_notes() {
        let root = tempfile::tempdir().unwrap().keep();
        let mut engine = Engine::open(&root.join("studyapp.sqlite3"), 1000).unwrap();
        engine
            .apply(
                Command::CreateNote {
                    title: "First note".into(),
                    content: "Original snapshot".into(),
                },
                1001,
            )
            .unwrap();
        let note = engine.snapshot().notes[0].id.clone();
        let mut cache = ReadModelCache::default();
        assert_eq!(
            cache
                .refresh(&engine.snapshot(), &Utc, 1001)
                .unrecorded_note_count,
            1
        );
        engine
            .apply(
                Command::Manual {
                    started_at: 940,
                    minutes: 1,
                    summary: "Confirmed learning".into(),
                    note_ids: vec![note.clone()],
                    version_ids: None,
                },
                1002,
            )
            .unwrap();
        assert_eq!(
            cache
                .refresh(&engine.snapshot(), &Utc, 1002)
                .unrecorded_note_count,
            0
        );
        engine
            .apply(
                Command::CreateNote {
                    title: "Detached note".into(),
                    content: "Retain this file".into(),
                },
                1003,
            )
            .unwrap();
        let detached = engine.snapshot().notes[1].id.clone();
        assert_eq!(
            cache
                .refresh(&engine.snapshot(), &Utc, 1003)
                .unrecorded_note_count,
            1
        );
        engine
            .apply(Command::UnmanageNote { note_id: detached }, 1004)
            .unwrap();
        assert_eq!(
            cache
                .refresh(&engine.snapshot(), &Utc, 1004)
                .unrecorded_note_count,
            0
        );
    }
}
