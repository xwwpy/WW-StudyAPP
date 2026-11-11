//! Virtual GPUI windows only; no native editor or operating-system input.
use crate::app::{Page, StudyApp};
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size, test::TestWindowExt};
use std::time::Duration;

fn fixture(cx: &mut TestAppContext) -> (Entity<StudyApp>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::apply(&Default::default(), false, cx);
    });
    let root = tempfile::tempdir().unwrap().keep();
    let worker = crate::worker::Worker::start(root.join("studyapp.sqlite3"), None);
    let initial = worker
        .receiver
        .recv_timeout(Duration::from_secs(60))
        .unwrap();
    let snapshot = initial.snapshot.unwrap();
    let preferences = initial.preferences.unwrap();
    let (app, visual) = cx.add_window_view(|window, cx| StudyApp::new(worker, window, cx));
    app.update(visual, |app, cx| {
        app.last_library = snapshot.data_path.clone();
        app.read_models = studyapp_desktop::read_models::ReadModelCache::default().refresh(
            &snapshot,
            &chrono::Local,
            chrono::Utc::now().timestamp(),
        );
        app.snapshot = Some(snapshot);
        app.startup_busy = false;
        app.preferences_ui.saved = Some(preferences);
        app.page = Page::Today;
        cx.notify();
    });
    (app, visual)
}

#[gpui_kit::test]
fn empty_library_has_a_fixed_timer_before_any_note_is_created(cx: &mut TestAppContext) {
    let (app, visual) = fixture(cx);
    visual.simulate_resize(size(px(1000.), px(700.)));
    visual.update(|window, cx| {
        window.render_frame(cx);
        let timer = window.find("today-timer").bounds();
        let scroll = window.find("page-scroll").bounds();
        assert!(timer.size.height > px(0.));
        assert!(timer.bottom() <= scroll.top());
        assert!(scroll.size.height >= px(160.));
        let app = app.read(cx);
        assert_eq!(app.read_models.managed_note_count, 0);
        assert!(!app.note_content.read(cx).creating());
    });
}

#[gpui_kit::test]
fn scrolling_today_keeps_the_timer_and_its_controls_fixed(cx: &mut TestAppContext) {
    let (app, visual) = fixture(cx);
    visual.simulate_resize(size(px(1000.), px(700.)));
    visual.update(|window, cx| {
        window.render_frame(cx);
        let fixed = window.find("today-timer").bounds();
        let header = window.find("today-header").bounds();
        app.update(cx, |app, cx| {
            app.page_scroll
                .set_offset(gpui_kit::point(px(0.), px(-1000.)));
            cx.notify();
        });
        window.render_frame(cx);
        assert!(app.read(cx).page_scroll.offset().y < px(0.));
        assert!(window.find("today-header").bounds().top() < header.top());
        assert_eq!(window.find("today-timer").bounds(), fixed);
        for id in [
            "timer-show-desktop",
            "pause",
            "manual-top",
            "timer-configure",
        ] {
            assert!(window.find(id).visible(), "{id}");
        }
    });
}

