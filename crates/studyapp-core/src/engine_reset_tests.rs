use super::*;

fn fixture() -> (PathBuf, Engine) {
    let directory = tempfile::tempdir().unwrap().keep();
    let database = directory.join("library.sqlite3");
    let mut engine = Engine::open(&database, 1_000).unwrap();
    let mut settings = engine.snapshot().settings;
    settings.interval_minutes = 25;
    settings.show_file_paths = true;
    engine
        .apply(Command::SaveSettings(settings), 1_000)
        .unwrap();
    (database, engine)
}

#[test]
fn reset_existing_preferences_preserves_records_without_scanning_sources() {
    let (database, mut engine) = fixture();
    let source = database.with_file_name("original.md");
    std::fs::write(&source, "# Original\ncontent").unwrap();
    engine
        .apply(Command::Import(vec![source.clone()]), 1_001)
        .unwrap();
    let note_id = engine.snapshot().notes[0].id.clone();
    engine
        .apply(
            Command::Manual {
                started_at: 100,
                minutes: 10,
                summary: "Retain learning and review".into(),
                note_ids: vec![note_id],
                version_ids: None,
            },
            1_002,
        )
        .unwrap();
    let mut expected = storage::load(&engine.conn).unwrap().unwrap();
    Engine::save_settings(&mut expected, Settings::default()).unwrap();
    let expected_versions = serde_json::to_value(&expected.versions).unwrap();
    let expected_state = serde_json::to_value(&expected).unwrap();
    // Normal Engine::open would scan this change. Maintenance must not do so.
    std::fs::write(&source, "# External change\nkeep exact bytes").unwrap();
    drop(engine);
    Engine::reset_existing_preferences_for_uninstall(&database).unwrap();
    let conn = Connection::open(&database).unwrap();
    let actual = storage::load(&conn).unwrap().unwrap();
    assert_eq!(actual.settings, Settings::default());
    assert_eq!(serde_json::to_value(&actual).unwrap(), expected_state);
    assert_eq!(
        serde_json::to_value(&actual.versions).unwrap(),
        expected_versions
    );
    assert_eq!(
        std::fs::read_to_string(source).unwrap(),
        "# External change\nkeep exact bytes"
    );
}

#[test]
fn reset_existing_preferences_refuses_pending_write_without_recovering_it() {
    let (database, engine) = fixture();
    engine
        .conn
        .execute("INSERT INTO write_intents(id,data) VALUES(1,'{}')", [])
        .unwrap();
    let before: String = engine
        .conn
        .query_row("SELECT data FROM app_state", [], |r| r.get(0))
        .unwrap();
    drop(engine);
    assert!(Engine::reset_existing_preferences_for_uninstall(&database).is_err());
    let conn = Connection::open(&database).unwrap();
    let after: String = conn
        .query_row("SELECT data FROM app_state", [], |r| r.get(0))
        .unwrap();
    let intents: i64 = conn
        .query_row("SELECT count(*) FROM write_intents", [], |r| r.get(0))
        .unwrap();
    assert_eq!(before, after);
    assert_eq!(intents, 1);
}

#[test]
fn reset_existing_preferences_never_creates_a_missing_database() {
    let directory = tempfile::tempdir().unwrap().keep();
    let database = directory.join("missing.sqlite3");
    assert!(Engine::reset_existing_preferences_for_uninstall(&database).is_err());
    assert!(!database.exists());
}

#[test]
fn reset_existing_preferences_rejects_foreign_or_future_schema() {
    for pragma in ["PRAGMA application_id=1", "PRAGMA user_version=999"] {
        let (database, engine) = fixture();
        engine.conn.execute_batch(pragma).unwrap();
        let before: String = engine
            .conn
            .query_row("SELECT data FROM app_state", [], |r| r.get(0))
            .unwrap();
        drop(engine);
        assert!(Engine::reset_existing_preferences_for_uninstall(&database).is_err());
        let conn = Connection::open(&database).unwrap();
        let after: String = conn
            .query_row("SELECT data FROM app_state", [], |r| r.get(0))
            .unwrap();
        assert_eq!(before, after);
    }
}
