// Test-first acceptance cases staged while another agent owns Cargo.
use super::*;

#[test]
fn empty_legacy_edit_group_uses_note_id_but_recovered_groups_stay_stable() {
    let id = uuid::Uuid::new_v4().to_string();
    let mut legacy = draft("historic");
    legacy.note_id = Some(id.clone());
    legacy.asset_group.clear();
    assert_eq!(DraftRecord::new(legacy.clone(), 1).draft.asset_group, id);
    legacy.asset_group = uuid::Uuid::new_v4().to_string();
    let recovered = legacy.asset_group.clone();
    assert_eq!(DraftRecord::new(legacy, 2).draft.asset_group, recovered);
}

#[test]
fn storage_protection_tracks_active_sources_and_pending_save_targets() {
    let mut first = DraftRecord::new(draft("source"), 1);
    first.draft.note_id = Some("original".into());
    first.save_target = Some("copy-in-flight".into());
    let mut second = first.clone();
    second.id = uuid::Uuid::new_v4().to_string();
    let mut retired = DraftRecord::new(draft("retired"), 1);
    retired.draft.note_id = Some("safe-to-clean".into());
    retired.retire(DraftStatus::Deleted, 2);
    assert_eq!(
        protected_note_ids([&first, &second, &retired]),
        ["original".to_owned(), "copy-in-flight".to_owned()].into()
    );
    first.retire(DraftStatus::Deleted, 3);
    assert!(protected_note_ids([&first, &second]).contains("original"));
    second.retire(
        DraftStatus::Saved {
            note_id: "original".into(),
            hash: "saved".into(),
        },
        4,
    );
    assert!(protected_note_ids([&first, &second, &retired]).is_empty());
}

fn draft(content: &str) -> NoteDraft {
    NoteDraft {
        note_id: None,
        expected_hash: None,
        folder_id: None,
        asset_group: uuid::Uuid::new_v4().to_string(),
        title: "独立草稿".into(),
        content: content.into(),
    }
}

fn library(root: &Path) -> String {
    root.join("studyapp.sqlite3").to_string_lossy().into_owned()
}

fn catalog(store: &DraftStore, library: &str) -> (Vec<DraftRecord>, Vec<String>) {
    store.load(library);
    loop {
        match store.receiver.recv_timeout(Duration::from_secs(5)).unwrap() {
            DraftUpdate::Catalog {
                records, errors, ..
            } => return (records, errors),
            DraftUpdate::Error { message, .. } => panic!("unexpected failure: {message}"),
            _ => {}
        }
    }
}

#[test]
fn interleaved_drafts_restart_with_exact_content_and_independent_identity() {
    let dir = tempfile::tempdir().unwrap().keep();
    let library = library(&dir);
    let a = DraftRecord::new(draft("A\r\n中文🧠\r\n"), 1);
    let mut b = DraftRecord::new(draft("B\n"), 2);
    let c = DraftRecord::new(draft("C\r混合\n"), 3);
    let store = DraftStore::new();
    store.save_record(&library, a.clone());
    store.save_record(&library, b.clone());
    let mut edited = b.draft.clone();
    edited.content = "B最新\n".into();
    b.update(edited, 4);
    store.save_record(&library, b.clone());
    store.save_record(&library, c.clone());
    drop(store);
    let (records, errors) = catalog(&DraftStore::new(), &library);
    assert!(errors.is_empty());
    assert_eq!(records.len(), 3);
    for expected in [a, b, c] {
        assert_eq!(
            records.iter().find(|r| r.id == expected.id),
            Some(&expected)
        );
    }
}

#[test]
fn legacy_migration_is_idempotent_keeps_bytes_and_does_not_resurrect_retired_record() {
    let dir = tempfile::tempdir().unwrap().keep();
    let library = library(&dir);
    let legacy = br#"{"note_id":null,"expected_hash":null,"title":"legacy","content":"raw\r\n"}"#;
    std::fs::write(DraftStore::path(&library), legacy).unwrap();
    let store = DraftStore::new();
    let (mut records, errors) = catalog(&store, &library);
    assert!(errors.is_empty());
    assert_eq!(records.len(), 1);
    let mut record = records.remove(0);
    assert_eq!(record.draft.content, "raw\r\n");
    assert!(!record.draft.asset_group.is_empty());
    record.retire(DraftStatus::Deleted, 9);
    store.persist_record(&library, record.clone(), 31);
    drop(store);
    let (records, errors) = catalog(&DraftStore::new(), &library);
    assert!(errors.is_empty());
    assert_eq!(records, vec![record]);
    assert_eq!(std::fs::read(DraftStore::path(&library)).unwrap(), legacy);
    std::fs::write(
        DraftStore::path(&library),
        br#"{"note_id":null,"expected_hash":null,"title":"new legacy","content":"later"}"#,
    )
    .unwrap();
    assert_eq!(catalog(&DraftStore::new(), &library).0.len(), 2);
}

