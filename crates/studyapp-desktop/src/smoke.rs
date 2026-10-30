use crate::app::{Page, StudyApp};
use gpui_kit::*;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use studyapp_core::{Command, Engine};

#[cfg(feature = "ui-test")]
pub(crate) fn capture(window: &mut Window, path: PathBuf) -> anyhow::Result<()> {
    #[cfg(not(target_os = "linux"))]
    window.render_to_image()?.save(path)?;
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
            .arg(path)
            .status()?;
        anyhow::ensure!(status.success(), "X11 window capture failed: {status}");
    }
    Ok(())
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
                "{}{}",
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
            cx.update(|cx| cx.quit());
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
        Box::pin(crate::smoke_visibility::first_open(window, &view, &database, cx)).await;
        for (index, page) in [Page::Today, Page::Notes, Page::Reviews, Page::Stats, Page::Settings].into_iter().enumerate() {
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
                view.update(cx, |app, cx| app.interval.update(cx, |s, cx| s.set_value("25", window, cx)));
                window.click("save-settings", cx);
            }).expect("settings click");
            cx.background_executor().timer(Duration::from_secs(1)).await;
            window.update(cx, |_, _, cx| {
                assert_eq!(view.read(cx).snapshot.as_ref().unwrap().settings.interval_minutes, 25);
            }).expect("settings persisted");
            let backup = database.with_file_name("smoke-backup.sqlite3");
            window.update(cx, |_, _, cx| {
                view.update(cx, |app, cx| app.send(Command::Backup(backup.clone()), cx));
            }).expect("backup command");
            cx.background_executor().timer(Duration::from_secs(1)).await;
            assert!(backup.exists());
            window.update(cx, |_, window, cx| {
                view.update(cx, |app, cx| app.interval.update(cx, |s, cx| s.set_value("10", window, cx)));
                window.click("save-settings", cx);
            }).expect("temporary settings");
            cx.background_executor().timer(Duration::from_secs(1)).await;
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
                window.click(SharedString::from(format!("associate-version-{version}")), cx);
                assert!(!view.read(cx).chosen_versions.contains(&version));
                view.update(cx, |app, cx| app.summary.update(cx, |s, cx| s.set_value("## 学习摘要\n\n- 确认阅读了**所有权**快照\n- `中文🧠`", window, cx)));
                window.click("save-learning", cx);
            }).expect("version selection validation");
            cx.background_executor().timer(Duration::from_secs(1)).await;
            window.update(cx, |_, window, cx| {
                assert!(view.read(cx).editor.is_some());
                assert!(view.read(cx).notice.as_ref().is_some_and(Result::is_err));
                let version = view.read(cx).snapshot.as_ref().unwrap().versions[0].id.clone();
                window.click(SharedString::from(format!("associate-version-{version}")), cx);
                capture(window, database.with_file_name("learning-editor.png")).unwrap();
                window.render_frame(cx);
                assert!(view.read(cx).summary_editor.read(cx).is_preview());
                assert!(window.try_find("summary-markdown-preview").is_some());
                capture(window, database.with_file_name("learning-summary-preview.png")).unwrap();
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
                window.click("settings-section-sync", cx);
                assert!(view.read(cx).preferences_ui.section == crate::settings_ui::SettingsSection::Sync,
                    "Sync category must be selected before using its controls");
                window.render_frame(cx);
                window.click(("sync-target", 1_usize), cx);
                let panel = view.read(cx).sync_panel.clone();
                panel.update(cx, |panel, cx| panel.folder.update(cx, |s, cx| s.set_value(sync_folder.to_string_lossy().into_owned(), window, cx)));
                window.click("save-settings", cx);
            }).expect("configure folder sync through interface");
            cx.background_executor().timer(Duration::from_secs(1)).await;
            window.update(cx, |_, window, cx| {
                #[cfg(windows)]
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
