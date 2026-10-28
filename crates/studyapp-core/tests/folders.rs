use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use studyapp_core::{Command, Engine, Snapshot};

fn fixture() -> (PathBuf, Engine) {
    // Keep all fixtures: the project forbids recursive filesystem deletion.
    let root = tempfile::tempdir().unwrap().keep();
    let engine = Engine::open(&root.join("study.sqlite"), 1000).unwrap();
    (root, engine)
}

fn folder(engine: &mut Engine, name: &str, parent_id: Option<&str>) -> String {
    engine
        .apply(
            Command::CreateFolder {
                name: name.into(),
                parent_id: parent_id.map(str::to_owned),
            },
            1001,
        )
        .unwrap();
    engine.snapshot().folders.last().unwrap().id.clone()
}

fn reject(engine: &mut Engine, command: Command) {
    let before = serde_json::to_value(engine.snapshot()).unwrap();
    assert!(engine.apply(command, 1002).is_err());
    assert_eq!(serde_json::to_value(engine.snapshot()).unwrap(), before);
}

fn rewrite_state(path: &Path, edit: impl FnOnce(&mut Value)) {
    let conn = rusqlite::Connection::open(path).unwrap();
    let raw: String = conn
        .query_row("SELECT data FROM app_state", [], |row| row.get(0))
        .unwrap();
    let mut state: Value = serde_json::from_str(&raw).unwrap();
    edit(&mut state);
    conn.execute("UPDATE app_state SET data=?1", [state.to_string()])
        .unwrap();
}

#[test]
fn folder_identity_and_note_membership_survive_restart_and_backup_restore() {
    let (root, mut engine) = fixture();
    let parent = folder(&mut engine, "研究", None);
    let child = folder(&mut engine, "Rust", Some(&parent));
    engine
        .apply(
            Command::CreateNoteInFolder {
                title: "所有权".into(),
                content: "保留原文\r\n".into(),
                folder_id: Some(child.clone()),
            },
            1002,
        )
        .unwrap();
    let note = engine.snapshot().notes[0].clone();
    assert_eq!(note.folder_id.as_deref(), Some(child.as_str()));
    engine
        .apply(
            Command::RenameFolder {
                folder_id: child.clone(),
                name: "借用与所有权".into(),
            },
            1003,
        )
        .unwrap();
    let expected_folders = engine.snapshot().folders;
    let backup = root.join("folders-backup.sqlite");
    engine.apply(Command::Backup(backup.clone()), 1004).unwrap();
    engine
        .apply(
            Command::MoveFolder {
                folder_id: child.clone(),
                parent_id: None,
            },
            1005,
        )
        .unwrap();
    engine
        .apply(
            Command::MoveNote {
                note_id: note.id.clone(),
                folder_id: None,
            },
            1006,
        )
        .unwrap();
    engine.apply(Command::Restore(backup), 1007).unwrap();
    assert_eq!(engine.snapshot().folders, expected_folders);
    assert_eq!(engine.snapshot().notes[0].folder_id, Some(child.clone()));
    assert_eq!(engine.snapshot().notes[0].path, note.path);
    assert_eq!(
        std::fs::read(&note.path).unwrap(),
        b"\xe4\xbf\x9d\xe7\x95\x99\xe5\x8e\x9f\xe6\x96\x87\r\n"
    );
    drop(engine);
    let engine = Engine::open(&root.join("study.sqlite"), 2000).unwrap();
    assert_eq!(engine.snapshot().folders, expected_folders);
    assert_eq!(engine.snapshot().notes[0].folder_id, Some(child));
    assert_eq!(engine.snapshot().notes[0].id, note.id);
    assert_eq!(engine.snapshot().versions.len(), 1);
}

