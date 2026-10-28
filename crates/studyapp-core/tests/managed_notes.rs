use std::path::{Path, PathBuf};
use studyapp_core::{Command, Engine};

fn fixture() -> (PathBuf, Engine) {
    let dir = tempfile::tempdir().unwrap().keep();
    let engine = Engine::open(&dir.join("learning.sqlite"), 1000).unwrap();
    (dir, engine)
}
fn create(engine: &mut Engine, content: &str) -> (String, String, PathBuf) {
    engine
        .apply(
            Command::CreateNote {
                title: "中文笔记".into(),
                content: content.into(),
            },
            1001,
        )
        .unwrap();
    let snapshot = engine.snapshot();
    let note = snapshot.notes.last().unwrap();
    assert!(note.managed);
    (
        note.id.clone(),
        snapshot.versions.last().unwrap().hash.clone(),
        note.path.clone().into(),
    )
}
fn save(
    engine: &mut Engine,
    id: &str,
    hash: &str,
    content: &str,
    now: i64,
) -> anyhow::Result<String> {
    engine.apply(
        Command::SaveNote {
            note_id: id.into(),
            expected_hash: hash.into(),
            content: content.into(),
        },
        now,
    )
}

#[test]
fn unchanged_save_is_silent_and_does_not_add_a_version() {
    let (_dir, mut engine) = fixture();
    let content = "# 原文\r\n\r\n不变的内容";
    let (id, hash, path) = create(&mut engine, content);
    let before = engine.snapshot();
    assert_eq!(save(&mut engine, &id, &hash, content, 1010).unwrap(), "");
    assert_eq!(
        serde_json::to_value(&*engine.snapshot().versions).unwrap(),
        serde_json::to_value(&*before.versions).unwrap()
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), content);
    assert!(
        !save(&mut engine, &id, &hash, "# 已修改", 1020)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn retained_draft_id_survives_commit_restart_and_image_removal() {
    let (dir, mut engine) = fixture();
    let note_id = "36b79f08-5128-43ab-a1cb-13f318c455f7";
    engine
        .apply(
            Command::CreateFolder {
                name: "drafts".into(),
                parent_id: None,
            },
            1001,
        )
        .unwrap();
    let folder_id = engine.snapshot().folders[0].id.clone();
    let content = format!("![image](studyapp-media:{note_id}/image.gif)");
    engine
        .apply(
            Command::CreateNoteWithId {
                note_id: note_id.into(),
                folder_id: Some(folder_id.clone()),
                title: "retained".into(),
                content,
            },
            1002,
        )
        .unwrap();
    let snapshot = engine.snapshot();
    assert_eq!(snapshot.notes[0].id, note_id);
    assert_eq!(snapshot.notes[0].folder_id.as_ref(), Some(&folder_id));
    let path = PathBuf::from(&snapshot.notes[0].path);
    save(
        &mut engine,
        note_id,
        &snapshot.versions.last().unwrap().hash,
        "images removed",
        1003,
    )
    .unwrap();
    drop(engine);
    let engine = Engine::open(&dir.join("learning.sqlite"), 2000).unwrap();
    assert_eq!(engine.snapshot().notes[0].id, note_id);
    assert_eq!(std::fs::read_to_string(path).unwrap(), "images removed");
}

#[test]
fn retained_draft_id_rejects_collision_invalid_uuid_and_unknown_folder_without_overwrite() {
    let (dir, mut engine) = fixture();
    let (note_id, _, path) = create(&mut engine, "original");
    for (id, folder_id) in [
        (note_id, None),
        ("../escape".into(), None),
        (
            "36b79f08-5128-43ab-a1cb-13f318c455f7".into(),
            Some("unknown-folder".into()),
        ),
    ] {
        assert!(
            engine
                .apply(
                    Command::CreateNoteWithId {
                        note_id: id,
                        folder_id,
                        title: "replacement".into(),
                        content: "replacement".into(),
                    },
                    1002
                )
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "original");
        assert_eq!(engine.snapshot().notes.len(), 1);
    }
    let occupied = "8529f643-e153-4a0b-983b-c32bb6d28da7";
    let occupied_path = dir.join("notes").join(format!("{occupied}.md"));
    std::fs::write(&occupied_path, "untracked original").unwrap();
    assert!(
        engine
            .apply(
                Command::CreateNoteWithId {
                    note_id: occupied.into(),
                    folder_id: None,
                    title: "replacement".into(),
                    content: "replacement".into(),
                },
                1003
            )
            .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(occupied_path).unwrap(),
        "untracked original"
    );
    assert_eq!(engine.snapshot().notes.len(), 1);
}

#[test]
fn create_save_noop_and_restart_keep_versions_dates_and_managed_path() {
    let (dir, mut engine) = fixture();
    let (id, hash, path) = create(&mut engine, "# 初始\n");
    assert_eq!(
        path,
        std::fs::canonicalize(&dir)
            .unwrap()
            .join("notes")
            .join(format!("{id}.md"))
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "# 初始\n");
    save(&mut engine, &id, &hash, "# 初始\n", 1002).unwrap();
    assert_eq!(engine.snapshot().versions.len(), 1);
    assert_eq!(engine.snapshot().notes[0].updated_at, 1001);
    save(&mut engine, &id, &hash, "# 修订\n正文", 1003).unwrap();
    assert_eq!(engine.snapshot().versions.len(), 2);
    assert_eq!(engine.snapshot().notes[0].created_at, 1001);
    assert_eq!(engine.snapshot().notes[0].updated_at, 1003);
    assert!(engine.snapshot().sessions.is_empty());
    drop(engine);
    let engine = Engine::open(&dir.join("learning.sqlite"), 2000).unwrap();
    assert_eq!(engine.snapshot().versions.len(), 2);
    assert_eq!(std::fs::read_to_string(path).unwrap(), "# 修订\n正文");
}

#[test]
fn cas_rejects_stale_snapshot_and_unscanned_disk_changes() {
    let (_, mut engine) = fixture();
    let (id, first, path) = create(&mut engine, "one");
    save(&mut engine, &id, &first, "two", 1002).unwrap();
    assert!(save(&mut engine, &id, &first, "stale", 1003).is_err());
    let latest = engine.snapshot().versions.last().unwrap().hash.clone();
    std::fs::write(&path, "external").unwrap();
    assert!(save(&mut engine, &id, &latest, "draft", 1004).is_err());
    assert_eq!(std::fs::read_to_string(path).unwrap(), "external");
    assert_eq!(engine.snapshot().versions.len(), 2);
}

#[test]
fn imported_notes_remain_read_only_and_exports_never_overwrite() {
    let (dir, mut engine) = fixture();
    let source = dir.join("external.md");
    std::fs::write(&source, "source").unwrap();
    engine
        .apply(Command::Import(vec![source.clone()]), 1001)
        .unwrap();
    let snapshot = engine.snapshot();
    let note = &snapshot.notes[0];
    assert!(!note.managed);
    assert!(
        save(
            &mut engine,
            &note.id,
            &snapshot.versions[0].hash,
            "edit",
            1002
        )
        .is_err()
    );
    let export = dir.join("export.md");
    let command = Command::ExportNote {
        note_id: note.id.clone(),
        version_id: None,
        path: export.clone(),
    };
    engine.apply(command.clone(), 1003).unwrap();
    assert_eq!(std::fs::read_to_string(&export).unwrap(), "source");
    assert!(engine.apply(command, 1004).is_err());
    assert_eq!(std::fs::read_to_string(source).unwrap(), "source");
}

#[test]
fn synced_changes_are_offline_and_supplied_ids_are_validated() {
    let (_, mut engine) = fixture();
    let id = uuid::Uuid::new_v4().to_string();
    engine
        .apply(
            Command::ReceiveSyncedNote {
                note_id: id.clone(),
                expected_hash: None,
                content: "# 远端".into(),
            },
            1001,
        )
        .unwrap();
    let hash = engine.snapshot().versions[0].hash.clone();
    engine
        .apply(
            Command::ReceiveSyncedNote {
                note_id: id.clone(),
                expected_hash: Some(hash),
                content: "# 更新".into(),
            },
            1002,
        )
        .unwrap();
    assert!(
        engine
            .snapshot()
            .versions
            .iter()
            .all(|version| version.offline)
    );
    assert!(
        engine
            .apply(
                Command::ReceiveSyncedNote {
                    note_id: id,
                    expected_hash: None,
                    content: "duplicate".into()
                },
                1003
            )
            .is_err()
    );
    for id in ["../outside", "NOT-A-UUID"] {
        assert!(
            engine
                .apply(
                    Command::ReceiveSyncedNote {
                        note_id: id.into(),
                        expected_hash: None,
                        content: "bad".into()
                    },
                    1003
                )
                .is_err()
        );
    }
}

#[test]
fn bounds_and_managed_relink_are_rejected_without_mutation() {
    let (dir, mut engine) = fixture();
    for (title, content) in [
        ("字".repeat(201), String::new()),
        ("ok".into(), "x".repeat(10 * 1024 * 1024 + 1)),
    ] {
        assert!(
            engine
                .apply(Command::CreateNote { title, content }, 1001)
                .is_err()
        );
    }
    assert!(engine.snapshot().notes.is_empty());
    let (id, _, path) = create(&mut engine, "original");
    let external = dir.join("external.md");
    std::fs::write(&external, "unrelated").unwrap();
    assert!(
        engine
            .apply(
                Command::Relink {
                    note_id: id,
                    path: external
                },
                1003
            )
            .is_err()
    );
    assert_eq!(Path::new(&engine.snapshot().notes[0].path), path);
}

#[test]
fn database_failure_leaves_original_file_and_snapshot_intact() {
    let (dir, mut engine) = fixture();
    let (id, hash, path) = create(&mut engine, "original");
    let conn = rusqlite::Connection::open(dir.join("learning.sqlite")).unwrap();
    conn.execute_batch("CREATE TRIGGER fail_intent BEFORE INSERT ON write_intents BEGIN SELECT RAISE(ABORT, 'disk full fixture'); END;").unwrap();
    assert!(save(&mut engine, &id, &hash, "draft", 1002).is_err());
    assert_eq!(std::fs::read_to_string(path).unwrap(), "original");
    assert_eq!(engine.snapshot().versions.len(), 1);
}

#[test]
fn disk_failure_is_visible_and_retry_recovers_durable_draft() {
    let (dir, mut engine) = fixture();
    std::fs::write(dir.join("notes"), "blocking file").unwrap();
    let command = Command::CreateNote {
        title: "draft".into(),
        content: "recover me".into(),
    };
    let before = serde_json::to_value(engine.snapshot()).unwrap();
    assert!(engine.apply(command.clone(), 1001).is_err());
    assert_eq!(serde_json::to_value(engine.snapshot()).unwrap(), before);
    assert_eq!(
        std::fs::read_to_string(dir.join("notes")).unwrap(),
        "blocking file"
    );
    let conn = rusqlite::Connection::open(dir.join("learning.sqlite")).unwrap();
    let pending: i64 = conn
        .query_row("SELECT count(*) FROM write_intents", [], |row| row.get(0))
        .unwrap();
    assert!((0..=1).contains(&pending));
    drop(conn);
    drop(engine);
    std::fs::rename(dir.join("notes"), dir.join("retained-blocker")).unwrap();
    let mut engine = Engine::open(&dir.join("learning.sqlite"), 1002).unwrap();
    // ENOTDIR can reject the command before journaling on Unix, whereas Windows
    // can reach the durable intent before discovering the obstructed directory.
    // An early rejection requires an explicit retry; a durable intent replays.
    if pending == 0 {
        assert!(engine.snapshot().notes.is_empty());
        engine.apply(command, 1003).unwrap();
    }
    assert_eq!(engine.snapshot().notes.len(), 1);
    assert_eq!(engine.snapshot().versions.len(), 1);
    assert_eq!(
        std::fs::read_to_string(&engine.snapshot().notes[0].path).unwrap(),
        "recover me"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("retained-blocker")).unwrap(),
        "blocking file"
    );
    let conn = rusqlite::Connection::open(dir.join("learning.sqlite")).unwrap();
    let pending: i64 = conn
        .query_row("SELECT count(*) FROM write_intents", [], |row| row.get(0))
        .unwrap();
    assert_eq!(pending, 0);
}

