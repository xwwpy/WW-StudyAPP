use super::*;
use studyapp_core::{Command, Engine};

const GIF: &[u8] = b"GIF89a\x01\0\x01\0\x80\0\0\0\0\0\xff\xff\xff!\xf9\x04\x01\0\0\0\0,\0\0\0\0\x01\0\x01\0\0\x02\x02D\x01\0;";

fn inventory(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, path: &Path, found: &mut BTreeMap<PathBuf, Vec<u8>>) {
        if !path.exists() {
            return;
        }
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(root, &path, found);
            } else {
                found.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    std::fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut found = BTreeMap::new();
    for name in ["images", "image-index"] {
        visit(root, &root.join(name), &mut found);
    }
    found
}

fn fixture() -> (PathBuf, String) {
    let root = tempfile::tempdir().unwrap().keep();
    let database = root.join("library.sqlite3");
    let refs: Vec<_> = (0..3)
        .map(|n| {
            let group = new_group();
            let key = format!("assets/{group}/historic-{n}.gif");
            publish_local(&database, &Default::default(), &key, GIF).unwrap();
            format!("{PREFIX}{group}/historic-{n}.gif")
        })
        .collect();
    (
        database,
        format!(
            "# 历史\r\n\r\n![内联]({})\r\n![引用][old]\r\n\r\n[old]: {} \"原图\"\r\n![第三]({})\r\n",
            refs[0], refs[1], refs[2]
        ),
    )
}

fn adopt(database: &Path, group: &str, content: &str) -> String {
    let MediaOutcome::Adopted { content, missing } = execute(
        database,
        MediaRequest::Adopt {
            policy: MediaAdoptionPolicy::PreserveManaged,
            generation: 1,
            request: 1,
            group: group.into(),
            title: "历史".into(),
            content: content.into(),
            source_path: None,
            settings: Default::default(),
        },
        None,
    )
    .unwrap() else {
        panic!()
    };
    assert!(missing.is_empty(), "{missing:?}");
    content
}

#[test]
fn historic_managed_refs_survive_adoption_retention_and_real_engine_reopen() {
    let (database, content) = fixture();
    let group = new_group();
    let baseline = inventory(database.parent().unwrap());
    let mut engine = Engine::open(&database, 100).unwrap();
    engine
        .apply(
            Command::CreateNoteWithId {
                note_id: group.clone(),
                folder_id: None,
                title: "历史".into(),
                content: content.clone(),
            },
            101,
        )
        .unwrap();
    let before = engine.snapshot();
    let note = before.notes[0].clone();
    let hash = before.versions[0].hash.clone();
    for now in [102, 103] {
        let saved = adopt(&database, &group, &content);
        assert_eq!(
            saved, content,
            "ordinary adoption must keep historical URI bytes"
        );
        let MediaOutcome::Retained { bindings, missing } = execute(
            &database,
            MediaRequest::Retain {
                generation: 1,
                request: 2,
                group: group.clone(),
                title: "历史".into(),
                content: content.clone(),
                source_path: None,
                settings: Default::default(),
            },
            None,
        )
        .unwrap() else {
            panic!()
        };
        assert!(bindings.is_empty());
        assert!(missing.is_empty());
        assert_eq!(
            engine
                .apply(
                    Command::SaveNote {
                        note_id: group.clone(),
                        expected_hash: hash.clone(),
                        content: saved
                    },
                    now
                )
                .unwrap(),
            ""
        );
        assert_eq!(engine.snapshot().notes[0].revision, note.revision);
        assert_eq!(engine.snapshot().notes[0].updated_at, note.updated_at);
        assert_eq!(engine.snapshot().versions.len(), 1);
        assert_eq!(engine.snapshot().versions[0].hash, hash);
        drop(engine);
        engine = Engine::open(&database, now + 10).unwrap();
        assert_eq!(std::fs::read_to_string(&note.path).unwrap(), content);
        assert_eq!(inventory(database.parent().unwrap()), baseline);
    }
    let edited = format!("{content}\r\n只添加正文。\r\n");
    engine
        .apply(
            Command::SaveNote {
                note_id: group,
                expected_hash: hash,
                content: adopt(&database, &note.id, &edited),
            },
            120,
        )
        .unwrap();
    let after = engine.snapshot();
    assert_eq!(after.versions.len(), 2);
    assert_eq!(after.versions[0].content, content);
    assert_eq!(after.versions[1].content, edited);
    assert_eq!(after.notes[0].revision, note.revision + 1);
    assert_eq!(inventory(database.parent().unwrap()), baseline);
}

