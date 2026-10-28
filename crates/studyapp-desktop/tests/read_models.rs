use chrono::{NaiveDate, TimeZone};
use std::sync::Arc;
use studyapp_core::{
    ActiveSpan, Review, ReviewEvent, Session, SessionStatus, Snapshot, TimerState,
};
use studyapp_desktop::{presentation::calendar_stats, read_models::ReadModelCache};

fn snapshot() -> Snapshot {
    Snapshot {
        settings: Default::default(),
        notes: vec![],
        folders: vec![],
        versions: Arc::new(vec![]),
        sessions: vec![],
        reviews: vec![],
        review_events: vec![],
        timer: TimerState {
            paused: false,
            active_seconds: 0,
            target_seconds: 3600,
            started_at: 0,
            spans: vec![],
        },
        data_path: "library".into(),
    }
}
fn session(id: &str, start: i64, end: i64, status: SessionStatus) -> Session {
    Session {
        id: id.into(),
        started_at: start,
        ended_at: end,
        active_seconds: (end - start) as u64,
        deadline: end + 60,
        status,
        summary: String::new(),
        suggested_summary: String::new(),
        note_ids: vec![],
        version_ids: vec![],
        spans: vec![ActiveSpan { start, end }],
        updated_at: end,
        revision: 1,
    }
}

