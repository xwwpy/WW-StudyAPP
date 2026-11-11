use studyapp_core::{Command, Engine, Settings, SnapshotMode};

fn setup(mode: SnapshotMode) -> (std::path::PathBuf, Engine, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap().keep();
    let mut engine = Engine::open(&dir.join("library.sqlite"), 1000).unwrap();
    let mut settings = engine.snapshot().settings;
    settings.snapshots.mode = mode;
    engine
        .apply(Command::SaveSettings(Box::new(settings)), 1001)
        .unwrap();
    let source = dir.join("source.md");
    std::fs::write(&source, "# Baseline\nold").unwrap();
    engine
        .apply(Command::Import(vec![source.clone()]), 1002)
        .unwrap();
    (dir, engine, source)
}
#[test]
fn legacy_settings_keep_snapshots_readable_and_get_a_safe_policy() {
    let mut legacy = serde_json::to_value(Settings::default()).unwrap();
    legacy.as_object_mut().unwrap().remove("snapshots");
    let restored: Settings = serde_json::from_value(legacy).unwrap();
    assert_eq!(restored.snapshots.mode, SnapshotMode::Automatic);
    assert_eq!(restored.snapshots.interval_seconds, 60);
    restored.validate().unwrap();
}
#[test]
fn passive_scans_only_capture_in_automatic_mode_and_explicit_capture_deduplicates() {
    for mode in SnapshotMode::ALL {
        let (_dir, mut engine, source) = setup(mode);
        let original = engine.snapshot().versions[0].clone();
        std::fs::write(source, "# Changed\nnew").unwrap();
        engine
            .apply(Command::Scan { offline: false }, 1003)
            .unwrap();
        engine
            .apply(Command::Scan { offline: false }, 1006)
            .unwrap();
        assert_eq!(
            engine.snapshot().versions.len(),
            if mode == SnapshotMode::Automatic {
                2
            } else {
                1
            }
        );
        engine.apply(Command::CaptureSnapshots, 1007).unwrap();
        assert_eq!(engine.snapshot().versions.len(), 2);
        assert_eq!(engine.snapshot().versions[0].id, original.id);
        assert_eq!(engine.snapshot().versions[0].content, original.content);
        engine.apply(Command::CaptureSnapshots, 1008).unwrap();
        assert_eq!(engine.snapshot().versions.len(), 2);
    }
}
#[test]
fn startup_respects_manual_modes_and_preserves_offline_provenance() {
    for mode in SnapshotMode::ALL {
        let (dir, engine, source) = setup(mode);
        drop(engine);
        std::fs::write(source, "# Offline content").unwrap();
        let mut engine = Engine::open(&dir.join("library.sqlite"), 2000).unwrap();
        assert_eq!(
            engine.snapshot().versions.len(),
            if mode == SnapshotMode::Automatic {
                2
            } else {
                1
            }
        );
        engine.apply(Command::CaptureSnapshots, 2001).unwrap();
        assert_eq!(engine.snapshot().versions.len(), 2);
        assert!(engine.snapshot().versions[1].offline);
    }
}
#[test]
fn scan_only_reports_missing_files_without_creating_history() {
    let (_dir, mut engine, source) = setup(SnapshotMode::ScanOnly);
    std::fs::remove_file(&source).unwrap();
    engine
        .apply(Command::Scan { offline: false }, 1004)
        .unwrap();
    assert!(engine.snapshot().notes[0].missing);
    assert_eq!(engine.snapshot().versions.len(), 1);
    std::fs::write(source, "recovered").unwrap();
    engine.apply(Command::CaptureSnapshots, 1005).unwrap();
    assert!(!engine.snapshot().notes[0].missing);
    assert_eq!(engine.snapshot().versions.len(), 2);
}
#[test]
fn explicit_editor_saves_remain_durable_without_creating_manual_snapshots() {
    for mode in [SnapshotMode::Manual, SnapshotMode::ScanOnly] {
        let (dir, mut engine, _) = setup(mode);
        engine
            .apply(
                Command::CreateNote {
                    title: "managed".into(),
                    content: "draft".into(),
                },
                1003,
            )
            .unwrap();
        let snap = engine.snapshot();
        let note = snap.notes.last().unwrap().clone();
        let count = snap.versions.len();
        let mut hash = snap.note_body(&note.id).unwrap().1.to_owned();
        for (now, content) in [(1004, "saved"), (1005, "saved again")] {
            engine
                .apply(
                    Command::SaveNote {
                        note_id: note.id.clone(),
                        expected_hash: hash.clone(),
                        content: content.into(),
                    },
                    now,
                )
                .unwrap();
            let snapshot = engine.snapshot();
            assert_eq!(snapshot.versions.len(), count);
            assert_eq!(snapshot.note_body(&note.id).unwrap().0, content);
            assert_eq!(std::fs::read_to_string(&note.path).unwrap(), content);
            let new_hash = snapshot.note_body(&note.id).unwrap().1.to_owned();
            assert_ne!(new_hash, hash);
            hash = new_hash;
        }
        engine.finish_snapshots(1006).unwrap();
        assert_eq!(engine.snapshot().versions.len(), count);
        drop(engine);
        let engine = Engine::open(&dir.join("library.sqlite"), 2000).unwrap();
        assert_eq!(engine.snapshot().versions.len(), count);
        assert_eq!(
            engine.snapshot().note_body(&note.id).unwrap(),
            ("saved again", hash.as_str())
        );
    }
}

