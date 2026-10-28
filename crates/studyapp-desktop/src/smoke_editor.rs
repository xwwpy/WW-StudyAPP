//! Native WebView/GPUI persistence integration. Browser tests cover real keyboard
//! selection and IME; these transactions cross the production IPC bridge.
use crate::{
    app::{Page, StudyApp},
    markdown_editor::MarkdownEditor,
};
use gpui_kit::{test::TestWindowExt, *};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};

async fn pause(cx: &AsyncApp) {
    cx.background_executor()
        .timer(Duration::from_millis(50))
        .await;
}

pub(crate) async fn bridge(
    window: AnyWindowHandle,
    editor: &Entity<MarkdownEditor>,
    mut action: Value,
    cx: &mut AsyncApp,
) -> Value {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let key = format!(
        "smoke-{}",
        SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    action["request"] = json!(key);
    window
        .update(cx, |_, _, cx| {
            editor
                .read(cx)
                .test(action)
                .expect("dispatch browser transaction")
        })
        .unwrap();
    for _ in 0..200 {
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
    }
    panic!("browser transaction timed out: {key}");
}

async fn settled(window: AnyWindowHandle, view: &Entity<StudyApp>, cx: &mut AsyncApp) {
    for _ in 0..200 {
        pause(cx).await;
        if window
            .update(cx, |_, _, cx| {
                let app = view.read(cx);
                !app.note_saving && app.draft_discard.is_none() && !app.draft_loading
            })
            .unwrap()
        {
            return;
        }
    }
    panic!("native note operation timed out");
}

async fn dismiss_error_notice(window: AnyWindowHandle, view: &Entity<StudyApp>, cx: &mut AsyncApp) {
    let dismissed = window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            if window.try_find("dismiss-notice").is_some() {
                window.click("dismiss-notice", cx);
                true
            } else {
                false
            }
        })
        .unwrap();
    if dismissed {
        crate::smoke_library::until(
            window,
            view,
            "error notification finished dismissal before recovery",
            |app, cx| app.notifications.read(cx).visible_count() == 0,
            cx,
        )
        .await;
    }
}