#[test]
fn offline_historic_refs_skip_provider_and_import_budget_but_validate_keys() {
    struct Offline;
    impl AssetProvider for Offline {
        fn get_asset(&self, _: &str) -> studyapp_sync::Result<Option<Vec<u8>>> {
            panic!("unchanged managed image downloaded")
        }
        fn put_asset(&self, _: &str, _: &[u8]) -> studyapp_sync::Result<()> {
            panic!("unchanged managed image uploaded")
        }
        fn check_connection(&self) -> studyapp_sync::Result<()> {
            panic!("unchanged managed image connected")
        }
    }
    let database = tempfile::tempdir().unwrap().keep().join("library.sqlite3");
    let content: String = (0..205)
        .map(|n| format!("![remote]({PREFIX}{}/offline-{n}.gif)\r\n", new_group()))
        .collect();
    let request = MediaRequest::Adopt {
        policy: MediaAdoptionPolicy::PreserveManaged,
        generation: 1,
        request: 1,
        group: new_group(),
        title: "offline".into(),
        content: content.clone(),
        source_path: None,
        settings: ImageSettings {
            storage: ImageStorage::Both,
            ..Default::default()
        },
    };
    for outcome in [
        execute(&database, request.clone(), Some(Box::new(Offline))).unwrap(),
        execute_with_provider(
            &database,
            request.clone(),
            Err(anyhow::anyhow!("locked credentials")),
        )
        .unwrap(),
        execute_with_provider_factory(&database, request, || panic!("credentials consulted"))
            .unwrap(),
    ] {
        let MediaOutcome::Adopted {
            content: saved,
            missing,
        } = outcome
        else {
            panic!()
        };
        assert_eq!(saved, content);
        assert!(missing.is_empty());
    }
    let MediaOutcome::Retained { bindings, missing } = execute_with_provider_factory(
        &database,
        MediaRequest::Retain {
            generation: 1,
            request: 2,
            group: new_group(),
            title: "offline".into(),
            content,
            source_path: None,
            settings: ImageSettings {
                storage: ImageStorage::Both,
                ..Default::default()
            },
        },
        || panic!("retention consulted credentials"),
    )
    .unwrap() else {
        panic!()
    };
    assert!(bindings.is_empty());
    assert!(missing.is_empty());
    let invalid = format!("![invalid]({PREFIX}../escape.gif)");
    let MediaOutcome::Adopted { content, missing } = execute(
        &database,
        MediaRequest::Adopt {
            policy: MediaAdoptionPolicy::PreserveManaged,
            generation: 1,
            request: 3,
            group: new_group(),
            title: "invalid".into(),
            content: invalid.clone(),
            source_path: None,
            settings: Default::default(),
        },
        None,
    )
    .unwrap() else {
        panic!()
    };
    assert_eq!(content, invalid);
    assert_eq!(missing.len(), 1);
    assert!(inventory(database.parent().unwrap()).is_empty());
}

#[test]
fn explicit_copy_has_independent_assets_and_subsequent_saves_are_stable() {
    let (database, original) = fixture();
    let before = inventory(database.parent().unwrap());
    let group = new_group();
    let request = MediaRequest::Adopt {
        policy: MediaAdoptionPolicy::CopyToGroup,
        generation: 1,
        request: 1,
        group: group.clone(),
        title: "copy".into(),
        content: original.clone(),
        source_path: None,
        settings: Default::default(),
    };
    let MediaOutcome::Adopted {
        content: copy,
        missing,
    } = execute(&database, request, None).unwrap()
    else {
        panic!()
    };
    assert!(missing.is_empty());
    assert_ne!(copy, original);
    let references = image_uses(&copy).unwrap();
    assert_eq!(references.len(), 3);
    for image in references {
        assert!(image.destination.starts_with(&format!("{PREFIX}{group}/")));
        let key = asset_key(&image.destination).unwrap();
        assert_eq!(
            std::fs::read(local_asset(&database, &Default::default(), &key).unwrap()).unwrap(),
            GIF
        );
    }
    let copied = inventory(database.parent().unwrap());
    for (name, bytes) in before {
        assert_eq!(copied[&name], bytes);
    }
    assert_eq!(adopt(&database, &group, &copy), copy);
    assert_eq!(adopt(&database, &new_group(), &copy), copy);
    assert_eq!(inventory(database.parent().unwrap()), copied);
}

#[test]
fn new_recovered_draft_and_encoded_external_binding_import_once() {
    let root = tempfile::tempdir().unwrap().keep();
    let database = root.join("library.sqlite3");
    let source = root.join("tracked.md");
    let external = root.join("原始 图.gif");
    let original = "# tracked\r\n![图](%E5%8E%9F%E5%A7%8B%20%E5%9B%BE.gif)\r\n";
    std::fs::write(&source, original).unwrap();
    std::fs::write(&external, GIF).unwrap();
    let group = new_group();
    let MediaOutcome::Retained { bindings, missing } = execute(
        &database,
        MediaRequest::Retain {
            generation: 1,
            request: 1,
            group: group.clone(),
            title: "new".into(),
            content: original.into(),
            source_path: Some(source.clone()),
            settings: Default::default(),
        },
        None,
    )
    .unwrap() else {
        panic!()
    };
    assert!(missing.is_empty());
    assert_eq!(bindings.len(), 1);
    let content = rewrite_images(original, &bindings).unwrap();
    let draft = crate::drafts::NoteDraft {
        asset_group: group.clone(),
        note_id: None,
        expected_hash: None,
        folder_id: None,
        title: "new".into(),
        content,
    };
    let recovered: crate::drafts::NoteDraft =
        serde_json::from_slice(&serde_json::to_vec(&draft).unwrap()).unwrap();
    let before = inventory(&root);
    let saved = adopt(&database, &recovered.asset_group, &recovered.content);
    assert_eq!(saved, recovered.content);
    let MediaOutcome::Retained { bindings, missing } = execute(
        &database,
        MediaRequest::Retain {
            generation: 1,
            request: 2,
            group: group.clone(),
            title: "new".into(),
            content: saved.clone(),
            source_path: Some(source.clone()),
            settings: Default::default(),
        },
        None,
    )
    .unwrap() else {
        panic!()
    };
    assert!(missing.is_empty());
    assert!(bindings.is_empty());
    let mut engine = Engine::open(&database, 100).unwrap();
    engine
        .apply(
            Command::CreateNoteWithId {
                note_id: group.clone(),
                folder_id: None,
                title: recovered.title,
                content: saved.clone(),
            },
            101,
        )
        .unwrap();
    assert_eq!(engine.snapshot().notes[0].id, group);
    assert_eq!(engine.snapshot().versions[0].content, saved);
    assert_eq!(inventory(&root), before);
    assert_eq!(std::fs::read_to_string(source).unwrap(), original);
    assert_eq!(std::fs::read(external).unwrap(), GIF);
}
