use crate::app::{Page, StudyApp};
use gpui_kit::*;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use studyapp_core::{Command, Engine};

#[cfg(feature = "ui-test")]
pub(crate) fn capture(window: &mut Window, path: PathBuf) -> anyhow::Result<()> {
    let started = std::time::Instant::now();
    eprintln!("native capture begin: {:?}", path.file_name());
    #[cfg(windows)]
    window.render_to_image()?.save(&path)?;
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::NSView;
        use wry::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        anyhow::ensure!(
            std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true"),
            "Native macOS captures are restricted to GitHub runners"
        );
        let RawWindowHandle::AppKit(raw) = HasWindowHandle::window_handle(window)?.as_raw() else {
            anyhow::bail!("macOS capture requires an AppKit window");
        };
        // SAFETY: the borrowed GPUI window owns this live NSView on the main
        // thread for this read; never activate, resize, or reparent it.
        let view = unsafe { raw.ns_view.cast::<NSView>().as_ref() };
        let id = view
            .window()
            .ok_or_else(|| anyhow::anyhow!("Capture window is closed"))?
            .windowNumber();
        // Capture the actual compositor, including WKWebView children, rather
        // than asking the debug Metal renderer for a second offscreen frame.
        let status = std::process::Command::new("/usr/sbin/screencapture")
            .args(["-x", "-o", "-l", &id.to_string()])
            .arg(&path)
            .status()?;
        anyhow::ensure!(status.success(), "macOS window capture failed: {status}");
    }
    #[cfg(target_os = "linux")]
    {
        use wry::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        // GPUI's Linux renderer has no readback implementation. Capture the
        // actual X11 window (including WebKit children) on the remote runner.
        anyhow::ensure!(
            std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true"),
            "Native Linux captures are restricted to GitHub runners"
        );
        let id = match HasWindowHandle::window_handle(window)?.as_raw() {
            RawWindowHandle::Xcb(handle) => u64::from(handle.window.get()),
            RawWindowHandle::Xlib(handle) => handle.window,
            _ => anyhow::bail!("Linux smoke requires an X11 window"),
        };
        let status = std::process::Command::new("import")
            .args(["-window", &id.to_string(), "-silent"])
            .arg(&path)
            .status()?;
        anyhow::ensure!(status.success(), "X11 window capture failed: {status}");
    }
    eprintln!(
        "native capture complete: {:?}, elapsedMs={}",
        path.file_name(),
        started.elapsed().as_millis()
    );
    Ok(())
}

#[cfg(all(feature = "ui-test", any(windows, test)))]
pub(crate) fn settings_capture_filename(window: &Window, label: &str) -> String {
    use gpui_kit::test::TestWindowExt;
    let layout = if window.try_find("settings-drawer-toggle").is_some() {
        "drawer"
    } else {
        "tree"
    };
    let pane_width = f32::from(window.find("settings-section-pane").bounds().size.width) as u32;
    format!(
        "settings-{label}-{}-{layout}-{pane_width}.png",
        f32::from(window.viewport_size().width) as u32
    )
}

#[cfg(all(feature = "ui-test", any(windows, test)))]
pub(crate) fn notification_capture_event(
    style: studyapp_core::NoticeStyle,
) -> studyapp_core::NoticeEvent {
    use studyapp_core::{NoticeCategory, NoticeEvent, NoticeSeverity, NoticeStyle};
    if matches!(style, NoticeStyle::Bubble | NoticeStyle::Snackbar) {
        NoticeEvent::new(
            NoticeCategory::OperationResult,
            NoticeSeverity::Success,
            "操作已完成。",
        )
    } else {
        NoticeEvent::new(
            NoticeCategory::SaveFailure,
            NoticeSeverity::Error,
            "这是通知显示回归样例。关闭提示不会丢弃尚未保存的内容。
长消息仍在卡片内部滚动，不移动下方页面。",
        )
    }
}

/// Yield to platform frames after navigation; consecutive Window::update calls
/// alone do not let the native frame loop present the selected page.
#[cfg(all(feature = "ui-test", any(windows, test)))]
pub(crate) async fn with_settings_capture<R>(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    target: &str,
    capture: impl FnOnce(&mut Window, &mut App) -> R,
    cx: &mut AsyncApp,
) -> R {
    use gpui_kit::test::TestWindowExt;
    let (send, receive) = async_channel::bounded(1);
    window
        .update(cx, |_, window, cx| {
            select_settings_page(window, view, target, cx);
            // The first frame commits parent/child invalidations; the next
            // observes their settled render. Neither callback captures pixels.
            window.on_next_frame(move |window, cx| {
                window.render_frame(cx);
                window.on_next_frame(move |window, cx| {
                    window.render_frame(cx);
                    let _ = send.try_send(());
                });
            });
        })
        .unwrap();
    let mut signal = std::pin::pin!(receive.recv());
    let mut timeout = std::pin::pin!(cx.background_executor().timer(Duration::from_secs(5)));
    std::future::poll_fn(|cx| {
        if let std::task::Poll::Ready(result) = signal.as_mut().poll(cx) {
            result.expect("settings capture frame was cancelled");
            return std::task::Poll::Ready(());
        }
        assert!(
            timeout.as_mut().poll(cx).is_pending(),
            "settings capture did not receive platform frames for {target}"
        );
        std::task::Poll::Pending
    })
    .await;
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            capture(window, cx)
        })
        .unwrap()
}

