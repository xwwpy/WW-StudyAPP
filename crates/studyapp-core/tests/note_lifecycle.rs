use std::path::{Path, PathBuf};
use studyapp_core::{Command, Engine};

fn fixture() -> (PathBuf, Engine) {
    // Retain fixtures: recursive file deletion is forbidden in this project.
    let root = tempfile::tempdir().unwrap().keep();
    let engine = Engine::open(&root.join("study.sqlite"), 1000).unwrap();
    (root, engine)
}

fn mark_detached_in_database(database: &Path) {
    let conn = rusqlite::Connection::open(database).unwrap();
    let raw: String = conn
        .query_row("SELECT data FROM app_state", [], |row| row.get(0))
        .unwrap();
    let mut state: serde_json::Value = serde_json::from_str(&raw).unwrap();
    state["notes"][0]["detached"] = true.into();
    conn.execute("UPDATE app_state SET data=?1", [state.to_string()])
        .unwrap();
}

#[test]
fn detached_source_is_not_scanned_on_restart_or_explicit_scan() {
    let (root, mut engine) = fixture();
    let source = root.join("source.md");
    std::fs::write(&source, "original").unwrap();
    engine
        .apply(Command::Import(vec![source.clone()]), 1001)
        .unwrap();
    drop(engine);
    mark_detached_in_database(&root.join("study.sqlite"));
    std::fs::write(&source, "changed while detached").unwrap();
    let mut engine = Engine::open(&root.join("study.sqlite"), 2000).unwrap();
    engine
        .apply(Command::Scan { offline: false }, 2001)
        .unwrap();
    engine
        .apply(Command::Scan { offline: false }, 2005)
        .unwrap();
    assert_eq!(engine.snapshot().versions.len(), 1);
    assert_eq!(
        std::fs::read_to_string(source).unwrap(),
        "changed while detached"
    );
}

#[test]
fn detached_managed_backup_does_not_overwrite_retained_markdown() {
    let (root, mut engine) = fixture();
    engine
        .apply(
            Command::CreateNote {
                title: "retained".into(),
                content: "original".into(),
            },
            1001,
        )
        .unwrap();
    let source = PathBuf::from(&engine.snapshot().notes[0].path);
    drop(engine);
    mark_detached_in_database(&root.join("study.sqlite"));
    let mut engine = Engine::open(&root.join("study.sqlite"), 1002).unwrap();
    let backup = root.join("backup.sqlite");
    engine.apply(Command::Backup(backup.clone()), 1003).unwrap();
    std::fs::write(&source, "retained external edit").unwrap();
    engine.apply(Command::Restore(backup), 1004).unwrap();
    assert_eq!(
        std::fs::read_to_string(source).unwrap(),
        "retained external edit"
    );
    assert_eq!(engine.snapshot().versions.len(), 1);
}

#[test]
fn schema_two_database_is_upgraded_to_protect_detached_state_from_old_clients() {
    let (root, engine) = fixture();
    drop(engine);
    let database = root.join("study.sqlite");
    {
        let conn = rusqlite::Connection::open(&database).unwrap();
        conn.pragma_update(None, "user_version", 2).unwrap();
    }
    let _engine = Engine::open(&database, 1001).unwrap();
    let conn = rusqlite::Connection::open(&database).unwrap();
    let schema: i32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(schema, 3);
}

#[test]
fn schema_two_backup_without_detached_field_restores_active_notes() {
    let (root, mut engine) = fixture();
    let source = root.join("legacy.md");
    std::fs::write(&source, "legacy content").unwrap();
    engine.apply(Command::Import(vec![source]), 1001).unwrap();
    let backup = root.join("schema-two.sqlite");
    engine.apply(Command::Backup(backup.clone()), 1002).unwrap();
    {
        let conn = rusqlite::Connection::open(&backup).unwrap();
        let raw: String = conn
            .query_row("SELECT data FROM app_state", [], |row| row.get(0))
            .unwrap();
        let mut state: serde_json::Value = serde_json::from_str(&raw).unwrap();
        state["notes"][0]
            .as_object_mut()
            .unwrap()
            .remove("detached");
        conn.execute("UPDATE app_state SET data=?1", [state.to_string()])
            .unwrap();
        conn.pragma_update(None, "user_version", 2).unwrap();
    }
    let id = engine.snapshot().notes[0].id.clone();
    engine
        .apply(
            Command::UnmanageNote {
                note_id: id.clone(),
            },
            1003,
        )
        .unwrap();
    engine.apply(Command::Restore(backup), 1004).unwrap();
    assert_eq!(engine.snapshot().notes[0].id, id);
    assert!(!engine.snapshot().notes[0].detached);
}

#[test]
fn failed_explicit_reimport_keeps_detached_note_and_history() {
    let (root, mut engine) = fixture();
    let source = root.join("retained.md");
    std::fs::write(&source, "original").unwrap();
    engine
        .apply(Command::Import(vec![source.clone()]), 1001)
        .unwrap();
    let note = engine.snapshot().notes[0].clone();
    engine
        .apply(
            Command::UnmanageNote {
                note_id: note.id.clone(),
            },
            1002,
        )
        .unwrap();
    std::fs::write(&source, [0xff, 0xfe]).unwrap();
    let message = engine.apply(Command::Import(vec![source]), 1003).unwrap();
    assert!(message.contains("失败 1 份"));
    assert_eq!(engine.snapshot().notes.len(), 1);
    assert!(engine.snapshot().notes[0].detached);
    assert_eq!(engine.snapshot().versions.len(), 1);
}