#[test]
fn restore_rebases_managed_paths_and_preserves_conflicting_local_content() {
    let (source_dir, mut source) = fixture();
    let (id, _, _) = create(&mut source, "backup text");
    let backup = source_dir.join("backup.sqlite");
    source.apply(Command::Backup(backup.clone()), 1002).unwrap();
    let conn = rusqlite::Connection::open(&backup).unwrap();
    let raw: String = conn
        .query_row("SELECT data FROM app_state", [], |row| row.get(0))
        .unwrap();
    let mut state: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let unrelated = source_dir.join("must-not-write.md");
    std::fs::write(&unrelated, "unrelated").unwrap();
    state["notes"][0]["path"] = serde_json::json!(unrelated);
    conn.execute("UPDATE app_state SET data=?1", [state.to_string()])
        .unwrap();
    drop(conn);
    let (target_dir, mut target) = fixture();
    std::fs::create_dir(target_dir.join("notes")).unwrap();
    let local = target_dir.join("notes").join(format!("{id}.md"));
    std::fs::write(&local, "local conflict").unwrap();
    target.apply(Command::Restore(backup), 1003).unwrap();
    assert_eq!(std::fs::read_to_string(&unrelated).unwrap(), "unrelated");
    assert_eq!(std::fs::read_to_string(&local).unwrap(), "backup text");
    assert!(
        target
            .snapshot()
            .versions
            .iter()
            .any(|v| v.content == "local conflict")
    );
    assert_eq!(
        Path::new(&target.snapshot().notes[0].path),
        std::fs::canonicalize(target_dir)
            .unwrap()
            .join("notes")
            .join(format!("{id}.md"))
    );
}

