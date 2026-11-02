use rusqlite::Connection;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
use studyapp_core::{Command, Engine, purge_managed_note_from_backup};

const SECRET: &str = "purge-secret-671321d4-unique-content";
fn fixture() -> (PathBuf, Engine) {
    let dir = tempfile::tempdir().unwrap().keep();
    let engine = Engine::open(&dir.join("learning.sqlite"), 1000).unwrap();
    (dir, engine)
}
fn create(engine: &mut Engine, content: &str) -> String {
    engine
        .apply(
            Command::CreateNote {
                title: "note".into(),
                content: content.into(),
            },
            1001,
        )
        .unwrap();
    engine.snapshot().notes.last().unwrap().id.clone()
}
fn files(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut found = BTreeMap::new();
    for item in std::fs::read_dir(dir).unwrap() {
        let path = item.unwrap().path();
        if path.is_dir() {
            found.extend(files(&path));
        } else if !path.to_string_lossy().ends_with(".sqlite")
            && !path.to_string_lossy().ends_with("-wal")
            && !path.to_string_lossy().ends_with("-shm")
        {
            found.insert(path.clone(), std::fs::read(path).unwrap());
        }
    }
    found
}
fn revision(engine: &Engine, id: &str) -> u64 {
    engine
        .snapshot()
        .notes
        .iter()
        .find(|n| n.id == id)
        .unwrap()
        .revision
}

#[test]
fn purge_removes_all_versions_durably_without_touching_owned_files() {
    let (dir, mut engine) = fixture();
    let id = create(&mut engine, SECRET);
    let hash = engine.snapshot().versions.last().unwrap().hash.clone();
    engine
        .apply(
            Command::SaveNote {
                note_id: id.clone(),
                expected_hash: hash,
                content: format!("{SECRET} edited"),
            },
            1010,
        )
        .unwrap();
    let other = create(&mut engine, "keep-other-content");
    engine
        .apply(
            Command::CreateFolder {
                name: "keep-folder".into(),
                parent_id: None,
            },
            1011,
        )
        .unwrap();
    let folder = engine.snapshot().folders[0].id.clone();
    engine
        .apply(
            Command::MoveNote {
                note_id: id.clone(),
                folder_id: Some(folder),
            },
            1012,
        )
        .unwrap();
    let before = files(&dir);
    let settings = engine.snapshot().settings;
    let timer = serde_json::to_value(engine.snapshot().timer).unwrap();
    engine
        .purge_managed_note(&id, revision(&engine, &id), 1020)
        .unwrap();
    assert_eq!(files(&dir), before);
    assert_eq!(engine.snapshot().notes[0].id, other);
    assert!(
        engine
            .snapshot()
            .versions
            .iter()
            .all(|v| v.note_id == other)
    );
    assert_eq!(engine.snapshot().settings, settings);
    assert_eq!(
        serde_json::to_value(engine.snapshot().timer).unwrap(),
        timer
    );
    assert_eq!(engine.snapshot().folders.len(), 1);
    for path in [dir.join("learning.sqlite"), dir.join("learning.sqlite-wal")] {
        if path.exists() {
            assert!(
                !std::fs::read(path)
                    .unwrap()
                    .windows(SECRET.len())
                    .any(|w| w == SECRET.as_bytes())
            );
        }
    }
    drop(engine);
    let engine = Engine::open(&dir.join("learning.sqlite"), 1030).unwrap();
    assert_eq!(engine.snapshot().notes.len(), 1);
    assert!(
        engine
            .snapshot()
            .versions
            .iter()
            .all(|v| v.note_id == other)
    );
}

#[test]
fn purge_preserves_learning_and_review_undo_but_clears_associations_and_generated_summary() {
    let (dir, mut engine) = fixture();
    let id = create(&mut engine, SECRET);
    let other = create(&mut engine, "keep");
    engine
        .apply(
            Command::Manual {
                started_at: 1000,
                minutes: 1,
                summary: "authored learning".into(),
                note_ids: vec![id.clone(), other.clone()],
                version_ids: None,
            },
            1100,
        )
        .unwrap();
    let review = engine.snapshot().reviews[0].id.clone();
    engine
        .apply(
            Command::CompleteReview {
                review_id: review.clone(),
                expected_stage: 0,
            },
            90000,
        )
        .unwrap();
    engine.checkpoint().unwrap();
    drop(engine);
    let conn = Connection::open(dir.join("learning.sqlite")).unwrap();
    let raw: String = conn
        .query_row("SELECT data FROM app_state", [], |r| r.get(0))
        .unwrap();
    let mut state: serde_json::Value = serde_json::from_str(&raw).unwrap();
    state["sessions"][0]["suggested_summary"] = SECRET.into();
    conn.execute("UPDATE app_state SET data=?1", [state.to_string()])
        .unwrap();
    drop(conn);
    let mut engine = Engine::open(&dir.join("learning.sqlite"), 90001).unwrap();
    engine
        .purge_managed_note(&id, revision(&engine, &id), 90002)
        .unwrap();
    let snapshot = engine.snapshot();
    assert_eq!(snapshot.sessions[0].summary, "authored learning");
    assert!(snapshot.sessions[0].suggested_summary.is_empty());
    assert_eq!(snapshot.sessions[0].note_ids, vec![other.clone()]);
    assert_eq!(snapshot.reviews[0].note_ids, vec![other.clone()]);
    engine
        .apply(Command::UndoReview { review_id: review }, 90003)
        .unwrap();
    assert_eq!(engine.snapshot().reviews[0].note_ids, vec![other]);
    drop(engine);
    Engine::open(&dir.join("learning.sqlite"), 90004).unwrap();
}