#[test]
fn unmanage_preserves_learning_reviews_versions_and_rejects_stale_writes() {
    let (root, mut engine) = fixture();
    engine
        .apply(
            Command::CreateNote {
                title: "history".into(),
                content: "retained content".into(),
            },
            1001,
        )
        .unwrap();
    let note = engine.snapshot().notes[0].clone();
    let hash = engine.snapshot().versions[0].hash.clone();
    engine
        .apply(
            Command::Manual {
                started_at: 1000,
                minutes: 1,
                summary: "retained learning".into(),
                note_ids: vec![note.id.clone()],
                version_ids: None,
            },
            2000,
        )
        .unwrap();
    let review = engine.snapshot().reviews[0].clone();
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
    let before = engine.snapshot();
    engine
        .apply(
            Command::UnmanageNote {
                note_id: note.id.clone(),
            },
            due + 1,
        )
        .unwrap();
    let after = engine.snapshot();
    assert!(after.notes[0].detached);
    assert!(after.notes[0].managed);
    assert_eq!(after.notes[0].created_at, note.created_at);
    for (a, b) in [
        (
            serde_json::to_value(&before.sessions).unwrap(),
            serde_json::to_value(&after.sessions).unwrap(),
        ),
        (
            serde_json::to_value(&before.reviews).unwrap(),
            serde_json::to_value(&after.reviews).unwrap(),
        ),
        (
            serde_json::to_value(&before.review_events).unwrap(),
            serde_json::to_value(&after.review_events).unwrap(),
        ),
        (
            serde_json::to_value(&*before.versions).unwrap(),
            serde_json::to_value(&*after.versions).unwrap(),
        ),
    ] {
        assert_eq!(a, b);
    }
    for command in [
        Command::SaveNote {
            note_id: note.id.clone(),
            expected_hash: hash.clone(),
            content: "late editor".into(),
        },
        Command::ReceiveSyncedNote {
            note_id: note.id.clone(),
            expected_hash: Some(hash),
            content: "late sync".into(),
        },
    ] {
        assert!(engine.apply(command, due + 2).is_err());
    }
    assert_eq!(
        std::fs::read_to_string(&note.path).unwrap(),
        "retained content"
    );
    drop(engine);
    let mut engine = Engine::open(&root.join("study.sqlite"), due + 3).unwrap();
    assert!(engine.snapshot().notes[0].detached);
    engine
        .apply(
            Command::UndoReview {
                review_id: review.id,
            },
            due + 4,
        )
        .unwrap();
    assert_eq!(engine.snapshot().reviews[0].stage, 0);
}

#[test]
fn explicit_reimport_reactivates_same_identity_and_captures_offline_changes() {
    for managed in [false, true] {
        let (root, mut engine) = fixture();
        let source = if managed {
            engine
                .apply(
                    Command::CreateNote {
                        title: "managed".into(),
                        content: "original".into(),
                    },
                    1001,
                )
                .unwrap();
            PathBuf::from(&engine.snapshot().notes[0].path)
        } else {
            let source = root.join("external.md");
            std::fs::write(&source, "original").unwrap();
            engine
                .apply(Command::Import(vec![source.clone()]), 1001)
                .unwrap();
            source
        };
        let note = engine.snapshot().notes[0].clone();
        engine
            .apply(
                Command::UnmanageNote {
                    note_id: note.id.clone(),
                },
                1002,
            )
            .unwrap();
        engine
            .apply(Command::Import(vec![source.clone()]), 1003)
            .unwrap();
        assert_eq!(engine.snapshot().versions.len(), 1);
        assert!(!engine.snapshot().notes[0].detached);
        engine
            .apply(
                Command::UnmanageNote {
                    note_id: note.id.clone(),
                },
                1004,
            )
            .unwrap();
        std::fs::write(&source, "modified while detached").unwrap();
        engine
            .apply(Command::Import(vec![source.clone()]), 1005)
            .unwrap();
        let snapshot = engine.snapshot();
        assert_eq!(snapshot.notes.len(), 1);
        assert_eq!(snapshot.notes[0].id, note.id);
        assert_eq!(snapshot.notes[0].created_at, note.created_at);
        assert_eq!(snapshot.notes[0].managed, managed);
        assert!(!snapshot.notes[0].detached);
        assert_eq!(snapshot.versions.len(), 2);
        assert!(snapshot.versions[1].offline);
        assert_eq!(snapshot.versions[1].content, "modified while detached");
        engine.apply(Command::Import(vec![source]), 1006).unwrap();
        assert_eq!(engine.snapshot().versions.len(), 2);
    }
}

#[test]
fn unmanage_removes_current_interval_attribution_but_keeps_completed_sessions() {
    let (_, mut engine) = fixture();
    engine
        .apply(
            Command::SaveSettings(studyapp_core::Settings {
                interval_minutes: 5,
                ..Default::default()
            }),
            1000,
        )
        .unwrap();
    engine.tick(1001, 1).unwrap();
    engine
        .apply(
            Command::CreateNote {
                title: "active".into(),
                content: "current interval".into(),
            },
            1001,
        )
        .unwrap();
    let id = engine.snapshot().notes[0].id.clone();
    engine
        .apply(Command::UnmanageNote { note_id: id }, 1002)
        .unwrap();
    engine.apply(Command::SetPaused(true), 1002).unwrap();
    engine.apply(Command::SetPaused(false), 1002).unwrap();
    for now in 1003..1302 {
        engine.tick(now, 1).unwrap();
    }
    let snapshot = engine.snapshot();
    assert_eq!(snapshot.sessions.len(), 1);
    assert!(snapshot.sessions[0].note_ids.is_empty());
    assert!(snapshot.sessions[0].version_ids.is_empty());
    assert_eq!(snapshot.versions.len(), 1);
}