#[test]
fn authoritative_settings_are_restored_before_the_first_scan() {
    let (root, engine, source) = setup(SnapshotMode::Automatic);
    let original = engine.snapshot().versions;
    drop(engine);
    std::fs::write(&source, "# Offline change").unwrap();
    let mut engine = Engine::open_deferred_scan(&root.join("library.sqlite"), 2000).unwrap();
    assert_eq!(
        serde_json::to_value(&engine.snapshot().versions).unwrap(),
        serde_json::to_value(&original).unwrap()
    );
    let mut settings = engine.settings().clone();
    settings.snapshots.mode = SnapshotMode::Manual;
    let prepared = engine.prepare_settings(settings).unwrap();
    engine.install_prepared_settings(prepared);
    engine.complete_startup_scan(2000).unwrap();
    assert_eq!(
        serde_json::to_value(&engine.snapshot().versions).unwrap(),
        serde_json::to_value(&original).unwrap()
    );
    engine.complete_startup_scan(2001).unwrap();
    engine.apply(Command::CaptureSnapshots, 2002).unwrap();
    let snapshot = engine.snapshot();
    assert_eq!(snapshot.versions.len(), original.len() + 1);
    assert!(snapshot.versions.last().unwrap().offline);
}

#[test]
fn stable_window_is_independent_and_further_edits_restart_it() {
    let (_dir, mut engine, source) = setup(SnapshotMode::Automatic);
    let mut settings = engine.snapshot().settings;
    settings.snapshots.interval_seconds = 600;
    settings.snapshots.stable_seconds = 10;
    engine
        .apply(Command::SaveSettings(Box::new(settings)), 1003)
        .unwrap();
    std::fs::write(&source, "first edit").unwrap();
    engine
        .apply(Command::Scan { offline: false }, 1010)
        .unwrap();
    engine
        .apply(Command::Scan { offline: false }, 1019)
        .unwrap();
    assert_eq!(engine.snapshot().versions.len(), 1);
    std::fs::write(&source, "second edit").unwrap();
    engine
        .apply(Command::Scan { offline: false }, 1020)
        .unwrap();
    engine
        .apply(Command::Scan { offline: false }, 1029)
        .unwrap();
    assert_eq!(engine.snapshot().versions.len(), 1);
    engine
        .apply(Command::Scan { offline: false }, 1030)
        .unwrap();
    assert_eq!(engine.snapshot().versions.len(), 2);
    assert_eq!(engine.snapshot().versions[1].content, "second edit");
    engine
        .apply(Command::Scan { offline: false }, 2000)
        .unwrap();
    assert_eq!(engine.snapshot().versions.len(), 2);
}