#[gpui_kit::test]
fn timer_details_follow_all_modes_and_pause_finish_states(cx: &mut TestAppContext) {
    let (app, visual) = fixture(cx);
    for kind in [
        studyapp_core::TimerKind::Study,
        studyapp_core::TimerKind::CountUp,
        studyapp_core::TimerKind::CountDown,
    ] {
        for (paused, finished, status) in [
            (false, false, "正在计时"),
            (true, false, "计时已暂停"),
            (true, true, "计时已结束"),
        ] {
            app.update(visual, |app, cx| {
                let snapshot = std::sync::Arc::make_mut(app.snapshot.as_mut().unwrap());
                snapshot.timer.kind = kind;
                snapshot.timer.label = "阅读一章 Rust".into();
                snapshot.timer.active_seconds = 125;
                snapshot.timer.target_seconds = if kind == studyapp_core::TimerKind::CountUp {
                    0
                } else {
                    600
                };
                snapshot.timer.paused = paused;
                snapshot.timer.finished_at = finished.then_some(1234);
                cx.notify();
            });
            visual.update(|window, cx| {
                window.render_frame(cx);
                assert_eq!(window.find("inline-timer-status").label(), Some(status));
                let label = if kind == studyapp_core::TimerKind::CountUp {
                    "已用 02:05"
                } else {
                    "剩余 07:55"
                };
                assert_eq!(window.find("today-timer-value").label(), Some(label));
                assert_eq!(
                    window.find("pause").label(),
                    Some(if finished {
                        "重新开始"
                    } else if paused {
                        "继续计时"
                    } else {
                        "暂停"
                    })
                );
                assert_eq!(
                    window.try_find("today-timer-progress").is_some(),
                    kind != studyapp_core::TimerKind::CountUp
                );
                assert!(window.find("timer-details").visible());
            });
        }
    }
}

#[gpui_kit::test]
fn desktop_toggle_reuses_current_timer_and_closing_does_not_reset_it(cx: &mut TestAppContext) {
    let (app, visual) = fixture(cx);
    visual.update(|window, cx| {
        app.update(cx, |app, cx| {
            let snapshot = std::sync::Arc::make_mut(app.snapshot.as_mut().unwrap());
            snapshot.timer.active_seconds = 125;
            snapshot.timer.kind = studyapp_core::TimerKind::CountDown;
            snapshot.timer.target_seconds = 600;
            snapshot.timer.paused = true;
            cx.notify();
        });
        window.render_frame(cx);
        assert_eq!(
            window.find("timer-show-desktop").label(),
            Some("显示桌面小组件")
        );
        assert_eq!(window.find("timer-show-desktop").checked(), Some(false));
        window.click("timer-show-desktop", cx);
        assert!(app.read(cx).timer_widget.is_some());
        window.render_frame(cx);
        assert_eq!(
            window.find("timer-show-desktop").label(),
            Some("隐藏桌面小组件")
        );
        assert_eq!(window.find("timer-show-desktop").checked(), Some(true));
        let widget = app.read(cx).timer_widget.unwrap();
        widget
            .update(cx, |_, widget, cx| {
                widget.render_frame(cx);
                assert_eq!(
                    widget.find("widget-timer-value").label(),
                    Some("剩余 07:55")
                );
            })
            .unwrap();
        window.click("timer-show-desktop", cx);
        assert!(app.read(cx).timer_widget.is_none());
        window.render_frame(cx);
        assert_eq!(window.find("timer-show-desktop").checked(), Some(false));
        let timer = &app.read(cx).snapshot.as_ref().unwrap().timer;
        assert_eq!(timer.active_seconds, 125);
        assert!(timer.paused);
    });
}

#[gpui_kit::test]
fn fixed_timer_returns_after_navigation_but_not_during_library_migration(cx: &mut TestAppContext) {
    let (app, visual) = fixture(cx);
    app.update(visual, |app, cx| {
        app.page = Page::Reviews;
        cx.notify();
    });
    visual.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("today-timer").is_none());
    });
    app.update(visual, |app, cx| {
        app.page = Page::Today;
        cx.notify();
    });
    visual.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.find("today-timer").visible());
    });
    app.update(visual, |app, cx| {
        app.storage_ui.migration_progress = Some("正在复制资料".into());
        cx.notify();
    });
    visual.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("today-timer").is_none());
    });
    app.update(visual, |app, cx| {
        app.storage_ui.migration_progress = None;
        cx.notify();
    });
    visual.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.find("today-timer").visible());
    });
}