pub async fn exercise(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    database: &Path,
    cx: &mut AsyncApp,
) -> String {
    let editor = window
        .update(cx, |_, window, cx| {
            assert_eq!(view.read(cx).snapshot.as_ref().unwrap().notes.len(), 2);
            // The import result is a real occluding toast. At the runner's
            // 1024 px width it covers the library's + button, so dismiss the
            // previous scenario's feedback before exercising this menu.
            view.update(cx, |app, cx| app.clear_notices(cx));
            window.render_frame(cx);
            crate::smoke_library::library_add(window, false, view, cx);
            assert!(
                view.read(cx)
                    .note_editor
                    .as_ref()
                    .is_some_and(|draft| { draft.note_id.is_none() && draft.content.is_empty() }),
                "New Markdown menu must open a distinct empty draft"
            );
            view.update(cx, |app, cx| {
                app.note_title
                    .update(cx, |s, cx| s.set_value("连续编辑测试", window, cx))
            });
            view.read(cx).note_content.clone()
        })
        .unwrap();
    for attempt in 0..400 {
        pause(cx).await;
        if window
            .update(cx, |_, _, cx| {
                assert!(
                    editor.read(cx).error().is_none(),
                    "{:?}",
                    editor.read(cx).error()
                );
                editor.read(cx).ready()
            })
            .unwrap()
        {
            break;
        }
        assert!(attempt < 399, "native WebView initialization timed out");
    }
    let first = "# 连续编辑测试\n\n第一次保存，**加粗知识点**与中文 🧠。\n\n```rust\nfn main() { println!(\"StudyApp\"); }\n```\n";
    bridge(
        window,
        &editor,
        json!({"type":"replace","from":0,"to":0,"text":first}),
        cx,
    )
    .await;
    let rendered = bridge(window, &editor, json!({"type":"snapshot"}), cx).await;
    assert_eq!(rendered["text"], first);
    assert_eq!(rendered["source"], false);
    #[cfg(target_os = "linux")]
    {
        // The initial navigation capture may precede WebKit readiness. Keep
        // actual X11 pixels after content has reached the live editor as well.
        window
            .update(cx, |_, window, cx| window.render_frame(cx))
            .unwrap();
        for _ in 0..4 {
            pause(cx).await;
        }
        window
            .update(cx, |_, window, _| {
                crate::smoke::capture(
                    window,
                    database.with_file_name("native-markdown-editor.png"),
                )
                .expect("capture initialized native editor");
            })
            .unwrap();
        assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
        let capture_root = |name: &str| {
            let status = std::process::Command::new("import")
                .args(["-window", "root", "-silent"])
                .arg(database.with_file_name(name))
                .status()
                .expect("capture X11 root");
            assert!(status.success(), "X11 root capture failed: {status}");
        };
        capture_root("native-markdown-editor-root.png");
        let native_windows =
            crate::web_editor_host::HostedEditor::diagnostic_windows("before-flush");
        for native_window in native_windows {
            use gtk::glib::translate::ToGlibPtr;
            // SAFETY: an owned GDK reference on its GTK foreground thread;
            // no GPUI borrows are held while expose/draw callbacks run.
            unsafe {
                gtk::gdk::ffi::gdk_window_process_updates(native_window.to_glib_none().0, 1);
            }
        }
        for _ in 0..4 {
            pause(cx).await;
            crate::web_editor_host::HostedEditor::pump_events();
        }
        crate::web_editor_host::HostedEditor::diagnostic_windows("after-flush");
        window
            .update(cx, |_, window, _| {
                crate::smoke::capture(
                    window,
                    database.with_file_name("native-markdown-editor-flushed.png"),
                )
                .expect("capture native editor after explicit GDK updates");
            })
            .unwrap();
        capture_root("native-markdown-editor-flushed-root.png");
    }
    #[cfg(windows)]
    {
        let count = window
            .update(cx, |_, _, cx| {
                editor.read(cx).inject_unfinished_character().unwrap();
                view.read(cx).snapshot.as_ref().unwrap().notes.len()
            })
            .unwrap();
        pause(cx).await;
        window
            .update(cx, |_, window, cx| window.click("save-note", cx))
            .unwrap();
        crate::smoke_library::until(window, view, "incomplete Unicode save refused without freezing editor", |app, cx| {
            !app.note_saving && app.note_intent.is_none()
                && app.notice.as_ref().is_some_and(|notice| matches!(notice, Err(message) if message.contains("字符输入未完成")))
                && app.note_content.read(cx).error().is_none()
        }, cx).await;
        let recovered = bridge(window, &editor, json!({"type":"snapshot"}), cx).await;
        assert_eq!(recovered["text"], first);
        assert_eq!(recovered["readonly"], false);
        window
            .update(cx, |_, _, cx| {
                assert_eq!(view.read(cx).snapshot.as_ref().unwrap().notes.len(), count)
            })
            .unwrap();
        bridge(
            window,
            &editor,
            json!({"type":"replace","from":0,"to":0,"text":"继续输入"}),
            cx,
        )
        .await;
        bridge(
            window,
            &editor,
            json!({"type":"replace","from":0,"to":4,"text":""}),
            cx,
        )
        .await;
        assert_eq!(
            bridge(window, &editor, json!({"type":"snapshot"}), cx).await["text"],
            first
        );
    }
    window
        .update(cx, |_, _, cx| editor.read(cx).inject_test_error(true))
        .unwrap();
    pause(cx).await;
    assert_eq!(
        bridge(window, &editor, json!({"type":"snapshot"}), cx).await["text"],
        first
    );
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| app.clear_notices(cx));
            crate::smoke_library::resize(window, size(px(1000.), px(700.)));
            window.render_frame(cx);
            editor.read(cx).inject_test_error(false);
        })
        .unwrap();
    for _ in 0..5 {
        pause(cx).await;
    }
    dismiss_error_notice(window, view, cx).await;
    window
        .update(cx, |_, window, cx| {
            assert!(editor.read(cx).error().is_some());
            window.render_frame(cx);
            window.click("recover-editor-bridge", cx);
        })
        .unwrap();
    for _ in 0..10 {
        pause(cx).await;
    }
    assert_eq!(
        bridge(window, &editor, json!({"type":"snapshot"}), cx).await["text"],
        first
    );
    window
        .update(cx, |_, window, cx| {
            crate::smoke_library::resize(window, size(px(1280.), px(860.)));
            window.render_frame(cx);
        })
        .unwrap();
    assert!(
        rendered["highlights"].as_u64().unwrap_or_default() > 0,
        "{rendered}"
    );
    // A single selection spans paragraphs; formatting and history stay in one editor.
    bridge(
        window,
        &editor,
        json!({"type":"select","anchor":2,"head":20}),
        cx,
    )
    .await;
    bridge(window, &editor, json!({"type":"command","name":"bold"}), cx).await;
    bridge(window, &editor, json!({"type":"command","name":"undo"}), cx).await;
    bridge(
        window,
        &editor,
        json!({"type":"command","name":"source"}),
        cx,
    )
    .await;
    let back = bridge(
        window,
        &editor,
        json!({"type":"command","name":"source"}),
        cx,
    )
    .await;
    assert_eq!(back["text"], first);
    bridge(window, &editor, json!({"type":"select","anchor":first.encode_utf16().count(),"head":first.encode_utf16().count()}), cx).await;
    #[cfg(windows)]
    let capture = window
        .update(cx, |_, _, cx| {
            editor
                .read(cx)
                .capture_png(database.with_file_name("note-editor.png"))
                .unwrap()
        })
        .unwrap();
    #[cfg(windows)]
    for attempt in 0..100 {
        pause(cx).await;
        if let Ok(result) = capture.try_recv() {
            result.unwrap();
            break;
        }
        assert!(attempt < 99, "WebView screenshot timeout");
    }
    window
        .update(cx, |_, window, cx| window.click("save-note", cx))
        .unwrap();
    settled(window, view, cx).await;
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| app.clear_notices(cx));
            window.render_frame(cx);
            editor.read(cx).inject_test_error(false);
        })
        .unwrap();
    for _ in 0..5 {
        pause(cx).await;
    }
    dismiss_error_notice(window, view, cx).await;
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("recover-editor-bridge", cx);
        })
        .unwrap();
    for _ in 0..10 {
        pause(cx).await;
    }
    assert_eq!(
        bridge(window, &editor, json!({"type":"snapshot"}), cx).await["readonly"],
        true
    );
    #[cfg(windows)]
    crate::smoke_visibility::managed_body_edit(window, view, &editor, cx).await;
    let (id, path) = window
        .update(cx, |_, window, cx| {
            let app = view.read(cx);
            #[cfg(windows)]
            assert!(
                app.note_editor.is_some(),
                "managed body click did not create draft: {:?}",
                app.notice
            );
            #[cfg(not(windows))]
            assert!(app.note_editor.is_none(), "{:?}", app.notice);
            let id = app.selected_note.clone().unwrap();
            let note = app
                .snapshot
                .as_ref()
                .unwrap()
                .notes
                .iter()
                .find(|n| n.id == id)
                .unwrap();
            assert!(note.managed);
            let path = note.path.clone();
            assert_eq!(std::fs::read_to_string(&path).unwrap(), first);
            #[cfg(not(windows))]
            window.click("edit-note", cx);
            let crlf = "# 中文 🧠\r\n\r\n第二次保存，保留历史。\r\n";
            editor.update(cx, |e, cx| e.set_value(crlf.into(), window, cx));
            (id, path)
        })
        .unwrap();
    bridge(
        window,
        &editor,
        json!({"type":"command","name":"source"}),
        cx,
    )
    .await;
    bridge(
        window,
        &editor,
        json!({"type":"command","name":"source"}),
        cx,
    )
    .await;
    // UTF-16 offset lands after the emoji, without normalizing untouched CRLF bytes.
    bridge(
        window,
        &editor,
        json!({"type":"replace","from":7,"to":7,"text":" 与高亮"}),
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            crate::smoke_library::editor_action(window, view, false, cx)
        })
        .unwrap();
    crate::smoke_library::until(
        window,
        view,
        "kept draft persisted and reader restored",
        |app, _| {
            app.recoverable_draft.is_some()
                && app.note_editor.is_none()
                && !app.note_saving
                && app.draft_persist.is_none()
                && app.note_intent.is_none()
        },
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            assert!(view.read(cx).recoverable_draft.is_some());
            crate::smoke_library::library_recovery(window, false, cx);
            assert_eq!(
                editor.read(cx).value(),
                "# 中文 🧠 与高亮\r\n\r\n第二次保存，保留历史。\r\n"
            );
            window.click("save-note", cx);
        })
        .unwrap();
    settled(window, view, cx).await;
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# 中文 🧠 与高亮\r\n\r\n第二次保存，保留历史。\r\n"
    );

    // Discard an active draft, then discard a separately persisted recovery draft.
    for recovered in [false, true] {
        window
            .update(cx, |_, window, cx| {
                view.update(cx, |app, cx| app.clear_notices(cx));
                window.render_frame(cx);
                crate::smoke_library::library_add(window, false, view, cx);
                assert!(
                    view.read(cx).note_editor.as_ref().is_some_and(|draft| {
                        draft.note_id.is_none() && draft.content.is_empty()
                    })
                );
            })
            .unwrap();
        bridge(
            window,
            &editor,
            json!({"type":"replace","from":0,"to":0,"text":"删除的草稿，不应复活"}),
            cx,
        )
        .await;
        let discarded_id = window
            .update(cx, |_, _, cx| {
                view.read(cx).active_draft_id.clone().unwrap()
            })
            .unwrap();
        if recovered {
            window
                .update(cx, |_, window, cx| {
                    crate::smoke_library::editor_action(window, view, false, cx)
                })
                .unwrap();
            crate::smoke_library::until(
                window,
                view,
                "recovery-test draft persisted before discard",
                |app, _| {
                    app.recoverable_draft.is_some()
                        && app.note_editor.is_none()
                        && !app.note_saving
                        && app.draft_persist.is_none()
                        && app.note_intent.is_none()
                },
                cx,
            )
            .await;
        }
        window
            .update(cx, |_, window, cx| {
                if recovered {
                    crate::smoke_library::library_recovery(window, true, cx);
                } else {
                    crate::smoke_library::editor_action(window, view, true, cx);
                }
                window.click("confirm-note-action", cx);
            })
            .unwrap();
        settled(window, view, cx).await;
        window
            .update(cx, |_, _, cx| {
                assert!(view.read(cx).note_editor.is_none());
                assert!(view.read(cx).recoverable_draft.is_none());
            })
            .unwrap();
        let record: crate::drafts::DraftRecord = serde_json::from_slice(
            &std::fs::read(
                crate::drafts::DraftStore::record_path(database.to_str().unwrap(), &discarded_id)
                    .unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(record.status, crate::drafts::DraftStatus::Deleted);
        assert_eq!(record.draft.content, "删除的草稿，不应复活");
    }
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.selected_note = Some(id.clone());
                app.reader_key.clear();
                cx.notify();
            });
            window.render_frame(cx);
            crate::smoke_library::reader_action(window, 2, cx);
            window.click("confirm-note-action", cx);
        })
        .unwrap();
    cx.background_executor()
        .timer(Duration::from_millis(700))
        .await;
    window
        .update(cx, |_, _, cx| {
            let app = view.read(cx);
            assert!(
                app.snapshot
                    .as_ref()
                    .unwrap()
                    .notes
                    .iter()
                    .find(|n| n.id == id)
                    .unwrap()
                    .detached
            );
            assert!(Path::new(&path).is_file());
            view.update(cx, |app, cx| {
                app.send(
                    studyapp_core::Command::Import(vec![path.clone().into()]),
                    cx,
                )
            });
        })
        .unwrap();
    cx.background_executor()
        .timer(Duration::from_millis(700))
        .await;
    window
        .update(cx, |_, _, cx| {
            let snapshot = view.read(cx).snapshot.as_ref().unwrap();
            assert!(!snapshot.notes.iter().find(|n| n.id == id).unwrap().detached);
            assert_eq!(
                snapshot.versions.iter().filter(|v| v.note_id == id).count(),
                2
            );
        })
        .unwrap();
    id
}