#[test]
fn rejects_stale_external_missing_and_pending_writes_without_mutation() {
    let (dir, mut engine) = fixture();
    let id = create(&mut engine, SECRET);
    let source = dir.join("external.md");
    std::fs::write(&source, "external").unwrap();
    engine.apply(Command::Import(vec![source]), 1002).unwrap();
    let external = engine
        .snapshot()
        .notes
        .iter()
        .find(|n| !n.managed)
        .unwrap()
        .clone();
    let before = serde_json::to_value(engine.snapshot()).unwrap();
    for (target, rev) in [
        (id.as_str(), 0),
        (external.id.as_str(), external.revision),
        ("8529f643-e153-4a0b-983b-c32bb6d28da7", 1),
    ] {
        assert!(engine.purge_managed_note(target, rev, 1003).is_err());
        assert_eq!(serde_json::to_value(engine.snapshot()).unwrap(), before);
    }
    let conn = Connection::open(dir.join("learning.sqlite")).unwrap();
    conn.execute("INSERT INTO write_intents VALUES(1,'{}')", [])
        .unwrap();
    assert!(
        engine
            .purge_managed_note(&id, revision(&engine, &id), 1004)
            .is_err()
    );
    assert_eq!(serde_json::to_value(engine.snapshot()).unwrap(), before);
    let count: i64 = conn
        .query_row("SELECT count(*) FROM write_intents", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn detached_managed_note_can_be_purged() {
    let (_dir, mut engine) = fixture();
    let id = create(&mut engine, SECRET);
    engine
        .apply(
            Command::UnmanageNote {
                note_id: id.clone(),
            },
            1002,
        )
        .unwrap();
    assert!(engine.snapshot().notes[0].detached);
    engine
        .purge_managed_note(&id, revision(&engine, &id), 1003)
        .unwrap();
    assert!(engine.snapshot().notes.is_empty());
}

#[test]
fn backup_scrub_verifies_identity_and_does_not_create_recovery_files() {
    let (dir, mut engine) = fixture();
    let id = create(&mut engine, SECRET);
    let other = create(&mut engine, "keep");
    let backup = dir.join("backup.sqlite");
    engine.apply(Command::Backup(backup.clone()), 1002).unwrap();
    let before = files(&dir);
    assert!(studyapp_core::finish_purge_cleanup(&backup, &id).is_err());
    assert!(studyapp_core::scrub_confirmed_backup(&backup, &id, [0; 32]).is_err());
    assert!(purge_managed_note_from_backup(&backup, &id).unwrap());
    assert!(!purge_managed_note_from_backup(&backup, &id).unwrap());
    assert_eq!(files(&dir), before);
    assert!(
        !std::fs::read(&backup)
            .unwrap()
            .windows(SECRET.len())
            .any(|w| w == SECRET.as_bytes())
    );
    let restored = Engine::open(&backup, 1003).unwrap();
    assert_eq!(restored.snapshot().notes[0].id, other);
    drop(restored);
    let wrong = dir.join("wrong.sqlite");
    let conn = Connection::open(&wrong).unwrap();
    conn.execute_batch("PRAGMA application_id=1; CREATE TABLE unrelated(value TEXT); INSERT INTO unrelated VALUES('leave intact');").unwrap();
    drop(conn);
    let bytes = std::fs::read(&wrong).unwrap();
    assert!(purge_managed_note_from_backup(&wrong, &id).is_err());
    assert_eq!(std::fs::read(&wrong).unwrap(), bytes);
}

#[test]
fn rejects_concurrent_database_changes_and_deferred_settings() {
    let (dir, mut engine) = fixture();
    let id = create(&mut engine, SECRET);
    let rev = revision(&engine, &id);
    let mut concurrent = Engine::open(&dir.join("learning.sqlite"), 1002).unwrap();
    create(&mut concurrent, "concurrent content");
    assert!(engine.purge_managed_note(&id, rev, 1003).is_err());
    assert_eq!(concurrent.snapshot().notes.len(), 2);
    drop(engine);
    drop(concurrent);
    let mut engine = Engine::open(&dir.join("learning.sqlite"), 1004).unwrap();
    let mut settings = engine.snapshot().settings;
    settings.interval_minutes = 30;
    let prepared = engine.prepare_settings(settings).unwrap();
    engine.install_prepared_settings(prepared);
    let before = serde_json::to_value(engine.snapshot()).unwrap();
    assert!(
        engine
            .purge_managed_note(&id, revision(&engine, &id), 1005)
            .is_err()
    );
    assert_eq!(serde_json::to_value(engine.snapshot()).unwrap(), before);
}

#[test]
fn failed_sql_transaction_keeps_live_state_and_versions() {
    let (dir, mut engine) = fixture();
    let id = create(&mut engine, SECRET);
    let conn = Connection::open(dir.join("learning.sqlite")).unwrap();
    conn.execute_batch("CREATE TRIGGER reject_purge BEFORE UPDATE ON app_state BEGIN SELECT RAISE(ABORT,'fixture failure'); END;").unwrap();
    let before = serde_json::to_value(engine.snapshot()).unwrap();
    assert!(
        engine
            .purge_managed_note(&id, revision(&engine, &id), 1003)
            .is_err()
    );
    assert_eq!(serde_json::to_value(engine.snapshot()).unwrap(), before);
    let count: i64 = conn
        .query_row("SELECT count(*) FROM note_versions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn pinned_wal_reader_reports_cleanup_failure_after_committed_removal() {
    let (dir, mut engine) = fixture();
    let id = create(&mut engine, SECRET);
    let conn = Connection::open(dir.join("learning.sqlite")).unwrap();
    conn.execute_batch("BEGIN; SELECT data FROM note_versions;")
        .unwrap();
    let error = engine
        .purge_managed_note(&id, revision(&engine, &id), 1003)
        .unwrap_err();
    assert!(error.to_string().contains("残留清理未完成"));
    assert!(engine.snapshot().notes.is_empty());
    drop(conn);
    // The verified scrub helper can finish cleanup without reporting another removal.
    studyapp_core::finish_purge_cleanup(&dir.join("learning.sqlite"), &id).unwrap();
    assert!(
        !std::fs::read(dir.join("learning.sqlite"))
            .unwrap()
            .windows(SECRET.len())
            .any(|w| w == SECRET.as_bytes())
    );
}

#[test]
fn backup_scrub_refuses_unsupported_schema_pending_intent_or_invalid_state() {
    let (dir, mut engine) = fixture();
    let id = create(&mut engine, SECRET);
    for (name, sql) in [
        ("future.sqlite", "PRAGMA user_version=999"),
        ("pending.sqlite", "INSERT INTO write_intents VALUES(1,'{}')"),
        ("invalid.sqlite", "UPDATE app_state SET data='{}'"),
    ] {
        let path = dir.join(name);
        engine.apply(Command::Backup(path.clone()), 1002).unwrap();
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(sql).unwrap();
        drop(conn);
        let before = std::fs::read(&path).unwrap();
        assert!(purge_managed_note_from_backup(&path, &id).is_err());
        assert_eq!(std::fs::read(path).unwrap(), before);
    }
}

#[test]
fn purge_handles_persisted_unattributed_and_offline_sets() {
    let (dir, mut engine) = fixture();
    let id = create(&mut engine, SECRET);
    let other = create(&mut engine, "keep");
    engine.checkpoint().unwrap();
    drop(engine);
    let conn = Connection::open(dir.join("learning.sqlite")).unwrap();
    let raw: String = conn
        .query_row("SELECT data FROM app_state", [], |r| r.get(0))
        .unwrap();
    let mut state: serde_json::Value = serde_json::from_str(&raw).unwrap();
    state["unattributed_notes"] = serde_json::json!([id, other]);
    state["offline_unreconciled_notes"] = serde_json::json!([other, id]);
    conn.execute("UPDATE app_state SET data=?1", [state.to_string()])
        .unwrap();
    drop(conn);
    let mut engine = Engine::open(&dir.join("learning.sqlite"), 1004).unwrap();
    engine
        .purge_managed_note(&id, revision(&engine, &id), 1005)
        .unwrap();
    drop(engine);
    let conn = Connection::open(dir.join("learning.sqlite")).unwrap();
    let raw: String = conn
        .query_row("SELECT data FROM app_state", [], |r| r.get(0))
        .unwrap();
    let state: serde_json::Value = serde_json::from_str(&raw).unwrap();
    for field in ["unattributed_notes", "offline_unreconciled_notes"] {
        assert!(
            !state[field]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!(id))
        );
    }
}