#[cfg(feature = "ui-test")]
pub(crate) fn styled_summary_geometry(window: &mut Window, database: &Path) {
    use gpui_kit::test::TestWindowExt;
    assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    let surface = window.find("native-html-surface").bounds();
    let word = window.find(("native-html-run", 0_usize)).bounds();
    assert!(
        word.size.height >= px(35.),
        "20px HTML text must use its 36px line height"
    );
    assert!(word.left() >= surface.left() && word.right() <= surface.right());
    std::fs::write(
        database.with_file_name("native-markdown-editor-default-summary-html.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "htmlSummary": true, "fontSize": 20, "lineHeight": 36,
            "actualHeight": f32::from(word.size.height),
            "surfaceWidth": f32::from(surface.size.width),
            "viewportWidth": f32::from(window.viewport_size().width),
        }))
        .unwrap(),
    )
    .unwrap();
}

pub fn prepare(database: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(
        !database.exists(),
        "冒烟测试必须使用新的空目录，以免改变已有资料"
    );
    let folder = database
        .parent()
        .ok_or_else(|| anyhow::anyhow!("无效测试目录"))?;
    // Smoke scripts deliberately pass a fresh path. Normal startup now leaves
    // directory access to the recoverable worker, so the fixture owns this step.
    std::fs::create_dir_all(folder)?;
    anyhow::ensure!(
        std::fs::read_dir(folder)?.next().transpose()?.is_none(),
        "冒烟测试必须使用新的空目录，以免改变已有资料"
    );
    let note = folder.join("example.md");
    std::fs::write(
        &note,
        "# Rust 所有权\n\n每个值都有一个所有者。\n\n- 借用允许访问数据\n- 生命周期约束引用\n\n```rust\nlet name = String::from(\"StudyApp\");\n```\n\n| 主题 | 状态 |\n| --- | --- |\n| 所有权 | 已学习 |\n",
    )?;
    let now = chrono::Utc::now().timestamp();
    let mut engine = Engine::open(database, now)?;
    engine.apply(Command::Import(vec![note]), now)?;
    let id = engine.snapshot().notes[0].id.clone();
    engine.apply(
        Command::Manual {
            version_ids: None,
            started_at: now - 172800,
            minutes: 45,
            summary: format!(
                "{}{}{}",
                "<span style='font-size:20px;line-height:1.8;color:#586D83;background-color:#90CAF940'>HTML 摘要🙂</span>

",
                "理解 Rust 所有权与借用规则。".repeat(8),
                "UnbrokenSummary".repeat(10)
            ),
            note_ids: vec![id],
        },
        now,
    )?;
    engine.checkpoint()?;
    Ok(())
}

#[cfg(feature = "ui-test")]
/// Navigate by actual drawer/tree interactions, including scrolled categories.
pub(crate) fn select_settings_page(
    window: &mut Window,
    view: &Entity<StudyApp>,
    page: &str,
    cx: &mut App,
) {
    use crate::smoke_library::{click_scrolled, reveal_scrolled};
    use gpui_kit::test::TestWindowExt;

    let target = match page {
        "appearance" | "appearance-overview" => "appearance.overview",
        "appearance-theme-editor" => "appearance.theme",
        "appearance-workspace" => "appearance.workspace",
        "appearance-widget" => "appearance.widget",
        "appearance-notifications" => "notices.general",
        "notifications" => "notices.rules",
        "storage" | "storage-overview" => "storage.overview",
        "storage-copies" => "storage.copies",
        "storage-images" => "storage.images",
        "storage-library" => "storage.library",
        "about-account" => "about",
        other => other,
    };
    let group = target.split('.').next().unwrap();
    let group_id = crate::settings_navigation::group_element_id(group);
    window.render_frame(cx);
    if window.try_find(group_id.clone()).is_none() {
        window.click("settings-drawer-toggle", cx);
        window.render_frame(cx);
    }
    assert!(window.try_find("settings-tree-width-reset").is_none());
    reveal_scrolled(window, group_id.clone(), "settings-tree-scroll", cx);
    let tree_viewport = window.find("settings-tree-scroll").bounds();
    assert!(
        window.find(group_id.clone()).bounds().right()
            <= tree_viewport.right() - gpui_kit::component::scroll::Scrollbar::width(),
        "category controls overlap the scrollbar lane"
    );
    let id = if target.contains('.') {
        if !view
            .read(cx)
            .preferences_ui
            .browsing
            .expanded
            .contains(group)
        {
            window.click(group_id, cx);
            window.render_frame(cx);
        }
        SharedString::from(format!("settings-target-{target}"))
    } else {
        group_id
    };
    click_scrolled(window, id, "settings-tree-scroll", cx);
    window.render_frame(cx);
    assert_eq!(view.read(cx).preferences_ui.browsing.selected, target);
    assert!(!view.read(cx).preferences_ui.settings_drawer);
}