/// The PowerShell runner verifies this last draft after normal window shutdown.
#[cfg(windows)]
pub async fn exercise_multiple_drafts(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    database: &Path,
    cx: &mut AsyncApp,
) {
    use crate::drafts::{DraftRecord, DraftStatus, DraftStore, DraftUpdate};
    use crate::smoke_library::{editor_action, library_add, until};
    let (editor, selected, baseline, source_files) = window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.clear_notices(cx);
                app.page = Page::Notes;
            });
            let app = view.read(cx);
            assert!(app.note_editor.is_none());
            let baseline = app
                .draft_catalog
                .values()
                .filter(|r| r.active())
                .cloned()
                .collect::<Vec<_>>();
            let sources = app
                .snapshot
                .as_ref()
                .unwrap()
                .notes
                .iter()
                .filter_map(|n| std::fs::read(&n.path).ok().map(|b| (n.path.clone(), b)))
                .collect::<Vec<_>>();
            let result = (
                app.note_content.clone(),
                app.selected_note.clone(),
                baseline,
                sources,
            );
            window.render_frame(cx);
            result
        })
        .unwrap();
    let mut expected = Vec::<(String, String, String, String)>::new();
    for (index, raw) in [
        "# 草稿甲\r\n\r\n第一份原文 🧠\r\n",
        "# 草稿乙\n\n第二份独立正文。\n",
        "# 草稿丙\r第三份混合换行\n",
    ]
    .into_iter()
    .enumerate()
    {
        let previous = window
            .update(cx, |_, _, cx| view.read(cx).active_draft_id.clone())
            .unwrap();
        window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                library_add(window, false, view, cx);
            })
            .unwrap();
        until(
            window,
            view,
            "New automatically parks the preceding document",
            |app, _| {
                app.active_draft_id.is_some()
                    && app.active_draft_id != previous
                    && app.note_editor.is_some()
                    && !app.note_saving
                    && app.draft_persist.is_none()
            },
            cx,
        )
        .await;
        let title = format!("多草稿验收 {}", index + 1);
        window
            .update(cx, |_, window, cx| {
                view.update(cx, |app, cx| {
                    app.note_title
                        .update(cx, |s, cx| s.set_value(title.clone(), window, cx));
                });
                editor.update(cx, |e, cx| e.set_value(raw.into(), window, cx));
                assert!(
                    view.read(cx).confirmation.is_none(),
                    "New required replacing another draft"
                );
            })
            .unwrap();
        #[cfg(windows)]
        if index == 0 {
            crate::smoke_visibility::type_into(window, &editor, "native_multi_draft_🧠", cx).await;
        }
        let (id, content, group) = window
            .update(cx, |_, _, cx| {
                let app = view.read(cx);
                let id = app.active_draft_id.clone().unwrap();
                (
                    id.clone(),
                    editor.read(cx).value().to_owned(),
                    app.draft_catalog[&id].draft.asset_group.clone(),
                )
            })
            .unwrap();
        assert!(
            expected
                .iter()
                .all(|(old, _, _, old_group)| old != &id && old_group != &group)
        );
        expected.push((id, content, title, group));
        window
            .update(cx, |_, _, cx| {
                let app = view.read(cx);
                assert_eq!(
                    app.draft_catalog.values().filter(|r| r.active()).count(),
                    baseline.len() + index + 1
                );
                for (id, content, title, group) in expected.iter().take(index) {
                    let record = &app.draft_catalog[id];
                    assert_eq!(&record.draft.content, content);
                    assert_eq!(&record.draft.title, title);
                    assert_eq!(&record.draft.asset_group, group);
                    assert!(
                        app.draft_durable[id] >= record.revision,
                        "switch completed before the exact draft was durable"
                    );
                }
            })
            .unwrap();
    }
    // Filtering is itself an asynchronous document transition; the active third
    // draft must be durable before any row can replace the persistent WebView.
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("folders-drafts", cx);
        })
        .unwrap();
    until(
        window,
        view,
        "Drafts folder parks the last active draft",
        |app, _| app.note_editor.is_none() && !app.note_saving && app.draft_persist.is_none(),
        cx,
    )
    .await;
    for (id, content, title, group) in &expected {
        window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window.click(SharedString::from(format!("draft-row-{id}")), cx);
            })
            .unwrap();
        until(
            window,
            view,
            "Drafts row opens its exact independent document",
            |app, cx| {
                app.active_draft_id.as_ref() == Some(id)
                    && app.note_editor.is_some()
                    && app.note_content.read(cx).value() == content
            },
            cx,
        )
        .await;
        window
            .update(cx, |_, window, cx| {
                let app = view.read(cx);
                assert_eq!(app.note_title.read(cx).value().as_ref(), title.as_str());
                assert_eq!(&app.note_editor.as_ref().unwrap().asset_group, group);
                window.render_frame(cx);
                let header = window.find("folders-drafts").bounds();
                let active = window
                    .find(SharedString::from(format!("draft-row-{id}")))
                    .bounds();
                let viewport = window.find("note-list-scroll").bounds();
                let first = expected
                    .iter()
                    .map(|(id, _, _, _)| {
                        window
                            .find(SharedString::from(format!("draft-row-{id}")))
                            .bounds()
                            .top()
                    })
                    .reduce(|a, b| if a < b { a } else { b })
                    .unwrap();
                assert!(
                    header.bottom() <= first,
                    "Drafts heading overlaps the first draft row"
                );
                assert!(
                    active.right() <= viewport.right() - px(10.),
                    "active draft covers the scrollbar gutter"
                );
            })
            .unwrap();
        #[cfg(windows)]
        crate::smoke_visibility::focus_body(window, &editor, cx).await;
    }
    #[cfg(windows)]
    crate::smoke_visibility::capture_window(
        window,
        &editor,
        &database.with_file_name("multiple-drafts-native.bmp"),
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| editor_action(window, view, false, cx))
        .unwrap();
    until(
        window,
        view,
        "all drafts parked for independent restart reader",
        |app, _| app.note_editor.is_none() && !app.note_saving && app.draft_persist.is_none(),
        cx,
    )
    .await;
    let library = window
        .update(cx, |_, _, cx| view.read(cx).last_library.clone())
        .unwrap();
    let store = DraftStore::new();
    store.load(&library);
    let mut restored = None;
    for attempt in 0..100 {
        while let Ok(update) = store.receiver.try_recv() {
            if let DraftUpdate::Catalog {
                records, errors, ..
            } = update
            {
                assert!(errors.is_empty(), "{errors:?}");
                restored = Some(records);
            }
        }
        if restored.is_some() {
            break;
        }
        assert!(
            attempt < 99,
            "independent draft store did not recover the catalog"
        );
        pause(cx).await;
    }
    let restored = restored.unwrap();
    drop(store);
    for (id, content, title, group) in &expected {
        let record = restored.iter().find(|r| &r.id == id).unwrap();
        assert!(record.active());
        assert_eq!(&record.draft.content, content);
        assert_eq!(&record.draft.title, title);
        assert_eq!(&record.draft.asset_group, group);
    }
    let untouched = expected
        .iter()
        .skip(1)
        .map(|(id, _, _, _)| {
            let path = DraftStore::record_path(&library, id).unwrap();
            (path.clone(), std::fs::read(path).unwrap())
        })
        .collect::<Vec<_>>();
    for (index, (id, content, _, _)) in expected.iter().enumerate() {
        window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window.click(SharedString::from(format!("draft-row-{id}")), cx);
            })
            .unwrap();
        until(
            window,
            view,
            "open exact draft before its individual deletion",
            |app, _| {
                app.active_draft_id.as_ref() == Some(id)
                    && app.note_editor.is_some()
                    && !app.note_saving
            },
            cx,
        )
        .await;
        window
            .update(cx, |_, window, cx| {
                editor_action(window, view, true, cx);
                window.click("confirm-note-action", cx);
            })
            .unwrap();
        until(
            window,
            view,
            "individual tombstone acknowledged before leaving",
            |app, _| app.note_editor.is_none() && app.draft_discard.is_none(),
            cx,
        )
        .await;
        let record: DraftRecord = serde_json::from_slice(
            &std::fs::read(DraftStore::record_path(&library, id).unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(record.status, DraftStatus::Deleted);
        assert_eq!(&record.draft.content, content);
        if index == 0 {
            for (path, bytes) in &untouched {
                assert_eq!(
                    &std::fs::read(path).unwrap(),
                    bytes,
                    "deleting A changed another draft"
                );
            }
        }
    }
    window
        .update(cx, |_, window, cx| {
            let app = view.read(cx);
            let active = app
                .draft_catalog
                .values()
                .filter(|r| r.active())
                .cloned()
                .collect::<Vec<_>>();
            assert_eq!(active, baseline);
            if let Some(id) = selected {
                view.update(cx, |app, cx| {
                    app.select_note_document(id, None, false, window, cx)
                });
            }
        })
        .unwrap();
    for (path, bytes) in source_files {
        assert_eq!(
            std::fs::read(path).unwrap(),
            bytes,
            "draft operations changed a source file"
        );
    }
    std::fs::write(database.with_file_name("native-multiple-drafts-result.txt"),"PASS: three independent drafts automatically park on New/filter/row switch; exact titles, raw CRLF/mixed newlines, Unicode, stable asset groups and native body focus survive independent store restart; one confirmed deletion preserves other draft bytes; all source files remain unchanged.\n").unwrap();
    exercise_save_view(window, view, database, cx).await;
}

