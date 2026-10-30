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