#[cfg(feature = "ui-test")]
fn settings_save_state(window: &Window, view: &Entity<StudyApp>, cx: &App) -> serde_json::Value {
    use gpui_kit::test::TestWindowExt;
    let app = view.read(cx);
    let button = window.find("save-settings");
    let bounds = button.bounds();
    let center = bounds.center();
    let occlusions = app.notifications.read(cx).occlusions();
    serde_json::json!({
        "page":format!("{:?}",app.page),
        "target":app.preferences_ui.browsing.selected,
        "value":app.interval.read(cx).value().as_str(),
        "dirty":app.settings_dirty(cx),
        "preferences_busy":app.preferences_ui.busy,
        "replacing":app.preferences_ui.replacing,
        "recording":app.preferences_ui.recording.is_some(),
        "storage_busy":app.storage_busy,
        "sync_busy":app.sync_panel.read(cx).status.busy,
        "modal":app.has_modal(cx),
        "error":app.preferences_ui.error,
        "window_active":window.is_window_active(),
        "viewport":[f32::from(window.viewport_size().width),f32::from(window.viewport_size().height)],
        "button":{"disabled":button.disabled(),"visible":button.visible(),
            "bounds":[f32::from(bounds.left()),f32::from(bounds.top()),
                f32::from(bounds.size.width),f32::from(bounds.size.height)]},
        "notification_covers_center":occlusions.iter().any(|bounds|bounds.contains(&center)),
        "notification_bounds":occlusions.iter().map(|bounds|[
            f32::from(bounds.left()),f32::from(bounds.top()),
            f32::from(bounds.size.width),f32::from(bounds.size.height)]).collect::<Vec<_>>()
    })
}

/// Edit the current numeric control, commit its change notification, then
/// click the real global save button in a separate update transaction.
#[cfg(feature = "ui-test")]
pub(crate) fn queue_interval_save(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    minutes: u32,
    evidence: Option<&Path>,
    cx: &mut AsyncApp,
) {
    use gpui_kit::component::input::InputEvent;
    use gpui_kit::test::TestWindowExt;
    window
        .update(cx, |_, window, cx| {
            select_settings_page(window, view, "learning", cx);
            let input = view.read(cx).interval.clone();
            input.update(cx, |input, cx| {
                input.set_value(minutes.to_string(), window, cx);
                cx.emit(InputEvent::Change);
            });
            assert!(view.read(cx).settings_dirty(cx));
        })
        .expect("edit learning interval");
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let before = settings_save_state(window, view, cx);
            window.click("save-settings", cx);
            let after = settings_save_state(window, view, cx);
            let queued = view.read(cx).preferences_ui.busy;
            if !queued && let Some(database) = evidence {
                let record = serde_json::json!({"before":before,"after":after});
                let path = database.with_file_name("settings-save-failure.json");
                if let Err(error) =
                    std::fs::write(path, serde_json::to_vec_pretty(&record).unwrap())
                {
                    eprintln!("could not write settings save diagnostic: {error}");
                }
                if let Err(error) =
                    capture(window, database.with_file_name("settings-save-failure.png"))
                {
                    eprintln!("could not capture settings save failure: {error}");
                }
            }
            assert!(
                queued,
                "settings save was not queued: before={before}; after={after}"
            );
        })
        .expect("queue settings save");
}

#[cfg(feature = "ui-test")]
async fn save_interval(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    database: &Path,
    minutes: u32,
    cx: &mut AsyncApp,
) {
    queue_interval_save(window, view, minutes, Some(database), cx);
    crate::smoke_library::until(
        window,
        view,
        "settings save acknowledged with the expected interval",
        |app, cx| {
            assert!(
                app.preferences_ui.error.is_none(),
                "{:?}",
                app.preferences_ui.error
            );
            !app.preferences_ui.busy
                && !app.settings_dirty(cx)
                && app
                    .snapshot
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.settings.interval_minutes == minutes)
                && app
                    .preferences_ui
                    .saved
                    .as_ref()
                    .is_some_and(|saved| saved.settings.interval_minutes == minutes)
        },
        cx,
    )
    .await;
}

