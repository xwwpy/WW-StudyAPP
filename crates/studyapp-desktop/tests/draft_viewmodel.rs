use studyapp_desktop::draft_viewmodel::{DraftLibraryIndex, DraftRow};

fn row<'a>(id: &'a str, revision: u64, updated_at: i64, content: &'a str) -> DraftRow<'a> {
    DraftRow {
        id,
        revision,
        updated_at,
        active: true,
        title: id,
        content,
        note_id: None,
        folder_id: None,
    }
}

#[test]
fn shared_draft_order_changes_identity_only_after_real_catalog_or_query_change() {
    use std::sync::Arc;
    let mut model = DraftLibraryIndex::default();
    model.refresh([row("a", 1, 1, "Alpha")]);
    let first = model.shared_ids();
    model.refresh([row("a", 1, 1, "Alpha")]);
    assert!(Arc::ptr_eq(&first, &model.shared_ids()));
    model.refresh([row("a", 2, 2, "Beta")]);
    let edited = model.shared_ids();
    assert!(!Arc::ptr_eq(&first, &edited));
    model.set_query("missing");
    model.refresh([row("a", 2, 2, "Beta")]);
    assert!(!Arc::ptr_eq(&edited, &model.shared_ids()));
    assert!(model.ids().is_empty());
}

#[test]
fn empty_query_never_searches_long_draft_bodies_and_query_runs_once_per_revision() {
    let body = format!("{}Needle", "正文".repeat(1_000_000));
    let mut model = DraftLibraryIndex::default();
    model.refresh([row("a", 1, 1, &body)]);
    assert_eq!(model.ids(), &["a"]);
    assert_eq!(model.body_bytes_searched, 0);
    assert!(model.set_query("needle"));
    let start = std::time::Instant::now();
    model.refresh([row("a", 1, 1, &body)]);
    let search = start.elapsed();
    assert_eq!(model.ids(), &["a"]);
    assert_eq!(model.body_bytes_searched, body.len());
    let start = std::time::Instant::now();
    for _ in 0..1000 {
        assert_eq!(model.ids()[0], "a");
    }
    eprintln!(
        "6 MiB draft: actual search {search:?}; 1,000 display reads {:?}",
        start.elapsed()
    );
    model.refresh([row("a", 1, 1, &body)]);
    assert_eq!(model.body_bytes_searched, body.len());
    assert!(
        !model.set_query("NEEDLE"),
        "Focus/caret notifications must not invalidate a normalized query"
    );
    model.refresh([row("a", 2, 2, "edited without match")]);
    assert!(model.ids().is_empty());
}

#[test]
fn draft_order_status_moves_query_and_removal_refresh_without_retaining_bodies() {
    let mut model = DraftLibraryIndex::default();
    model.refresh([row("a", 1, 10, "Alpha"), row("b", 1, 20, "Beta")]);
    assert_eq!(model.ids(), &["b", "a"]);
    assert_eq!(model.active_count(), 2);
    model.set_query("ALPHA");
    model.refresh([row("a", 1, 10, "Alpha"), row("b", 1, 20, "Beta")]);
    assert_eq!(model.ids(), &["a"]);
    let mut moved = row("a", 2, 30, "Alpha");
    moved.folder_id = Some("new-folder");
    moved.note_id = Some("saved-note");
    model.refresh([moved, row("b", 1, 20, "Beta")]);
    assert_eq!(model.ids(), &["a"]);
    let mut retired = row("a", 3, 40, "Alpha");
    retired.active = false;
    model.refresh([retired, row("b", 1, 20, "Beta")]);
    assert_eq!(model.active_count(), 1);
    assert!(model.ids().is_empty());
    model.set_query("");
    model.refresh([row("b", 1, 20, "Beta")]);
    assert_eq!(model.ids(), &["b"]);
    model.refresh([]);
    assert!(model.ids().is_empty());
    assert_eq!(model.active_count(), 0);
}
