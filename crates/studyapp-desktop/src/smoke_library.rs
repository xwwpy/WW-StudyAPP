//! Native regression coverage for logical folders, history and library storage.
//! Fixtures are retained beside the smoke library for inspection after a run.
use crate::{
    app::{Page, StudyApp},
    folders_ui::FolderFilter,
    markdown_editor::MarkdownEditor,
    worker::Work,
};
use gpui_kit::{test::TestWindowExt, *};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use studyapp_core::{Command, Note};

const TIMEOUT: Duration = Duration::from_secs(5);

/// Reproduce the hosted Windows runner's client-area limit without changing
/// the desktop display or imposing a limit on ordinary application windows.
pub(crate) fn resize(window: &mut Window, requested: Size<Pixels>) {
    let requested = if std::env::var_os("WW_STUDYAPP_SMOKE_SMALL_WINDOW").is_some() {
        size(
            requested.width.min(px(1024.)),
            requested.height.min(px(720.)),
        )
    } else {
        requested
    };
    window.resize(requested);
}

/// Exercise the compact library dropdown and its keyboard navigation. PopupMenu
/// items deliberately have no app-owned button IDs; the chosen item is asserted
/// by each caller's resulting application state.
pub(crate) fn library_add(
    window: &mut Window,
    folder: bool,
    view: &Entity<StudyApp>,
    cx: &mut App,
) {
    // Previous scenario results can overlap + on a narrow window. Notification
    // geometry, dismissal and queue behavior have their own native coverage.
    view.update(cx, |app, cx| app.clear_notices(cx));
    window.render_frame(cx);
    window.click("library-add-menu", cx);
    window.press("down", cx);
    if folder {
        window.press("down", cx);
    }
    window.press("enter", cx);
}

pub(crate) fn editor_action(
    window: &mut Window,
    view: &Entity<StudyApp>,
    discard: bool,
    cx: &mut App,
) {
    let has_copy = view
        .read(cx)
        .note_editor
        .as_ref()
        .unwrap()
        .note_id
        .is_some();
    window.click("note-editor-menu", cx);
    for _ in 0..1 + usize::from(has_copy) + usize::from(discard) {
        window.press("down", cx);
    }
    window.press("enter", cx);
}

pub(crate) fn library_recovery(window: &mut Window, delete: bool, cx: &mut App) {
    window.click("library-more-menu", cx);
    for _ in 0..2 + usize::from(delete) {
        window.press("down", cx);
    }
    window.press("enter", cx);
}

/// The reader's compact menu orders Export, Move, Unmanage, then Relink.
pub(crate) fn reader_action(window: &mut Window, index: usize, cx: &mut App) {
    window.click("note-more-menu", cx);
    for _ in 0..=index {
        window.press("down", cx);
    }
    window.press("enter", cx);
}

async fn pause(cx: &AsyncApp) {
    cx.background_executor()
        .timer(Duration::from_millis(40))
        .await;
}

pub(crate) async fn until(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    description: &str,
    condition: impl FnMut(&StudyApp, &App) -> bool,
    cx: &mut AsyncApp,
) {
    until_for(window, view, description, TIMEOUT, condition, cx).await;
}

async fn until_for(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    description: &str,
    timeout: Duration,
    mut condition: impl FnMut(&StudyApp, &App) -> bool,
    cx: &mut AsyncApp,
) {
    let started = Instant::now();
    loop {
        if window
            .update(cx, |_, _, cx| condition(view.read(cx), cx))
            .expect("smoke window is open")
        {
            return;
        }
        if started.elapsed() >= timeout {
            let diagnostic = window.update(cx, |_, _, cx| {
                let app = view.read(cx);
                format!("page={:?}; notice={:?}; storage_busy={}; note_saving={}; draft_loading={}; media_busy={}; preferences_busy={}; replacing={}; dirty={}; leave={}; preferences_error={:?}; modal={}; last_library={}; notes={}; active_draft={:?}; draft_folder={:?}; intent={:?}; pending_save={:?}; pending_persist={:?}",
                    app.page, app.notice, app.storage_busy, app.note_saving, app.draft_loading,
                    app.media_busy(), app.preferences_ui.busy, app.preferences_ui.replacing,
                    app.settings_dirty(cx), app.preferences_ui.leave.is_some(), app.preferences_ui.error,
                    app.has_modal(cx), app.last_library, app.snapshot.as_ref().map_or(0, |snapshot| snapshot.notes.len()),
                    app.active_draft_id, app.note_editor.as_ref().map(|draft| &draft.folder_id), app.note_intent.is_some(),
                    app.pending_note_save, app.draft_persist.as_ref().map(|pending| (&pending.id, pending.revision, pending.token)))
            }).unwrap();
            panic!("library smoke timed out: {description}; {diagnostic}");
        }
        pause(cx).await;
    }
}

pub(crate) async fn settled(window: AnyWindowHandle, view: &Entity<StudyApp>, cx: &mut AsyncApp) {
    until(
        window,
        view,
        "editor and storage settled",
        |app, _| {
            !app.note_saving
                && !app.storage_busy
                && !app.draft_loading
                && app.note_intent.is_none()
                && app.draft_persist.is_none()
                && app.draft_discard.is_none()
        },
        cx,
    )
    .await;
}