#[cfg(feature = "ui-test")]
fn reveal_learning_control(
    window: &mut Window,
    view: &Entity<StudyApp>,
    id: impl Into<ElementId>,
    cx: &mut App,
) {
    use gpui_kit::test::TestWindowExt;
    let id = id.into();
    window.render_frame(cx);
    let scroll = view.read(cx).page_scroll.clone();
    let target = window.find(id.clone()).bounds();
    let viewport = scroll.bounds();
    let overflow = (target.bottom() - viewport.bottom() + px(8.)).max(px(0.));
    scroll.set_offset(scroll.offset() - point(px(0.), overflow));
    window.render_frame(cx);
    assert!(
        window.find(id).visible(),
        "learning control must be in the scroll viewport"
    );
}

pub fn exercise(window: AnyWindowHandle, view: Entity<StudyApp>, database: PathBuf, cx: &mut App) {
    cx.spawn(async move |cx| {
        cx.background_executor().timer(Duration::from_secs(2)).await;
        #[cfg(all(windows, feature = "ui-test"))]
        if std::env::var("WW_STUDYAPP_SMOKE_SCENARIO").as_deref() == Ok("updates") {
            Box::pin(crate::smoke_updates::exercise(window, &view, &database, cx)).await;
            cx.update(|cx| cx.quit());
            return;
        }
        #[cfg(all(windows, feature = "ui-test"))]
        if std::env::var("WW_STUDYAPP_SMOKE_SCENARIO").as_deref() == Ok("inputs-ime") {
            Box::pin(crate::smoke_inputs::exercise_ime(window, &view, &database, cx)).await;
            cx.update(|cx| cx.quit());
            return;
        }
        #[cfg(all(windows, feature = "ui-test"))]
        if std::env::var("WW_STUDYAPP_SMOKE_SCENARIO").as_deref() == Ok("inputs") {
            Box::pin(crate::smoke_inputs::exercise(window, &view, &database, cx)).await;
            cx.update(|cx| cx.quit());
            return;
        }
        #[cfg(all(windows, feature = "ui-test"))]
        if std::env::var("WW_STUDYAPP_SMOKE_SCENARIO").as_deref() == Ok("note-rename") {
            Box::pin(crate::smoke_note_rename::exercise(window, &view, &database, cx)).await;
            // The scenario retains a live draft and ends with a real native
            // close. Let its durable-close acknowledgement terminate the app.
            return;
        }
        #[cfg(all(windows, feature = "ui-test"))]
        if std::env::var("WW_STUDYAPP_SMOKE_SCENARIO").as_deref() == Ok("window-drag") {
            // Movement has its own visibility precondition; unrelated keyboard
            // editing remains covered by the full smoke's strict first_open.
            Box::pin(crate::smoke_window_drag::exercise(window, &view, &database, cx)).await;
            cx.update(|cx| cx.quit());
            return;
        }
        #[cfg(all(windows, feature = "ui-test"))]
        if std::env::var("WW_STUDYAPP_SMOKE_SCENARIO").as_deref() == Ok("timer-widget") {
            Box::pin(crate::smoke_timer_widget::exercise(window, &view, &database, cx)).await;
            // The widget probe finishes through the real durable-close path.
            // Forcing quit here would bypass the very lifecycle it verifies.
            return;
        }
        #[cfg(all(windows, feature = "ui-test"))]
        Box::pin(crate::smoke_visibility::first_open(window, &view, &database, cx)).await;
        #[cfg(all(feature = "ui-test", any(target_os = "macos", target_os = "linux")))]
        Box::pin(crate::smoke_editor::default_preview(window, &view, &database, cx)).await;
        #[cfg(all(feature = "ui-test", not(test)))]
        Box::pin(crate::smoke_timer_pin::exercise(window, &view, &database, cx)).await;
        for (index, page) in [Page::Today, Page::Notes, Page::Reviews, Page::Stats, Page::Settings].into_iter().enumerate() {
            #[cfg(feature = "ui-test")]
            eprintln!("native page frame begin: {index}, {page:?}");
            let result = window.update(cx, |_, window, cx| {
                view.update(cx, |app, cx| {
                    app.page = page;
                    if page == Page::Notes {
                        app.selected_note = app.snapshot.as_ref().and_then(|s| s.notes.first().map(|n| n.id.clone()));
                    }
                    cx.notify();
                });
                window.refresh();
                window.draw(cx).clear(cx);
            });
            if let Err(error) = result { eprintln!("smoke window failed: {error}"); std::process::exit(2); }
            #[cfg(feature = "ui-test")]
            eprintln!("native page frame complete: {index}, {page:?}");
            cx.background_executor().timer(Duration::from_millis(700)).await;
            #[cfg(feature = "ui-test")]
            {
                let screenshot = database.with_file_name(format!("page-{index}.png"));
                let capture = window.update(cx, |_, window, _| -> anyhow::Result<()> {
                    capture(window, screenshot)?;
                    Ok(())
                });
                if !matches!(capture, Ok(Ok(()))) { eprintln!("screenshot failed: {capture:?}"); std::process::exit(3); }
            }
            #[cfg(not(feature = "ui-test"))]
            let _ = index;
        }
        for attempt in 0..200 {
            let ready = window.update(cx, |_, _, cx| {
                let editor = view.read(cx).note_content.read(cx);
                assert!(editor.error().is_none(), "{:?}", editor.error());
                editor.ready()
            }).expect("editor startup");
            if ready { break; }
            assert!(attempt < 199, "offline editor did not become ready");
            cx.background_executor().timer(Duration::from_millis(50)).await;
        }
        #[cfg(feature = "ui-test")]
        {
            use gpui_kit::test::TestWindowExt;
            #[cfg(windows)]
            if std::env::var("WW_STUDYAPP_SMOKE_SCENARIO").as_deref() == Ok("settings") {
                Box::pin(crate::smoke_settings::exercise(window, &view, &database, cx)).await;
                cx.update(|cx| cx.quit());
                return;
            }
            #[cfg(windows)]
            if std::env::var("WW_STUDYAPP_SMOKE_SCENARIO").as_deref() == Ok("clipboard") {
                Box::pin(crate::smoke_clipboard::exercise(window, &view, &database, cx)).await;
                cx.update(|cx| cx.quit());
                return;
            }
            #[cfg(windows)]
            if std::env::var("WW_STUDYAPP_SMOKE_SCENARIO").as_deref() == Ok("shortcuts") {
                Box::pin(crate::smoke_workspace::saved_shortcuts(window, &view, &database, cx)).await;
                std::fs::write(database.with_file_name("shortcut-only-result.txt"), "PASS: isolated custom shortcut scenario; full native smoke was not run.\n").unwrap();
                cx.update(|cx| cx.quit());
                return;
            }
            #[cfg(windows)]
            Box::pin(crate::smoke_statistics::exercise(window, &view, &database, cx)).await;
            for width in [1000., 1400.] {
            window.update(cx, |_, window, _| crate::smoke_library::resize(window, size(px(width), px(900.)))).expect("review resize");
            cx.background_executor().timer(Duration::from_millis(400)).await;
            window.update(cx, |_, window, cx| {
                window.click(("nav", 2_usize), cx);
                assert_eq!(view.read(cx).page, Page::Reviews);
                let review = view.read(cx).snapshot.as_ref().expect("loaded snapshot").reviews[0].clone();
                window.render_frame(cx);
                let card = window.find(SharedString::from(format!("review-card-{}", review.id))).bounds();
                let title = window.find("review-summary").bounds();
                let status = window.find(SharedString::from(format!("review-status-{}", review.id))).bounds();
                let check = window.find(SharedString::from(format!("review-check-{}", review.id))).bounds();
                assert!(title.right() <= status.left() && title.left() >= check.right(), "long review summary overlaps controls");
                assert!(status.right() <= card.right() && title.size.height > px(40.), "review card must wrap its complete long summary");
                assert!(review.title.contains(&"UnbrokenSummary".repeat(10)), "stored summary was truncated");
                styled_summary_geometry(window, &database);
                if width == 1400. { capture(window, database.with_file_name("native-markdown-editor-summary-html.png")).unwrap(); }
                if width == 1400. { window.click(SharedString::from(format!("review-check-{}", review.id)), cx); }
            }).expect("review resize and click");
            }
            cx.background_executor().timer(Duration::from_secs(1)).await;
            window.update(cx, |_, window, cx| {
                assert_eq!(view.read(cx).snapshot.as_ref().unwrap().review_events.len(), 1);
                window.click(("review-filter", 1_usize), cx);
                let review = view.read(cx).snapshot.as_ref().unwrap().reviews[0].clone();
                window.click(SharedString::from(format!("undo-{}", review.id)), cx);
            }).expect("undo click");
            cx.background_executor().timer(Duration::from_secs(1)).await;
            window.update(cx, |_, window, cx| {
                assert!(view.read(cx).snapshot.as_ref().unwrap().review_events.is_empty());
                window.click(("nav", 0_usize), cx);
                window.click("pause", cx);
            }).expect("pause click");
            cx.background_executor().timer(Duration::from_secs(1)).await;
            window.update(cx, |_, window, cx| {
                assert!(view.read(cx).snapshot.as_ref().unwrap().timer.paused);
                window.click(("nav", 4_usize), cx);
            }).expect("settings navigation");
            save_interval(window, &view, &database, 25, cx).await;
            window.update(cx, |_, _, cx| {
                assert_eq!(view.read(cx).snapshot.as_ref().unwrap().settings.interval_minutes, 25);
            }).expect("settings persisted");
            let backup = database.with_file_name("smoke-backup.sqlite3");
            window.update(cx, |_, _, cx| {
                view.update(cx, |app, cx| app.send(Command::Backup(backup.clone()), cx));
            }).expect("backup command");
            cx.background_executor().timer(Duration::from_secs(1)).await;
            assert!(backup.exists());
            save_interval(window, &view, &database, 10, cx).await;
            window.update(cx, |_, window, cx| {
                assert_eq!(view.read(cx).snapshot.as_ref().unwrap().settings.interval_minutes, 10);
                view.update(cx, |app, cx| {
                    app.open_editor(crate::app::EditorMode::Manual, window, cx);
                    app.send(Command::Restore(backup), cx);
                });
            }).expect("restore command");
            cx.background_executor().timer(Duration::from_secs(1)).await;
            window.update(cx, |_, _, cx| {
                let app = view.read(cx);
                assert_eq!(app.snapshot.as_ref().unwrap().settings.interval_minutes, 10, "restore preserves independently saved preferences");
                assert_eq!(app.interval.read(cx).value().as_ref(), "10");
                assert!(app.editor.is_none());
            }).expect("restore refreshed interface");
            window.update(cx, |_, window, cx| {
                window.click(("nav", 0_usize), cx);
                window.click("manual-top", cx);
                let version = view.read(cx).snapshot.as_ref().unwrap().versions[0].id.clone();
                assert!(view.read(cx).chosen_versions.contains(&version));
                reveal_learning_control(window, &view, format!("associate-version-{version}"), cx);
                window.click(SharedString::from(format!("associate-version-{version}")), cx);
                assert!(!view.read(cx).chosen_versions.contains(&version));
                view.update(cx, |app, cx| app.summary.update(cx, |s, cx| s.set_value("## 学习摘要\n\n- 确认阅读了**所有权**快照\n- `中文🧠`", window, cx)));
                reveal_learning_control(window, &view, "save-learning", cx);
                window.click("save-learning", cx);
            }).expect("version selection validation");
            cx.background_executor().timer(Duration::from_secs(1)).await;
            window.update(cx, |_, window, cx| {
                assert!(view.read(cx).editor.is_some());
                assert!(view.read(cx).notice.as_ref().is_some_and(Result::is_err));
                let version = view.read(cx).snapshot.as_ref().unwrap().versions[0].id.clone();
                reveal_learning_control(window, &view, format!("associate-version-{version}"), cx);
                window.click(SharedString::from(format!("associate-version-{version}")), cx);
                capture(window, database.with_file_name("learning-editor.png")).unwrap();
                window.render_frame(cx);
                assert!(view.read(cx).summary_editor.read(cx).is_preview());
                assert!(window.try_find("summary-markdown-preview").is_some());
                capture(window, database.with_file_name("learning-summary-preview.png")).unwrap();
                reveal_learning_control(window, &view, "save-learning", cx);
                window.click("save-learning", cx);
            }).expect("save selected version");
            cx.background_executor().timer(Duration::from_secs(1)).await;
            window.update(cx, |_, window, cx| {
                let app = view.read(cx);
                assert!(app.editor.is_none());
                let snapshot = app.snapshot.as_ref().unwrap();
                assert_eq!(snapshot.sessions.len(), 2);
                let source = "## 学习摘要\n\n- 确认阅读了**所有权**快照\n- `中文🧠`";
                let session = snapshot.sessions.iter().find(|s| s.summary == source).unwrap();
                assert_eq!(session.version_ids, vec![snapshot.versions[0].id.clone()]);
                assert_eq!(snapshot.reviews.len(), 2);
                let session_id = session.id.clone();
                view.update(cx, |app, cx| app.open_editor(crate::app::EditorMode::Edit(session_id), window, cx));
                assert_eq!(view.read(cx).summary.read(cx).value().as_str(), source);
                assert!(view.read(cx).summary_editor.read(cx).is_preview());
                window.click("cancel-editor", cx);
            }).expect("saved learning snapshot association");

            // Exercise native file-drop delivery, then the actual managed-note controls.
            let dropped = database.with_file_name("dropped.md");
            std::fs::write(&dropped, "# 拖入的 Markdown\n文件拖放集成验证").unwrap();
            window.update(cx, |_, window, cx| {
                window.click(("nav", 1_usize), cx);
                let position = point(px(450.), px(200.));
                let count = view.read(cx).snapshot.as_ref().unwrap().notes.len();
                window.dispatch_event(PlatformInput::FileDrop(FileDropEvent::Entered { position, paths: ExternalPaths(vec![dropped.clone()].into_iter().collect()) }), cx);
                window.render_frame(cx);
                assert!(view.read(cx).external_drop.active());
                assert!(window.try_find("external-drop-overlay").is_some());
                #[cfg(windows)]
                assert_eq!(view.read(cx).note_content.read(cx).native_state().unwrap()["controllerVisible"], false);
                window.press("escape", cx);
                window.render_frame(cx);
                assert!(!view.read(cx).external_drop.active(), "Escape retained the drag overlay");
                assert_eq!(view.read(cx).snapshot.as_ref().unwrap().notes.len(), count);
                view.update(cx, |app, cx| { app.storage_busy = true; cx.notify(); });
                window.dispatch_event(PlatformInput::FileDrop(FileDropEvent::Entered { position, paths: ExternalPaths(vec![dropped.clone()].into_iter().collect()) }), cx);
                window.render_frame(cx);
                assert_eq!(view.read(cx).external_drop.blocked(), Some(crate::external_drop::DropBlock::Busy));
                window.dispatch_event(PlatformInput::FileDrop(FileDropEvent::Submit { position }), cx);
                window.render_frame(cx);
                assert!(!view.read(cx).external_drop.active());
                assert_eq!(view.read(cx).snapshot.as_ref().unwrap().notes.len(), count, "busy drop imported content");
                view.update(cx, |app, cx| { app.storage_busy = false; cx.notify(); });
                window.dispatch_event(PlatformInput::FileDrop(FileDropEvent::Entered { position, paths: ExternalPaths(vec![dropped].into_iter().collect()) }), cx);
                window.render_frame(cx);
                window.dispatch_event(PlatformInput::FileDrop(FileDropEvent::Pending { position }), cx);
                window.render_frame(cx);
                window.dispatch_event(PlatformInput::FileDrop(FileDropEvent::Submit { position }), cx);
            }).expect("native file drop");
            cx.background_executor().timer(Duration::from_secs(1)).await;
            let managed_id = Box::pin(crate::smoke_editor::exercise(window, &view, &database, cx)).await;
            #[cfg(windows)]
            Box::pin(crate::smoke_editor::exercise_multiple_drafts(window, &view, &database, cx)).await;
            let sync_folder = database.with_file_name("sync-target");
            std::fs::create_dir(&sync_folder).unwrap();
            window.update(cx, |_, window, cx| {
                let snapshot = view.read(cx).snapshot.as_ref().unwrap();
                assert_eq!(snapshot.versions.iter().filter(|v| v.note_id == managed_id).count(), 2);
                view.update(cx, |app, cx| app.clear_notices(cx));
                window.click(("nav", 4_usize), cx);
                select_settings_page(window, &view, "sync", cx);
                assert!(view.read(cx).preferences_ui.section == crate::settings_ui::SettingsSection::Sync,
                    "Sync category must be selected before using its controls");
                window.render_frame(cx);
                studyapp_ui::settings_components::select_choice(window, "sync-target", 1, cx);
                let panel = view.read(cx).sync_panel.clone();
                panel.update(cx, |panel, cx| panel.folder.update(cx, |s, cx| s.set_value(sync_folder.to_string_lossy().into_owned(), window, cx)));
                window.click("save-settings", cx);
            }).expect("configure folder sync through interface");
            cx.background_executor().timer(Duration::from_secs(1)).await;
            window.update(cx, |_, window, cx| {
                crate::smoke_library::reveal_scrolled(window, "test-sync".into(), "page-scroll", cx);
                window.click("test-sync", cx);
            }).expect("test folder connection");
            cx.background_executor().timer(Duration::from_secs(1)).await;
            window.update(cx, |_, window, cx| {
                assert!(view.read(cx).sync_panel.read(cx).status.message.contains("连接测试成功"), "{:?}", view.read(cx).sync_panel.read(cx).status);
                window.click("sync-now", cx);
            }).expect("sync notes through interface");
            cx.background_executor().timer(Duration::from_secs(1)).await;
            window.update(cx, |_, window, cx| {
                let panel = view.read(cx).sync_panel.read(cx);
                assert!(!panel.status.busy);
                assert!(panel.status.last_success.is_some(), "{}", panel.status.message);
                assert!(sync_folder.join("StudyApp/notes").join(format!("{managed_id}.md")).is_file());
                window.render_frame(cx);
                capture(window, database.with_file_name("sync-settings.png")).unwrap();
            }).expect("synced markdown verified");
            #[cfg(windows)]
            Box::pin(crate::smoke_library::exercise(window, &view, &database, cx)).await;
            #[cfg(windows)]
            Box::pin(crate::smoke_interactions::exercise(window, &view, &database, cx)).await;
            #[cfg(windows)]
            Box::pin(crate::smoke_settings::exercise(window, &view, &database, cx)).await;
            #[cfg(windows)]
            Box::pin(crate::smoke_workspace::exercise(window, &view, &database, cx)).await;
            #[cfg(windows)]
            Box::pin(crate::smoke_editor::original_documents(window, &view, &database, cx)).await;
            Box::pin(crate::smoke_library::application_migration(window,&view,&database,cx)).await;
        }
        let report = database.with_file_name("smoke-result.txt");
        #[cfg(feature = "ui-test")]
        let result = "PASS: five native pages rendered; screenshots captured; navigation, reviews, pause, settings, backup restore, learning versions, native file drop, continuous WebView Markdown editing, offline syntax highlighting, cross-paragraph undo/redo, source toggle, CRLF roundtrip, async save, draft recovery/deletion, unmanage/reimport, long version history scrolling and native input after history, logical folders, safe single-copy cleanup, library migration, folder connection and actual Markdown sync verified.\n";
        #[cfg(not(feature = "ui-test"))]
        let result = "PASS: release executable launched, five native pages rendered and offline WebView editor became ready; interactive assertions and screenshots require ui-test.\n";
        std::fs::write(report, result).expect("write smoke report");
        #[cfg(feature = "ui-test")]
        crate::smoke_editor::close_with_draft(window, &view, cx).await;
        #[cfg(not(feature = "ui-test"))]
        cx.update(|cx| cx.quit());
    }).detach();
}

