//! Runner-only acceptance of user-opened controllers, never notification fallbacks.
use crate::{
    app::{Page, StudyApp},
    smoke_library::{click_scrolled, until},
};
use gpui_kit::{test::TestWindowExt, *};
use serde_json::json;
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};
use studyapp_core::{NoticeCategory, NoticeLayout, NoticeSeverity, NoticeStyle};

async fn export_preview(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    cx: &mut AsyncApp,
) -> Entity<crate::markdown_editor::MarkdownEditor> {
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| app.clear_notices(cx));
            window.render_frame(cx);
            crate::smoke_library::reader_action(window, 0, cx);
        })
        .unwrap();
    until(
        window,
        view,
        "user-opened export preview",
        |app, _| app.export_panel.is_some(),
        cx,
    )
    .await;
    window
        .update(cx, |_, _, cx| {
            view.read(cx).export_panel.as_ref().unwrap().preview.clone()
        })
        .unwrap()
}

async fn exercise_export(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    database: &Path,
    cx: &mut AsyncApp,
) {
    let (note, content, original_page) = window
        .update(cx, |_, window, cx| {
            let app = view.read(cx);
            assert!(!app.settings_dirty(cx) && app.note_editor.is_none());
            let snapshot = app.snapshot.as_ref().unwrap();
            let note = snapshot.notes.first().unwrap().clone();
            let content = snapshot.note_body(&note.id).unwrap().0.to_owned();
            let page = app.page;
            view.update(cx, |app, cx| {
                app.clear_notices(cx);
                app.navigate(Page::Notes, window, cx);
                app.select_note_document(note.id.clone(), None, false, window, cx);
            });
            (note, content, page)
        })
        .unwrap();
    let source = std::fs::read(&note.path).unwrap();
    until(
        window,
        view,
        "reader ready for export",
        |app, cx| app.note_content.read(cx).ready() && !app.library_controls_busy(cx),
        cx,
    )
    .await;
    let destination = database.with_file_name("native-explicit-export.md");
    assert!(!destination.exists());
    let preview = export_preview(window, view, cx).await;
    crate::smoke_visibility::visible(window, &preview, cx).await;
    let state = crate::smoke_editor::bridge(window, &preview, json!({"type":"snapshot"}), cx).await;
    assert_eq!(state["readonly"], true);
    assert_eq!(state["text"], content);
    assert_eq!(state["canRequestEdit"], false);
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let left = window.find("share-document-preview").bounds();
            let target = window.find("share-export-file").bounds();
            let frame = window.find("modal-frame").bounds();
            let footer = window.find("share-close").bounds();
            assert!(
                left.size.width > px(100.) && left.right() <= target.left(),
                "real share targets must occupy their own column: {left:?}, {target:?}"
            );
            assert!(frame.intersect(&footer) == footer);
            window.press("enter", cx);
            assert!(view.read(cx).export_panel.is_some());
            assert!(!destination.exists());
        })
        .unwrap();
    crate::smoke_visibility::capture_window(
        window,
        &preview,
        &database.with_file_name("native-share-preview.bmp"),
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| window.click("share-close", cx))
        .unwrap();
    until(
        window,
        view,
        "export cancel closes without writing",
        |app, _| app.export_panel.is_none(),
        cx,
    )
    .await;
    assert!(!destination.exists());
    let preview = export_preview(window, view, cx).await;
    crate::smoke_visibility::visible(window, &preview, cx).await;
    window
        .update(cx, |_, window, cx| window.press("escape", cx))
        .unwrap();
    until(
        window,
        view,
        "export escape closes without writing",
        |app, _| app.export_panel.is_none(),
        cx,
    )
    .await;
    assert!(!destination.exists());
    for collision in [false, true] {
        let preview = export_preview(window, view, cx).await;
        crate::smoke_visibility::visible(window, &preview, cx).await;
        window
            .update(cx, |_, window, cx| {
                view.update(cx, |app, _| {
                    app.export_panel.as_mut().unwrap().test_destination = Some(destination.clone());
                });
                window.render_frame(cx);
                window.click("share-export-file", cx);
                assert!(view.read(cx).export_panel.is_none());
            })
            .unwrap();
        until(
            window,
            view,
            "real worker export outcome",
            |app, cx| {
                app.notifications
                    .read(cx)
                    .test_visible_events()
                    .iter()
                    .any(|(_, event)| {
                        event.category == NoticeCategory::ExportResult
                            && (event.severity >= NoticeSeverity::Warning) == collision
                    })
            },
            cx,
        )
        .await;
        assert_eq!(std::fs::read(&destination).unwrap(), content.as_bytes());
        assert_eq!(std::fs::read(&note.path).unwrap(), source);
    }
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.clear_notices(cx);
                app.navigate(original_page, window, cx);
            });
        })
        .unwrap();
    std::fs::write(database.with_file_name("native-share-result.json"), serde_json::to_vec_pretty(&json!({
        "controller":"reader export menu", "readonlyExactSavedBody":true,
        "separateTargetColumn":true,"cancelAndEscapeDoNotWrite":true,"enterDoesNotExport":true,
        "chooserSubstituted":true,"realWorkerExport":true,"existingFileRefused":true,"sourceUnchanged":true,
    })).unwrap()).unwrap();
}