#[test]
fn sibling_names_are_trimmed_and_unique_but_different_parents_can_share_names() {
    let (_, mut engine) = fixture();
    let rust = folder(&mut engine, "  Rust  ", None);
    assert_eq!(engine.snapshot().folders[0].name, "Rust");
    reject(
        &mut engine,
        Command::CreateFolder {
            name: "rust".into(),
            parent_id: None,
        },
    );
    let other = folder(&mut engine, "其他", None);
    let nested = folder(&mut engine, "Rust", Some(&other));
    engine
        .apply(
            Command::RenameFolder {
                folder_id: rust,
                name: "Rust".into(),
            },
            1002,
        )
        .unwrap();
    reject(
        &mut engine,
        Command::RenameFolder {
            folder_id: other,
            name: "RUST".into(),
        },
    );
    reject(
        &mut engine,
        Command::MoveFolder {
            folder_id: nested,
            parent_id: None,
        },
    );
}

#[test]
fn invalid_names_and_unknown_objects_are_rejected_without_mutation() {
    let (_, mut engine) = fixture();
    let existing = folder(&mut engine, "现有目录", None);
    for name in [
        "".to_owned(),
        " \t ".to_owned(),
        ".".to_owned(),
        "..".to_owned(),
        "a/b".to_owned(),
        "a\\b".to_owned(),
        "a\nb".to_owned(),
        "a\0b".to_owned(),
        "字".repeat(201),
    ] {
        reject(
            &mut engine,
            Command::CreateFolder {
                name: name.clone(),
                parent_id: None,
            },
        );
        reject(
            &mut engine,
            Command::RenameFolder {
                folder_id: existing.clone(),
                name,
            },
        );
    }
    for command in [
        Command::CreateFolder {
            name: "新目录".into(),
            parent_id: Some("unknown".into()),
        },
        Command::MoveFolder {
            folder_id: existing,
            parent_id: Some("unknown".into()),
        },
        Command::MoveFolder {
            folder_id: "unknown".into(),
            parent_id: None,
        },
        Command::RenameFolder {
            folder_id: "unknown".into(),
            name: "新名称".into(),
        },
        Command::DeleteFolder {
            folder_id: "unknown".into(),
        },
        Command::MoveNote {
            note_id: "unknown".into(),
            folder_id: None,
        },
    ] {
        reject(&mut engine, command);
    }
}

#[test]
fn folder_moves_reject_cycles_and_excessive_descendant_depth() {
    let (_, mut engine) = fixture();
    let mut chain = vec![folder(&mut engine, "层1", None)];
    for depth in 2..=8 {
        let parent = chain.last().unwrap().clone();
        chain.push(folder(&mut engine, &format!("层{depth}"), Some(&parent)));
    }
    reject(
        &mut engine,
        Command::CreateFolder {
            name: "层9".into(),
            parent_id: Some(chain[7].clone()),
        },
    );
    for parent in [chain[0].clone(), chain[7].clone()] {
        reject(
            &mut engine,
            Command::MoveFolder {
                folder_id: chain[0].clone(),
                parent_id: Some(parent),
            },
        );
    }
    let branch = folder(&mut engine, "分支", None);
    let leaf = folder(&mut engine, "叶子", Some(&branch));
    reject(
        &mut engine,
        Command::MoveFolder {
            folder_id: branch.clone(),
            parent_id: Some(chain[6].clone()),
        },
    );
    engine
        .apply(
            Command::MoveFolder {
                folder_id: branch.clone(),
                parent_id: Some(chain[5].clone()),
            },
            1003,
        )
        .unwrap();
    assert_eq!(
        engine
            .snapshot()
            .folders
            .iter()
            .find(|f| f.id == leaf)
            .unwrap()
            .parent_id,
        Some(branch)
    );
}