#[gpui_kit::test]
fn countdown_presets_only_prepare_setup_until_explicit_confirmation(cx: &mut TestAppContext) {
    let (app, visual) = fixture(cx);
    visual.update(|window, cx| {
        window.render_frame(cx);
        window.click("timer-configure", cx);
        window.click("timer-setup-CountDown", cx);
        let before = app.read(cx).snapshot.as_ref().unwrap().timer.clone();
        let minutes = app.read(cx).effective_widget_settings().countdown_presets[0];
        window.click(
            gpui_kit::SharedString::from(format!("timer-setup-preset-{minutes}")),
            cx,
        );
        assert_eq!(
            app.read(cx).timer_duration.read(cx).value().as_ref(),
            minutes.to_string()
        );
        let after = app.read(cx).snapshot.as_ref().unwrap().timer.clone();
        assert_eq!(after.kind, before.kind);
        assert_eq!(after.active_seconds, before.active_seconds);
        assert_eq!(after.target_seconds, before.target_seconds);
        window.click("timer-cancel-setup", cx);
        assert!(app.read(cx).timer_setup.is_none());
        assert!(app.read(cx).timer_pending.is_none());
    });
}

#[gpui_kit::test]
fn fixed_timer_fits_small_windows_large_fonts_and_long_counter_values(cx: &mut TestAppContext) {
    let (app, visual) = fixture(cx);
    for (width, height, font) in [(1000., 700., 14), (1000., 700., 22), (1280., 860., 22)] {
        visual.simulate_resize(size(px(width), px(height)));
        for kind in [
            studyapp_core::TimerKind::Study,
            studyapp_core::TimerKind::CountUp,
            studyapp_core::TimerKind::CountDown,
        ] {
            app.update(visual, |app, cx| {
                app.preferences_ui
                    .saved
                    .as_mut()
                    .unwrap()
                    .settings
                    .appearance
                    .font_size = font;
                let snapshot = std::sync::Arc::make_mut(app.snapshot.as_mut().unwrap());
                snapshot.timer.kind = kind;
                snapshot.timer.active_seconds = if kind == studyapp_core::TimerKind::CountUp {
                    u64::MAX
                } else {
                    61
                };
                snapshot.timer.target_seconds = if kind == studyapp_core::TimerKind::CountUp {
                    0
                } else {
                    86400
                };
                snapshot.timer.label = "很长的计时名称需要截断而不是挤走控制按钮".repeat(3);
                cx.notify();
            });
            visual.update(|window, cx| {
                window.render_frame(cx);
                let timer = window.find("today-timer").bounds();
                let scroll = window.find("page-scroll").bounds();
                assert!(
                    scroll.size.height >= px(160.),
                    "body too short at {width}x{height}, font {font}: {scroll:?}"
                );
                assert!(timer.bottom() <= scroll.top());
                for id in [
                    "today-timer-value",
                    "timer-show-desktop",
                    "pause",
                    "manual-top",
                    "timer-configure",
                ] {
                    let bounds = window.find(id).bounds();
                    assert!(bounds.size.width > px(0.) && bounds.size.height > px(0.));
                    assert!(
                        bounds.left() >= timer.left() && bounds.right() <= timer.right(),
                        "{id}: {bounds:?} outside {timer:?}"
                    );
                    assert!(
                        bounds.top() >= timer.top() && bounds.bottom() <= timer.bottom(),
                        "{id}: {bounds:?} outside {timer:?}"
                    );
                }
            });
        }
    }
}

