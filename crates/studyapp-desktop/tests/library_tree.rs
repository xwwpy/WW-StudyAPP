use studyapp_core::LibrarySort;
use studyapp_desktop::library_tree::{StudyStatus, compare_entries};
#[test]
fn status_is_based_on_confirmed_version_hash_not_latest_capture() {
    assert_eq!(
        StudyStatus::from_hashes(Some("same"), Some("same")),
        StudyStatus::Recorded
    );
    assert_eq!(
        StudyStatus::from_hashes(Some("later"), Some("same")),
        StudyStatus::Modified
    );
    assert_eq!(
        StudyStatus::from_hashes(Some("first"), None),
        StudyStatus::Added
    );
}
#[test]
fn equal_sort_values_keep_identity_ties_stable_in_both_directions() {
    use std::cmp::Ordering;
    for sort in [
        LibrarySort::NameAscending,
        LibrarySort::NameDescending,
        LibrarySort::Newest,
        LibrarySort::Oldest,
    ] {
        assert_eq!(
            compare_entries(sort, ("Same", 10, "a"), ("same", 10, "b")),
            Ordering::Less
        );
    }
}
use std::sync::Arc;
use studyapp_core::{Note, NoteFolder, NoteVersion, Session, Settings, Snapshot, TimerState};
use studyapp_desktop::library_tree::{LibraryIndex, source_label};
fn note(id: &str, folder: Option<&str>, managed: bool) -> Note {
    Note {
        id: id.into(),
        path: format!("/{id}.md"),
        title: id.into(),
        folder_id: folder.map(str::to_owned),
        title_override: false,
        managed,
        detached: false,
        missing: false,
        error: None,
        created_at: 1,
        updated_at: 2,
        revision: 1,
    }
}
fn version(id: &str, hash: &str) -> NoteVersion {
    NoteVersion {
        id: id.into(),
        note_id: "note".into(),
        content: format!("content-{id}"),
        hash: hash.into(),
        captured_at: 2,
        offline: false,
        summary: String::new(),
        diff: String::new(),
    }
}
fn snapshot() -> Snapshot {
    Snapshot {
        settings: Settings::default(),
        notes: vec![note("note", Some("child"), false)],
        folders: vec![
            NoteFolder {
                id: "root".into(),
                name: "Root".into(),
                parent_id: None,
            },
            NoteFolder {
                id: "child".into(),
                name: "Child".into(),
                parent_id: Some("root".into()),
            },
        ],
        versions: Arc::new(vec![version("v1", "same")]),
        sessions: vec![],
        reviews: vec![],
        review_events: vec![],
        timer: TimerState {
            paused: true,
            active_seconds: 0,
            target_seconds: 60,
            started_at: 0,
            spans: vec![],
        },
        data_path: "library".into(),
    }
}
fn learned(ids: &[&str]) -> Session {
    Session {
        id: "session".into(),
        started_at: 0,
        ended_at: 10,
        active_seconds: 10,
        deadline: 10,
        status: studyapp_core::SessionStatus::Learned,
        summary: String::new(),
        suggested_summary: String::new(),
        note_ids: vec!["note".into()],
        version_ids: ids.iter().map(|s| s.to_string()).collect(),
        spans: vec![],
        updated_at: 10,
        revision: 1,
    }
}