#[test]
fn moving_tracked_notes_changes_only_logical_metadata() {
    let (root, mut engine) = fixture();
    let source = root.join("external.md");
    std::fs::write(&source, "外部笔记\r\n原格式保留").unwrap();
    engine
        .apply(Command::Import(vec![source.clone()]), 1001)
        .unwrap();
    let destination = folder(&mut engine, "外部来源", None);
    let note = engine.snapshot().notes[0].clone();
    engine
        .apply(
            Command::Manual {
                started_at: 1000,
                minutes: 1,
                summary: "保留学习关联".into(),
                note_ids: vec![note.id.clone()],
                version_ids: None,
            },
            2000,
        )
        .unwrap();
    let before = engine.snapshot();
    let original = std::fs::read(&source).unwrap();
    engine
        .apply(
            Command::MoveNote {
                note_id: note.id.clone(),
                folder_id: Some(destination.clone()),
            },
            2001,
        )
        .unwrap();
    let after = engine.snapshot();
    assert_eq!(after.notes[0].path, note.path);
    assert!(!after.notes[0].managed);
    assert_eq!(after.notes[0].folder_id, Some(destination.clone()));
    assert_eq!(after.notes[0].revision, note.revision + 1);
    assert_eq!(std::fs::read(&source).unwrap(), original);
    for (before, after) in [
        (
            serde_json::to_value(&before.versions),
            serde_json::to_value(&after.versions),
        ),
        (
            serde_json::to_value(&before.sessions),
            serde_json::to_value(&after.sessions),
        ),
        (
            serde_json::to_value(&before.reviews),
            serde_json::to_value(&after.reviews),
        ),
    ] {
        assert_eq!(before.unwrap(), after.unwrap());
    }
    engine
        .apply(
            Command::MoveNote {
                note_id: note.id.clone(),
                folder_id: Some(destination),
            },
            2002,
        )
        .unwrap();
    assert_eq!(engine.snapshot().notes[0].revision, after.notes[0].revision);
    reject(
        &mut engine,
        Command::MoveNote {
            note_id: note.id,
            folder_id: Some("unknown".into()),
        },
    );
}

#[test]
fn only_empty_folders_can_be_deleted_and_detached_notes_still_count() {
    let (_, mut engine) = fixture();
    let parent = folder(&mut engine, "父目录", None);
    let child = folder(&mut engine, "子目录", Some(&parent));
    reject(
        &mut engine,
        Command::DeleteFolder {
            folder_id: parent.clone(),
        },
    );
    engine
        .apply(
            Command::CreateNoteInFolder {
                title: "保留".into(),
                content: "原文件不删除".into(),
                folder_id: Some(child.clone()),
            },
            1002,
        )
        .unwrap();
    let note = engine.snapshot().notes[0].clone();
    reject(
        &mut engine,
        Command::DeleteFolder {
            folder_id: child.clone(),
        },
    );
    engine
        .apply(
            Command::UnmanageNote {
                note_id: note.id.clone(),
            },
            1003,
        )
        .unwrap();
    reject(
        &mut engine,
        Command::DeleteFolder {
            folder_id: child.clone(),
        },
    );
    engine
        .apply(
            Command::MoveNote {
                note_id: note.id,
                folder_id: None,
            },
            1004,
        )
        .unwrap();
    engine
        .apply(Command::DeleteFolder { folder_id: child }, 1005)
        .unwrap();
    engine
        .apply(Command::DeleteFolder { folder_id: parent }, 1006)
        .unwrap();
    assert!(engine.snapshot().folders.is_empty());
    assert!(engine.snapshot().notes[0].detached);
    assert_eq!(std::fs::read_to_string(note.path).unwrap(), "原文件不删除");
    assert_eq!(engine.snapshot().versions.len(), 1);
}