#[gpui_kit::test]
fn worker_poll_keeps_the_main_window_after_an_observing_window_closes(cx: &mut TestAppContext) {
    use gpui_kit::{
        AppContext, Context, IntoElement, ParentElement, Render, Window, WindowOptions, div,
    };
    struct Observer(Entity<StudyApp>);
    impl Render for Observer {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            // Reading the owner while drawing associates it with this window.
            let pinned = self
                .0
                .read(cx)
                .snapshot
                .as_ref()
                .unwrap()
                .settings
                .timer_widget
                .pinned;
            div().child(format!("pin observer {pinned}"))
        }
    }
    let (app, visual) = fixture(cx);
    let (sender, receiver) = std::sync::mpsc::channel();
    let mut snapshot = app.update(visual, |app, _| {
        app.worker.receiver = receiver;
        app.snapshot.as_ref().unwrap().as_ref().clone()
    });
    snapshot.settings.timer_widget.pinned = false;
    let snapshot = std::sync::Arc::new(snapshot);
    let read_models = studyapp_desktop::read_models::ReadModelCache::default().refresh(
        &snapshot,
        &chrono::Local,
        chrono::Utc::now().timestamp(),
    );
    visual.run_until_parked();
    let observing = visual.update(|_, cx| {
        cx.open_window(WindowOptions::default(), |_, cx| {
            cx.new(|_| Observer(app.clone()))
        })
        .unwrap()
    });
    let observing: gpui_kit::AnyWindowHandle = observing.into();
    observing
        .update(visual, |_, window, cx| window.render_frame(cx))
        .unwrap();
    // Keep the real main window alive without drawing the owner's view again.
    // A covered/cached window cannot be relied on to restore inferred affinity.
    struct Covered;
    impl Render for Covered {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }
    visual.update(|window, cx| {
        window.replace_root(cx, |_, _| Covered);
        window.render_frame(cx);
    });
    observing
        .update(visual, |_, window, _| window.remove_window())
        .unwrap();
    let mut async_cx = visual.to_async();
    assert!(
        app.downgrade()
            .update_in(&mut async_cx, |_, _, _| ())
            .is_err(),
        "fixture must invalidate inferred affinity without closing the real main window"
    );
    sender
        .send(crate::worker::Update {
            snapshot: Some(snapshot),
            read_models: Some(read_models),
            ..Default::default()
        })
        .unwrap();
    visual.executor().advance_clock(Duration::from_millis(300));
    visual.run_until_parked();
    assert!(
        !app.read_with(visual, |app, _| app
            .snapshot
            .as_ref()
            .unwrap()
            .settings
            .timer_widget
            .pinned),
        "worker replies must still be consumed in the explicitly owned main window"
    );
}

/// The deterministic GPUI clock is never advanced. Queue and signal delivery
/// occur on the test thread, as required by GPUI's deterministic scheduler.
#[gpui_kit::test]
fn worker_receipt_wakes_the_main_view_without_advancing_its_poll_timer(cx: &mut TestAppContext) {
    let (app, visual) = fixture(cx);
    let (sender, receiver) = std::sync::mpsc::channel();
    let (signal, ready) = async_channel::bounded(1);
    let (mut snapshot, mut preferences) = app.update(visual, |app, _| {
        app.worker.receiver = receiver;
        (
            app.snapshot.as_ref().unwrap().as_ref().clone(),
            app.preferences_ui.saved.as_ref().unwrap().clone(),
        )
    });
    let target = !snapshot.settings.timer_widget.pinned;
    snapshot.settings.timer_widget.pinned = target;
    preferences.settings.timer_widget.pinned = target;
    let snapshot = std::sync::Arc::new(snapshot);
    let read_models = studyapp_desktop::read_models::ReadModelCache::default().refresh(
        &snapshot,
        &chrono::Local,
        chrono::Utc::now().timestamp(),
    );
    let _wake = visual.update(|window, cx| {
        app.update(cx, |_, cx| StudyApp::spawn_worker_wake(ready, window, cx))
    });
    visual.run_until_parked();
    sender
        .send(crate::worker::Update {
            snapshot: Some(snapshot),
            read_models: Some(read_models),
            preferences: Some(preferences),
            ..Default::default()
        })
        .unwrap();
    signal.try_send(()).unwrap();
    visual.run_until_parked();
    assert!(
        app.read_with(visual, |app, _| {
            app.snapshot.as_ref().unwrap().settings.timer_widget.pinned == target
                && app
                    .preferences_ui
                    .saved
                    .as_ref()
                    .unwrap()
                    .settings
                    .timer_widget
                    .pinned
                    == target
        }),
        "receipt delivery, not the periodic timer, must update both accepted views"
    );
}