async fn bridge(
    window: AnyWindowHandle,
    editor: &Entity<MarkdownEditor>,
    mut action: Value,
    cx: &mut AsyncApp,
) -> Value {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let key = format!(
        "library-smoke-{}",
        SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    action["request"] = json!(key);
    window
        .update(cx, |_, _, cx| {
            editor.read(cx).test(action).expect("editor test request")
        })
        .unwrap();
    let started = Instant::now();
    loop {
        pause(cx).await;
        if let Some(result) = window
            .update(cx, |_, _, cx| {
                editor.update(cx, |editor, _| {
                    assert!(editor.error().is_none(), "{:?}", editor.error());
                    editor.test_results.remove(&key)
                })
            })
            .unwrap()
        {
            return result;
        }
        assert!(started.elapsed() < TIMEOUT, "editor response: {key}");
    }
}

fn send(window: AnyWindowHandle, view: &Entity<StudyApp>, command: Command, cx: &mut AsyncApp) {
    window
        .update(cx, |_, _, cx| {
            view.update(cx, |app, cx| app.send(command, cx))
        })
        .unwrap();
}

pub(crate) async fn create_note(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    title: &str,
    folder_id: Option<String>,
    cx: &mut AsyncApp,
) -> Note {
    send(
        window,
        view,
        Command::CreateNoteInFolder {
            title: title.into(),
            content: format!("# {title}\n\n原始内容。\n"),
            folder_id,
        },
        cx,
    );
    until(
        window,
        view,
        &format!("create managed fixture {title}"),
        |app, _| {
            app.snapshot
                .as_ref()
                .is_some_and(|s| s.notes.iter().any(|n| n.title == title))
        },
        cx,
    )
    .await;
    window
        .update(cx, |_, _, cx| {
            view.read(cx)
                .snapshot
                .as_ref()
                .unwrap()
                .notes
                .iter()
                .find(|n| n.title == title)
                .unwrap()
                .clone()
        })
        .unwrap()
}

/// Scroll only as far as needed, using the same hit testing as a native click.
pub(crate) fn click_scrolled(window: &mut Window, id: SharedString, scroll: &str, cx: &mut App) {
    reveal_scrolled(window, id.clone(), scroll, cx);
    window.click(id, cx);
}

pub(crate) fn reveal_scrolled(window: &mut Window, id: SharedString, scroll: &str, cx: &mut App) {
    window.render_frame(cx);
    let scroll = if scroll == "folder-tree-scroll" {
        "note-list-scroll"
    } else {
        scroll
    };
    let settings_scroll = if window.try_find("settings-section-scroll").is_some() {
        "settings-section-scroll"
    } else {
        "page-scroll"
    };
    // Save/reset are fixed outside the section scroll viewport.
    let fixed_header = matches!(id.as_ref(), "save-settings" | "reset-settings");
    let scroll = if scroll == "page-scroll" && !fixed_header {
        settings_scroll
    } else {
        scroll
    };
    let mut diagnostic = String::new();
    let mut searched_from_start = false;
    for _ in 0..35 {
        window.render_frame(cx);
        let viewport = Bounds::new(point(px(0.), px(0.)), window.viewport_size());
        let target = window.try_find(id.clone());
        let container = window.try_find(SharedString::from(scroll.to_owned()));
        // Modal actions are siblings of the scrolling body. Clip them against
        // their own fixed region and the dialog, never against the body or the
        // settings page behind the modal. The click still uses native hit testing.
        let modal = window.try_find("modal-frame").filter(|modal| {
            target
                .as_ref()
                .or(container.as_ref())
                .is_some_and(|element| element.path().starts_with(modal.path()))
        });
        let in_modal = modal.is_some();
        let fixed_modal_region = in_modal
            .then(|| {
                ["modal-header", "modal-footer"].into_iter().find_map(|id| {
                    let region = window.try_find(id)?;
                    target.as_ref().and_then(|target| {
                        (region.visible() && target.path().starts_with(region.path()))
                            .then_some(region)
                    })
                })
            })
            .flatten();
        let fixed_modal = fixed_modal_region.is_some();
        let container = fixed_modal_region.or(container);
        let outer = if in_modal {
            window.try_find(if fixed_modal {
                "modal-frame"
            } else {
                "modal-content-scroll"
            })
        } else {
            (!fixed_header)
                .then(|| window.try_find("settings-section-scroll"))
                .flatten()
        };
        let modal_clip = modal
            .as_ref()
            .map_or(viewport, |modal| viewport.intersect(&modal.bounds()));
        let outer_clip = outer
            .as_ref()
            .map_or(modal_clip, |outer| modal_clip.intersect(&outer.bounds()));
        let click_clip = container.as_ref().map_or(outer_clip, |container| {
            outer_clip.intersect(&container.bounds())
        });
        diagnostic = format!(
            "viewport={viewport:?}; target={:?}; inner={:?}; outer={:?}; click_clip={click_clip:?}",
            target
                .as_ref()
                .map(|target| (target.visible(), target.bounds())),
            container.as_ref().map(|container| container.bounds()),
            outer.as_ref().map(|outer| outer.bounds())
        );
        if let Some(target) = target.as_ref()
            && target.visible()
            && click_clip.size.width > px(0.)
            && click_clip.size.height > px(0.)
            && click_clip.contains(&target.bounds().center())
        {
            return;
        }
        let nested = scroll != "page-scroll" && scroll != settings_scroll;
        // A row can be inside the inner list while below the Settings clipping
        // edge. Scrolling that list cannot reveal it when the list has no
        // overflow; move its outer ancestor instead. A clipped list still
        // receives its own wheel only through its actually visible portion.
        let scroll_outer = nested
            && (click_clip.size.height <= px(0.)
                || target.as_ref().is_some_and(|target| {
                    container.as_ref().is_some_and(|container| {
                        container.bounds().contains(&target.bounds().center())
                    }) && !outer_clip.contains(&target.bounds().center())
                }));
        let active_clip = if scroll_outer { outer_clip } else { click_clip };
        let target_y = target
            .as_ref()
            .map(|target| target.bounds().center().y)
            .or_else(|| {
                container
                    .as_ref()
                    .map(|container| container.bounds().center().y)
            })
            .unwrap_or(active_clip.bottom());
        // An unmounted virtual row has no bounds and may be above us. Search
        // from the beginning once, then walk down through visible rows. This
        // still exercises the list's real wheel dispatch and clipping.
        let delta = if target.is_none() && !searched_from_start {
            searched_from_start = true;
            100_000.
        } else if target_y < active_clip.top() {
            160.
        } else {
            -160.
        };
        if !in_modal && (scroll_outer || !nested) {
            scroll_settings(window, delta, cx);
        } else {
            window.dispatch_event(
                ScrollWheelEvent {
                    position: active_clip.center(),
                    delta: ScrollDelta::Pixels(point(px(0.), px(delta))),
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
        }
    }
    if let Ok(folder) = std::env::var("WW_STUDYAPP_SMOKE_EVIDENCE_DIR") {
        window
            .render_to_image()
            .unwrap()
            .save(std::path::Path::new(&folder).join("scroll-reveal-failure.png"))
            .unwrap();
    }
    panic!("could not reveal native button {id}; {diagnostic}");
}

/// Scroll in the section's padding, where nested file lists cannot consume it.
pub(crate) fn scroll_settings(window: &mut Window, delta: f32, cx: &mut App) {
    let page = window
        .try_find("settings-section-scroll")
        .unwrap_or_else(|| window.find("page-scroll"))
        .bounds();
    window.dispatch_event(
        ScrollWheelEvent {
            position: point(page.origin.x + px(5.), page.center().y),
            delta: ScrollDelta::Pixels(point(px(0.), px(delta))),
            ..Default::default()
        }
        .to_platform_input(),
        cx,
    );
    window.render_frame(cx);
}

#[cfg(windows)]
pub async fn exercise(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    database: &Path,
    cx: &mut AsyncApp,
) {
    let started = Instant::now();
    settled(window, view, cx).await;
    let (editor, external_files) = window
        .update(cx, |_, window, cx| {
            crate::smoke_library::resize(window, size(px(1440.), px(1000.)));
            view.update(cx, |app, cx| {
                app.clear_notices(cx);
                assert!(app.note_editor.is_none() && app.recoverable_draft.is_none());
                app.page = Page::Notes;
                app.history = false;
                app.search.update(cx, |s, cx| s.set_value("", window, cx));
                app.folders
                    .update(cx, |panel, cx| panel.set_filter(FolderFilter::All, cx));
                cx.notify();
            });
            let app = view.read(cx);
            let external_files = app
                .snapshot
                .as_ref()
                .unwrap()
                .notes
                .iter()
                .filter(|n| !n.managed && !n.missing)
                .map(|n| {
                    (
                        n.id.clone(),
                        PathBuf::from(&n.path),
                        std::fs::read(&n.path).expect("existing external source"),
                    )
                })
                .collect::<Vec<_>>();
            assert!(!external_files.is_empty(), "external source fixture exists");
            (app.note_content.clone(), external_files)
        })
        .unwrap();

    let mut parent_id = None;
    let mut child_id = None;
    for name in ["原生目录冒烟", "长历史与编辑"] {
        send(
            window,
            view,
            Command::CreateFolder {
                name: name.into(),
                parent_id: parent_id.clone(),
            },
            cx,
        );
        until(
            window,
            view,
            "logical folder created",
            |app, _| {
                app.snapshot
                    .as_ref()
                    .unwrap()
                    .folders
                    .iter()
                    .any(|f| f.name == name)
            },
            cx,
        )
        .await;
        let id = window
            .update(cx, |_, _, cx| {
                view.read(cx)
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .folders
                    .iter()
                    .find(|f| f.name == name)
                    .unwrap()
                    .id
                    .clone()
            })
            .unwrap();
        if parent_id.is_none() {
            parent_id = Some(id);
        } else {
            child_id = Some(id);
        }
    }
    let child_id = child_id.unwrap();
    let long_title = "研究".repeat(100);
    let note = create_note(window, view, &long_title, None, cx).await;
    let original_file = std::fs::read(&note.path).unwrap();
    window
        .update(cx, |_, window, cx| {
            // This fixture is created by a background command, so it must not
            // change the user's current reader selection implicitly.
            view.update(cx, |app, cx| app.clear_notices(cx));
            window.render_frame(cx);
            click_scrolled(
                window,
                SharedString::from(format!("note-{}", note.id)),
                "note-list-scroll",
                cx,
            );
            assert_eq!(view.read(cx).selected_note.as_ref(), Some(&note.id));
            reader_action(window, 1, cx);
            window.click(
                SharedString::from(format!("folder-destination-{child_id}")),
                cx,
            );
            click_scrolled(window, "folder-submit".into(), "folder-action-scroll", cx);
        })
        .unwrap();
    until(
        window,
        view,
        "move note through folder panel",
        |app, _| {
            app.snapshot.as_ref().unwrap().notes.iter().any(|n| {
                n.id == note.id && n.folder_id.as_ref() == Some(&child_id) && n.path == note.path
            })
        },
        cx,
    )
    .await;
    assert_eq!(std::fs::read(&note.path).unwrap(), original_file);
    window
        .update(cx, |_, window, cx| {
            window.click(SharedString::from(format!("folder-row-{child_id}")), cx);
            assert_eq!(
                view.read(cx).folders.read(cx).filter(),
                &FolderFilter::Folder(child_id.clone())
            );
            window.render_frame(cx);
            assert!(
                window
                    .try_find(SharedString::from(format!("note-{}", note.id)))
                    .is_some()
            );
            for (id, _, _) in &external_files {
                assert!(
                    window
                        .try_find(SharedString::from(format!("note-{id}")))
                        .is_some(),
                    "folder selection must not filter other branches out of the unified tree"
                );
            }
            window.click("folders-root", cx);
            assert_eq!(view.read(cx).folders.read(cx).filter(), &FolderFilter::Root);
            window.render_frame(cx);
            assert!(
                window
                    .try_find(SharedString::from(format!("note-{}", note.id)))
                    .is_some(),
                "selecting Unclassified must retain expanded folder notes"
            );
            // Click a note in another branch, then accept a timer-only snapshot.
            // An obsolete folder filter must not switch the reader back.
            window.click(SharedString::from(format!("note-{}", note.id)), cx);
            let same_snapshot = view.read(cx).snapshot.as_ref().unwrap().clone();
            view.update(cx, |app, cx| {
                app.accept_snapshot(same_snapshot, false, window, cx)
            });
            assert_eq!(view.read(cx).selected_note.as_ref(), Some(&note.id));
            window.click(SharedString::from(format!("folder-expand-{child_id}")), cx);
            window.render_frame(cx);
            assert!(
                window
                    .try_find(SharedString::from(format!("note-{}", note.id)))
                    .is_none(),
                "collapsed branch kept constructing its notes"
            );
            assert_eq!(
                view.read(cx).selected_note.as_ref(),
                Some(&note.id),
                "collapsing a branch changed the document"
            );
            window.click(SharedString::from(format!("folder-expand-{child_id}")), cx);
            window.click("folders-all", cx);
            assert_eq!(view.read(cx).folders.read(cx).filter(), &FolderFilter::All);
        })
        .unwrap();

    let long_content = format!(
        "# {long_title}\n\n![图片](studyapp-media:{})\n\n{}\n",
        "abcdef0123456789".repeat(100),
        (0..1650)
            .map(|i| format!("新增第 {i:04} 行"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let expected_hash = window
        .update(cx, |_, _, cx| {
            view.read(cx)
                .snapshot
                .as_ref()
                .unwrap()
                .versions
                .iter()
                .rev()
                .find(|v| v.note_id == note.id)
                .unwrap()
                .hash
                .clone()
        })
        .unwrap();
    send(
        window,
        view,
        Command::SaveNote {
            note_id: note.id.clone(),
            expected_hash,
            content: long_content.clone(),
        },
        cx,
    );
    until(
        window,
        view,
        "long history version",
        |app, _| {
            app.snapshot
                .as_ref()
                .unwrap()
                .versions
                .iter()
                .any(|v| v.note_id == note.id && v.content == long_content)
        },
        cx,
    )
    .await;
    window
        .update(cx, |_, window, _| {
            crate::smoke_library::resize(window, size(px(1000.), px(700.)))
        })
        .unwrap();
    for _ in 0..50 {
        pause(cx).await;
        if window
            .update(cx, |_, window, _| {
                let actual = window.viewport_size();
                (actual.width - px(1000.)).abs() <= px(1.)
                    && (actual.height - px(700.)).abs() <= px(1.)
            })
            .unwrap()
        {
            break;
        }
    }
    window
        .update(cx, |_, window, cx| {
            assert!(
                window.viewport_size().width <= px(1001.),
                "resize did not settle: {:?}",
                window.viewport_size()
            );
            assert!(
                window.viewport_size().height <= px(701.),
                "resize did not settle: {:?}",
                window.viewport_size()
            );
            // The preceding version save is fixture setup. Its success toast
            // must not intercept the history-tab interaction under test.
            view.update(cx, |app, cx| app.clear_notices(cx));
            // Selecting All above deliberately chooses its first note. A
            // background fixture save preserves that reader selection.
            click_scrolled(
                window,
                SharedString::from(format!("note-{}", note.id)),
                "note-list-scroll",
                cx,
            );
            assert_eq!(view.read(cx).selected_note.as_ref(), Some(&note.id));
            assert!(view.read(cx).selected_version.is_none());
            let latest = view
                .read(cx)
                .snapshot
                .as_ref()
                .unwrap()
                .versions
                .iter()
                .rev()
                .find(|version| version.note_id == note.id)
                .unwrap();
            assert_eq!(latest.content, long_content);
            assert!(!latest.diff.is_empty());
            window.click("history-tab", cx);
            window.render_frame(cx);
            assert!(
                view.read(cx).history,
                "history tab interaction was intercepted"
            );
            assert!(view.read(cx).history_preview.rows().len() > 1500);
        })
        .unwrap();
    let history_layout_started = Instant::now();
    loop {
        pause(cx).await;
        let laid_out = window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                view.read(cx)
                    .history_preview
                    .scroll
                    .0
                    .borrow()
                    .base_handle
                    .bounds()
                    .size
                    .height
                    > px(0.)
            })
            .unwrap();
        if laid_out {
            break;
        }
        assert!(
            history_layout_started.elapsed() < TIMEOUT,
            "long history did not lay out a positive viewport at 1000x700"
        );
    }
    window
        .update(cx, |_, window, cx| {
            assert!(
                view.read(cx)
                    .history_preview
                    .scroll
                    .0
                    .borrow()
                    .base_handle
                    .bounds()
                    .size
                    .height
                    > px(0.),
                "long title must leave a scrollable history viewport at 1000x700"
            );
            // Pane-only navigation changes must reflow without a window resize.
            let outer = window.viewport_size();
            let before_width = view
                .read(cx)
                .history_preview
                .scroll
                .0
                .borrow()
                .base_handle
                .bounds()
                .size
                .width;
            let before_rows = view.read(cx).history_preview.rows();
            window.click("toggle-navigation", cx);
            window.render_frame(cx);
            let after_width = view
                .read(cx)
                .history_preview
                .scroll
                .0
                .borrow()
                .base_handle
                .bounds()
                .size
                .width;
            assert_eq!(window.viewport_size(), outer);
            assert_ne!(
                before_width, after_width,
                "navigation toggle must change actual history width"
            );
            assert!(
                !std::sync::Arc::ptr_eq(&before_rows, &view.read(cx).history_preview.rows()),
                "pane resize must rebuild rows"
            );
            window.click("toggle-navigation", cx);
            window.render_frame(cx);
            let app = view.read(cx);
            assert!(app.history_preview.rows().len() > 1500);
            eprintln!(
                "history rows={}, viewport={:?}, items={:?}",
                app.history_preview.rows().len(),
                app.history_preview.scroll.0.borrow().base_handle.bounds(),
                app.history_preview.scroll.0.borrow().last_item_size
            );
            window.scroll(
                "history-diff-scroll",
                ScrollDelta::Pixels(point(px(0.), px(-440.))),
                cx,
            );
            assert!(
                view.read(cx)
                    .history_preview
                    .scroll
                    .0
                    .borrow()
                    .base_handle
                    .offset()
                    .y
                    < px(0.),
                "native wheel scroll must move the diff"
            );
            let app = view.read(cx);
            app.history_preview
                .scroll
                .scroll_to_item(app.history_preview.rows().len() - 1, ScrollStrategy::Bottom);
            window.render_frame(cx);
            window
                .render_to_image()
                .unwrap()
                .save(database.with_file_name("library-long-history.png"))
                .unwrap();
            assert!(
                view.read(cx)
                    .history_preview
                    .scroll
                    .0
                    .borrow()
                    .base_handle
                    .offset()
                    .y
                    < px(0.)
            );
            window
                .render_to_image()
                .unwrap()
                .save(database.with_file_name("library-long-history.png"))
                .unwrap();
            let last_row = view.read(cx).history_preview.rows().len() - 1;
            assert!(window.find(("diff-line", last_row)).visible());
            window.click("edit-note", cx);
            assert!(!view.read(cx).history);
            assert!(view.read(cx).selected_version.is_none());
            assert_eq!(
                view.read(cx).note_editor.as_ref().unwrap().note_id.as_ref(),
                Some(&note.id)
            );
            view.update(cx, |app, cx| {
                app.storage_busy = true;
                app.save_note(window, cx);
                assert!(!app.note_saving);
                assert!(app.note_intent.is_none());
                assert!(app.note_editor.is_some());
                app.storage_busy = false;
                cx.notify();
            });
        })
        .unwrap();
    let before = bridge(window, &editor, json!({"type":"snapshot"}), cx).await;
    assert_eq!(before["readonly"], false);
    assert_eq!(before["text"], long_content);
    let sentinel = "原生输入确认：从历史返回编辑后可继续保存 🧠\n";
    let input = window
        .update(cx, |_, _, cx| {
            editor
                .read(cx)
                .insert_text(sentinel)
                .expect("send actual WebView2 keyboard text")
        })
        .unwrap();
    let input_started = Instant::now();
    loop {
        match input.try_recv() {
            Ok(result) => {
                result.expect("WebView2 Input.insertText");
                break;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                panic!("WebView2 input completion disconnected")
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                assert!(input_started.elapsed() < TIMEOUT, "native input completion");
                pause(cx).await;
            }
        }
    }
    let after = bridge(window, &editor, json!({"type":"snapshot"}), cx).await;
    assert_eq!(after["readonly"], false);
    let saved_content = after["text"].as_str().unwrap().to_owned();
    assert!(saved_content.contains(sentinel));
    assert!(saved_content.contains("新增第 1649 行"));
    window
        .update(cx, |_, window, cx| window.click("save-note", cx))
        .unwrap();
    until(
        window,
        view,
        "save native input after history",
        |app, _| {
            !app.note_saving
                && app.note_editor.is_none()
                && app
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .versions
                    .iter()
                    .any(|v| v.note_id == note.id && v.content == saved_content)
        },
        cx,
    )
    .await;
    assert_eq!(std::fs::read_to_string(&note.path).unwrap(), saved_content);
    window
        .update(cx, |_, window, cx| {
            crate::smoke_library::resize(window, size(px(1440.), px(1000.)));
            window.render_frame(cx);
        })
        .unwrap();

    // Deleting a logical folder that only contains an unsaved draft must leave
    // that draft editable and saveable in the root.
    send(
        window,
        view,
        Command::CreateFolder {
            name: "草稿目录移除测试".into(),
            parent_id: None,
        },
        cx,
    );
    until(
        window,
        view,
        "draft folder created",
        |app, _| {
            app.snapshot
                .as_ref()
                .unwrap()
                .folders
                .iter()
                .any(|f| f.name == "草稿目录移除测试")
        },
        cx,
    )
    .await;
    let draft_folder = window
        .update(cx, |_, window, cx| {
            let id = view
                .read(cx)
                .snapshot
                .as_ref()
                .unwrap()
                .folders
                .iter()
                .find(|f| f.name == "草稿目录移除测试")
                .unwrap()
                .id
                .clone();
            window.click(SharedString::from(format!("folder-row-{id}")), cx);
            id
        })
        .unwrap();
    // FolderEvent::Filter is delivered at the end of this input update. Match
    // two real clicks: finish navigation before creating the draft in its folder.
    window
        .update(cx, |_, window, cx| {
            let id = &draft_folder;
            crate::smoke_library::library_add(window, false, view, cx);
            view.update(cx, |app, cx| {
                assert_eq!(
                    app.note_editor.as_ref().unwrap().folder_id.as_ref(),
                    Some(id)
                );
                app.note_title.update(cx, |title, cx| {
                    title.set_value("目录删除后的草稿", window, cx)
                });
            });
        })
        .unwrap();
    let draft_text = "目录删除后仍保留的草稿内容。\n";
    bridge(
        window,
        &editor,
        json!({"type":"replace","from":0,"to":0,"text":draft_text}),
        cx,
    )
    .await;
    send(
        window,
        view,
        Command::DeleteFolder {
            folder_id: draft_folder.clone(),
        },
        cx,
    );
    until(
        window,
        view,
        "deleted draft folder falls back to root",
        |app, _| {
            app.snapshot
                .as_ref()
                .unwrap()
                .folders
                .iter()
                .all(|f| f.id != draft_folder)
                && app
                    .note_editor
                    .as_ref()
                    .is_some_and(|draft| draft.folder_id.is_none())
        },
        cx,
    )
    .await;
    let draft_after = bridge(window, &editor, json!({"type":"snapshot"}), cx).await;
    assert_eq!(draft_after["text"], draft_text);
    assert_eq!(draft_after["readonly"], false);
    window
        .update(cx, |_, window, cx| window.click("save-note", cx))
        .unwrap();
    until_for(
        window,
        view,
        "draft saved after its folder was deleted",
        // Durable draft barrier, file write and terminal draft receipt have a
        // separate bounded IO budget on hosted Windows runners.
        Duration::from_secs(20),
        |app, _| {
            app.note_editor.is_none()
                && !app.note_saving
                && app.snapshot.as_ref().unwrap().notes.iter().any(|n| {
                    n.title == "目录删除后的草稿"
                        && n.folder_id.is_none()
                        && std::fs::read_to_string(&n.path).is_ok_and(|text| text == draft_text)
                })
        },
        cx,
    )
    .await;

    let disposable = create_note(window, view, "单份副本清理测试", parent_id.clone(), cx).await;
    let disposable_history = window
        .update(cx, |_, window, cx| {
            let versions = view
                .read(cx)
                .snapshot
                .as_ref()
                .unwrap()
                .versions
                .iter()
                .filter(|v| v.note_id == disposable.id)
                .map(|v| (v.id.clone(), v.content.clone()))
                .collect::<Vec<_>>();
            window.click(("nav", 4_usize), cx);
            window.click("settings-section-storage", cx);
            window.click("storage-copies", cx);
            click_scrolled(window, "refresh-storage".into(), "page-scroll", cx);
            versions
        })
        .unwrap();
    until(
        window,
        view,
        "space report refreshed",
        |app, _| {
            !app.storage_busy
                && app.storage_report.as_ref().is_some_and(|r| {
                    r.copies
                        .iter()
                        .any(|copy| copy.note_id == disposable.id && copy.deletable)
                })
        },
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            let report = view.read(cx).storage_report.as_ref().unwrap();
            assert!(report.database_bytes > 0 && report.managed_bytes > 0);
            assert!(
                external_files
                    .iter()
                    .all(|(id, _, _)| { report.copies.iter().all(|copy| &copy.note_id != id) })
            );
            let delete = SharedString::from(format!("delete-copy-{}", disposable.id));
            reveal_scrolled(window, delete.clone(), "storage-copy-list", cx);
            eprintln!("storage delete fixture: viewport={:?}, target={:?}, inner={:?}, outer={:?}, save={:?}, notifications={:?}", window.viewport_size(), window.find(delete.clone()).bounds(), window.find("storage-copy-list").bounds(), window.find("settings-section-scroll").bounds(), window.find("save-settings").bounds(), view.read(cx).notifications.read(cx).test_state());
            window.render_to_image().unwrap().save(database.with_file_name("storage-delete-before-click.png")).unwrap();
            window.click(delete, cx);
            assert_eq!(view.read(cx).storage_confirm.as_ref(), Some(&disposable.id));
            window.render_frame(cx);
            assert!(window.find("modal-content-scroll").visible());
            window.click("confirm-delete-copy", cx);
        })
        .unwrap();
    until(
        window,
        view,
        "single managed copy deleted",
        |app, _| {
            !app.storage_busy
                && app
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .notes
                    .iter()
                    .any(|n| n.id == disposable.id && n.detached)
                && !Path::new(&disposable.path).exists()
        },
        cx,
    )
    .await;
    window
        .update(cx, |_, _, cx| {
            let snapshot = view.read(cx).snapshot.as_ref().unwrap();
            for (id, content) in &disposable_history {
                assert!(
                    snapshot
                        .versions
                        .iter()
                        .any(|v| &v.id == id && &v.content == content)
                );
            }
            assert_eq!(
                snapshot
                    .notes
                    .iter()
                    .find(|n| n.id == note.id)
                    .unwrap()
                    .folder_id
                    .as_ref(),
                Some(&child_id)
            );
        })
        .unwrap();
    for (_, path, bytes) in &external_files {
        assert_eq!(&std::fs::read(path).unwrap(), bytes);
    }
    assert_eq!(std::fs::read_to_string(&note.path).unwrap(), saved_content);

    settled(window, view, cx).await;
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.clear_notices(cx);
                app.new_note(window, cx);
            });
            window.render_frame(cx);
        })
        .unwrap();
    let migration_draft = "迁移前已确认并落盘的最后输入 🧠";
    bridge(
        window,
        &editor,
        json!({"type":"replace","from":0,"to":0,"text":migration_draft}),
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            crate::smoke_library::editor_action(window, view, false, cx)
        })
        .unwrap();
    settled(window, view, cx).await;
    let original_database = std::fs::canonicalize(database).unwrap();
    let original_directory = original_database.parent().unwrap().to_path_buf();
    let target = original_directory.with_file_name(format!(
        "studyapp-native-migrated-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&target).expect("fresh migration destination");
    window
        .update(cx, |_, _, cx| {
            view.update(cx, |app, cx| {
                assert!(app.note_editor.is_none());
                app.storage_work(Work::MigrateLibrary(target.clone()), cx);
            })
        })
        .unwrap();
    until(
        window,
        view,
        "migration switched to copied library",
        |app, _| {
            !app.storage_busy
                && !app.draft_loading
                && Path::new(&app.last_library).parent() == Some(target.as_path())
        },
        cx,
    )
    .await;
    window
        .update(cx, |_, _, cx| {
            let app = view.read(cx);
            assert_eq!(
                app.recoverable_draft.as_ref().unwrap().content,
                migration_draft
            );
            let snapshot = app.snapshot.as_ref().unwrap();
            let migrated = snapshot.notes.iter().find(|n| n.id == note.id).unwrap();
            assert!(Path::new(&migrated.path).starts_with(&target));
            assert_eq!(migrated.folder_id.as_ref(), Some(&child_id));
            assert_eq!(
                std::fs::read_to_string(&migrated.path).unwrap(),
                saved_content
            );
            assert!(
                snapshot
                    .folders
                    .iter()
                    .any(|f| f.id == child_id && f.parent_id == parent_id)
            );
            assert!(
                snapshot
                    .versions
                    .iter()
                    .any(|v| v.note_id == note.id && v.content == saved_content)
            );
            assert!(
                snapshot
                    .notes
                    .iter()
                    .any(|n| n.id == disposable.id && n.detached)
            );
            for (id, path, bytes) in &external_files {
                let external = snapshot.notes.iter().find(|n| &n.id == id).unwrap();
                assert_eq!(Path::new(&external.path), path);
                assert_eq!(&std::fs::read(path).unwrap(), bytes);
            }
            app.worker
                .sender
                .send(Work::SwitchLibrary(original_directory.clone()))
                .unwrap();
        })
        .unwrap();
    until(
        window,
        view,
        "return to original library for close recovery",
        |app, _| {
            Path::new(&app.last_library) == original_database
                && !app.draft_loading
                && !app.storage_busy
        },
        cx,
    )
    .await;
    assert!(original_database.is_file());
    assert_eq!(std::fs::read_to_string(&note.path).unwrap(), saved_content);
    window
        .update(cx, |_, window, cx| {
            assert_eq!(
                view.read(cx).recoverable_draft.as_ref().unwrap().content,
                migration_draft
            );
            view.update(cx, |app, cx| app.begin_discard(window, cx));
        })
        .unwrap();
    settled(window, view, cx).await;
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.page = Page::Notes;
                app.history = false;
                app.selected_note = Some(note.id.clone());
                app.selected_version = None;
                app.folders
                    .update(cx, |panel, cx| panel.set_filter(FolderFilter::All, cx));
                assert!(app.note_editor.is_none() && app.recoverable_draft.is_none());
                cx.notify();
            });
            window.render_frame(cx);
        })
        .unwrap();
    std::fs::write(
        database.with_file_name("library-smoke-result.txt"),
        format!(
            "PASS: nested folders, native move/filter buttons, >1500 diff rows scrolled, busy save rejected without wedging the editor, native WebView2 input and save, deleted draft folder fallback and save, storage refresh and individual confirmed deletion, preserved history/external sources, migration and return.\nElapsed: {:?}\nMigration copy retained: {}\n",
            started.elapsed(), target.display()
        ),
    )
    .unwrap();
    eprintln!("native library smoke passed in {:?}", started.elapsed());
}