fn fail_final_commit(dir: &Path) -> rusqlite::Connection {
    let conn = rusqlite::Connection::open(dir.join("learning.sqlite")).unwrap();
    conn.execute_batch("CREATE TRIGGER fail_commit BEFORE UPDATE ON app_state BEGIN SELECT RAISE(ABORT, 'commit failure fixture'); END;").unwrap();
    conn
}

#[test]
fn failed_final_database_commit_recovers_and_keeps_original_backups() {
    let (dir, mut engine) = fixture();
    let (id, hash, path) = create(&mut engine, "original");
    let conn = fail_final_commit(&dir);
    assert!(save(&mut engine, &id, &hash, "durable draft", 1002).is_err());
    assert_eq!(engine.snapshot().versions.len(), 1);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "durable draft");
    let pending: i64 = conn
        .query_row("SELECT count(*) FROM write_intents", [], |row| row.get(0))
        .unwrap();
    assert_eq!(pending, 1);
    assert!(
        std::fs::read_dir(dir.join("notes"))
            .unwrap()
            .flatten()
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".original-")
                    && std::fs::read_to_string(entry.path()).unwrap() == "original"
            })
    );
    conn.execute_batch("DROP TRIGGER fail_commit").unwrap();
    drop(conn);
    drop(engine);
    let engine = Engine::open(&dir.join("learning.sqlite"), 1003).unwrap();
    assert_eq!(engine.snapshot().versions.len(), 2);
    assert_eq!(
        engine.snapshot().versions.last().unwrap().content,
        "durable draft"
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), "durable draft");
}