#[cfg(windows)]
async fn exercise_save_view(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    database: &Path,
    cx: &mut AsyncApp,
) {
    use crate::smoke_library::{library_add, until};
    let editor = window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| app.clear_notices(cx));
            window.render_frame(cx);
            library_add(window, false, view, cx);
            view.read(cx).note_content.clone()
        })
        .unwrap();
    until(
        window,
        view,
        "new scroll-preservation draft",
        |app, _| app.note_editor.is_some() && !app.note_saving,
        cx,
    )
    .await;
    let text = (0..160)
        .map(|index| format!("Paragraph {index}: 保存保留当前位置 🧠\n\n"))
        .collect::<String>();
    bridge(
        window,
        &editor,
        json!({"type":"replace","from":0,"to":0,"text":text}),
        cx,
    )
    .await;
    let caret = text.find("Paragraph 95").unwrap();
    let caret = text[..caret].encode_utf16().count();
    bridge(
        window,
        &editor,
        json!({"type":"select","anchor":caret,"head":caret}),
        cx,
    )
    .await;
    for _ in 0..12 {
        pause(cx).await;
    }
    bridge(
        window,
        &editor,
        json!({"type":"replace","from":caret,"to":caret,"text":"UNDO-SAVE-MARKER"}),
        cx,
    )
    .await;
    let before = bridge(window, &editor, json!({"type":"snapshot"}), cx).await;
    assert!(
        before["scrollTop"].as_f64().unwrap() > 500.,
        "save fixture never scrolled"
    );
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("save-note", cx);
        })
        .unwrap();
    until(
        window,
        view,
        "save finished without resetting view",
        |app, _| app.note_editor.is_none() && !app.note_saving && app.draft_persist.is_none(),
        cx,
    )
    .await;
    let saved = bridge(window, &editor, json!({"type":"snapshot"}), cx).await;
    assert_eq!(saved["text"], before["text"]);
    assert_eq!(saved["selection"], before["selection"]);
    assert_eq!(
        saved["generation"], before["generation"],
        "unchanged save reloaded the document"
    );
    assert!(
        (saved["scrollTop"].as_f64().unwrap() - before["scrollTop"].as_f64().unwrap()).abs() < 5.,
        "save moved the viewport: {before} -> {saved}"
    );
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("edit-note", cx);
        })
        .unwrap();
    until(
        window,
        view,
        "reopen saved note without replacing state",
        |app, _| app.note_editor.is_some() && !app.note_saving,
        cx,
    )
    .await;
    let reopened = bridge(window, &editor, json!({"type":"snapshot"}), cx).await;
    assert_eq!(reopened["generation"], saved["generation"]);
    assert_eq!(reopened["selection"], saved["selection"]);
    let undone = bridge(window, &editor, json!({"type":"command","name":"undo"}), cx).await;
    assert_eq!(
        undone["text"], text,
        "save destroyed the independent undo step"
    );
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.confirmation = app
                    .active_draft_id
                    .clone()
                    .map(crate::editor_lifecycle::NoteConfirmation::DiscardDraft);
                app.confirm_note_action(window, cx);
            });
        })
        .unwrap();
    until(
        window,
        view,
        "discard only undo-test recovery draft",
        |app, _| app.note_editor.is_none() && app.draft_discard.is_none() && !app.note_saving,
        cx,
    )
    .await;
    std::fs::write(database.with_file_name("native-save-view-result.txt"), "PASS: saving a scrolled long document keeps its generation, text, caret and viewport; reopening retains undo; only the test recovery draft is tombstoned.\n").unwrap();
}