#[cfg(test)]
mod tests {
    use super::prepare;
    use studyapp_core::Engine;

    #[test]
    #[cfg(feature = "ui-test")]
    fn native_notification_capture_events_reach_requested_feedback_surfaces() {
        use studyapp_core::{
            NoticeEnvironment, NoticeRoute, NoticeRule, NoticeStyle, NotificationAppearance,
            NotificationSettings,
        };
        for style in NoticeStyle::ALL {
            let event = super::notification_capture_event(style);
            event.validate().unwrap();
            let mut policy = NotificationSettings::default();
            policy.rules.insert(
                event.category,
                NoticeRule {
                    style: Some(style),
                    ..Default::default()
                },
            );
            let expected = if matches!(
                style,
                NoticeStyle::Bubble
                    | NoticeStyle::Snackbar
                    | NoticeStyle::CornerCard
                    | NoticeStyle::Banner
                    | NoticeStyle::ModalCard
            ) {
                style
            } else {
                NoticeStyle::CornerCard
            };
            assert_eq!(
                policy.route_event(
                    &event,
                    &NotificationAppearance::default(),
                    NoticeEnvironment {
                        foreground: true,
                        desktop_available: false,
                        anchor_available: false,
                        minute: 720,
                    }
                ),
                NoticeRoute::Foreground(expected),
                "capture fixture does not exercise {style:?}"
            );
        }
    }

