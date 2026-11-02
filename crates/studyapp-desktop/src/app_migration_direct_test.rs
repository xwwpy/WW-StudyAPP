use super::*;

// Keep the direct application migration exercised by the original Windows
// regression, independently of the additional legacy-library-copy scenario.
#[test]
fn direct_migration_purge_preserves_source_image_and_index_hardlinks() {
    let (base, paths, mut engine) = fixture();
    let id = uuid::Uuid::new_v4().to_string();
    engine
        .apply(
            Command::CreateNoteWithId {
                note_id: id.clone(),
                title: "image".into(),
                content: format!("![asset](studyapp-media:{id}/one.gif)"),
                folder_id: None,
            },
            1010,
        )
        .unwrap();
    let image = Path::new("images").join(&id).join("one.gif");
    let index = Path::new("image-index").join(&id).join("one.gif.json");
    let source = &paths.effective.data;
    let pending = format!(".studyapp-image-{}.pending", uuid::Uuid::new_v4());
    for rel in [&image, &index] {
        let path = source.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let content = if *rel == image {
            b"gif".to_vec()
        } else {
            serde_json::to_vec(&source.join(&image)).unwrap()
        };
        fs::write(&path, content).unwrap();
        fs::hard_link(&path, path.with_file_name(&pending)).unwrap();
    }
    let target = base.join("new");
    let mut migration = prepare(&mut engine, paths.clone(), &target, 1011, |_| {}).unwrap();
    migration.commit().unwrap();
    let mut next = Engine::open(&migration.database, 1012).unwrap();
    for rel in [&image, &index] {
        let path = target.join("library").join(rel);
        let identity = |p: &Path| crate::app_usage::identity(&File::open(p).unwrap()).unwrap();
        assert_eq!(identity(&path), identity(&path.with_file_name(&pending)));
        assert_ne!(identity(&path), identity(&source.join(rel)));
    }
    let plan = crate::storage_backend::purge::prepare(&next.snapshot(), &id).unwrap();
    assert!(
        !plan
            .files
            .keys()
            .any(|path| path.starts_with(fs::canonicalize(source).unwrap())),
        "A migrated-library purge must never claim a retained source file"
    );
    crate::storage_backend::purge::execute(&mut next, &plan, 1013).unwrap();
    for rel in [&image, &index] {
        let path = target.join("library").join(rel);
        assert!(!path.exists() && !path.with_file_name(&pending).exists());
        assert!(source.join(rel).exists(), "Source file removed: {rel:?}");
        assert!(
            source.join(rel).with_file_name(&pending).exists(),
            "Source hardlink removed: {rel:?}"
        );
    }
}