async fn save_sync(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    expected: &crate::sync_runtime::SyncSettings,
    cx: &mut AsyncApp,
) {
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("save-settings", cx);
            assert!(
                view.read(cx).preferences_ui.busy,
                "sync fixture must use the actual save queue"
            );
        })
        .unwrap();
    until(
        window,
        view,
        "exact committed sync settings",
        |app, cx| {
            assert!(
                app.preferences_ui.error.is_none(),
                "{:?}",
                app.preferences_ui.error
            );
            !app.preferences_ui.busy
                && !app.settings_dirty(cx)
                && app
                    .preferences_ui
                    .saved
                    .as_ref()
                    .is_some_and(|saved| saved.sync == *expected)
                && app.sync_panel.read(cx).status.settings == *expected
        },
        cx,
    )
    .await;
}

async fn exercise_activity(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    database: &Path,
    cx: &mut AsyncApp,
) {
    use crate::sync_runtime::{SyncSettings, SyncTarget};
    let original = window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.navigate(Page::Settings, window, cx);
                app.clear_notices(cx);
            });
            crate::smoke::select_settings_page(window, view, "sync", cx);
            let original = view.read(cx).preferences_ui.saved.clone().unwrap();
            assert!(
                matches!(
                    original.sync.target,
                    SyncTarget::Disabled | SyncTarget::Folder { .. }
                ),
                "isolated fixture must not replace an existing WebDAV credential"
            );
            assert!(
                original.sync.auto_minutes.is_none(),
                "fixture requires manual sync"
            );
            original
        })
        .unwrap();
    // Hold real TLS handshakes, not a made-up task/progress value. Releasing
    // each handshake fails the real controller and exposes its retry action.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let target = SyncSettings {
        target: SyncTarget::WebDav {
            url: format!("https://{}/", listener.local_addr().unwrap()),
            username: "native-task-fixture".into(),
        },
        auto_minutes: None,
    };
    let entered = Arc::new(AtomicUsize::new(0));
    let observed = entered.clone();
    let (release, wait_release) = mpsc::channel();
    let server = std::thread::spawn(move || {
        use std::io::{Read, Write};
        for attempt in 1..=2 {
            let deadline = std::time::Instant::now() + Duration::from_secs(15);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "native task did not connect"
                        );
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("native task listener: {error}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut bytes = [0; 4096];
            assert!(stream.read(&mut bytes).unwrap() > 0);
            observed.store(attempt, Ordering::SeqCst);
            wait_release.recv_timeout(Duration::from_secs(15)).unwrap();
            stream.write_all(b"not a TLS response").unwrap();
        }
    });
    window
        .update(cx, |_, window, cx| {
            crate::smoke_settings::select_choice(window, "sync-target", 2, cx);
            let panel = view.read(cx).sync_panel.clone();
            panel.update(cx, |panel, cx| {
                let SyncTarget::WebDav { url, username } = &target.target else {
                    unreachable!()
                };
                panel
                    .url
                    .update(cx, |input, cx| input.set_value(url.clone(), window, cx));
                panel.username.update(cx, |input, cx| {
                    input.set_value(username.clone(), window, cx)
                });
                panel.password.update(cx, |input, cx| {
                    input.set_value("isolated-dummy-password", window, cx)
                });
                cx.notify();
            });
        })
        .unwrap();
    save_sync(window, view, &target, cx).await;
    window
        .update(cx, |_, window, cx| {
            click_scrolled(window, "sync-now".into(), "settings-section-scroll", cx)
        })
        .unwrap();
    until(
        window,
        view,
        "actual in-flight sync task",
        |app, cx| app.sync_panel.read(cx).status.busy && entered.load(Ordering::SeqCst) == 1,
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("titlebar-task-activity", cx);
            window.render_frame(cx);
            let panel = window.find("task-activity-panel").bounds();
            assert!(panel.left() > window.viewport_size().width * 0.5);
            assert!(window.try_find("sync-task").is_some());
            assert!(
                window.try_find("activity-retry-sync").is_none(),
                "running sync cannot offer retry"
            );
            crate::smoke::capture(
                window,
                database.with_file_name("notification-actual-task-running.png"),
            )
            .unwrap();
            window.click("close-activity", cx);
            assert!(!view.read(cx).activity_panel.read(cx).open);
            assert!(
                view.read(cx).sync_panel.read(cx).status.busy,
                "closing the real drawer cancelled the task"
            );
        })
        .unwrap();
    release.send(()).unwrap();
    until(
        window,
        view,
        "hidden task reaches real failure",
        |app, cx| {
            !app.activity_panel.read(cx).open
                && !app.sync_panel.read(cx).status.busy
                && app.sync_panel.read(cx).status.failed
        },
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| app.clear_notices(cx));
            window.render_frame(cx);
            window.click("titlebar-task-activity", cx);
            window.render_frame(cx);
            assert!(window.try_find("activity-retry-sync").is_some());
            crate::smoke::capture(
                window,
                database.with_file_name("notification-actual-task-failed.png"),
            )
            .unwrap();
            window.click("activity-retry-sync", cx);
        })
        .unwrap();
    until(
        window,
        view,
        "task drawer retries through real sync worker",
        |app, cx| app.sync_panel.read(cx).status.busy && entered.load(Ordering::SeqCst) == 2,
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.preferences_ui
                    .draft
                    .as_mut()
                    .unwrap()
                    .appearance
                    .notifications
                    .layouts
                    .insert(NoticeStyle::ActivityView, NoticeLayout::Alternative);
                app.refresh_appearance(window, cx);
                cx.notify();
            });
            window.render_frame(cx);
            assert!(window.try_find("sync-task").is_some());
            let panel = window.find("task-activity-panel").bounds();
            assert!((panel.center().x - window.viewport_size().width * 0.5).abs() < px(2.));
            crate::smoke::capture(
                window,
                database.with_file_name("notification-actual-task-table.png"),
            )
            .unwrap();
            window.click("close-activity", cx);
            assert!(view.read(cx).sync_panel.read(cx).status.busy);
            view.update(cx, |app, cx| {
                app.preferences_ui.draft.as_mut().unwrap().appearance =
                    original.settings.appearance.clone();
                app.refresh_appearance(window, cx);
            });
        })
        .unwrap();
    release.send(()).unwrap();
    until(
        window,
        view,
        "retried hidden task finishes",
        |app, cx| !app.sync_panel.read(cx).status.busy && app.sync_panel.read(cx).status.failed,
        cx,
    )
    .await;
    server.join().unwrap();
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| app.clear_notices(cx));
            let index = match &original.sync.target {
                SyncTarget::Disabled => 0,
                SyncTarget::Folder { .. } => 1,
                SyncTarget::WebDav { .. } => unreachable!(),
            };
            crate::smoke_settings::select_choice(window, "sync-target", index, cx);
            if let SyncTarget::Folder { path } = &original.sync.target {
                let folder = view.read(cx).sync_panel.read(cx).folder.clone();
                folder.update(cx, |input, cx| input.set_value(path.clone(), window, cx));
            }
        })
        .unwrap();
    save_sync(window, view, &original.sync, cx).await;
    window
        .update(cx, |_, _, cx| {
            let saved = view.read(cx).preferences_ui.saved.as_ref().unwrap();
            assert_eq!(saved.settings, original.settings);
            assert_eq!(saved.shortcuts, original.shortcuts);
            assert_eq!(saved.sync, original.sync);
            assert_eq!(saved.image_secret_saved, original.image_secret_saved);
            assert!(!view.read(cx).sync_panel.read(cx).status.credential_saved);
        })
        .unwrap();
    std::fs::write(database.with_file_name("native-task-result.json"), serde_json::to_vec_pretty(&json!({
        "controller":"real TLS sync worker", "runningDrawer":true,"unmeasuredProgress":true,
        "closeDoesNotCancel":true,"hiddenTaskFinishes":true,"realFailureAndRetry":true,
        "attempts":entered.load(Ordering::SeqCst),"alternativeTable":true,"preferencesRestored":true,
    })).unwrap()).unwrap();
}

pub(crate) async fn exercise(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    database: &Path,
    cx: &mut AsyncApp,
) {
    assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    exercise_export(window, view, database, cx).await;
    exercise_activity(window, view, database, cx).await;
}