#[test]
fn terminal_record_rejects_late_active_snapshot_without_erasing_other_draft() {
    let dir = tempfile::tempdir().unwrap().keep();
    let library = library(&dir);
    let a = DraftRecord::new(draft("A"), 1);
    let b = DraftRecord::new(draft("B"), 1);
    let store = DraftStore::new();
    store.save_record(&library, a.clone());
    store.save_record(&library, b.clone());
    let mut deleted = a.clone();
    deleted.retire(DraftStatus::Deleted, 2);
    store.persist_record(&library, deleted.clone(), 7);
    store.save_record(&library, a);
    drop(store);
    let records = catalog(&DraftStore::new(), &library).0;
    assert!(records.contains(&deleted));
    assert!(records.contains(&b));
    assert_eq!(records.len(), 2);
}

#[test]
fn corrupt_record_does_not_hide_valid_records_or_change_bad_bytes() {
    let dir = tempfile::tempdir().unwrap().keep();
    let library = library(&dir);
    let good = DraftRecord::new(draft("safe"), 1);
    let store = DraftStore::new();
    store.save_record(&library, good.clone());
    drop(store);
    let bad_id = uuid::Uuid::new_v4().to_string();
    let path = DraftStore::record_path(&library, &bad_id).unwrap();
    std::fs::write(&path, b"{broken").unwrap();
    let (records, errors) = catalog(&DraftStore::new(), &library);
    assert_eq!(records, vec![good]);
    assert_eq!(errors.len(), 1);
    assert_eq!(std::fs::read(path).unwrap(), b"{broken");
}

#[test]
fn explicit_persist_acknowledges_exact_revision_and_drain_keeps_both_libraries() {
    let dir = tempfile::tempdir().unwrap().keep();
    let first = library(&dir.join("a"));
    let second = library(&dir.join("b"));
    let a = DraftRecord::new(draft("A"), 1);
    let b = DraftRecord::new(draft("B"), 1);
    let store = DraftStore::new();
    store.save_record(&first, a.clone());
    store.persist_record(&second, b.clone(), 73);
    loop {
        if let DraftUpdate::Written {
            library,
            id,
            revision,
            token: Some(73),
        } = store.receiver.recv_timeout(Duration::from_secs(5)).unwrap()
        {
            assert_eq!(library, second);
            assert_eq!(id, b.id);
            assert_eq!(revision, b.revision);
            break;
        }
    }
    store.drain(&first, 74);
    loop {
        if let DraftUpdate::Drained {
            library,
            token: 74,
            result,
        } = store.receiver.recv_timeout(Duration::from_secs(5)).unwrap()
        {
            assert_eq!(library, first);
            result.unwrap();
            break;
        }
    }
    drop(store);
    assert_eq!(catalog(&DraftStore::new(), &first).0, vec![a]);
    assert_eq!(catalog(&DraftStore::new(), &second).0, vec![b]);
}

#[test]
fn invalid_record_identity_cannot_escape_drafts_directory() {
    let dir = tempfile::tempdir().unwrap().keep();
    let library = library(&dir);
    for bad in [
        "../outside",
        "",
        "UPPER",
        "00000000-0000-0000-0000-000000000000/../../x",
    ] {
        assert!(DraftStore::record_path(&library, bad).is_err());
    }
}

#[test]
fn two_new_records_never_replace_legacy_recovery_bytes() {
    let dir = tempfile::tempdir().unwrap().keep();
    let library = library(&dir);
    let original =
        br#"{"note_id":null,"expected_hash":null,"title":"legacy","content":"original"}"#;
    std::fs::write(DraftStore::path(&library), original).unwrap();
    let store = DraftStore::new();
    store.save_record(&library, DraftRecord::new(draft("first\r\nexact 🧠"), 1));
    store.save_record(&library, DraftRecord::new(draft("second\nindependent"), 2));
    drop(store);
    assert_eq!(std::fs::read(DraftStore::path(&library)).unwrap(), original);
    assert_eq!(
        std::fs::read_dir(DraftStore::directory(&library))
            .unwrap()
            .count(),
        2
    );
}