#[test]
fn preview_and_selected_capture_preserve_source_and_unselected_history() {
    let (dir, mut engine, source) = setup(SnapshotMode::Manual);
    let second = dir.join("second.md");
    std::fs::write(&second, "second baseline").unwrap();
    engine
        .apply(Command::Import(vec![second.clone()]), 1003)
        .unwrap();
    std::fs::write(&source, "changed first\r\n").unwrap();
    std::fs::write(&second, "changed second").unwrap();
    engine.apply(Command::PreviewSnapshots, 1004).unwrap();
    let snapshot = engine.snapshot();
    assert_eq!(snapshot.versions.len(), 2);
    assert_eq!(snapshot.snapshot_changes.len(), 2);
    let first = snapshot
        .snapshot_changes
        .iter()
        .find(|c| c.current == "changed first\r\n")
        .unwrap();
    assert!(first.diff.contains("+changed first"));
    assert_eq!(first.previous, "# Baseline\nold");
    let selected = studyapp_core::SnapshotSelection {
        note_id: first.note_id.clone(),
        expected_hash: first.expected_hash.clone(),
    };
    engine
        .apply(
            Command::CaptureSelectedSnapshots(vec![selected.clone()]),
            1005,
        )
        .unwrap();
    let saved = engine.snapshot();
    assert_eq!(saved.versions.len(), 3);
    assert_eq!(saved.versions[0].id, snapshot.versions[0].id);
    assert_eq!(saved.versions[1].id, snapshot.versions[1].id);
    assert_eq!(saved.versions[2].content, "changed first\r\n");
    assert_eq!(saved.snapshot_changes.len(), 1);
    assert_eq!(saved.snapshot_changes[0].current, "changed second");
    assert_eq!(std::fs::read(&source).unwrap(), b"changed first\r\n");
    assert_eq!(std::fs::read(&second).unwrap(), b"changed second");
    engine
        .apply(Command::CaptureSelectedSnapshots(vec![selected]), 1006)
        .unwrap();
    assert_eq!(engine.snapshot().versions.len(), 3);
}

#[test]
fn stale_selection_rejects_the_entire_batch_without_partial_history() {
    let (dir, mut engine, source) = setup(SnapshotMode::Manual);
    let second = dir.join("second.md");
    std::fs::write(&second, "baseline").unwrap();
    engine
        .apply(Command::Import(vec![second.clone()]), 1003)
        .unwrap();
    std::fs::write(&source, "preview one").unwrap();
    std::fs::write(&second, "preview two").unwrap();
    engine.apply(Command::PreviewSnapshots, 1004).unwrap();
    let snapshot = engine.snapshot();
    let selected = snapshot
        .snapshot_changes
        .iter()
        .map(|change| studyapp_core::SnapshotSelection {
            note_id: change.note_id.clone(),
            expected_hash: change.expected_hash.clone(),
        })
        .collect();
    // The second selection is stale: validating/saving the first must not commit it.
    std::fs::write(&second, "unseen new text").unwrap();
    assert!(
        engine
            .apply(Command::CaptureSelectedSnapshots(selected), 1005)
            .is_err()
    );
    assert_eq!(engine.snapshot().versions.len(), snapshot.versions.len());
    assert_eq!(engine.snapshot().snapshot_changes.len(), 2);
    assert_eq!(std::fs::read_to_string(source).unwrap(), "preview one");
    assert_eq!(std::fs::read_to_string(second).unwrap(), "unseen new text");
}

#[test]
fn final_capture_obeys_mode_and_reports_unreadable_sources() {
    for mode in SnapshotMode::ALL {
        let (_dir, mut engine, source) = setup(mode);
        std::fs::write(&source, "final edit").unwrap();
        engine.finish_snapshots(1003).unwrap();
        assert_eq!(
            engine.snapshot().versions.len(),
            if mode == SnapshotMode::Automatic {
                2
            } else {
                1
            }
        );
        std::fs::remove_file(&source).unwrap();
        assert_eq!(
            engine.finish_snapshots(1004).is_err(),
            mode == SnapshotMode::Automatic
        );
    }
}

