//! Remote-only updater fixture; all release checks and downloads are intercepted.
use crate::{
    app::{Page, StudyApp},
    smoke_library::until,
    update_ui::schedule_startup,
};
use gpui_kit::{test::TestWindowExt, *};
use std::path::Path;

fn release(version: &str) -> String {
    let name = format!("WW-StudyApp-{version}-windows-x86_64.zip");
    let url = format!("https://github.com/xwwpy/WW-StudyAPP/releases/download/v{version}/{name}");
    serde_json::json!({
        "tag_name": format!("v{version}"), "draft": false, "prerelease": false,
        "body": "# Update fixture\n\n- **Formatting** and `code`\n\n[Help](https://example.com/help)", "html_url": format!("https://github.com/xwwpy/WW-StudyAPP/releases/tag/v{version}"),
        "assets": [
            {"name": name, "browser_download_url": url, "state": "uploaded", "size": 1024, "digest": format!("sha256:{}", "a".repeat(64))},
            {"name": format!("{name}.sha256"), "browser_download_url": format!("{url}.sha256"), "state": "uploaded", "size": 112, "digest": format!("sha256:{}", "b".repeat(64))}
        ]
    }).to_string()
}

pub(crate) async fn exercise(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    database: &Path,
    cx: &mut AsyncApp,
) {
    assert_eq!(
        std::env::var("WW_STUDYAPP_SMOKE_SCENARIO").as_deref(),
        Ok("updates")
    );
    until(
        window,
        view,
        "update fixture library",
        |app, _| app.snapshot.is_some(),
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            let panel = view.read(cx).update_panel.clone();
            panel.update(cx, |panel, _| panel.fixture_response(None));
            window.render_frame(cx);
            assert!(window.try_find("sidebar-update").is_none(), "no idle card");
            for _ in 0..3 {
                schedule_startup(panel.clone(), cx);
            }
        })
        .unwrap();
    until(
        window,
        view,
        "quiet startup check",
        |app, cx| app.update_panel.read(cx).fixture_state() == (1, 0, false, false),
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            for page in [
                Page::Settings,
                Page::Today,
                Page::Notes,
                Page::Stats,
                Page::Settings,
            ] {
                view.update(cx, |app, cx| {
                    app.page = page;
                    cx.notify();
                });
                schedule_startup(view.read(cx).update_panel.clone(), cx);
                window.render_frame(cx);
                assert!(
                    window.try_find("sidebar-update").is_none(),
                    "no startup-error card"
                );
            }
            let panel = view.read(cx).update_panel.clone();
            assert_eq!(panel.read(cx).fixture_state(), (1, 0, false, false));
            panel.update(cx, |panel, cx| {
                panel.fixture_response(Some(&release("99.0.0")));
                panel.fixture_manual_check(cx);
            });
            window.render_frame(cx);
            assert!(
                window.try_find("sidebar-update").is_none(),
                "no checking card"
            );
        })
        .unwrap();
    until(
        window,
        view,
        "available release",
        |app, cx| app.update_panel.read(cx).available_version() == Some("99.0.0"),
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("update-dialog").is_some(),
                "a detected release opens the dialog"
            );
            let panel = view.read(cx).update_panel.clone();
            let content = panel.read(cx).dialog_content().expect("update content");
            assert!(content.notes.contains("# Update fixture"));
            assert!(content.notes.contains("**Formatting**"));
            assert!(matches!(
                content.phase,
                studyapp_ui::update_dialog::UpdateDialogPhase::Ready
            ));
            assert!(window.try_find("update-dialog-notes").is_some());
            assert!(window.try_find("update-dialog-install").is_some());
            window
                .render_to_image()
                .unwrap()
                .save(database.with_file_name("update-available.png"))
                .unwrap();
            window.click("update-dialog-later", cx);
            window.render_frame(cx);
            assert!(
                window.try_find("update-dialog").is_none(),
                "later dismisses the dialog"
            );
            assert!(
                !view
                    .read(cx)
                    .update_panel
                    .read(cx)
                    .fixture_automatic_prompt_allowed("99.0.0")
            );
            assert!(
                view.read(cx)
                    .update_panel
                    .read(cx)
                    .fixture_automatic_prompt_allowed("99.0.1")
            );
            let button = window.find("sidebar-update").bounds();
            let version = window.find("sidebar-version").bounds();
            let sidebar = window.find("navigation-pane").bounds();
            assert!(button.bottom() <= version.top());
            assert!(button.left() >= sidebar.left() && button.right() <= sidebar.right());
            assert!(button.size.width > px(100.) && button.size.height >= px(32.));
            let page = view.read(cx).page;
            window.click("sidebar-update", cx);
            window.render_frame(cx);
            assert!(
                window.try_find("update-dialog").is_some(),
                "the update chip reopens the dialog"
            );
            window.click("update-dialog-later", cx);
            window.render_frame(cx);
            assert!(window.try_find("update-dialog").is_none());
            assert_eq!(view.read(cx).page, page);
            let panel = view.read(cx).update_panel.clone();
            panel.update(cx, |panel, cx| {
                panel.fixture_response(Some(&release(env!("CARGO_PKG_VERSION"))));
                panel.fixture_manual_check(cx);
            });
        })
        .unwrap();
    until(
        window,
        view,
        "up-to-date reply",
        |app, cx| app.update_panel.read(cx).fixture_state() == (3, 0, false, false),
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("sidebar-update").is_none(),
                "no up-to-date card"
            );
            view.read(cx).update_panel.clone().update(cx, |panel, cx| {
                panel.fixture_response(None);
                panel.fixture_manual_check(cx);
            });
        })
        .unwrap();
    until(
        window,
        view,
        "visible manual failure",
        |app, cx| app.update_panel.read(cx).fixture_state() == (4, 0, false, true),
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("sidebar-update").is_none(),
                "no failure card"
            );
        })
        .unwrap();
    std::fs::write(
        database.with_file_name("update-result.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "startup_checks": 1, "manual_checks": 3, "download_requests": 0,
            "quiet_startup_error": true, "available_card_above_version": true,
            "no_idle_checking_error_or_current_card": true,
        }))
        .unwrap(),
    )
    .unwrap();
}