#[test]
fn escaped_large_markdown_and_image_context_survive_restart() {
    let dir = tempfile::tempdir().unwrap().keep();
    let library = library(&dir);
    let mut record = DraftRecord::new(draft(&"\\".repeat(7 * 1024 * 1024)), 1);
    record.source_path = Some(dir.join("external/note.md"));
    record
        .media_bindings
        .insert("图.png".into(), "studyapp-media:stable/pic.png".into());
    record.draft.note_id = Some(uuid::Uuid::new_v4().to_string());
    record.draft.expected_hash = Some("original-source-hash".into());
    record.draft.folder_id = Some("original-folder".into());
    record.save_target = record.draft.note_id.clone();
    let store = DraftStore::new();
    store.save_record(&library, record.clone());
    drop(store);
    let (records, errors) = catalog(&DraftStore::new(), &library);
    assert!(errors.is_empty());
    assert_eq!(records, vec![record]);
}

#[test]
fn oversized_record_is_reported_without_changing_valid_disk_content() {
    let dir = tempfile::tempdir().unwrap().keep();
    let library = library(&dir);
    let mut record = DraftRecord::new(draft("original"), 1);
    let store = DraftStore::new();
    store.persist_record(&library, record.clone(), 1);
    while !matches!(
        store.receiver.recv_timeout(Duration::from_secs(5)).unwrap(),
        DraftUpdate::Written { token: Some(1), .. }
    ) {}
    let path = DraftStore::record_path(&library, &record.id).unwrap();
    let original = std::fs::read(&path).unwrap();
    record.update(draft(&"x".repeat(10 * 1024 * 1024 + 1)), 2);
    store.persist_record(&library, record, 2);
    loop {
        if let DraftUpdate::Error {
            token: Some(2),
            message,
            ..
        } = store.receiver.recv_timeout(Duration::from_secs(5)).unwrap()
        {
            assert!(message.contains("10 MiB"));
            break;
        }
    }
    drop(store);
    assert_eq!(std::fs::read(path).unwrap(), original);
}

#[test]
fn continuous_snapshots_do_not_postpone_durability_until_typing_stops() {
    let dir = tempfile::tempdir().unwrap().keep();
    let library = library(&dir);
    let mut record = DraftRecord::new(draft("0"), 1);
    let store = DraftStore::new();
    let started = Instant::now();
    let mut acknowledged = false;
    while started.elapsed() < Duration::from_secs(3) {
        record.update(draft(&format!("revision {}", record.revision)), 2);
        store.save_record(&library, record.clone());
        if let Ok(DraftUpdate::Written { id, .. }) =
            store.receiver.recv_timeout(Duration::from_millis(40))
        {
            assert_eq!(id, record.id);
            acknowledged = true;
            break;
        }
    }
    assert!(
        acknowledged,
        "continuous input starved the first-write deadline"
    );
}

#[cfg(windows)]
#[test]
fn locked_record_emits_only_exact_failure_and_keeps_other_draft_writable() {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = tempfile::tempdir().unwrap().keep();
    let library = library(&dir);
    let mut a = DraftRecord::new(draft("original"), 1);
    let b = DraftRecord::new(draft("other"), 1);
    let store = DraftStore::new();
    store.persist_record(&library, a.clone(), 11);
    while !matches!(
        store.receiver.recv_timeout(Duration::from_secs(5)).unwrap(),
        DraftUpdate::Written {
            token: Some(11),
            ..
        }
    ) {}
    let path = DraftStore::record_path(&library, &a.id).unwrap();
    let original = std::fs::read(&path).unwrap();
    let locked = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&path)
        .unwrap();
    a.update(draft("latest"), 2);
    store.persist_record(&library, a.clone(), 12);
    store.persist_record(&library, b.clone(), 13);
    let mut failed = false;
    let mut other = false;
    while !failed || !other {
        match store.receiver.recv_timeout(Duration::from_secs(5)).unwrap() {
            DraftUpdate::Error {
                id: Some(id),
                revision,
                token: Some(12),
                ..
            } => {
                assert_eq!(id, a.id);
                assert_eq!(revision, a.revision);
                failed = true;
            }
            DraftUpdate::Written {
                id,
                token: Some(13),
                ..
            } => {
                assert_eq!(id, b.id);
                other = true;
            }
            DraftUpdate::Written {
                token: Some(12), ..
            } => panic!("locked write falsely acknowledged"),
            _ => {}
        }
    }
    assert_eq!(std::fs::read(&path).unwrap(), original);
    drop(locked);
    store.drain(&library, 14);
    while !matches!(
        store.receiver.recv_timeout(Duration::from_secs(5)).unwrap(),
        DraftUpdate::Drained {
            token: 14,
            result: Ok(()),
            ..
        }
    ) {}
    drop(store);
    assert_eq!(read_record(&path).unwrap(), a);
}