/// The PowerShell runner verifies this last draft after normal window shutdown.
pub async fn close_with_draft(window: AnyWindowHandle, view: &Entity<StudyApp>, cx: &mut AsyncApp) {
    let editor = window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.clear_notices(cx);
                app.new_note(window, cx);
            });
            window.render_frame(cx);
            view.read(cx).note_content.clone()
        })
        .unwrap();
    #[cfg(windows)]
    crate::smoke_visibility::type_into(window, &editor, "关闭窗口前的末次输入 🧠", cx).await;
    #[cfg(not(windows))]
    bridge(
        window,
        &editor,
        json!({"type":"replace","from":0,"to":0,"text":"关闭窗口前的末次输入 🧠"}),
        cx,
    )
    .await;
    window
        .update(cx, |_, _, cx| {
            let app = view.read(cx);
            let id = app.active_draft_id.as_ref().unwrap();
            let root = std::path::Path::new(&app.last_library).parent().unwrap();
            std::fs::write(root.join("native-close-draft-id.txt"), id).unwrap();
        })
        .unwrap();
    #[cfg(windows)]
    crate::smoke_window_drag::click_window_control(
        window,
        view,
        "window-close",
        windows::Win32::UI::WindowsAndMessaging::HTCLOSE,
        cx,
    )
    .await;
    #[cfg(not(windows))]
    window
        .update(cx, |_, window, cx| {
            window.click("window-close", cx);
            assert!(
                view.read(cx).note_editor.is_some(),
                "pointer close must wait for the editor and disk acknowledgements"
            );
        })
        .unwrap();
}