#[test]
fn prepared_folder_relationships_reject_cycles_current_parent_and_subtree_overflow() {
    let mut snapshot = snapshot();
    snapshot.folders = (0..8)
        .map(|depth| NoteFolder {
            id: depth.to_string(),
            name: format!("Depth {depth}"),
            parent_id: (depth > 0).then(|| (depth - 1).to_string()),
        })
        .chain([NoteFolder {
            id: "other".into(),
            name: "Other".into(),
            parent_id: None,
        }])
        .collect();
    let index = LibraryIndex::build(&snapshot);
    for (moving, target) in [
        ("0", Some("0")),
        ("0", Some("7")),
        ("1", Some("0")),
        ("other", Some("7")),
        ("other", Some("gone")),
        ("gone", None),
    ] {
        assert!(
            !index.can_move_folder(moving, target),
            "invalid move {moving} -> {target:?}"
        );
    }
    assert!(index.can_move_folder("other", Some("6")));
    assert!(
        !index.can_move_folder("0", Some("other")),
        "The entire subtree must fit"
    );
    assert!(
        index.can_move_folder("1", Some("other")),
        "Exactly eight levels remain valid"
    );
    assert!(index.can_move_folder("2", Some("other")));
    assert!(index.can_move_folder("1", None));
    snapshot.folders[1].parent_id = Some("other".into());
    assert!(!LibraryIndex::build(&snapshot).can_move_folder("1", Some("other")));
    snapshot.notes.reverse();
    let index = LibraryIndex::build(&snapshot);
    assert_eq!(
        snapshot.notes[index.note("note").unwrap().position].id,
        "note"
    );
}
#[test]
fn snapshot_index_tracks_confirmed_hash_and_source_independently() {
    let mut snapshot = snapshot();
    assert_eq!(
        LibraryIndex::build(&snapshot).note("note").unwrap().status,
        StudyStatus::Added
    );
    let mut pending = learned(&["v1"]);
    pending.status = studyapp_core::SessionStatus::Pending;
    snapshot.sessions.push(pending);
    assert_eq!(
        LibraryIndex::build(&snapshot).note("note").unwrap().status,
        StudyStatus::Added
    );
    snapshot.sessions.clear();
    snapshot.sessions.push(learned(&["v1"]));
    assert_eq!(
        LibraryIndex::build(&snapshot).note("note").unwrap().status,
        StudyStatus::Recorded
    );
    snapshot.versions = Arc::new(vec![version("v1", "same"), version("v2", "changed")]);
    assert_eq!(
        LibraryIndex::build(&snapshot).note("note").unwrap().status,
        StudyStatus::Modified
    );
    snapshot.sessions.push(learned(&["v2"]));
    assert_eq!(
        LibraryIndex::build(&snapshot).note("note").unwrap().status,
        StudyStatus::Recorded
    );
    snapshot.versions = Arc::new(vec![
        version("v1", "same"),
        version("v2", "changed"),
        version("v3", "changed"),
    ]);
    assert_eq!(
        LibraryIndex::build(&snapshot).note("note").unwrap().status,
        StudyStatus::Recorded
    );
    assert_eq!(source_label(&snapshot.notes[0]), "外部跟踪 · 只读");
    snapshot.notes[0].managed = true;
    assert_eq!(source_label(&snapshot.notes[0]), "应用管理副本");
}
#[test]
fn timer_and_pointer_reads_never_reindex_version_bodies() {
    let mut snapshot = snapshot();
    let index = LibraryIndex::build(&snapshot);
    assert_eq!(index.content_bodies_indexed, 1);
    snapshot.timer.active_seconds += 1;
    assert!(!index.needs_update(&snapshot));
    for _ in 0..1000 {
        assert!(index.matches("note", "content-v1"));
        assert_eq!(index.note("note").unwrap().version_count, 1);
    }
    assert_eq!(index.content_bodies_indexed, 1);
    assert_eq!(index.folder_updated("root"), 2);
    assert_eq!(index.folder_updated("child"), 2);
    snapshot.notes[0].title = "Changed title".into();
    assert!(index.needs_update(&snapshot));
}

#[test]
fn search_retains_matching_note_ancestors_and_prunes_unrelated_folders() {
    let mut snapshot = snapshot();
    snapshot.folders.push(NoteFolder {
        id: "other".into(),
        name: "Unrelated".into(),
        parent_id: None,
    });
    let index = LibraryIndex::build(&snapshot);
    let ancestors = index.matching_folders("content-v1");
    assert!(ancestors.contains("root"));
    assert!(ancestors.contains("child"));
    assert!(!ancestors.contains("other"));
    assert!(index.matching_folders("no match").is_empty());
}

#[test]
fn collapsed_ancestors_skip_note_construction_but_search_reveals_matches() {
    let snapshot = snapshot();
    let index = LibraryIndex::build(&snapshot);
    let collapsed = [String::from("root")].into_iter().collect();
    assert!(!index.note_visible("note", &collapsed, ""));
    assert!(index.note_visible("note", &collapsed, "content-v1"));
    assert!(index.note_visible("note", &Default::default(), ""));
}
#[test]
fn never_learned_changes_use_original_hash_without_implying_recorded() {
    let mut snapshot = snapshot();
    assert_eq!(
        LibraryIndex::build(&snapshot).note("note").unwrap().status,
        StudyStatus::Added
    );
    snapshot.versions = Arc::new(vec![version("v1", "same"), version("v2", "edited")]);
    assert_eq!(
        LibraryIndex::build(&snapshot).note("note").unwrap().status,
        StudyStatus::Modified
    );
    snapshot.versions = Arc::new(vec![
        version("v1", "same"),
        version("v2", "edited"),
        version("v3", "same"),
    ]);
    assert_eq!(
        LibraryIndex::build(&snapshot).note("note").unwrap().status,
        StudyStatus::Added
    );
}
