//! Isolated Windows runner scenario for managed-note display metadata renaming.
//! Run only through WW_STUDYAPP_SMOKE_SCENARIO=note-rename with a fresh library.
use crate::{
    app::{Page, StudyApp},
    folders_ui::FolderFilter,
    note_actions::{NoteAction, NoteActionKind},
    smoke_library::{create_note, settled, until},
};
use gpui_kit::{component::WindowExt, test::TestWindowExt, *};
use serde_json::json;
use std::path::Path;
use studyapp_core::Command;

async fn open_rename(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    note_id: &str,
    cx: &mut AsyncApp,
) {
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| app.clear_notices(cx));
            window.render_frame(cx);
            window.right_click(SharedString::from(format!("note-{note_id}")), cx);
        })
        .unwrap();
    until(
        window,
        view,
        "rename context menu",
        |app, _| app.active_note_menu.is_some(),
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            // Existing move/reveal/copy ordering stays stable; rename is appended.
            for _ in 0..4 {
                window.press("down", cx);
            }
            window.press("enter", cx);
        })
        .unwrap();
    until(
        window,
        view,
        "rename dialog",
        |app, cx| app.folders.read(cx).dialog_open(),
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("note-rename-input", cx);
        })
        .unwrap();
}

pub async fn exercise(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    database: &Path,
    cx: &mut AsyncApp,
) {
    settled(window, view, cx).await;
    let target = create_note(window, view, "重命名目标", None, cx).await;
    let selected = create_note(window, view, "保持选择的笔记", None, cx).await;
    let original_bytes = std::fs::read(&target.path).unwrap();
    let (library, history, original_version) = window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.page = Page::Notes;
                app.search
                    .update(cx, |input, cx| input.set_value("", window, cx));
                app.folders
                    .update(cx, |panel, cx| panel.set_filter(FolderFilter::All, cx));
                app.selected_note = Some(selected.id.clone());
                let version = app
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .versions
                    .iter()
                    .find(|v| v.note_id == selected.id)
                    .unwrap()
                    .id
                    .clone();
                app.selected_version = Some(version);
                app.history = true;
                cx.notify();
            });
            let app = view.read(cx);
            (
                app.last_library.clone(),
                app.snapshot.as_ref().unwrap().versions.clone(),
                app.selected_version.clone(),
            )
        })
        .unwrap();
    open_rename(window, view, &target.id, cx).await;
    window
        .update(cx, |_, window, cx| {
            let input = window
                .focused_input(cx)
                .unwrap()
                .as_input()
                .unwrap()
                .clone();
            assert_eq!(input.read(cx).value().as_ref(), "重命名目标");
            window.press("escape", cx);
        })
        .unwrap();
    until(
        window,
        view,
        "Escape cancels rename",
        |app, cx| !app.folders.read(cx).dialog_open(),
        cx,
    )
    .await;
    window
        .update(cx, |_, _, cx| {
            let app = view.read(cx);
            assert_eq!(
                app.snapshot
                    .as_ref()
                    .unwrap()
                    .notes
                    .iter()
                    .find(|n| n.id == target.id)
                    .unwrap()
                    .title,
                target.title
            );
            assert_eq!(app.selected_note.as_ref(), Some(&selected.id));
            assert_eq!(app.selected_version, original_version);
        })
        .unwrap();

    open_rename(window, view, &target.id, cx).await;
    let input = window
        .update(cx, |_, window, cx| {
            window
                .focused_input(cx)
                .unwrap()
                .as_input()
                .unwrap()
                .clone()
        })
        .unwrap();
    window
        .update(cx, |_, window, cx| window.press("ctrl-a", cx))
        .unwrap();
    until(
        window,
        view,
        "rename input selects existing title",
        |_, cx| input.read(cx).selected_range() == (0..input.read(cx).text().len()),
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            window.input("持久显示名称", cx);
            window.press("enter", cx);
        })
        .unwrap();
    until(
        window,
        view,
        "Enter persists captured target rename",
        |app, _| {
            app.snapshot
                .as_ref()
                .unwrap()
                .notes
                .iter()
                .any(|n| n.id == target.id && n.title == "持久显示名称")
        },
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            let app = view.read(cx);
            assert_eq!(app.selected_note.as_ref(), Some(&selected.id));
            assert_eq!(app.selected_version, original_version);
            assert_eq!(
                serde_json::to_value(&app.snapshot.as_ref().unwrap().versions).unwrap(),
                serde_json::to_value(&history).unwrap()
            );
            window.render_frame(cx);
            assert!(
                window
                    .try_find(SharedString::from(format!("note-{}", target.id)))
                    .is_some()
            );
        })
        .unwrap();
    assert_eq!(std::fs::read(&target.path).unwrap(), original_bytes);

    // Metadata rename while a different document has a live unsaved draft must
    // retain its content, identity, asset ownership and original hash.
    let draft = window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.selected_version = None;
                app.open_note_editor(false, window, cx);
            });
            view.read(cx).note_editor.clone().expect("draft opened")
        })
        .unwrap();
    open_rename(window, view, &target.id, cx).await;
    window
        .update(cx, |_, window, cx| {
            let input = window
                .focused_input(cx)
                .unwrap()
                .as_input()
                .unwrap()
                .clone();
            input.update(cx, |input, cx| input.set_value("Busy rejected", window, cx));
            view.update(cx, |app, _| app.note_saving = true);
            window.press("enter", cx);
        })
        .unwrap();
    until(
        window,
        view,
        "busy submission finishes without a command",
        |app, cx| !app.folders.read(cx).dialog_open(),
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.note_saving = false;
                assert_eq!(app.note_editor.as_ref(), Some(&draft));
                let current = app
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .notes
                    .iter()
                    .find(|n| n.id == target.id)
                    .unwrap();
                assert_eq!(current.title, "持久显示名称");
                let current_revision = current.revision;
                let stale = NoteAction {
                    note_id: target.id.clone(),
                    library_path: "different-library".into(),
                    kind: NoteActionKind::Rename {
                        expected_revision: current_revision,
                    },
                };
                app.handle_note_action(stale, window, cx);
                assert!(!app.folders.read(cx).dialog_open());
                app.handle_note_rename_command(
                    &library,
                    Command::RenameNote {
                        note_id: target.id.clone(),
                        title: "Rejected stale".into(),
                        expected_revision: target.revision,
                    },
                    cx,
                );
                assert_eq!(app.note_editor.as_ref(), Some(&draft));
            });
        })
        .unwrap();
    open_rename(window, view, &target.id, cx).await;
    window
        .update(cx, |_, window, cx| {
            let input = window
                .focused_input(cx)
                .unwrap()
                .as_input()
                .unwrap()
                .clone();
            input.update(cx, |input, cx| {
                input.set_value("保留草稿后名称", window, cx)
            });
            window.press("enter", cx);
        })
        .unwrap();
    until(
        window,
        view,
        "successful rename preserves another live draft",
        |app, _| {
            app.snapshot
                .as_ref()
                .unwrap()
                .notes
                .iter()
                .any(|n| n.id == target.id && n.title == "保留草稿后名称")
        },
        cx,
    )
    .await;
    window
        .update(cx, |_, _, cx| {
            let app = view.read(cx);
            assert_eq!(app.note_editor.as_ref(), Some(&draft));
            assert_eq!(app.selected_note.as_ref(), Some(&selected.id));
        })
        .unwrap();
    assert_eq!(std::fs::read(&target.path).unwrap(), original_bytes);
    std::fs::write(
        database.with_file_name("native-note-rename-evidence.json"),
        serde_json::to_vec_pretty(&json!({
            "targetNoteId": target.id, "selectedNoteId": selected.id,
            "prefilled": true, "escapeCancelled": true, "enterPersisted": true,
            "historyAndMarkdownPreserved": true, "busyRejected": true,
            "staleLibraryAndRevisionRejected": true, "liveDraftPreserved": true
        }))
        .unwrap(),
    )
    .unwrap();
}
