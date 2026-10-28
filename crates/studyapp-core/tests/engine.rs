use std::path::PathBuf;
use studyapp_core::*;
struct TestDir(std::path::PathBuf);
impl TestDir {
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

fn fixture() -> (TestDir, Engine) {
    let dir = TestDir(tempfile::tempdir().unwrap().keep());
    let engine = Engine::open(&dir.path().join("learning.sqlite"), 1000).unwrap();
    (dir, engine)
}
fn short(engine: &mut Engine, now: i64) {
    engine
        .apply(
            Command::SaveSettings(Settings {
                interval_minutes: 5,
                answer_minutes: 1,
                review_days: vec![1, 3],
                show_file_paths: false,
                layout: Default::default(),
                appearance: Default::default(),
                images: Default::default(),
            }),
            now,
        )
        .unwrap();
}
fn cycle(engine: &mut Engine, start: i64) {
    for n in 1..=300 {
        engine.tick(start + n, 1).unwrap();
    }
}
#[test]
fn timer_excludes_pause_sleep_and_restart_but_expires_pending() {
    let (dir, mut engine) = fixture();
    short(&mut engine, 1000);
    cycle(&mut engine, 1000);
    let state = engine.snapshot();
    assert_eq!(state.sessions.len(), 1);
    assert_eq!(state.sessions[0].active_seconds, 300);
    assert_eq!(state.sessions[0].status, SessionStatus::Pending);
    engine.apply(Command::SetPaused(true), 1300).unwrap();
    engine.tick(1400, 100).unwrap();
    assert_eq!(engine.snapshot().sessions[0].status, SessionStatus::Missed);
    assert_eq!(engine.snapshot().timer.active_seconds, 0);
    engine.apply(Command::SetPaused(false), 1400).unwrap();
    engine.tick(1401, 1).unwrap();
    engine.tick(2400, 999).unwrap();
    assert_eq!(engine.snapshot().timer.active_seconds, 1);
    drop(engine);
    let engine = Engine::open(&dir.path().join("learning.sqlite"), 10000).unwrap();
    assert_eq!(engine.snapshot().timer.active_seconds, 1);
    assert_eq!(engine.snapshot().sessions.len(), 1);
}
#[test]
fn submit_and_review_are_transactional_and_undo_restores_due_exactly() {
    let (_dir, mut engine) = fixture();
    short(&mut engine, 1000);
    cycle(&mut engine, 1000);
    let id = engine.snapshot().sessions[0].id.clone();
    engine
        .apply(
            Command::Submit {
                version_ids: None,
                session_id: id.clone(),
                summary: "学习 Rust".into(),
                note_ids: vec![],
            },
            1301,
        )
        .unwrap();
    assert!(
        engine
            .apply(
                Command::Submit {
                    version_ids: None,
                    session_id: id.clone(),
                    summary: "重复".into(),
                    note_ids: vec![]
                },
                1302
            )
            .is_err()
    );
    let review = engine.snapshot().reviews[0].clone();
    let due = review.next_review_at.unwrap();
    assert!(
        engine
            .apply(
                Command::CompleteReview {
                    review_id: review.id.clone(),
                    expected_stage: 0
                },
                due - 1
            )
            .is_err()
    );
    engine
        .apply(
            Command::CompleteReview {
                review_id: review.id.clone(),
                expected_stage: 0,
            },
            due + 100,
        )
        .unwrap();
    assert!(
        engine
            .apply(
                Command::CompleteReview {
                    review_id: review.id.clone(),
                    expected_stage: 0
                },
                due + 100
            )
            .is_err()
    );
    engine
        .apply(
            Command::UndoReview {
                review_id: review.id.clone(),
            },
            due + 200,
        )
        .unwrap();
    let undone = engine.snapshot();
    assert_eq!(undone.reviews[0].next_review_at, Some(due));
    assert_eq!(undone.reviews[0].stage, 0);
    assert!(undone.review_events.is_empty());
    engine
        .apply(
            Command::EditSession {
                version_ids: None,
                session_id: id,
                summary: String::new(),
                learned: false,
                note_ids: vec![],
            },
            due + 201,
        )
        .unwrap();
    assert!(engine.snapshot().reviews.is_empty());
}
#[test]
fn markdown_debounces_diffs_retains_missing_files_and_offline_changes() {
    let (dir, mut engine) = fixture();
    short(&mut engine, 1000);
    let path = dir.path().join("note.md");
    std::fs::write(&path, "\u{feff}# 标题\n初始\n").unwrap();
    engine
        .apply(Command::Import(vec![path.clone(), path.clone()]), 1000)
        .unwrap();
    assert_eq!(engine.snapshot().notes.len(), 1);
    assert_eq!(engine.snapshot().versions.len(), 1);
    std::fs::write(&path, "# 标题\n新增知识\n").unwrap();
    engine
        .apply(Command::Scan { offline: false }, 1001)
        .unwrap();
    assert_eq!(engine.snapshot().versions.len(), 1);
    engine
        .apply(Command::Scan { offline: false }, 1004)
        .unwrap();
    assert_eq!(engine.snapshot().versions.len(), 2);
    assert!(engine.snapshot().versions[1].diff.contains("+新增知识"));
    cycle(&mut engine, 1000);
    assert!(
        engine.snapshot().sessions[0]
            .suggested_summary
            .contains("新增知识")
    );
    std::fs::write(&path, "# 离线编辑\n新内容\n").unwrap();
    drop(engine);
    let engine = Engine::open(&dir.path().join("learning.sqlite"), 2000).unwrap();
    assert_eq!(engine.snapshot().versions.len(), 3);
    assert!(engine.snapshot().versions[2].offline);
}
#[test]
fn backup_restore_roundtrips_and_rejects_live_or_foreign_databases() {
    let (dir, mut engine) = fixture();
    let backup = dir.path().join("saved.sqlite");
    engine.apply(Command::Backup(backup.clone()), 1000).unwrap();
    short(&mut engine, 1000);
    engine.apply(Command::Restore(backup), 1001).unwrap();
    assert_eq!(engine.snapshot().settings.interval_minutes, 60);
    assert!(
        engine
            .apply(Command::Backup(dir.path().join("learning.sqlite")), 1001)
            .is_err()
    );
    let alien = dir.path().join("alien.sqlite");
    rusqlite::Connection::open(&alien).unwrap();
    assert!(engine.apply(Command::Restore(alien), 1002).is_err());
    assert_eq!(engine.snapshot().settings.interval_minutes, 60);
}
#[test]
fn validation_and_interval_changes_leave_state_consistent() {
    let (_dir, mut engine) = fixture();
    engine.tick(1001, 1).unwrap();
    short(&mut engine, 1001);
    assert_eq!(engine.snapshot().timer.target_seconds, 3600);
    let mut invalid = engine.snapshot().settings;
    invalid.review_days = vec![0];
    assert!(engine.apply(Command::SaveSettings(invalid), 1002).is_err());
    assert_eq!(engine.snapshot().settings.review_days, vec![1, 3]);
    assert!(
        engine
            .apply(Command::Import(vec![PathBuf::from("missing.md")]), 1002)
            .unwrap()
            .contains("失败")
    );
}
#[test]
fn stats_split_confirmed_active_spans_at_midnight_and_ignore_missed() {
    let (_dir, mut engine) = fixture();
    engine
        .apply(
            Command::Manual {
                version_ids: None,
                started_at: 86340,
                minutes: 2,
                summary: "跨日学习".into(),
                note_ids: vec![],
            },
            86500,
        )
        .unwrap();
    let stats = daily_stats(&engine.snapshot(), 0, "1970-01-02", 2);
    assert_eq!(stats[0].learned_seconds, 60);
    assert_eq!(stats[1].learned_seconds, 60);
    assert_eq!(stats[0].learned_count, 0);
    assert_eq!(stats[1].learned_count, 1);
}
#[test]
fn persisted_active_versions_survive_restart_and_pause_excludes_changes() {
    let (dir, mut engine) = fixture();
    short(&mut engine, 1000);
    let path = dir.path().join("study.md");
    std::fs::write(&path, "# 阅读\n起始\n").unwrap();
    engine
        .apply(Command::Import(vec![path.clone()]), 1000)
        .unwrap();
    engine.tick(1001, 1).unwrap();
    std::fs::write(&path, "# 阅读\n活动内容\n").unwrap();
    engine
        .apply(Command::Scan { offline: false }, 1002)
        .unwrap();
    engine
        .apply(Command::Scan { offline: false }, 1005)
        .unwrap();
    engine.tick(1005, 4).unwrap();
    engine.apply(Command::SetPaused(true), 1005).unwrap();
    std::fs::write(&path, "# 阅读\n暂停内容\n").unwrap();
    engine
        .apply(Command::Scan { offline: false }, 1006)
        .unwrap();
    engine
        .apply(Command::Scan { offline: false }, 1009)
        .unwrap();
    drop(engine);
    let mut engine = Engine::open(&dir.path().join("learning.sqlite"), 2000).unwrap();
    engine.apply(Command::SetPaused(false), 2000).unwrap();
    for n in 1..=295 {
        engine.tick(2000 + n, 1).unwrap();
    }
    let s = engine.snapshot().sessions[0].clone();
    assert!(s.suggested_summary.contains("活动内容"));
    assert!(!s.suggested_summary.contains("暂停内容"));
    assert_eq!(s.version_ids.len(), 1);
    assert_eq!(s.spans.len(), 2);
}
#[test]
fn clock_rollback_never_adds_overlapping_active_time() {
    let (_dir, mut engine) = fixture();
    engine.tick(1010, 10).unwrap();
    engine.tick(900, 1).unwrap();
    engine.tick(901, 1).unwrap();
    engine.tick(1011, 1).unwrap();
    assert_eq!(engine.snapshot().timer.active_seconds, 11);
    let spans = engine.snapshot().timer.spans;
    assert!(spans.windows(2).all(|s| s[0].end <= s[1].start));
}
#[test]
fn filesystem_errors_retain_history_and_relink_checks_duplicate_path() {
    let (dir, mut engine) = fixture();
    let original = dir.path().join("original.md");
    let second = dir.path().join("second.md");
    std::fs::write(&original, "# 原笔记\n正文").unwrap();
    std::fs::write(&second, "# 第二份").unwrap();
    engine
        .apply(
            Command::Import(vec![original.clone(), second.clone()]),
            1000,
        )
        .unwrap();
    let note = engine.snapshot().notes[0].clone();
    std::fs::rename(&original, dir.path().join("renamed.md")).unwrap();
    engine
        .apply(Command::Scan { offline: false }, 1001)
        .unwrap();
    assert!(engine.snapshot().notes[0].missing);
    assert_eq!(engine.snapshot().versions.len(), 2);
    assert!(
        engine
            .apply(
                Command::Relink {
                    note_id: note.id.clone(),
                    path: second
                },
                1002
            )
            .is_err()
    );
    engine
        .apply(
            Command::Relink {
                note_id: note.id,
                path: dir.path().join("renamed.md"),
            },
            1003,
        )
        .unwrap();
    assert!(!engine.snapshot().notes[0].missing);
    assert_eq!(engine.snapshot().versions.len(), 2);
}
#[test]
fn review_history_survives_edits_defaults_change_and_completes_final_stage() {
    let (_dir, mut engine) = fixture();
    short(&mut engine, 1000);
    engine
        .apply(
            Command::Manual {
                version_ids: None,
                started_at: 900,
                minutes: 1,
                summary: "初稿".into(),
                note_ids: vec![],
            },
            1000,
        )
        .unwrap();
    let first = engine.snapshot();
    let session = first.sessions[0].id.clone();
    let review = first.reviews[0].clone();
    engine
        .apply(
            Command::CompleteReview {
                review_id: review.id.clone(),
                expected_stage: 0,
            },
            100000,
        )
        .unwrap();
    engine
        .apply(
            Command::EditSession {
                version_ids: None,
                session_id: session,
                summary: "修改标题".into(),
                learned: true,
                note_ids: vec![],
            },
            100001,
        )
        .unwrap();
    engine
        .apply(Command::SaveSettings(Settings::default()), 100001)
        .unwrap();
    engine
        .apply(
            Command::UndoReview {
                review_id: review.id.clone(),
            },
            100002,
        )
        .unwrap();
    assert_eq!(engine.snapshot().reviews[0].title, "修改标题");
    assert_eq!(engine.snapshot().reviews[0].intervals, vec![1, 3]);
    engine
        .apply(
            Command::CompleteReview {
                review_id: review.id.clone(),
                expected_stage: 0,
            },
            100003,
        )
        .unwrap();
    engine
        .apply(
            Command::CompleteReview {
                review_id: review.id.clone(),
                expected_stage: 1,
            },
            400000,
        )
        .unwrap();
    assert_eq!(engine.snapshot().reviews[0].next_review_at, None);
    assert!(
        engine
            .apply(
                Command::CompleteReview {
                    review_id: review.id.clone(),
                    expected_stage: 2
                },
                500000
            )
            .is_err()
    );
    engine
        .apply(
            Command::UndoReview {
                review_id: review.id,
            },
            500000,
        )
        .unwrap();
    assert_eq!(engine.snapshot().reviews[0].next_review_at, Some(359203));
    assert_eq!(engine.snapshot().review_events.len(), 1);
}
#[test]
fn failed_sql_write_rolls_back_memory_and_database() {
    let (dir, mut engine) = fixture();
    let external = rusqlite::Connection::open(dir.path().join("learning.sqlite")).unwrap();
    external.execute_batch("CREATE TRIGGER reject_settings BEFORE UPDATE ON app_state BEGIN SELECT RAISE(ABORT,'test disk failure'); END;").unwrap();
    assert!(
        engine
            .apply(
                Command::SaveSettings(Settings {
                    interval_minutes: 5,
                    answer_minutes: 1,
                    review_days: vec![2],
                    show_file_paths: false,
                    layout: Default::default(),
                    appearance: Default::default(),
                    images: Default::default(),
                }),
                1000
            )
            .is_err()
    );
    assert_eq!(engine.snapshot().settings.interval_minutes, 60);
    let raw: String = external
        .query_row("SELECT data FROM app_state WHERE id=1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&raw).unwrap()["settings"]["interval_minutes"],
        60
    );
}
#[test]
fn cloud_and_wechat_services_are_explicitly_unavailable() {
    use studyapp_core::integration::*;
    let services = UnavailableServices;
    assert!(services.begin_wechat_authorization().is_err());
    assert!(services.poll_authorization("anything").is_err());
    assert!(services.restore_session().is_err());
    assert!(services.sign_out().is_err());
    assert!(services.synchronize(StorageMode::Cloud).is_err());
}
#[test]
fn restoring_backup_marks_later_source_changes_offline() {
    let (dir, mut engine) = fixture();
    let path = dir.path().join("restore-note.md");
    std::fs::write(&path, "# 初始").unwrap();
    engine
        .apply(Command::Import(vec![path.clone()]), 1000)
        .unwrap();
    let backup = dir.path().join("before-edit.sqlite");
    engine.apply(Command::Backup(backup.clone()), 1001).unwrap();
    std::fs::write(&path, "# 备份以后的内容").unwrap();
    engine.apply(Command::Restore(backup), 2000).unwrap();
    assert_eq!(engine.snapshot().versions.len(), 2);
    assert!(engine.snapshot().versions[1].offline);
    assert_eq!(engine.snapshot().timer.active_seconds, 0);
}
#[test]
fn backup_with_inconsistent_activity_is_rejected_without_changing_live_state() {
    let (dir, mut engine) = fixture();
    let backup = dir.path().join("corrupted.sqlite");
    engine.apply(Command::Backup(backup.clone()), 1000).unwrap();
    let alien = rusqlite::Connection::open(&backup).unwrap();
    let raw: String = alien
        .query_row("SELECT data FROM app_state WHERE id=1", [], |r| r.get(0))
        .unwrap();
    let mut data: serde_json::Value = serde_json::from_str(&raw).unwrap();
    data["timer"]["active_seconds"] = serde_json::json!(10);
    alien
        .execute("UPDATE app_state SET data=?1", [data.to_string()])
        .unwrap();
    drop(alien);
    assert!(engine.apply(Command::Restore(backup), 1001).is_err());
    assert_eq!(engine.snapshot().timer.active_seconds, 0);
}
#[test]
fn invalid_encoding_oversize_and_non_markdown_imports_are_partial_failures() {
    let (dir, mut engine) = fixture();
    let invalid = dir.path().join("invalid.md");
    let huge = dir.path().join("huge.md");
    let other = dir.path().join("other.txt");
    let valid = dir.path().join("valid.markdown");
    std::fs::write(&invalid, [0xff, 0xfe]).unwrap();
    std::fs::File::create(&huge)
        .unwrap()
        .set_len(10 * 1024 * 1024 + 1)
        .unwrap();
    std::fs::write(&other, "text").unwrap();
    std::fs::write(&valid, "# Valid\n").unwrap();
    let message = engine
        .apply(Command::Import(vec![invalid, huge, other, valid]), 1000)
        .unwrap();
    assert!(message.contains("失败 3"));
    assert_eq!(engine.snapshot().notes.len(), 1);
    assert_eq!(engine.snapshot().versions.len(), 1);
}
#[test]
fn activity_ten_second_tick_splits_boundary_and_settings_take_next_cycle() {
    let (_dir, mut engine) = fixture();
    short(&mut engine, 1000);
    for n in 1..=299 {
        engine.tick(1000 + n, 1).unwrap();
    }
    engine
        .apply(
            Command::SaveSettings(Settings {
                interval_minutes: 10,
                answer_minutes: 2,
                review_days: vec![2],
                show_file_paths: false,
                layout: Default::default(),
                appearance: Default::default(),
                images: Default::default(),
            }),
            1299,
        )
        .unwrap();
    engine.tick(1309, 10).unwrap();
    let snapshot = engine.snapshot();
    assert_eq!(snapshot.sessions.len(), 1);
    assert_eq!(snapshot.sessions[0].ended_at, 1300);
    assert_eq!(snapshot.sessions[0].deadline, 1420);
    assert_eq!(snapshot.timer.active_seconds, 9);
    assert_eq!(snapshot.timer.target_seconds, 600);
    assert_eq!(snapshot.timer.spans[0].start, 1300);
}

#[test]
fn learned_review_and_undo_persist_through_restarts() {
    let (dir, mut engine) = fixture();
    short(&mut engine, 1000);
    engine
        .apply(
            Command::Manual {
                version_ids: None,
                started_at: 900,
                minutes: 1,
                summary: "持久化复习".into(),
                note_ids: vec![],
            },
            1000,
        )
        .unwrap();
    let review = engine.snapshot().reviews[0].clone();
    let due = review.next_review_at;
    engine
        .apply(
            Command::CompleteReview {
                review_id: review.id.clone(),
                expected_stage: 0,
            },
            100000,
        )
        .unwrap();
    engine.checkpoint().unwrap();
    drop(engine);
    let mut engine = Engine::open(&dir.path().join("learning.sqlite"), 100001).unwrap();
    assert_eq!(engine.snapshot().review_events.len(), 1);
    engine
        .apply(
            Command::UndoReview {
                review_id: review.id,
            },
            100002,
        )
        .unwrap();
    drop(engine);
    let engine = Engine::open(&dir.path().join("learning.sqlite"), 100003).unwrap();
    assert_eq!(engine.snapshot().reviews[0].next_review_at, due);
    assert!(engine.snapshot().review_events.is_empty());
}
#[test]
fn backup_rejects_wal_shm_and_does_not_overwrite_existing_files() {
    let (dir, mut engine) = fixture();
    for name in ["learning.sqlite-wal", "learning.sqlite-shm"] {
        assert!(
            engine
                .apply(Command::Backup(dir.path().join(name)), 1000)
                .is_err()
        );
    }
    let existing = dir.path().join("existing.sqlite");
    std::fs::write(&existing, "保留内容").unwrap();
    assert!(
        engine
            .apply(Command::Backup(existing.clone()), 1000)
            .is_err()
    );
    assert_eq!(std::fs::read_to_string(existing).unwrap(), "保留内容");
}
#[test]
fn stats_counts_review_events_and_corrections_with_timezone_offset() {
    let (_dir, mut engine) = fixture();
    engine
        .apply(
            Command::Manual {
                version_ids: None,
                started_at: 57540,
                minutes: 2,
                summary: "北京时间午夜".into(),
                note_ids: vec![],
            },
            57700,
        )
        .unwrap();
    let snapshot = engine.snapshot();
    let review = snapshot.reviews[0].clone();
    let session = snapshot.sessions[0].id.clone();
    let due = review.next_review_at.unwrap();
    engine
        .apply(
            Command::CompleteReview {
                review_id: review.id.clone(),
                expected_stage: 0,
            },
            due,
        )
        .unwrap();
    let stats = daily_stats(&engine.snapshot(), 8 * 3600, "1970-01-03", 3);
    assert_eq!(stats[0].learned_seconds, 60);
    assert_eq!(stats[1].learned_seconds, 60);
    assert_eq!(stats[2].review_count, 1);
    engine
        .apply(
            Command::EditSession {
                version_ids: None,
                session_id: session,
                summary: String::new(),
                learned: false,
                note_ids: vec![],
            },
            due + 1,
        )
        .unwrap();
    let stats = daily_stats(&engine.snapshot(), 8 * 3600, "1970-01-03", 3);
    assert_eq!(stats.iter().map(|s| s.learned_seconds).sum::<u64>(), 0);
    assert_eq!(stats[1].missed_count, 1);
    assert_eq!(stats[2].review_count, 0);
}
#[test]
fn paused_candidate_captured_after_resume_never_enters_learning_suggestions() {
    let (dir, mut engine) = fixture();
    short(&mut engine, 1000);
    let path = dir.path().join("paused-candidate.md");
    std::fs::write(&path, "# 基线\n").unwrap();
    engine
        .apply(Command::Import(vec![path.clone()]), 1000)
        .unwrap();
    engine.tick(1001, 1).unwrap();
    engine.apply(Command::SetPaused(true), 1001).unwrap();
    std::fs::write(&path, "# 暂停时修改\n不应归入学习\n").unwrap();
    engine
        .apply(Command::Scan { offline: false }, 1002)
        .unwrap();
    engine.apply(Command::SetPaused(false), 1003).unwrap();
    engine
        .apply(Command::Scan { offline: false }, 1005)
        .unwrap();
    engine.tick(1005, 2).unwrap();
    for second in 1..=297 {
        engine.tick(1005 + second, 1).unwrap();
    }
    let snapshot = engine.snapshot();
    assert_eq!(snapshot.versions.len(), 2);
    assert!(snapshot.sessions[0].suggested_summary.is_empty());
    assert!(snapshot.sessions[0].version_ids.is_empty());
}
#[test]
fn unobserved_paused_edit_is_baselined_before_resuming_activity() {
    let (dir, mut engine) = fixture();
    short(&mut engine, 1000);
    let path = dir.path().join("unscanned-paused.md");
    std::fs::write(&path, "# 基线\n").unwrap();
    engine
        .apply(Command::Import(vec![path.clone()]), 1000)
        .unwrap();
    engine.tick(1001, 1).unwrap();
    engine.apply(Command::SetPaused(true), 1001).unwrap();
    std::fs::write(&path, "# 未扫描的暂停修改\n历史保留\n").unwrap();
    engine.apply(Command::SetPaused(false), 1010).unwrap();
    engine
        .apply(Command::Scan { offline: false }, 1011)
        .unwrap();
    engine
        .apply(Command::Scan { offline: false }, 1014)
        .unwrap();
    engine.tick(1014, 4).unwrap();
    for second in 1..=295 {
        engine.tick(1014 + second, 1).unwrap();
    }
    let snapshot = engine.snapshot();
    assert_eq!(snapshot.versions.len(), 2);
    assert!(snapshot.sessions[0].suggested_summary.is_empty());
    assert!(snapshot.sessions[0].version_ids.is_empty());
}
#[test]
fn submitted_and_corrected_reviews_derive_learning_date_from_session_end() {
    let (_dir, mut engine) = fixture();
    short(&mut engine, 1000);
    cycle(&mut engine, 1000);
    let session_id = engine.snapshot().sessions[0].id.clone();
    engine
        .apply(
            Command::Submit {
                version_ids: None,
                session_id: session_id.clone(),
                summary: "定时提交".into(),
                note_ids: vec![],
            },
            1310,
        )
        .unwrap();
    assert_eq!(engine.snapshot().reviews[0].learned_at, 1300);
    assert_eq!(
        engine.snapshot().reviews[0].next_review_at,
        Some(1300 + 86400)
    );
    engine
        .apply(
            Command::EditSession {
                version_ids: None,
                session_id: session_id.clone(),
                summary: String::new(),
                learned: false,
                note_ids: vec![],
            },
            2000,
        )
        .unwrap();
    engine
        .apply(
            Command::EditSession {
                version_ids: None,
                session_id,
                summary: "一周后更正".into(),
                learned: true,
                note_ids: vec![],
            },
            1300 + 7 * 86400,
        )
        .unwrap();
    assert_eq!(engine.snapshot().reviews[0].learned_at, 1300);
    assert_eq!(
        engine.snapshot().reviews[0].next_review_at,
        Some(1300 + 86400)
    );
}
fn alter_backup(path: &std::path::Path, mutate: impl FnOnce(&mut serde_json::Value)) {
    let connection = rusqlite::Connection::open(path).unwrap();
    let raw: String = connection
        .query_row("SELECT data FROM app_state WHERE id=1", [], |r| r.get(0))
        .unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    mutate(&mut value);
    connection
        .execute("UPDATE app_state SET data=?1", [value.to_string()])
        .unwrap();
}
fn two_completed_reviews(engine: &mut Engine) -> Vec<Review> {
    short(engine, 1000);
    for start in [800, 900] {
        engine
            .apply(
                Command::Manual {
                    version_ids: None,
                    started_at: start,
                    minutes: 1,
                    summary: format!("复习 {start}"),
                    note_ids: vec![],
                },
                1000,
            )
            .unwrap();
    }
    let reviews = engine.snapshot().reviews;
    for review in &reviews {
        engine
            .apply(
                Command::CompleteReview {
                    review_id: review.id.clone(),
                    expected_stage: 0,
                },
                100000,
            )
            .unwrap();
    }
    reviews
}
#[test]
fn backup_rejects_invalid_full_undo_models_and_preserves_live_state() {
    let (dir, mut engine) = fixture();
    two_completed_reviews(&mut engine);
    let cases: Vec<(&str, serde_json::Value)> = vec![
        ("stage", serde_json::json!(999)),
        ("intervals", serde_json::json!([])),
        ("next_review_at", serde_json::json!(null)),
        ("last_review_at", serde_json::json!(55)),
        ("learned_at", serde_json::json!(1)),
        ("session_id", serde_json::json!("foreign")),
        ("revision", serde_json::json!(0)),
        ("note_ids", serde_json::json!(["missing-note"])),
        ("title", serde_json::json!("")),
    ];
    for (index, (field, invalid)) in cases.into_iter().enumerate() {
        let backup = dir.path().join(format!("bad-undo-{index}.sqlite"));
        engine
            .apply(Command::Backup(backup.clone()), 100001)
            .unwrap();
        alter_backup(&backup, |state| state["undo"][0]["review"][field] = invalid);
        assert!(
            engine.apply(Command::Restore(backup), 100002).is_err(),
            "accepted corrupt undo field {field}"
        );
        assert_eq!(engine.snapshot().reviews[0].stage, 1);
        assert_eq!(engine.snapshot().review_events.len(), 2);
    }
}
#[test]
fn backup_rejects_swapped_or_duplicated_undo_event_associations() {
    let (dir, mut engine) = fixture();
    two_completed_reviews(&mut engine);
    for duplicate in [false, true] {
        let backup = dir
            .path()
            .join(format!("undo-association-{duplicate}.sqlite"));
        engine
            .apply(Command::Backup(backup.clone()), 100001)
            .unwrap();
        alter_backup(&backup, |state| {
            let first = state["undo"][0]["event_id"].clone();
            state["undo"][0]["event_id"] = state["undo"][1]["event_id"].clone();
            if !duplicate {
                state["undo"][1]["event_id"] = first;
            }
        });
        assert!(engine.apply(Command::Restore(backup), 100002).is_err());
        assert_eq!(engine.snapshot().review_events.len(), 2);
    }
}
#[test]
fn independent_review_undo_preserves_each_plan_order_after_restart() {
    let (dir, mut engine) = fixture();
    let reviews = two_completed_reviews(&mut engine);
    for review in &reviews {
        engine
            .apply(
                Command::CompleteReview {
                    review_id: review.id.clone(),
                    expected_stage: 1,
                },
                400000,
            )
            .unwrap();
    }
    drop(engine);
    let mut engine = Engine::open(&dir.path().join("learning.sqlite"), 400001).unwrap();
    for review in &reviews {
        engine
            .apply(
                Command::UndoReview {
                    review_id: review.id.clone(),
                },
                400002,
            )
            .unwrap();
        let snapshot = engine.snapshot();
        let updated = snapshot.reviews.iter().find(|r| r.id == review.id).unwrap();
        assert_eq!(updated.stage, 1);
        assert_eq!(updated.next_review_at, Some(359200));
    }
    for review in reviews.iter().rev() {
        engine
            .apply(
                Command::UndoReview {
                    review_id: review.id.clone(),
                },
                400003,
            )
            .unwrap();
        let snapshot = engine.snapshot();
        let updated = snapshot.reviews.iter().find(|r| r.id == review.id).unwrap();
        assert_eq!(updated.stage, 0);
        assert_eq!(updated.next_review_at, review.next_review_at);
    }
    assert!(engine.snapshot().review_events.is_empty());
    drop(engine);
    assert!(Engine::open(&dir.path().join("learning.sqlite"), 400004).is_ok());
}
#[test]
fn backup_rejects_reordered_undo_history_within_one_review() {
    let (dir, mut engine) = fixture();
    let reviews = two_completed_reviews(&mut engine);
    engine
        .apply(
            Command::CompleteReview {
                review_id: reviews[0].id.clone(),
                expected_stage: 1,
            },
            400000,
        )
        .unwrap();
    let backup = dir.path().join("reordered-undo.sqlite");
    engine
        .apply(Command::Backup(backup.clone()), 400001)
        .unwrap();
    alter_backup(&backup, |state| {
        state["undo"].as_array_mut().unwrap().swap(0, 2)
    });
    assert!(engine.apply(Command::Restore(backup), 400002).is_err());
}
#[test]
fn unreadable_note_at_resume_is_baselined_when_reading_recovers() {
    let (dir, mut engine) = fixture();
    short(&mut engine, 1000);
    let path = dir.path().join("unreadable-resume.md");
    std::fs::write(&path, "# 基线\n").unwrap();
    engine
        .apply(Command::Import(vec![path.clone()]), 1000)
        .unwrap();
    engine.tick(1001, 1).unwrap();
    engine.apply(Command::SetPaused(true), 1001).unwrap();
    std::fs::write(&path, [0xff]).unwrap();
    engine.apply(Command::SetPaused(false), 1010).unwrap();
    std::fs::write(&path, "# 恢复读取\n暂停修改\n").unwrap();
    engine
        .apply(Command::Scan { offline: false }, 1011)
        .unwrap();
    engine
        .apply(Command::Scan { offline: false }, 1014)
        .unwrap();
    engine.tick(1014, 4).unwrap();
    std::fs::write(&path, "# 恢复读取\n暂停修改\n活动新增\n").unwrap();
    engine
        .apply(Command::Scan { offline: false }, 1015)
        .unwrap();
    engine
        .apply(Command::Scan { offline: false }, 1018)
        .unwrap();
    engine.tick(1018, 4).unwrap();
    for second in 1..=291 {
        engine.tick(1018 + second, 1).unwrap();
    }
    let snapshot = engine.snapshot();
    assert_eq!(snapshot.sessions[0].version_ids.len(), 1);
    assert!(snapshot.sessions[0].suggested_summary.contains("活动新增"));
    assert!(!snapshot.sessions[0].suggested_summary.contains("暂停修改"));
}
fn deferred_offline_reconciliation_case(restore: bool, missing: bool) {
    let (dir, mut engine) = fixture();
    short(&mut engine, 1000);
    let source = dir.path().join("offline-recovery.md");
    std::fs::write(&source, "# 主题\n基线\n").unwrap();
    engine
        .apply(Command::Import(vec![source.clone()]), 1000)
        .unwrap();
    std::fs::write(&source, "# 主题\n基线\n已运行学习\n").unwrap();
    engine
        .apply(Command::Scan { offline: false }, 1001)
        .unwrap();
    engine
        .apply(Command::Scan { offline: false }, 1004)
        .unwrap();
    engine.tick(1004, 4).unwrap();
    let legitimate_version = engine.snapshot().versions[1].id.clone();
    let backup = dir.path().join("before-unreadable.sqlite");
    if restore {
        engine.apply(Command::Backup(backup.clone()), 1004).unwrap();
    }
    if missing {
        std::fs::rename(&source, dir.path().join("retained-original.md")).unwrap();
    } else {
        std::fs::write(&source, [0xff]).unwrap();
    }
    if restore {
        engine.apply(Command::Restore(backup), 2000).unwrap();
    } else {
        drop(engine);
        engine = Engine::open(&dir.path().join("learning.sqlite"), 2000).unwrap();
    }
    assert!(engine.snapshot().notes[0].error.is_some());
    assert_eq!(engine.snapshot().notes[0].missing, missing);
    assert_eq!(engine.snapshot().timer.active_seconds, 4);
    engine.checkpoint().unwrap();
    let connection = rusqlite::Connection::open(dir.path().join("learning.sqlite")).unwrap();
    let saved: String = connection
        .query_row("SELECT data FROM app_state WHERE id=1", [], |row| {
            row.get(0)
        })
        .unwrap();
    let saved: serde_json::Value = serde_json::from_str(&saved).unwrap();
    assert!(
        saved["offline_unreconciled_notes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|id| id.as_str() == Some(engine.snapshot().notes[0].id.as_str()))
    );
    drop(connection);
    // Failed reconciliation must also survive a second shutdown/restart while
    // the source remains unavailable.
    drop(engine);
    let mut engine = Engine::open(&dir.path().join("learning.sqlite"), 2000).unwrap();
    assert!(engine.snapshot().notes[0].error.is_some());
    std::fs::write(&source, "# 主题\n基线\n已运行学习\n离线知识\n").unwrap();
    engine
        .apply(Command::Scan { offline: false }, 2001)
        .unwrap();
    engine
        .apply(Command::Scan { offline: false }, 2004)
        .unwrap();
    let recovered = engine.snapshot();
    assert_eq!(recovered.versions.len(), 3);
    assert!(
        recovered.versions[2].offline,
        "recovered version lost offline provenance: restore={restore}, missing={missing}"
    );
    let offline_version = recovered.versions[2].id.clone();
    std::fs::write(&source, "# 主题\n基线\n已运行学习\n离线知识\n当前学习\n").unwrap();
    engine
        .apply(Command::Scan { offline: false }, 2005)
        .unwrap();
    engine
        .apply(Command::Scan { offline: false }, 2008)
        .unwrap();
    let current_version = engine.snapshot().versions[3].id.clone();
    assert!(!engine.snapshot().versions[3].offline);
    engine.tick(2008, 8).unwrap();
    for second in 1..=288 {
        engine.tick(2008 + second, 1).unwrap();
    }
    let session = engine.snapshot().sessions[0].clone();
    assert_eq!(session.active_seconds, 300);
    assert_eq!(session.version_ids.len(), 2);
    assert!(session.version_ids.contains(&legitimate_version));
    assert!(session.version_ids.contains(&current_version));
    assert!(!session.version_ids.contains(&offline_version));
    assert!(session.suggested_summary.contains("已运行学习"));
    assert!(session.suggested_summary.contains("当前学习"));
    assert!(!session.suggested_summary.contains("离线知识"));
}
#[test]
fn startup_defers_offline_classification_until_missing_or_unreadable_sources_recover() {
    for missing in [false, true] {
        deferred_offline_reconciliation_case(false, missing);
    }
}
#[test]
fn restore_defers_offline_classification_until_missing_or_unreadable_sources_recover() {
    for missing in [false, true] {
        deferred_offline_reconciliation_case(true, missing);
    }
}
fn import_two_notes(dir: &TestDir, engine: &mut Engine) -> (String, String) {
    let a = dir.path().join("version-a.md");
    let b = dir.path().join("version-b.md");
    std::fs::write(&a, "# A\n基线 A\n").unwrap();
    std::fs::write(&b, "# B\n基线 B\n").unwrap();
    engine.apply(Command::Import(vec![a, b]), 1000).unwrap();
    let snapshot = engine.snapshot();
    (snapshot.notes[0].id.clone(), snapshot.notes[1].id.clone())
}
#[test]
fn submitting_unchanged_reading_anchors_the_selected_note_snapshot() {
    let (dir, mut engine) = fixture();
    short(&mut engine, 1000);
    let (a, _) = import_two_notes(&dir, &mut engine);
    let baseline = engine.snapshot().versions[0].id.clone();
    cycle(&mut engine, 1000);
    let session_id = engine.snapshot().sessions[0].id.clone();
    engine
        .apply(
            Command::Submit {
                session_id,
                summary: "阅读未修改的 A".into(),
                note_ids: vec![a],
                version_ids: None,
            },
            1301,
        )
        .unwrap();
    assert_eq!(engine.snapshot().sessions[0].version_ids, vec![baseline]);
}
#[test]
fn replacing_detected_note_filters_its_versions_and_anchors_the_new_selection() {
    let (dir, mut engine) = fixture();
    short(&mut engine, 1000);
    let (a, b) = import_two_notes(&dir, &mut engine);
    let baseline_b = engine.snapshot().versions[1].id.clone();
    std::fs::write(dir.path().join("version-a.md"), "# A\n活动修改 A\n").unwrap();
    engine
        .apply(Command::Scan { offline: false }, 1001)
        .unwrap();
    engine
        .apply(Command::Scan { offline: false }, 1004)
        .unwrap();
    let changed_a = engine.snapshot().versions[2].id.clone();
    cycle(&mut engine, 1000);
    let pending = engine.snapshot().sessions[0].clone();
    assert_eq!(pending.version_ids, vec![changed_a.clone()]);
    engine
        .apply(
            Command::Submit {
                session_id: pending.id.clone(),
                summary: "实际阅读 B".into(),
                note_ids: vec![b],
                version_ids: None,
            },
            1301,
        )
        .unwrap();
    assert_eq!(engine.snapshot().sessions[0].version_ids, vec![baseline_b]);
    engine
        .apply(
            Command::EditSession {
                session_id: pending.id,
                summary: "改为 A".into(),
                learned: true,
                note_ids: vec![a],
                version_ids: None,
            },
            1400,
        )
        .unwrap();
    assert_eq!(engine.snapshot().sessions[0].version_ids, vec![changed_a]);
}
#[test]
fn manual_snapshot_anchor_survives_later_source_and_summary_edits() {
    let (dir, mut engine) = fixture();
    let (a, _) = import_two_notes(&dir, &mut engine);
    let baseline = engine.snapshot().versions[0].id.clone();
    engine
        .apply(
            Command::Manual {
                started_at: 1000,
                minutes: 1,
                summary: "最初阅读".into(),
                note_ids: vec![a.clone()],
                version_ids: None,
            },
            1061,
        )
        .unwrap();
    let session = engine.snapshot().sessions[0].id.clone();
    std::fs::write(dir.path().join("version-a.md"), "# A\n后来的内容\n").unwrap();
    engine
        .apply(Command::Scan { offline: false }, 1100)
        .unwrap();
    engine
        .apply(Command::Scan { offline: false }, 1103)
        .unwrap();
    engine
        .apply(
            Command::EditSession {
                session_id: session,
                summary: "只更正摘要".into(),
                learned: true,
                note_ids: vec![a],
                version_ids: None,
            },
            1200,
        )
        .unwrap();
    assert_eq!(
        engine.snapshot().sessions[0].version_ids,
        vec![baseline.clone()]
    );
    drop(engine);
    let engine = Engine::open(&dir.path().join("learning.sqlite"), 2000).unwrap();
    assert_eq!(engine.snapshot().sessions[0].version_ids, vec![baseline]);
}
#[test]
fn default_version_anchor_uses_learning_time_and_explicit_note_fallback() {
    let (dir, mut engine) = fixture();
    let (a, _) = import_two_notes(&dir, &mut engine);
    let baseline = engine.snapshot().versions[0].id.clone();
    std::fs::write(dir.path().join("version-a.md"), "# A\n新内容\n").unwrap();
    engine
        .apply(Command::Scan { offline: false }, 1100)
        .unwrap();
    engine
        .apply(Command::Scan { offline: false }, 1103)
        .unwrap();
    let latest = engine.snapshot().versions[2].id.clone();
    engine
        .apply(
            Command::Manual {
                started_at: 990,
                minutes: 1,
                summary: "导入后阅读".into(),
                note_ids: vec![a.clone()],
                version_ids: None,
            },
            1200,
        )
        .unwrap();
    engine
        .apply(
            Command::Manual {
                started_at: 800,
                minutes: 1,
                summary: "补记导入前的阅读".into(),
                note_ids: vec![a],
                version_ids: None,
            },
            1200,
        )
        .unwrap();
    assert_eq!(engine.snapshot().sessions[0].version_ids, vec![baseline]);
    assert_eq!(engine.snapshot().sessions[1].version_ids, vec![latest]);
}
#[test]
fn explicit_version_range_is_persisted_deduplicated_and_validated_atomically() {
    let (dir, mut engine) = fixture();
    let (a, b) = import_two_notes(&dir, &mut engine);
    let first = engine.snapshot().versions[0].id.clone();
    let foreign = engine.snapshot().versions[1].id.clone();
    std::fs::write(dir.path().join("version-a.md"), "# A\n第二版本\n").unwrap();
    engine
        .apply(Command::Scan { offline: false }, 1010)
        .unwrap();
    engine
        .apply(Command::Scan { offline: false }, 1013)
        .unwrap();
    let second = engine.snapshot().versions[2].id.clone();
    engine
        .apply(
            Command::Manual {
                started_at: 1000,
                minutes: 1,
                summary: "选择两个版本".into(),
                note_ids: vec![a.clone()],
                version_ids: Some(vec![first.clone(), second.clone(), second.clone()]),
            },
            1100,
        )
        .unwrap();
    let id = engine.snapshot().sessions[0].id.clone();
    assert_eq!(
        engine.snapshot().sessions[0].version_ids,
        vec![first.clone(), second.clone()]
    );
    drop(engine);
    let mut engine = Engine::open(&dir.path().join("learning.sqlite"), 1200).unwrap();
    assert_eq!(
        engine.snapshot().sessions[0].version_ids,
        vec![first.clone(), second.clone()]
    );
    for (notes, versions) in [
        (vec![a.clone()], vec![foreign]),
        (vec![a.clone()], vec!["missing-version".into()]),
        (vec![a.clone()], vec![]),
        (vec![a.clone(), b], vec![first]),
    ] {
        assert!(
            engine
                .apply(
                    Command::EditSession {
                        session_id: id.clone(),
                        summary: "无效选择".into(),
                        learned: true,
                        note_ids: notes,
                        version_ids: Some(versions)
                    },
                    1201
                )
                .is_err()
        );
        assert_eq!(engine.snapshot().sessions[0].summary, "选择两个版本");
        assert_eq!(engine.snapshot().sessions[0].version_ids.len(), 2);
    }
    engine
        .apply(
            Command::EditSession {
                session_id: id,
                summary: "只选第二版本".into(),
                learned: true,
                note_ids: vec![a],
                version_ids: Some(vec![second.clone()]),
            },
            1202,
        )
        .unwrap();
    assert_eq!(engine.snapshot().sessions[0].version_ids, vec![second]);
}
#[test]
fn backup_rejects_versions_from_unassociated_notes() {
    let (dir, mut engine) = fixture();
    let (a, _) = import_two_notes(&dir, &mut engine);
    engine
        .apply(
            Command::Manual {
                started_at: 1000,
                minutes: 1,
                summary: "A 的学习".into(),
                note_ids: vec![a],
                version_ids: None,
            },
            1100,
        )
        .unwrap();
    let foreign = engine.snapshot().versions[1].id.clone();
    let backup = dir.path().join("foreign-version.sqlite");
    engine.apply(Command::Backup(backup.clone()), 1100).unwrap();
    alter_backup(&backup, |state| {
        state["sessions"][0]["version_ids"] = serde_json::json!([foreign])
    });
    assert!(engine.apply(Command::Restore(backup), 1200).is_err());
    assert_eq!(engine.snapshot().sessions[0].summary, "A 的学习");
}
