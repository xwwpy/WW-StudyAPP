use super::*;

fn fixture() -> (PathBuf, Engine, Note) {
    let root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.scratch/note-fixes-20261026/test-temp");
    std::fs::create_dir_all(&root).unwrap();
    let directory = tempfile::Builder::new()
        .prefix("rename-")
        .tempdir_in(root)
        .unwrap()
        .keep();
    let database = directory.join("library.sqlite3");
    let mut engine = Engine::open(&database, 1000).unwrap();
    engine
        .apply(
            Command::CreateNote {
                title: "Original".into(),
                content: "# Original\n![image](studyapp-media:old/image.png)\nbody".into(),
            },
            1001,
        )
        .unwrap();
    let note = engine.snapshot().notes[0].clone();
    (database, engine, note)
}

fn rename(engine: &mut Engine, note: &Note, title: &str, now: i64) -> Result<String> {
    engine.apply(
        Command::RenameNote {
            note_id: note.id.clone(),
            title: title.into(),
            expected_revision: note.revision,
        },
        now,
    )
}

#[test]
fn rename_persists_metadata_and_preserves_content_history_folder_and_associations() {
    let (database, mut engine, note) = fixture();
    engine
        .apply(
            Command::CreateFolder {
                name: "Folder".into(),
                parent_id: None,
            },
            1002,
        )
        .unwrap();
    let folder_id = engine.snapshot().folders[0].id.clone();
    engine
        .apply(
            Command::MoveNote {
                note_id: note.id.clone(),
                folder_id: Some(folder_id),
            },
            1003,
        )
        .unwrap();
    engine
        .apply(
            Command::Manual {
                started_at: 100,
                minutes: 10,
                summary: "Learning".into(),
                note_ids: vec![note.id.clone()],
                version_ids: None,
            },
            1004,
        )
        .unwrap();
    let image_group = database.parent().unwrap().join("images/old");
    std::fs::create_dir_all(&image_group).unwrap();
    let image = image_group.join("image.png");
    std::fs::write(&image, b"synthetic immutable image bytes").unwrap();
    let image_bytes = std::fs::read(&image).unwrap();
    let image_inventory = || {
        let mut entries: Vec<_> = std::fs::read_dir(&image_group)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        entries.sort();
        entries
    };
    let images_before = image_inventory();
    let before = engine.snapshot();
    let bytes = std::fs::read(&note.path).unwrap();
    rename(&mut engine, &before.notes[0], "  新名称 / : ?  ", 1005).unwrap();
    let mut expected = serde_json::to_value(&before).unwrap();
    expected["notes"][0]["title"] = serde_json::json!("新名称 / : ?");
    expected["notes"][0]["title_override"] = serde_json::json!(true);
    expected["notes"][0]["updated_at"] = serde_json::json!(1005);
    expected["notes"][0]["revision"] = serde_json::json!(before.notes[0].revision + 1);
    assert_eq!(serde_json::to_value(engine.snapshot()).unwrap(), expected);
    assert_eq!(std::fs::read(&note.path).unwrap(), bytes);
    assert_eq!(std::fs::read(&image).unwrap(), image_bytes);
    assert_eq!(image_inventory(), images_before);
    drop(engine);
    let reopened = Engine::open(&database, 1005).unwrap().snapshot();
    assert_eq!(reopened.notes[0].title, "新名称 / : ?");
    assert!(reopened.notes[0].title_override);
    assert_eq!(
        serde_json::to_value(reopened.versions).unwrap(),
        expected["versions"]
    );
}

#[test]
fn explicit_rename_survives_body_heading_edits_and_backup_restore() {
    let (database, mut engine, note) = fixture();
    rename(&mut engine, &note, "Pinned", 1002).unwrap();
    for (now, content) in [
        (1003, "# Original\nbody edit"),
        (1004, "# New heading\nbody edit"),
    ] {
        let snapshot = engine.snapshot();
        engine
            .apply(
                Command::SaveNote {
                    note_id: note.id.clone(),
                    expected_hash: snapshot.versions.last().unwrap().hash.clone(),
                    content: content.into(),
                },
                now,
            )
            .unwrap();
        assert_eq!(engine.snapshot().notes[0].title, "Pinned");
    }
    let backup = database.with_file_name("backup.sqlite3");
    engine.apply(Command::Backup(backup.clone()), 1005).unwrap();
    let latest = engine.snapshot().notes[0].clone();
    rename(&mut engine, &latest, "Temporary", 1006).unwrap();
    engine.apply(Command::Restore(backup), 1007).unwrap();
    drop(engine);
    let reopened = Engine::open(&database, 1008).unwrap().snapshot();
    assert_eq!(reopened.notes[0].title, "Pinned");
    assert!(reopened.notes[0].title_override);
    assert_eq!(reopened.versions.len(), 3);
}