    #[test]
    fn prepare_rejects_nonempty_directory_without_overwriting_notes_or_creating_database() {
        let directory = tempfile::tempdir().unwrap().keep();
        let note = directory.join("example.md");
        let original = "# 用户原有笔记\n保留这些内容。\n";
        std::fs::write(&note, original).unwrap();
        let database = directory.join("studyapp.sqlite3");

        assert!(prepare(&database).is_err());
        assert_eq!(std::fs::read_to_string(&note).unwrap(), original);
        assert!(!database.exists());
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
    }

    #[test]
    fn prepare_accepts_an_existing_empty_directory() {
        let directory = tempfile::tempdir().unwrap().keep();
        let database = directory.join("studyapp.sqlite3");

        prepare(&database).unwrap();

        assert!(database.is_file());
        assert!(directory.join("example.md").is_file());
        let engine = Engine::open(&database, chrono::Utc::now().timestamp()).unwrap();
        assert_eq!(engine.snapshot().notes.len(), 1);
        assert_eq!(engine.snapshot().sessions.len(), 1);
    }

    #[test]
    fn prepare_creates_a_fresh_missing_directory_without_touching_neighbors() {
        let root = tempfile::tempdir().unwrap().keep();
        let neighbor = root.join("existing-note.md");
        let original = "# 已有笔记\n测试目录之外的内容保持不变。\n";
        std::fs::write(&neighbor, original).unwrap();
        let directory = root.join("fresh/nested-smoke");
        let database = directory.join("studyapp.sqlite3");
        assert!(!directory.exists());

        prepare(&database).unwrap();

        assert!(database.is_file());
        assert!(directory.join("example.md").is_file());
        assert_eq!(std::fs::read_to_string(&neighbor).unwrap(), original);
        let engine = Engine::open(&database, chrono::Utc::now().timestamp()).unwrap();
        assert_eq!(engine.snapshot().notes.len(), 1);
        assert_eq!(engine.snapshot().sessions.len(), 1);
    }
}