#[gpui_kit::test]
fn wide_dashboard_uses_all_remaining_width_with_navigation_open_or_closed(cx: &mut TestAppContext) {
    let (app, visual) = fixture(cx);
    for width in [1000., 1600., 2400., 1280.] {
        for navigation in [true, false] {
            app.update(visual, |app, cx| {
                app.layout.navigation_visible = navigation;
                cx.notify();
            });
            visual.simulate_resize(size(px(width), px(900.)));
            visual.update(|window, cx| {
                window.render_frame(cx);
                let fixed = window.find("today-timer-fixed").bounds();
                let timer = window.find("today-timer").bounds();
                let header = window.find("today-header").bounds();
                let expected = fixed.size.width - px(64.);
                assert!((timer.size.width - expected).abs() < px(1.), "timer is capped instead of filling {width}px with navigation={navigation}: {timer:?} within {fixed:?}");
                assert!((header.size.width - expected).abs() < px(1.), "dashboard is capped: {header:?} within {fixed:?}");
            });
        }
    }
}

#[gpui_kit::test]
fn ordinary_pages_share_a_full_width_scrollable_content_surface(cx: &mut TestAppContext) {
    let (app, visual) = fixture(cx);
    for page in [Page::Today, Page::Reviews, Page::Stats] {
        for width in [1000., 1600., 2400.] {
            for navigation in [true, false] {
                app.update(visual, |app, cx| {
                    app.page = page;
                    app.layout.navigation_visible = navigation;
                    cx.notify();
                });
                visual.simulate_resize(size(px(width), px(900.)));
                visual.update(|window, cx| {
                    window.render_frame(cx);
                    let viewport = window.find("page-scroll").bounds();
                    let content = window.find("page-content").bounds();
                    assert!(
                        (content.size.width - viewport.size.width + px(64.)).abs() < px(1.),
                        "{page:?} capped at {width}: {content:?} within {viewport:?}"
                    );
                    assert!(content.size.height > px(0.));
                });
            }
        }
    }
}

#[gpui_kit::test]
fn settings_sections_use_the_full_remaining_pane_including_cached_surfaces(
    cx: &mut TestAppContext,
) {
    use crate::settings_ui::{AppearancePage, SettingsSection};
    let (app, visual) = fixture(cx);
    visual.update(|window, cx| app.update(cx, |app, cx| app.navigate(Page::Settings, window, cx)));
    for width in [1000., 1600., 2400.] {
        for navigation in [true, false] {
            visual.simulate_resize(size(px(width), px(1000.)));
            for (section, id, gutter) in [
                // The scroll body reserves pr_3 (12px) for its scrollbar;
                // page_content itself has no horizontal padding or width cap.
                (SettingsSection::Learning, "settings-page-content", 12.),
                (
                    SettingsSection::Notifications,
                    "settings-notifications-surface",
                    0.,
                ),
                (
                    SettingsSection::Appearance,
                    "settings-appearance-surface",
                    0.,
                ),
                (SettingsSection::Shortcuts, "settings-shortcuts-surface", 0.),
            ] {
                app.update(visual, |app, cx| {
                    app.layout.navigation_visible = navigation;
                    app.select_settings_section(section, cx);
                    if section == SettingsSection::Appearance {
                        app.select_appearance_page(AppearancePage::Overview, cx);
                    }
                    app.preferences_ui.section_changed_at = None;
                    cx.notify();
                });
                visual.update(|window, cx| {
                    window.render_frame(cx);
                    let pane = window.find("settings-section-pane").bounds();
                    let body = window.find(id).bounds();
                    assert!(
                        (body.size.width - pane.size.width + px(gutter)).abs() < px(1.),
                        "{id} capped at {width}, navigation={navigation}: {body:?} in {pane:?}"
                    );
                    assert!(body.size.height > px(0.));
                });
            }
        }
    }
}