#[cfg(test)]
mod tests {
    use super::{click_scrolled, reveal_scrolled};
    use gpui_kit::{
        Context, InteractiveElement, IntoElement, ListOffset, ParentElement, Render, SharedString,
        Styled, TestAppContext, TestSupportExt, Window, div, px, test::TestWindowExt,
    };
    use studyapp_ui::settings_components::{VirtualRows, virtual_rows};

    #[gpui_kit::test]
    fn reveal_modal_footer_uses_its_fixed_bounds_and_dispatches_the_click(cx: &mut TestAppContext) {
        use gpui_kit::{
            AppContext, Bounds, WindowBounds, WindowOptions, component::button::Button, point, size,
        };
        use studyapp_ui::{
            feedback::{ModalFocus, modal_frame_shell},
            modal_frame::ModalFrame,
        };
        struct LongModal {
            focus: ModalFocus,
            clicks: usize,
        }
        impl Render for LongModal {
            fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                self.focus.sync_with_initial(true, None, window, cx);
                modal_frame_shell(
                    ModalFrame::new(
                        "确认操作",
                        div().h(px(900.)).flex_shrink_0().child(
                            div()
                                .id("nested-modal-list")
                                .test_support()
                                .h(px(180.))
                                .child("说明"),
                        ),
                    )
                    .scroll_handle(self.focus.scroll())
                    .footer(
                        Button::new("fixed-footer-submit")
                            .label("确定")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.clicks += 1;
                                cx.notify();
                            })),
                    ),
                    &self.focus,
                    window,
                    cx,
                )
            }
        }
        cx.update(gpui_kit::init);
        let (handle, view) = cx.update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(1000.), px(700.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |_, cx| {
                    cx.new(|cx| LongModal {
                        focus: ModalFocus::new(cx),
                        clicks: 0,
                    })
                },
            )
            .unwrap()
        });
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            let button = window.find("fixed-footer-submit").bounds();
            assert!(
                !window
                    .find("modal-content-scroll")
                    .bounds()
                    .contains(&button.center())
            );
            assert!(
                window
                    .find("modal-footer")
                    .bounds()
                    .contains(&button.center())
            );
            let offset = view.read(cx).focus.scroll().offset();
            click_scrolled(
                window,
                "fixed-footer-submit".into(),
                "modal-content-scroll",
                cx,
            );
            assert_eq!(view.read(cx).clicks, 1);
            click_scrolled(
                window,
                "fixed-footer-submit".into(),
                "nested-modal-list",
                cx,
            );
            assert_eq!(view.read(cx).clicks, 2);
            assert_eq!(view.read(cx).focus.scroll().offset(), offset);
        })
        .unwrap();
    }

    struct VirtualForm(VirtualRows<()>);

    impl Render for VirtualForm {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.0
                .sync_with_height(&(0..50).collect::<Vec<_>>(), (), px(60.));
            div()
                .flex()
                .flex_col()
                .w(px(400.))
                .h(px(240.))
                .child(virtual_rows(&self.0.scroll, |ix, _, _| {
                    div()
                        .id(SharedString::from(format!("virtual-row-{ix}")))
                        .test_support()
                        .h(px(60.))
                        .into_any_element()
                }))
        }
    }

    #[gpui_kit::test]
    fn reveal_virtual_row_searches_above_and_below_the_current_viewport(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(|_, _| VirtualForm(VirtualRows::default()));
        cx.update(|window, cx| window.render_frame(cx));
        view.update(cx, |view, cx| {
            view.0.scroll.scroll_to(ListOffset {
                item_ix: 25,
                offset_in_item: px(0.),
            });
            cx.notify();
        });
        cx.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("virtual-row-0").is_none());
            for id in ["virtual-row-0", "virtual-row-49"] {
                reveal_scrolled(window, id.into(), "settings-section-scroll", cx);
                assert!(
                    window
                        .find("settings-section-scroll")
                        .bounds()
                        .contains(&window.find(id).bounds().center())
                );
            }
        });
    }

    #[gpui_kit::test]
    fn reveal_partially_clipped_modal_body_button_scrolls_before_click(cx: &mut TestAppContext) {
        use gpui_kit::{
            AppContext, Bounds, WindowBounds, WindowOptions, component::button::Button, point, size,
        };
        use studyapp_ui::{
            feedback::{ModalFocus, modal_frame_shell},
            modal_frame::ModalFrame,
        };
        struct ClippedBodyModal {
            focus: ModalFocus,
            clicks: usize,
        }
        impl Render for ClippedBodyModal {
            fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                self.focus.sync_with_initial(true, None, window, cx);
                let body = div()
                    .flex()
                    .flex_col()
                    .flex_shrink_0()
                    .child(div().h(px(600.)).flex_shrink_0())
                    .child(
                        div().flex_shrink_0().child(
                            Button::new("clipped-body-button")
                                .label("正文操作")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.clicks += 1;
                                    cx.notify();
                                })),
                        ),
                    )
                    .child(div().h(px(600.)).flex_shrink_0());
                modal_frame_shell(
                    ModalFrame::new("正文裁切", body)
                        .scroll_handle(self.focus.scroll())
                        .footer(div().h(px(64.)).w_full().child("固定页脚")),
                    &self.focus,
                    window,
                    cx,
                )
            }
        }
        cx.update(gpui_kit::init);
        let (handle, view) = cx.update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(1000.), px(700.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |_, cx| {
                    cx.new(|cx| ClippedBodyModal {
                        focus: ModalFocus::new(cx),
                        clicks: 0,
                    })
                },
            )
            .unwrap()
        });
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            let body = window.find("modal-content-scroll").bounds();
            let button = window.find("clipped-body-button").bounds();
            let scroll = view.read(cx).focus.scroll().clone();
            // Leave a visible strip of the body button, with its center below
            // the body's clip edge and geometrically inside the fixed footer.
            let offset = scroll.offset();
            scroll.set_offset(point(
                offset.x,
                offset.y + body.bottom() - button.top() - button.size.height / 4.,
            ));
            window.render_frame(cx);
            let body = window.find("modal-content-scroll");
            let footer = window.find("modal-footer");
            let button = window.find("clipped-body-button");
            assert!(button.visible());
            assert!(button.path().starts_with(body.path()));
            assert!(!button.path().starts_with(footer.path()));
            assert!(body.bounds().intersect(&button.bounds()).size.height > px(0.));
            assert!(!body.bounds().contains(&button.bounds().center()));
            assert!(footer.bounds().contains(&button.bounds().center()));
            let offset = scroll.offset();
            click_scrolled(
                window,
                "clipped-body-button".into(),
                "modal-content-scroll",
                cx,
            );
            assert_eq!(view.read(cx).clicks, 1);
            assert!(scroll.offset().y < offset.y);
            assert!(
                window
                    .find("modal-content-scroll")
                    .bounds()
                    .contains(&window.find("clipped-body-button").bounds().center())
            );
        })
        .unwrap();
    }
}
