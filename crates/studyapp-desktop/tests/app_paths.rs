use std::path::{Path, PathBuf};
use studyapp_desktop::app_paths::{AppDirectories, AppPaths};
fn fixture() -> (PathBuf, AppDirectories, PathBuf) {
    let root = tempfile::tempdir().unwrap().keep();
    let dirs = AppDirectories {
        data: root.join("data"),
        config: root.join("config"),
        local_data: root.join("local"),
        cache: root.join("cache"),
        updates: root.join("updates"),
    };
    std::fs::create_dir_all(&dirs.config).unwrap();
    let locator = dirs.config.join("storage-location.json");
    (root, dirs, locator)
}
#[test]
fn override_matrix_and_library_precedence() {
    let (root, dirs, locator) = fixture();
    let custom = root.join("custom");
    let library = root.join("chosen");
    std::fs::write(
        &locator,
        serde_json::to_vec(&serde_json::json!({"schema":1,"root":custom,"library":library}))
            .unwrap(),
    )
    .unwrap();
    std::fs::write(
        dirs.config.join("location.json"),
        serde_json::to_vec(&root.join("legacy")).unwrap(),
    )
    .unwrap();
    let paths = AppPaths::from_defaults(dirs, locator).unwrap();
    assert_eq!(paths.effective.cache, custom.join("cache"));
    assert_eq!(paths.effective.data, custom.join("library"));
    assert_eq!(paths.resolve_library(None).unwrap(), library);
    assert_eq!(
        paths.resolve_library(Some(&root.join("explicit"))).unwrap(),
        root.join("explicit")
    );
}
#[test]
fn corrupt_override_never_falls_back() {
    let (_, dirs, locator) = fixture();
    std::fs::write(&locator, b"{broken").unwrap();
    assert!(AppPaths::from_defaults(dirs, locator).is_err());
}
#[test]
fn relative_and_parent_roots_rejected() {
    for path in [Path::new("relative").to_owned(), fixture().0.join("x/../y")] {
        let (_, dirs, locator) = fixture();
        std::fs::write(
            &locator,
            serde_json::to_vec(&serde_json::json!({"schema":1,"root":path})).unwrap(),
        )
        .unwrap();
        assert!(AppPaths::from_defaults(dirs, locator).is_err());
    }
}
#[test]
fn legacy_and_default_resolution_and_update_pointer_fallback() {
    let (root, dirs, locator) = fixture();
    let paths = AppPaths::from_defaults(dirs.clone(), locator.clone()).unwrap();
    assert_eq!(paths.resolve_library(None).unwrap(), dirs.data);
    let legacy = root.join("legacy");
    std::fs::write(
        dirs.config.join("location.json"),
        serde_json::to_vec(&legacy).unwrap(),
    )
    .unwrap();
    assert_eq!(paths.resolve_library(None).unwrap(), legacy);
    let custom = root.join("custom");
    std::fs::write(
        &locator,
        serde_json::to_vec(&serde_json::json!({"schema":1,"root":custom})).unwrap(),
    )
    .unwrap();
    std::fs::create_dir_all(&dirs.local_data).unwrap();
    let old = dirs.local_data.join("update-last.txt");
    std::fs::write(&old, b"active transaction identity").unwrap();
    let paths = AppPaths::from_defaults(dirs, locator).unwrap();
    assert_eq!(paths.update_pointer().unwrap(), old);
    assert_eq!(std::fs::read(&old).unwrap(), b"active transaction identity");
}

#[test]
fn custom_library_fallback_and_invalid_library_are_strict() {
    let (root, dirs, locator) = fixture();
    let custom = root.join("custom");
    std::fs::write(
        &locator,
        serde_json::to_vec(&serde_json::json!({"schema":1,"root":custom})).unwrap(),
    )
    .unwrap();
    let paths = AppPaths::from_defaults(dirs.clone(), locator.clone()).unwrap();
    assert_eq!(paths.resolve_library(None).unwrap(), custom.join("library"));
    std::fs::write(
        &locator,
        serde_json::to_vec(&serde_json::json!({"schema":1,"root":custom,"library":"relative"}))
            .unwrap(),
    )
    .unwrap();
    assert!(AppPaths::from_defaults(dirs, locator).is_err());
}