#[test]
fn restart_after_file_change_preserves_both_pending_draft_and_external_content() {
    let (dir, mut engine) = fixture();
    let (id, hash, path) = create(&mut engine, "original");
    let conn = fail_final_commit(&dir);
    assert!(save(&mut engine, &id, &hash, "durable draft", 1002).is_err());
    std::fs::write(&path, "external after interruption").unwrap();
    conn.execute_batch("DROP TRIGGER fail_commit").unwrap();
    drop(conn);
    drop(engine);
    let engine = Engine::open(&dir.join("learning.sqlite"), 1003).unwrap();
    let snapshot = engine.snapshot();
    assert_eq!(snapshot.versions.len(), 3);
    assert!(
        snapshot.notes[0]
            .error
            .as_deref()
            .unwrap()
            .contains("版本历史")
    );
    assert_eq!(snapshot.versions[1].content, "durable draft");
    assert_eq!(snapshot.versions[2].content, "external after interruption");
    assert!(snapshot.versions[2].offline);
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "external after interruption"
    );
}

#[test]
fn restart_with_missing_published_file_replays_journal_when_original_was_retained() {
    let (dir, mut engine) = fixture();
    let (id, hash, path) = create(&mut engine, "original");
    let conn = fail_final_commit(&dir);
    assert!(save(&mut engine, &id, &hash, "draft", 1002).is_err());
    // Simulate interruption after preserving the original and before publication.
    std::fs::rename(&path, dir.join("retained-published.md")).unwrap();
    conn.execute_batch("DROP TRIGGER fail_commit").unwrap();
    drop(conn);
    drop(engine);
    let engine = Engine::open(&dir.join("learning.sqlite"), 1003).unwrap();
    assert_eq!(std::fs::read_to_string(path).unwrap(), "draft");
    assert_eq!(engine.snapshot().versions.len(), 2);
}

