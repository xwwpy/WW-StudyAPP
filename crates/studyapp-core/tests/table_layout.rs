use studyapp_core::{
    Command, Engine, TableAlignment, TableAnchor, TableDefaults, TableSizing, TableView,
};
const DOCUMENT: &str = "36b79f08-5128-43ab-a1cb-13f318c455f7";
const TABLE: &str = "0123456789abcdef0123456789abcdef";
fn value() -> TableView {
    TableView {
        anchor: TableAnchor {
            header: "1".repeat(32),
            content: "2".repeat(32),
            context: "3".repeat(32),
            occurrence: 0,
        },
        sizing: Some(TableSizing::Adaptive),
        widths: Some(vec![2500, 7500]),
        alignments: vec![None, Some(TableAlignment::Center)],
    }
}
fn set(engine: &mut Engine, value: Option<TableView>) -> anyhow::Result<String> {
    engine.apply(
        Command::SetTableView {
            document: DOCUMENT.into(),
            table: TABLE.into(),
            value,
        },
        1001,
    )
}
#[test]
fn table_views_survive_draft_commit_restart_backup_and_restore_without_touching_source() {
    let root = tempfile::tempdir().unwrap().keep();
    let db = root.join("study.sqlite");
    let mut engine = Engine::open(&db, 1000).unwrap();
    assert_eq!(set(&mut engine, Some(value())).unwrap(), "");
    let source = "| A | B |\r\n| --- | ---: |\r\n| 中文 | 🙂 |\r\n";
    engine
        .apply(
            Command::CreateNoteWithId {
                note_id: DOCUMENT.into(),
                title: "table".into(),
                content: source.into(),
                folder_id: None,
            },
            1002,
        )
        .unwrap();
    let before = engine.snapshot();
    let path = before.notes[0].path.clone();
    let backup = root.join("backup.sqlite");
    engine.apply(Command::Backup(backup.clone()), 1003).unwrap();
    set(&mut engine, None).unwrap();
    assert!(engine.snapshot().table_views.is_empty());
    engine.apply(Command::Restore(backup), 1004).unwrap();
    assert_eq!(engine.snapshot().table_views[DOCUMENT][TABLE], value());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), source);
    assert_eq!(engine.snapshot().versions.len(), before.versions.len());
    drop(engine);
    let mut engine = Engine::open(&db, 2000).unwrap();
    assert_eq!(engine.snapshot().table_views[DOCUMENT][TABLE], value());
    let note = engine.snapshot().notes[0].clone();
    engine
        .purge_managed_note(&note.id, note.revision, 2001)
        .unwrap();
    assert!(engine.snapshot().table_views.is_empty());
}
#[test]
fn invalid_table_metadata_is_rejected_atomically_and_does_not_replace_other_settings() {
    let root = tempfile::tempdir().unwrap().keep();
    let mut engine = Engine::open(&root.join("study.sqlite"), 1000).unwrap();
    set(&mut engine, Some(value())).unwrap();
    let before = serde_json::to_value(engine.snapshot()).unwrap();
    let mut invalid = vec![];
    let mut v = value();
    v.widths = Some(vec![0, 10000]);
    invalid.push(v);
    let mut v = value();
    v.widths = Some(vec![10000, 10000]);
    invalid.push(v);
    let mut v = value();
    v.alignments = vec![None; 65];
    invalid.push(v);
    let mut v = value();
    v.anchor.header = "<script>".into();
    invalid.push(v);
    let mut v = value();
    v.anchor.occurrence = 1000001;
    invalid.push(v);
    for v in invalid {
        assert!(set(&mut engine, Some(v)).is_err());
        assert_eq!(serde_json::to_value(engine.snapshot()).unwrap(), before);
    }
    assert!(
        engine
            .apply(
                Command::SetTableView {
                    document: "../foreign".into(),
                    table: TABLE.into(),
                    value: Some(value())
                },
                1001
            )
            .is_err()
    );
    assert_eq!(serde_json::to_value(engine.snapshot()).unwrap(), before);
    let mut settings = engine.snapshot().settings;
    settings.appearance.table_defaults = TableDefaults {
        sizing: TableSizing::Adaptive,
        alignment: TableAlignment::Right,
    };
    engine
        .apply(Command::SaveSettings(Box::new(settings.clone())), 1001)
        .unwrap();
    set(&mut engine, Some(value())).unwrap();
    assert_eq!(
        engine.snapshot().settings.appearance.table_defaults,
        settings.appearance.table_defaults
    );
}
#[test]
fn legacy_state_defaults_and_per_document_limits_are_safe() {
    let root = tempfile::tempdir().unwrap().keep();
    let db = root.join("study.sqlite");
    drop(Engine::open(&db, 1000).unwrap());
    let conn = rusqlite::Connection::open(&db).unwrap();
    let raw: String = conn
        .query_row("SELECT data FROM app_state", [], |r| r.get(0))
        .unwrap();
    let mut old: serde_json::Value = serde_json::from_str(&raw).unwrap();
    old.as_object_mut().unwrap().remove("table_views");
    old["settings"]["appearance"]
        .as_object_mut()
        .unwrap()
        .remove("table_defaults");
    conn.execute("UPDATE app_state SET data=?1", [old.to_string()])
        .unwrap();
    drop(conn);
    let mut engine = Engine::open(&db, 1000).unwrap();
    assert!(engine.snapshot().table_views.is_empty());
    assert_eq!(
        engine.snapshot().settings.appearance.table_defaults,
        TableDefaults::default()
    );
    for i in 0..128 {
        engine
            .apply(
                Command::SetTableView {
                    document: DOCUMENT.into(),
                    table: format!("{i:032x}"),
                    value: Some(value()),
                },
                1001,
            )
            .unwrap();
    }
    let before = engine.snapshot().table_views;
    assert!(
        engine
            .apply(
                Command::SetTableView {
                    document: DOCUMENT.into(),
                    table: format!("{:032x}", 128),
                    value: Some(value())
                },
                1001
            )
            .is_err()
    );
    assert_eq!(engine.snapshot().table_views, before);
    engine
        .apply(
            Command::SetTableView {
                document: DOCUMENT.into(),
                table: format!("{:032x}", 0),
                value: None,
            },
            1001,
        )
        .unwrap();
    engine
        .apply(
            Command::SetTableView {
                document: DOCUMENT.into(),
                table: format!("{:032x}", 128),
                value: Some(value()),
            },
            1001,
        )
        .unwrap();
}