#[test]
fn new_transaction_never_overwrites_legacy_recovery_evidence() {
    let (root, dirs, locator) = fixture();
    let custom = root.join("custom");
    std::fs::write(
        &locator,
        serde_json::to_vec(&serde_json::json!({"schema":1,"root":custom})).unwrap(),
    )
    .unwrap();
    std::fs::create_dir_all(&dirs.local_data).unwrap();
    let old = dirs.local_data.join("update-last.txt");
    std::fs::write(&old, b"active legacy transaction").unwrap();
    let paths = AppPaths::from_defaults(dirs, locator).unwrap();
    assert!(
        paths
            .publish_update_pointer(&root.join("new-stage"))
            .is_err()
    );
    assert_eq!(std::fs::read(old).unwrap(), b"active legacy transaction");
    assert!(!paths.effective.local_data.join("update-last.txt").exists());
}
#[test]
fn new_transaction_writes_effective_local_data_and_preserves_existing_pointer() {
    let (root, dirs, locator) = fixture();
    std::fs::write(
        &locator,
        serde_json::to_vec(&serde_json::json!({"schema":1,"root":root.join("custom")})).unwrap(),
    )
    .unwrap();
    let paths = AppPaths::from_defaults(dirs, locator).unwrap();
    let stage = root.join("new-stage");
    paths.publish_update_pointer(&stage).unwrap();
    assert_eq!(
        paths.update_pointer().unwrap(),
        paths.effective.local_data.join("update-last.txt")
    );
    assert!(
        paths
            .publish_update_pointer(&root.join("another-stage"))
            .is_err()
    );
    assert_eq!(
        std::fs::read(paths.update_pointer().unwrap()).unwrap(),
        stage.to_string_lossy().as_bytes()
    );
}

fn finish_stage(stage: &Path, state: &str) {
    std::fs::create_dir_all(stage).unwrap();
    std::fs::write(
        stage.join("transaction.json"),
        br#"{"schema":1,"id":"fixture-transaction"}"#,
    )
    .unwrap();
    std::fs::write(
        stage.join(format!("status-{state}-fixture.json")),
        serde_json::to_vec(&serde_json::json!({"state":state,"message":"fixture"})).unwrap(),
    )
    .unwrap();
}
#[test]
fn consecutive_updates_replace_terminal_pointer_but_keep_old_evidence() {
    let (root, dirs, locator) = fixture();
    let paths = AppPaths::from_defaults(dirs, locator).unwrap();
    let first = root.join("first");
    paths.publish_update_pointer(&first).unwrap();
    finish_stage(&first, "healthy");
    let second = root.join("second");
    paths.publish_update_pointer(&second).unwrap();
    assert_eq!(
        std::fs::read(paths.update_pointer().unwrap()).unwrap(),
        second.to_string_lossy().as_bytes()
    );
    assert!(first.join("transaction.json").exists());
}
#[test]
fn terminal_legacy_pointer_is_retained_after_root_change() {
    let (root, dirs, locator) = fixture();
    std::fs::create_dir_all(&dirs.local_data).unwrap();
    let legacy_stage = root.join("legacy-stage");
    finish_stage(&legacy_stage, "recovered");
    let old = dirs.local_data.join("update-last.txt");
    std::fs::write(&old, legacy_stage.to_string_lossy().as_bytes()).unwrap();
    std::fs::write(
        &locator,
        serde_json::to_vec(&serde_json::json!({"schema":1,"root":root.join("custom")})).unwrap(),
    )
    .unwrap();
    let paths = AppPaths::from_defaults(dirs, locator).unwrap();
    paths.publish_update_pointer(&root.join("new")).unwrap();
    assert_eq!(
        std::fs::read(old).unwrap(),
        legacy_stage.to_string_lossy().as_bytes()
    );
    assert!(paths.effective.local_data.join("update-last.txt").exists());
}

#[test]
fn relocation_refuses_failed_or_active_evidence() {
    for state in ["failed", "installing", "healthy"] {
        let (root, dirs, locator) = fixture();
        let paths = AppPaths::from_defaults(dirs, locator).unwrap();
        let stage = root.join("stage");
        paths.publish_update_pointer(&stage).unwrap();
        finish_stage(&stage, state);
        assert_eq!(
            paths.verify_relocation_allowed().is_ok(),
            state == "healthy"
        );
    }
}

#[test]
fn active_legacy_transaction_survives_new_root_preparation() {
    let (root, dirs, locator) = fixture();
    let previous = root.join("active-stage");
    finish_stage(&previous, "installing");
    std::fs::create_dir_all(&dirs.local_data).unwrap();
    let old = dirs.local_data.join("update-last.txt");
    std::fs::write(&old, previous.to_string_lossy().as_bytes()).unwrap();
    std::fs::write(
        &locator,
        serde_json::to_vec(&serde_json::json!({"schema":1,"root":root.join("custom")})).unwrap(),
    )
    .unwrap();
    let paths = AppPaths::from_defaults(dirs, locator).unwrap();
    assert!(
        paths
            .publish_update_pointer(&root.join("new-stage"))
            .is_err()
    );
    assert_eq!(
        std::fs::read(old).unwrap(),
        previous.to_string_lossy().as_bytes()
    );
    assert!(!paths.effective.local_data.join("update-last.txt").exists());
}