#[test]
fn version_one_database_and_backup_default_imported_notes_to_read_only() {
    let (dir, mut engine) = fixture();
    let source = dir.join("v1.md");
    std::fs::write(&source, "legacy").unwrap();
    engine.apply(Command::Import(vec![source]), 1001).unwrap();
    let backup = dir.join("v1-backup.sqlite");
    engine.apply(Command::Backup(backup.clone()), 1002).unwrap();
    drop(engine);
    for database in [&backup, &dir.join("learning.sqlite")] {
        let conn = rusqlite::Connection::open(database).unwrap();
        let raw: String = conn
            .query_row("SELECT data FROM app_state", [], |row| row.get(0))
            .unwrap();
        let mut state: serde_json::Value = serde_json::from_str(&raw).unwrap();
        state["notes"][0].as_object_mut().unwrap().remove("managed");
        conn.execute("UPDATE app_state SET data=?1", [state.to_string()])
            .unwrap();
        conn.execute_batch("DROP TABLE write_intents; PRAGMA user_version=1;")
            .unwrap();
    }
    let mut engine = Engine::open(&dir.join("learning.sqlite"), 1003).unwrap();
    assert!(!engine.snapshot().notes[0].managed);
    engine.apply(Command::Restore(backup), 1004).unwrap();
    assert!(!engine.snapshot().notes[0].managed);
    let conn = rusqlite::Connection::open(dir.join("learning.sqlite")).unwrap();
    let schema: i32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(schema, 3);
}

#[test]
fn synced_content_is_excluded_from_active_learning_and_history_exports_are_exact() {
    let (dir, mut engine) = fixture();
    engine
        .apply(
            Command::SaveSettings(studyapp_core::Settings {
                interval_minutes: 5,
                answer_minutes: 1,
                review_days: vec![1],
                show_file_paths: false,
                layout: Default::default(),
                appearance: Default::default(),
                images: Default::default(),
            }),
            1000,
        )
        .unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    engine
        .apply(
            Command::ReceiveSyncedNote {
                note_id: id.clone(),
                expected_hash: None,
                content: "# 同步基线".into(),
            },
            1001,
        )
        .unwrap();
    let hash = engine.snapshot().versions[0].hash.clone();
    let version_id = engine.snapshot().versions[0].id.clone();
    save(&mut engine, &id, &hash, "# 主动学习\n已编辑", 1002).unwrap();
    let hash = engine.snapshot().versions[1].hash.clone();
    let active_id = engine.snapshot().versions[1].id.clone();
    engine
        .apply(
            Command::ReceiveSyncedNote {
                note_id: id.clone(),
                expected_hash: Some(hash),
                content: "# 远端新内容".into(),
            },
            1003,
        )
        .unwrap();
    for second in 1..=300 {
        engine.tick(1000 + second, 1).unwrap();
    }
    assert_eq!(engine.snapshot().sessions[0].version_ids, vec![active_id]);
    assert!(
        !engine.snapshot().sessions[0]
            .suggested_summary
            .contains("远端新内容")
    );
    let destination = dir.join("historical.md");
    engine
        .apply(
            Command::ExportNote {
                note_id: id,
                version_id: Some(version_id),
                path: destination.clone(),
            },
            1301,
        )
        .unwrap();
    assert_eq!(std::fs::read_to_string(destination).unwrap(), "# 同步基线");
}