#[test]
fn invalid_create_destination_leaves_no_note_or_write_intent() {
    let (root, mut engine) = fixture();
    reject(
        &mut engine,
        Command::CreateNoteInFolder {
            title: "未写入".into(),
            content: "不得创建文件".into(),
            folder_id: Some("unknown".into()),
        },
    );
    assert!(engine.snapshot().notes.is_empty());
    assert!(engine.snapshot().versions.is_empty());
    let conn = rusqlite::Connection::open(root.join("study.sqlite")).unwrap();
    assert_eq!(
        conn.query_row("SELECT count(*) FROM write_intents", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    engine
        .apply(
            Command::CreateNote {
                title: "旧 API".into(),
                content: "仍可使用".into(),
            },
            1003,
        )
        .unwrap();
    assert_eq!(engine.snapshot().notes[0].folder_id, None);
}

#[test]
fn legacy_database_and_snapshot_missing_folder_fields_default_to_root() {
    for schema in [2, 3] {
        let (root, mut engine) = fixture();
        engine
            .apply(
                Command::CreateNote {
                    title: "旧笔记".into(),
                    content: "旧内容".into(),
                },
                1001,
            )
            .unwrap();
        let mut snapshot = serde_json::to_value(engine.snapshot()).unwrap();
        snapshot.as_object_mut().unwrap().remove("folders");
        snapshot["notes"][0]
            .as_object_mut()
            .unwrap()
            .remove("folder_id");
        let snapshot: Snapshot = serde_json::from_value(snapshot).unwrap();
        assert!(snapshot.folders.is_empty());
        assert_eq!(snapshot.notes[0].folder_id, None);
        drop(engine);
        let path = root.join("study.sqlite");
        rewrite_state(&path, |state| {
            state.as_object_mut().unwrap().remove("folders");
            state["notes"][0]
                .as_object_mut()
                .unwrap()
                .remove("folder_id");
        });
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.pragma_update(None, "user_version", schema).unwrap();
        }
        let engine = Engine::open(&path, 2000).unwrap();
        assert!(engine.snapshot().folders.is_empty());
        assert_eq!(engine.snapshot().notes[0].folder_id, None);
    }
}

#[test]
fn corrupt_backup_folder_graph_or_note_reference_is_rejected_before_restore() {
    let (root, mut engine) = fixture();
    let parent = folder(&mut engine, "安全目录", None);
    engine
        .apply(
            Command::CreateNoteInFolder {
                title: "现有笔记".into(),
                content: "现有原文".into(),
                folder_id: Some(parent.clone()),
            },
            1002,
        )
        .unwrap();
    let before = serde_json::to_value(engine.snapshot()).unwrap();
    let path = engine.snapshot().notes[0].path.clone();
    let folder_value = |id: &str, name: &str, parent: Option<&str>| json!({ "id": id, "name": name, "parent_id": parent });
    let corrupt = [
        json!([
            folder_value(&parent, "目录", None),
            folder_value(&parent, "重复 ID", None)
        ]),
        json!([folder_value(&parent, "目录", Some("missing"))]),
        json!([folder_value(&parent, "目录", Some(&parent))]),
        json!([
            folder_value(&parent, "目录", Some("other")),
            folder_value("other", "循环", Some(&parent))
        ]),
        json!([
            folder_value(&parent, "Rust", None),
            folder_value("other", "rust", None)
        ]),
        json!([folder_value(&parent, "../错误", None)]),
        json!([]),
        json!(
            (0..9)
                .map(|index| {
                    let id = if index == 0 {
                        parent.clone()
                    } else {
                        format!("level-{index}")
                    };
                    let ancestor = if index == 0 {
                        None
                    } else if index == 1 {
                        Some(parent.clone())
                    } else {
                        Some(format!("level-{}", index - 1))
                    };
                    folder_value(&id, &format!("目录{index}"), ancestor.as_deref())
                })
                .collect::<Vec<_>>()
        ),
    ];
    for (index, folders) in corrupt.into_iter().enumerate() {
        let backup = root.join(format!("bad-folders-{index}.sqlite"));
        engine.apply(Command::Backup(backup.clone()), 1003).unwrap();
        rewrite_state(&backup, |state| state["folders"] = folders);
        assert!(engine.apply(Command::Restore(backup), 1004).is_err());
        assert_eq!(serde_json::to_value(engine.snapshot()).unwrap(), before);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "现有原文");
    }
}