#[test]
fn manual_current_body_survives_backup_restore_and_exports_without_a_new_snapshot() {
    let (dir, mut engine, _) = setup(SnapshotMode::Manual);
    engine
        .apply(
            Command::CreateNote {
                title: "managed".into(),
                content: "baseline".into(),
            },
            1003,
        )
        .unwrap();
    let before = engine.snapshot();
    let note = before.notes.last().unwrap();
    let expected_hash = before.note_body(&note.id).unwrap().1.to_owned();
    engine
        .apply(
            Command::SaveNote {
                note_id: note.id.clone(),
                expected_hash,
                content: "durable body\r\n".into(),
            },
            1004,
        )
        .unwrap();
    let backup = dir.join("manual-backup.sqlite");
    engine.apply(Command::Backup(backup.clone()), 1005).unwrap();
    let export = dir.join("current-export.md");
    engine
        .apply(
            Command::ExportNote {
                note_id: note.id.clone(),
                version_id: None,
                path: export.clone(),
            },
            1006,
        )
        .unwrap();
    assert_eq!(std::fs::read(export).unwrap(), b"durable body\r\n");
    drop(engine);
    let mut engine = Engine::open(&dir.join("library.sqlite"), 2000).unwrap();
    engine.apply(Command::Restore(backup), 2001).unwrap();
    assert_eq!(engine.snapshot().versions.len(), before.versions.len());
    assert_eq!(
        engine.snapshot().note_body(&note.id).unwrap().0,
        "durable body\r\n"
    );
    assert_eq!(std::fs::read(&note.path).unwrap(), b"durable body\r\n");
}

#[test]
fn failed_finalization_is_reported_durably_until_a_successful_retry() {
    let (dir, mut engine, source) = setup(SnapshotMode::Automatic);
    let retained = source.with_extension("retained");
    std::fs::rename(&source, &retained).unwrap();
    assert!(engine.finish_snapshots(1003).is_err());
    assert!(engine.snapshot_warning().is_some());
    drop(engine);
    let mut engine = Engine::open(&dir.join("library.sqlite"), 2000).unwrap();
    assert!(engine.snapshot_warning().is_some());
    std::fs::rename(retained, source).unwrap();
    engine.finish_snapshots(2001).unwrap();
    assert!(engine.snapshot_warning().is_none());
    drop(engine);
    let engine = Engine::open(&dir.join("library.sqlite"), 3000).unwrap();
    assert!(engine.snapshot_warning().is_none());
}

#[test]
fn a_complete_explicit_retry_clears_the_durable_finalization_warning() {
    let (dir, mut engine, source) = setup(SnapshotMode::Automatic);
    let retained = source.with_extension("retained");
    std::fs::rename(&source, &retained).unwrap();
    assert!(engine.finish_snapshots(1003).is_err());
    std::fs::rename(retained, source).unwrap();
    engine.apply(Command::CaptureSnapshots, 1004).unwrap();
    assert!(engine.snapshot_warning().is_none());
    drop(engine);
    let engine = Engine::open(&dir.join("library.sqlite"), 2000).unwrap();
    assert!(engine.snapshot_warning().is_none());
}

#[test]
fn saving_back_to_the_last_snapshot_clears_only_the_pending_body() {
    let (_dir, mut engine, _) = setup(SnapshotMode::Manual);
    engine
        .apply(
            Command::CreateNote {
                title: "managed".into(),
                content: "baseline".into(),
            },
            1003,
        )
        .unwrap();
    let snapshot = engine.snapshot();
    let note = snapshot.notes.last().unwrap();
    for (now, content) in [(1004, "temporary change"), (1005, "baseline")] {
        let expected_hash = engine.snapshot().note_body(&note.id).unwrap().1.to_owned();
        engine
            .apply(
                Command::SaveNote {
                    note_id: note.id.clone(),
                    expected_hash,
                    content: content.into(),
                },
                now,
            )
            .unwrap();
    }
    assert_eq!(engine.snapshot().versions.len(), snapshot.versions.len());
    assert!(!engine.snapshot().saved_bodies.contains_key(&note.id));
    assert_eq!(engine.snapshot().pending_snapshot_count, 0);
    assert_eq!(engine.snapshot().note_body(&note.id).unwrap().0, "baseline");
    assert_eq!(std::fs::read_to_string(&note.path).unwrap(), "baseline");
}