#[test]
fn calendar_index_matches_reference_across_dst_and_midnight() {
    for (month, day, hours) in [(3, 8, 23), (11, 1, 25)] {
        let zone = chrono_tz::America::New_York;
        let start = zone
            .with_ymd_and_hms(2026, month, day, 0, 0, 0)
            .unwrap()
            .timestamp();
        let end = start + hours * 3600;
        let mut snapshot = snapshot();
        snapshot.sessions.push(session(
            "learned",
            start - 3600,
            end + 3600,
            SessionStatus::Learned,
        ));
        snapshot
            .sessions
            .push(session("missed", start, end, SessionStatus::Missed));
        snapshot.review_events.push(ReviewEvent {
            id: "event".into(),
            review_id: "review".into(),
            completed_at: end,
            stage: 0,
        });
        let model = ReadModelCache::default().refresh(&snapshot, &zone, end);
        let expected = calendar_stats(
            &snapshot,
            &zone,
            zone.timestamp_opt(end, 0).unwrap().date_naive(),
            91,
        );
        assert_eq!(
            serde_json::to_value(model.calendar()).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
        assert_eq!(model.today_learned_seconds(), 3600);
        assert_eq!(
            model.record_positions(&format!("2026-{month:02}-{:02}", day + 1), 1),
            &[0]
        );
    }
}

#[test]
fn timer_ticks_reuse_data_but_rollover_timezone_edits_and_library_changes_invalidate() {
    let mut snapshot = snapshot();
    let now = chrono::Utc
        .with_ymd_and_hms(2026, 10, 24, 23, 59, 59)
        .unwrap()
        .timestamp();
    snapshot
        .sessions
        .push(session("one", now - 120, now, SessionStatus::Learned));
    let mut cache = ReadModelCache::default();
    let initial = cache.refresh(&snapshot, &chrono::Utc, now);
    snapshot.timer.active_seconds += 1;
    let tick = cache.refresh(&snapshot, &chrono::Utc, now);
    assert!(Arc::ptr_eq(&initial, &tick));
    let midnight = cache.refresh(&snapshot, &chrono::Utc, now + 1);
    assert!(!Arc::ptr_eq(&tick, &midnight));
    assert_eq!(midnight.today_learned_seconds(), 0);
    let zone = chrono::FixedOffset::east_opt(3600).unwrap();
    let moved_zone = cache.refresh(&snapshot, &zone, now + 1);
    assert!(!Arc::ptr_eq(&midnight, &moved_zone));
    assert_eq!(moved_zone.record_positions("2026-10-25", 1), &[0]);
    snapshot.sessions[0].status = SessionStatus::Missed;
    snapshot.sessions[0].revision += 1;
    let edited = cache.refresh(&snapshot, &zone, now + 1);
    assert_eq!(edited.learned_count, 0);
    assert_eq!(edited.record_positions("2026-10-25", 2), &[0]);
    snapshot.data_path = "other-library".into();
    assert!(!Arc::ptr_eq(
        &edited,
        &cache.refresh(&snapshot, &zone, now + 1)
    ));
}

#[test]
fn review_index_preserves_stable_order_due_boundaries_and_undo() {
    let mut snapshot = snapshot();
    let now = chrono::Utc
        .with_ymd_and_hms(2026, 10, 24, 12, 0, 0)
        .unwrap()
        .timestamp();
    for (id, due) in [
        ("later", Some(now + 1)),
        ("a", Some(now)),
        ("b", Some(now)),
        ("complete", None),
    ] {
        snapshot.reviews.push(Review {
            id: id.into(),
            session_id: id.into(),
            title: id.into(),
            note_ids: vec![],
            learned_at: now - 86400,
            last_review_at: None,
            next_review_at: due,
            stage: 0,
            intervals: vec![1],
            revision: 1,
        });
    }
    snapshot.review_events.push(ReviewEvent {
        id: "event".into(),
        review_id: "a".into(),
        completed_at: now,
        stage: 0,
    });
    let mut cache = ReadModelCache::default();
    let model = cache.refresh(&snapshot, &chrono::Utc, now);
    assert_eq!(model.review_positions(0, now), &[1, 2]);
    assert_eq!(model.review_positions(0, now + 1), &[1, 2, 0]);
    assert_eq!(model.review_positions(1, now), &[1]);
    assert_eq!(model.review_positions(2, now), &[1, 2, 0, 3]);
    assert!(model.review_completed_today("a"));
    snapshot.review_events.clear();
    let undone = cache.refresh(&snapshot, &chrono::Utc, now);
    assert!(undone.review_positions(1, now).is_empty());
    assert!(!undone.review_completed_today("a"));
}

#[test]
fn large_snapshot_build_and_real_filtered_reads_have_bounded_work() {
    let mut snapshot = snapshot();
    let now = chrono::Utc
        .with_ymd_and_hms(2026, 10, 24, 12, 0, 0)
        .unwrap()
        .timestamp();
    for i in 0..20_000 {
        let end = now - i * 1800;
        snapshot.sessions.push(session(
            &i.to_string(),
            end - 600,
            end,
            SessionStatus::Learned,
        ));
    }
    let start = std::time::Instant::now();
    let model = ReadModelCache::default().refresh(&snapshot, &chrono::Utc, now);
    let build = start.elapsed();
    assert_eq!(model.learned_count, 20_000);
    assert_eq!(model.work.sessions_indexed, 20_000);
    assert!(model.work.span_day_intersections <= 20_000 + 91);
    let start = std::time::Instant::now();
    let old_heat = calendar_stats(&snapshot, &chrono::Utc, model.date, 91);
    let old_bars = calendar_stats(&snapshot, &chrono::Utc, model.date, 30);
    let old_render = start.elapsed();
    assert_eq!(
        serde_json::to_value(model.calendar()).unwrap(),
        serde_json::to_value(old_heat).unwrap()
    );
    assert_eq!(
        serde_json::to_value(model.recent_days(30)).unwrap(),
        serde_json::to_value(old_bars).unwrap()
    );
    let start = std::time::Instant::now();
    let mut displayed = 0;
    for _ in 0..1000 {
        let rows = model.record_positions("2026-10-24", 1);
        displayed += rows
            .iter()
            .take(50)
            .map(|&index| snapshot.sessions[index].active_seconds)
            .sum::<u64>();
        assert_eq!(model.calendar().len(), 91);
    }
    assert!(displayed > 0);
    eprintln!(
        "20,000 sessions: old render calendar work {old_render:?}; background model build {build:?}; 1,000 actual date/filter/display reads {:?}",
        start.elapsed()
    );
    assert_eq!(model.date, NaiveDate::from_ymd_opt(2026, 10, 24).unwrap());
}

#[test]
fn version_replacement_reordering_and_note_membership_keep_positions_aligned() {
    let mut snapshot = snapshot();
    for id in ["a", "b"] {
        snapshot.notes.push(studyapp_core::Note {
            id: id.into(),
            path: format!("/{id}.md"),
            title: id.into(),
            folder_id: None,
            title_override: false,
            managed: true,
            detached: false,
            missing: false,
            error: None,
            created_at: 1,
            updated_at: 1,
            revision: 1,
        });
    }
    snapshot.versions = Arc::new(
        [("a1", "a"), ("b1", "b"), ("a2", "a")]
            .into_iter()
            .map(|(id, note_id)| studyapp_core::NoteVersion {
                id: id.into(),
                note_id: note_id.into(),
                content: id.into(),
                hash: id.into(),
                captured_at: 1,
                offline: false,
                summary: String::new(),
                diff: String::new(),
            })
            .collect(),
    );
    let mut cache = ReadModelCache::default();
    let first = cache.refresh(&snapshot, &chrono::Utc, 100);
    assert_eq!(first.note_position("b"), Some(1));
    assert_eq!(first.version_positions("a"), &[0, 2]);
    let selected = ["a".into(), "b".into()].into_iter().collect();
    assert_eq!(first.selected_version_positions(&selected), [2, 1, 0]);
    snapshot.notes.swap(0, 1);
    snapshot.notes[0].detached = true;
    Arc::make_mut(&mut snapshot.versions).swap(0, 1);
    let replaced = cache.refresh(&snapshot, &chrono::Utc, 100);
    assert_eq!(replaced.note_position("b"), Some(0));
    assert_eq!(replaced.version_positions("a"), &[1, 2]);
    assert_eq!(replaced.managed_note_count, 1);
    assert!(!Arc::ptr_eq(&first.library, &replaced.library));
    // An older snapshot's indexes remain valid while a deferred library switch waits.
    assert_eq!(first.note_position("b"), Some(1));
    assert_eq!(first.version_positions("a"), &[0, 2]);
}
