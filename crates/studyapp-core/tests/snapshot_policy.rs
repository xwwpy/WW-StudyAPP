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
fn explicit_editor_saves_remain_durable_in_manual_mode() {
    let (_dir, mut engine, _) = setup(SnapshotMode::Manual);
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
    let note = snap.notes.last().unwrap();
    let version = snap.versions.last().unwrap();
    engine
        .apply(
            Command::SaveNote {
                note_id: note.id.clone(),
                expected_hash: version.hash.clone(),
                content: "saved".into(),
            },
            1004,
        )
        .unwrap();
    assert_eq!(engine.snapshot().versions.last().unwrap().content, "saved");
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