#[test]
fn rename_validation_and_cas_leave_all_state_unchanged_on_rejection_or_noop() {
    let (_, mut engine, note) = fixture();
    let before = serde_json::to_value(engine.snapshot()).unwrap();
    engine.conn.execute_batch("CREATE TRIGGER reject_metadata_write BEFORE UPDATE ON app_state BEGIN SELECT RAISE(ABORT, 'metadata write fixture'); END;").unwrap();
    assert!(rename(&mut engine, &note, "Changed", 1002).is_err());
    assert_eq!(serde_json::to_value(engine.snapshot()).unwrap(), before);
    assert_eq!(
        rename(&mut engine, &note, "  Original  ", 1002).unwrap(),
        ""
    );
    assert_eq!(serde_json::to_value(engine.snapshot()).unwrap(), before);
    engine
        .conn
        .execute_batch("DROP TRIGGER reject_metadata_write;")
        .unwrap();
    for title in [
        "".to_string(),
        "  ".into(),
        "x\ny".into(),
        "x\ty".into(),
        "x\u{2028}y".into(),
        "x\u{2029}y".into(),
        "x\0y".into(),
        "\nOriginal".into(),
        "中".repeat(201),
    ] {
        assert!(
            rename(&mut engine, &note, &title, 1003).is_err(),
            "accepted {title:?}"
        );
        assert_eq!(serde_json::to_value(engine.snapshot()).unwrap(), before);
    }
    let mut stale = note.clone();
    stale.revision += 1;
    assert!(rename(&mut engine, &stale, "New", 1003).is_err());
    let mut gone = note.clone();
    gone.id = "gone".into();
    assert!(rename(&mut engine, &gone, "New", 1003).is_err());
    assert_eq!(serde_json::to_value(engine.snapshot()).unwrap(), before);
    rename(&mut engine, &note, &"中".repeat(200), 1004).unwrap();
    let latest = engine.snapshot().notes[0].clone();
    assert!(rename(&mut engine, &note, "Original", 1005).is_err());
    assert_eq!(
        rename(&mut engine, &latest, &format!(" {} ", latest.title), 1006).unwrap(),
        ""
    );
    assert_eq!(engine.snapshot().notes[0].revision, latest.revision);
    assert_eq!(engine.snapshot().notes[0].updated_at, latest.updated_at);
}

#[test]
fn tracked_detached_notes_rejected_and_duplicate_names_allowed() {
    let (database, mut engine, note) = fixture();
    let source = database.with_file_name("tracked.md");
    std::fs::write(&source, "# Tracked\noriginal bytes").unwrap();
    engine
        .apply(Command::Import(vec![source.clone()]), 1002)
        .unwrap();
    let tracked = engine
        .snapshot()
        .notes
        .iter()
        .find(|n| !n.managed)
        .unwrap()
        .clone();
    assert!(rename(&mut engine, &tracked, "New", 1003).is_err());
    assert_eq!(
        std::fs::read_to_string(&source).unwrap(),
        "# Tracked\noriginal bytes"
    );
    rename(&mut engine, &note, "Tracked", 1003).unwrap();
    assert_eq!(
        engine
            .snapshot()
            .notes
            .iter()
            .filter(|n| n.title == "Tracked")
            .count(),
        2
    );
    engine
        .apply(
            Command::UnmanageNote {
                note_id: note.id.clone(),
            },
            1004,
        )
        .unwrap();
    let detached = engine.snapshot().notes[0].clone();
    assert!(rename(&mut engine, &detached, "Detached", 1005).is_err());
}

#[test]
fn legacy_note_defaults_and_unrenamed_or_tracked_heading_updates_keep_existing_behavior() {
    let (_, mut engine, note) = fixture();
    let mut json = serde_json::to_value(&note).unwrap();
    json.as_object_mut().unwrap().remove("title_override");
    let legacy: Note = serde_json::from_value(json).unwrap();
    assert!(!legacy.title_override);
    for managed in [true, false] {
        engine.state.notes[0].managed = managed;
        engine.state.notes[0].title_override = !managed;
        crate::notes::append_version(
            &mut engine.state,
            0,
            "# Derived\nbody".into(),
            1002,
            false,
            true,
        );
        assert_eq!(engine.state.notes[0].title, "Derived");
    }
}