#[test]
fn restoring_a_missing_managed_note_repairs_file_and_clears_old_error() {
    let (dir, mut engine) = fixture();
    let (_, _, path) = create(&mut engine, "restore this");
    std::fs::rename(&path, dir.join("retained-missing-original.md")).unwrap();
    engine.apply(Command::Scan { offline: true }, 1002).unwrap();
    assert!(engine.snapshot().notes[0].missing);
    let backup = dir.join("missing-managed.sqlite");
    engine.apply(Command::Backup(backup.clone()), 1003).unwrap();
    engine.apply(Command::Restore(backup), 1004).unwrap();
    assert_eq!(std::fs::read_to_string(path).unwrap(), "restore this");
    assert!(!engine.snapshot().notes[0].missing);
    assert!(engine.snapshot().notes[0].error.is_none());
}

#[test]
fn a_recovered_write_conflict_still_reconciles_other_sources_offline() {
    let (dir, mut engine) = fixture();
    let source = dir.join("other-source.md");
    std::fs::write(&source, "baseline").unwrap();
    engine
        .apply(Command::Import(vec![source.clone()]), 1000)
        .unwrap();
    let (id, hash, path) = create(&mut engine, "managed original");
    let conn = fail_final_commit(&dir);
    assert!(save(&mut engine, &id, &hash, "managed draft", 1002).is_err());
    std::fs::write(&path, "managed external").unwrap();
    std::fs::write(&source, "offline external source").unwrap();
    conn.execute_batch("DROP TRIGGER fail_commit").unwrap();
    drop(conn);
    drop(engine);
    let engine = Engine::open(&dir.join("learning.sqlite"), 1003).unwrap();
    let snapshot = engine.snapshot();
    assert!(
        snapshot
            .versions
            .iter()
            .any(|version| version.content == "offline external source" && version.offline)
    );
    assert!(
        snapshot
            .notes
            .iter()
            .find(|note| note.id == id)
            .unwrap()
            .error
            .is_some()
    );
}

#[test]
fn explicit_creation_title_keeps_conflict_copy_identifiable() {
    let (_, mut engine) = fixture();
    engine
        .apply(
            Command::CreateNote {
                title: "冲突副本 · 原笔记".into(),
                content: "# 原笔记\n正文保持不变".into(),
            },
            1001,
        )
        .unwrap();
    assert_eq!(engine.snapshot().notes[0].title, "冲突副本 · 原笔记");
    assert_eq!(
        engine.snapshot().versions[0].content,
        "# 原笔记\n正文保持不变"
    );
}

#[test]
fn synced_copy_keeps_its_title_is_offline_and_never_overwrites() {
    let (_, mut engine) = fixture();
    let note_id = uuid::Uuid::new_v4().to_string();
    engine
        .apply(
            Command::ReceiveSyncedCopy {
                note_id: note_id.clone(),
                title: "远端副本 · 导入笔记".into(),
                content: "# 原始标题\n下载内容".into(),
            },
            1001,
        )
        .unwrap();
    let snapshot = engine.snapshot();
    assert_eq!(snapshot.notes[0].title, "远端副本 · 导入笔记");
    assert!(snapshot.notes[0].managed);
    assert!(snapshot.versions[0].offline);
    assert!(
        engine
            .apply(
                Command::ReceiveSyncedCopy {
                    note_id,
                    title: "试图覆盖".into(),
                    content: "不应写入".into(),
                },
                1002
            )
            .is_err()
    );
    assert!(
        engine
            .apply(
                Command::ReceiveSyncedCopy {
                    note_id: uuid::Uuid::new_v4().to_string(),
                    title: "字".repeat(201),
                    content: "无效标题".into(),
                },
                1002
            )
            .is_err()
    );
    assert_eq!(engine.snapshot().versions.len(), 1);
    assert_eq!(
        std::fs::read_to_string(&snapshot.notes[0].path).unwrap(),
        "# 原始标题\n下载内容"
    );
    engine
        .apply(
            Command::SaveSettings(studyapp_core::Settings {
                interval_minutes: 5,
                answer_minutes: 1,
                review_days: vec![1],
                show_file_paths: false,
                layout: Default::default(),
                appearance: Default::default(),
                images: Default::default(),
            }),
            1002,
        )
        .unwrap();
    for second in 1..=300 {
        engine.tick(1000 + second, 1).unwrap();
    }
    assert!(engine.snapshot().sessions[0].version_ids.is_empty());
    assert!(engine.snapshot().sessions[0].suggested_summary.is_empty());
}
