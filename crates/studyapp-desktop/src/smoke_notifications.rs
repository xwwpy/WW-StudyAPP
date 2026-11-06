//! Native focus/visual acceptance. Invoked only by Windows runner smoke profiles.
use crate::{
    app::{Page, StudyApp},
    settings_ui::SettingsSection,
};
use gpui_kit::{test::TestWindowExt, *};
use std::{
    path::Path,
    time::{Duration, Instant},
};
use studyapp_core::{NoticeCategory, NoticeEvent, NoticeRule, NoticeSeverity, NoticeStyle};

async fn settle(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    cx: &mut AsyncApp,
    condition: impl Fn(&StudyApp, &Window, &App) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let ready = window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                condition(view.read(cx), window, cx)
            })
            .expect("notification main window exists");
        if ready {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "notification state did not settle within 5 seconds"
        );
        cx.background_executor()
            .timer(Duration::from_millis(50))
            .await;
    }
}
fn foreground() -> isize {
    unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().0 as isize }
}
struct Witness;
impl Render for Witness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child("Notification focus witness")
    }
}
fn configure(app: &mut StudyApp, style: NoticeStyle, cx: &mut Context<StudyApp>) {
    let mut policy = studyapp_core::NotificationSettings::default();
    policy.rules.insert(
        NoticeCategory::SaveFailure,
        NoticeRule {
            style: Some(style),
            ..Default::default()
        },
    );
    let appearance = studyapp_core::NotificationAppearance {
        reduce_motion: true,
        max_visible: 1,
        ..Default::default()
    };
    let saved = app
        .preferences_ui
        .saved
        .as_mut()
        .expect("saved native settings");
    saved.settings.notifications = policy.clone();
    saved.settings.appearance.notifications = appearance.clone();
    let draft = app.preferences_ui.draft.as_mut().expect("settings draft");
    draft.notifications = policy;
    draft.appearance.notifications = appearance;
    app.clear_notices(cx);
    cx.notify();
}
fn event() -> NoticeEvent {
    let mut event = NoticeEvent::new(
        NoticeCategory::SaveFailure,
        NoticeSeverity::Error,
        "这是通知显示回归样例。关闭提示不会丢弃尚未保存的内容。\n长消息仍在卡片内部滚动，不移动下方页面。",
    );
    event.anchor = Some("settings-save".into());
    event
}

async fn fitted_desktop(
    handle: AnyWindowHandle,
    ids: &[u64],
    database: &Path,
    phase: &str,
    cx: &mut AsyncApp,
) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let observation = handle.update(cx, |_, window, cx| {
            window.render_frame(cx);
            let cards: Vec<_> = ids.iter().filter_map(|id| window.try_find(format!("desktop-notice-{id}")).map(|item| item.bounds())).collect();
            let viewport = window.viewport_size();
            let height = cards.iter().map(|card| f32::from(card.size.height)).sum::<f32>() + 16. + ids.len().saturating_sub(1) as f32 * 8.;
            let fitted = cards.len() == ids.len() && (f32::from(viewport.height) - height).abs() <= 2.
                && cards.iter().all(|card| card.left() >= px(7.) && card.top() >= px(7.) && card.right() <= viewport.width - px(7.) && card.bottom() <= viewport.height - px(7.));
            #[cfg(not(test))]
            let native = crate::timer_widget_window::pin_observation(window).unwrap();
            #[cfg(test)]
            let native = serde_json::Value::Null;
            #[cfg(not(test))]
            if fitted {
                assert_eq!(native["nonClientRendering"], false, "DWM must not draw a notification shell");
                assert_eq!(native["pinned"], true);
                assert_eq!(native["managed"], true);
                use windows::Win32::{Foundation::HWND, UI::WindowsAndMessaging::*};
                use wry::raw_window_handle::{HasWindowHandle, RawWindowHandle};
                let RawWindowHandle::Win32(raw) = HasWindowHandle::window_handle(window).unwrap().as_raw() else { panic!("Win32 notification required") };
                let style = unsafe { GetWindowLongPtrW(HWND(raw.hwnd.get() as *mut _), GWL_EXSTYLE) };
                assert_ne!(style & WS_EX_NOACTIVATE.0 as isize, 0);
            }
            let platform = window.bounds();
            serde_json::json!({"fitted":fitted,"viewport":[f32::from(viewport.width),f32::from(viewport.height)],
                "platformBounds":[f32::from(platform.left()),f32::from(platform.top()),f32::from(platform.size.width),f32::from(platform.size.height)],
                "cards":cards.iter().map(|card| [f32::from(card.left()),f32::from(card.top()),f32::from(card.size.width),f32::from(card.size.height)]).collect::<Vec<_>>(),"native":native})
        }).unwrap();
        std::fs::write(
            database.with_file_name(format!("native-notification-fit-{phase}.json")),
            serde_json::to_vec_pretty(&observation).unwrap(),
        )
        .unwrap();
        if observation["fitted"] == true {
            return observation;
        }
        assert!(
            Instant::now() < deadline,
            "desktop card did not fit real content within 5 seconds: {phase}, {observation}"
        );
        cx.background_executor()
            .timer(Duration::from_millis(30))
            .await;
    }
}

async fn measure_notification_cache(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    database: &Path,
    cx: &mut AsyncApp,
) {
    let mut results = Vec::new();
    for appearance in [false, true] {
        window
            .update(cx, |_, _, cx| {
                view.update(cx, |app, cx| {
                    app.select_settings_section(
                        if appearance {
                            SettingsSection::Appearance
                        } else {
                            SettingsSection::Notifications
                        },
                        cx,
                    );
                    if appearance {
                        app.select_appearance_page(
                            crate::settings_ui::AppearancePage::Notifications,
                            cx,
                        );
                    }
                    app.preferences_ui.section_changed_at = None;
                })
            })
            .unwrap();
        // Warm the same parent-notify path before measuring steady-state work.
        // Keep these draws separate and visible in the receipt; the following
        // twenty measured parent notifications must still all build zero rows.
        let warmup_started = Instant::now();
        let mut warmup_frames = Vec::new();
        let mut quiet = 0;
        loop {
            let (before, viewport) = window
                .update(cx, |_, window, cx| {
                    let before = view.read(cx).notification_render_work();
                    view.update(cx, |_, cx| cx.notify());
                    (before, window.viewport_size())
                })
                .unwrap();
            let (builders, key_matches, mounted, stable_viewport) = window
                .update(cx, |_, window, cx| {
                    window.draw(cx).clear(cx);
                    let app = view.read(cx);
                    (
                        app.notification_render_work() - before,
                        if appearance {
                            app.notification_appearance_cache_matches(cx)
                        } else {
                            app.notification_cache_matches(cx)
                        },
                        window
                            .try_find(if appearance {
                                "notification-width-more"
                            } else {
                                "notification-update-badge"
                            })
                            .is_some(),
                        window.viewport_size() == viewport,
                    )
                })
                .unwrap();
            warmup_frames.push(serde_json::json!({"builders":builders,"keyMatches":key_matches,"mounted":mounted,"stableViewport":stable_viewport}));
            quiet = if builders == 0 && key_matches && mounted && stable_viewport {
                quiet + 1
            } else {
                0
            };
            std::fs::write(
                database.with_file_name("settings-notification-cache-warmup.json"),
                serde_json::to_vec_pretty(
                    &serde_json::json!({"appearance":appearance,"frames":warmup_frames}),
                )
                .unwrap(),
            )
            .unwrap();
            if warmup_frames.len() >= 5 && quiet >= 3 {
                break;
            }
            assert!(
                warmup_started.elapsed() < Duration::from_secs(5),
                "notification cache did not reach a mounted stable baseline within 5 seconds: {warmup_frames:?}"
            );
            cx.background_executor()
                .timer(Duration::from_millis(30))
                .await;
        }
        window
            .update(cx, |_, window, cx| {
                let app = view.read(cx);
                assert_eq!(app.page, Page::Settings);
                assert!(!app.settings_search_active(cx));
                assert!(
                    app.notification_render_work() > 0,
                    "notification page never built controls; appearance={appearance}"
                );
                let control = if appearance {
                    "notification-width-more"
                } else {
                    "notification-update-badge"
                };
                assert!(
                    window.try_find(control).is_some(),
                    "notification benchmark did not mount {control}"
                );
                assert!(if appearance {
                    app.notification_appearance_cache_matches(cx)
                } else {
                    app.notification_cache_matches(cx)
                });
            })
            .unwrap();
        let mut calls = Vec::new();
        let mut timings = Vec::new();
        for _ in 0..20 {
            let before = window
                .update(cx, |_, _, cx| {
                    let before = view.read(cx).notification_render_work();
                    view.update(cx, |_, cx| cx.notify());
                    before
                })
                .unwrap();
            let (elapsed, count) = window
                .update(cx, |_, window, cx| {
                    let start = Instant::now();
                    window.draw(cx).clear(cx);
                    (
                        start.elapsed().as_micros() as u64,
                        view.read(cx).notification_render_work() - before,
                    )
                })
                .unwrap();
            calls.push(count);
            timings.push(elapsed);
        }
        assert!(
            calls.iter().all(|count| *count == 0),
            "notification cached surface rebuilt on parent-only frames: {calls:?}"
        );
        let before = window
            .update(cx, |_, _, cx| {
                let before = view.read(cx).notification_render_work();
                view.update(cx, |app, cx| {
                    let draft = app.preferences_ui.draft.as_mut().unwrap();
                    if appearance {
                        draft.appearance.notifications.width =
                            if draft.appearance.notifications.width == 420 {
                                400
                            } else {
                                420
                            };
                    } else {
                        draft.notifications.do_not_disturb = !draft.notifications.do_not_disturb;
                    }
                    cx.notify();
                });
                before
            })
            .unwrap();
        let began = Instant::now();
        let mut edit_frames = Vec::new();
        let changed = loop {
            let (changed, key_matches, mounted) = window
                .update(cx, |_, window, cx| {
                    window.draw(cx).clear(cx);
                    let app = view.read(cx);
                    (
                        app.notification_render_work() - before,
                        if appearance {
                            app.notification_appearance_cache_matches(cx)
                        } else {
                            app.notification_cache_matches(cx)
                        },
                        window
                            .try_find(if appearance {
                                "notification-width-more"
                            } else {
                                "notification-dnd"
                            })
                            .is_some(),
                    )
                })
                .unwrap();
            edit_frames.push(
                serde_json::json!({"builders":changed,"keyMatches":key_matches,"mounted":mounted}),
            );
            std::fs::write(
                database.with_file_name("settings-notification-cache-edit.json"),
                serde_json::to_vec_pretty(
                    &serde_json::json!({"appearance":appearance,"frames":edit_frames}),
                )
                .unwrap(),
            )
            .unwrap();
            if changed > 0 && key_matches && mounted {
                break changed;
            }
            assert!(
                began.elapsed() < Duration::from_secs(5),
                "notification edit did not render within 5 seconds: appearance={appearance}, frames={edit_frames:?}"
            );
            // The parent sets the child cache key during draw; cx.notify for
            // that leaf is flushed after this update, not inside its prepaint.
            cx.background_executor()
                .timer(Duration::from_millis(30))
                .await;
        };
        assert!(
            changed < 22,
            "relevant notification edit must rebuild visible controls only: {changed}"
        );
        results.push(serde_json::json!({"appearance":appearance,"parent_notify_builder_calls":calls,"parent_notify_draw_microseconds":timings,"relevant_edit_builder_calls":changed,"edit_frames":edit_frames,"warmup_frames":warmup_frames}));
    }
    std::fs::write(
        database.with_file_name("settings-notification-cache-timing.json"),
        serde_json::to_vec_pretty(&results).unwrap(),
    )
    .unwrap();
}

pub async fn exercise(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    database: &Path,
    cx: &mut AsyncApp,
) {
    assert_eq!(
        std::env::var("GITHUB_ACTIONS").as_deref(),
        Ok("true"),
        "native notification acceptance is runner-only"
    );
    let original = window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                assert!(!app.has_required_modal(cx));
                let original = (
                    app.preferences_ui.saved.clone(),
                    app.preferences_ui.draft.clone(),
                    app.page,
                    app.preferences_ui.section,
                    app.preferences_ui.appearance_page,
                    app.preferences_ui
                        .search
                        .as_ref()
                        .map(|input| input.read(cx).value()),
                );
                app.navigate(Page::Settings, window, cx);
                window.activate_window();
                cx.notify();
                original
            })
        })
        .unwrap();
    settle(window, view, cx, |app, window, _| {
        window.is_window_active() && app.page == Page::Settings
    })
    .await;
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                if let Some(search) = app.preferences_ui.search.clone() {
                    search.update(cx, |input, cx| input.set_value("", window, cx));
                }
                app.select_settings_section(SettingsSection::Notifications, cx);
            })
        })
        .unwrap();
    settle(window, view, cx, |_, window, _| {
        window.try_find("notification-update-badge").is_some()
    })
    .await;
    measure_notification_cache(window, view, database, cx).await;
    let mut accepted = Vec::new();
    for style in NoticeStyle::ALL {
        window
            .update(cx, |_, _, cx| {
                view.update(cx, |app, cx| configure(app, style, cx))
            })
            .unwrap();
        settle(window, view, cx, |_, window, _| {
            window.try_find("save-settings").is_some()
        })
        .await;
        let event_id = window
            .update(cx, |_, _, cx| {
                view.update(cx, |app, cx| {
                    let event = event();
                    let id = event.id;
                    assert_eq!(app.emit_notification(event, cx), Some(id));
                    id
                })
            })
            .unwrap();
        settle(window, view, cx, |app, window, cx| {
            let center = app.notifications.read(cx);
            center
                .test_state()
                .first()
                .is_some_and(|(id, _)| window.try_find(format!("notice-{id}")).is_some())
        })
        .await;
        window
            .update(cx, |_, window, cx| {
                let center = view.read(cx).notifications.clone();
                let id = center.read(cx).test_state()[0].0;
                assert_eq!(
                    center.read(cx).modal_visible(),
                    style == NoticeStyle::ModalCard
                );
                let bounds = window.find(format!("notice-{id}")).bounds();
                assert!(bounds.size.width > px(100.) && bounds.size.height > px(32.));
                assert!(bounds.left() >= px(0.) && bounds.right() <= window.viewport_size().width);
                crate::smoke::capture(
                    window,
                    database.with_file_name(format!("notification-{style:?}.png")),
                )
                .unwrap();
                window.click(format!("dismiss-notice-{id}"), cx);
            })
            .unwrap();
        settle(window, view, cx, |app, _, cx| {
            app.notifications.read(cx).visible_count() == 0
        })
        .await;
        accepted.push(event_id.to_string());
    }
    window
        .update(cx, |_, _, cx| {
            view.update(cx, |app, cx| configure(app, NoticeStyle::CornerCard, cx))
        })
        .unwrap();
    // Native editor/pixel prerequisites may temporarily raise this process's
    // main fixture to TOPMOST. Return only our known main window to the normal
    // band before asking the independent, non-topmost witness to take focus.
    window
        .update(cx, |_, window, _| {
            use windows::Win32::{
                Foundation::HWND,
                UI::WindowsAndMessaging::{
                    HWND_NOTOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos,
                },
            };
            use wry::raw_window_handle::{HasWindowHandle, RawWindowHandle};
            let RawWindowHandle::Win32(raw) = window.window_handle().unwrap().as_raw() else {
                panic!("Win32 main fixture required")
            };
            unsafe {
                SetWindowPos(
                    HWND(raw.hwnd.get() as *mut _),
                    Some(HWND_NOTOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                )
                .unwrap()
            };
        })
        .unwrap();
    // Drain earlier modal focus restoration before requesting witness focus.
    cx.background_executor()
        .timer(Duration::from_millis(50))
        .await;
    let witness = cx
        .update(|cx| {
            cx.open_window(
                WindowOptions {
                    focus: false,
                    show: true,
                    ..Default::default()
                },
                |window, cx| {
                    let content = cx.new(|_| Witness);
                    cx.new(|cx| gpui_kit::base::Root::new(content, window, cx))
                },
            )
        })
        .unwrap();
    // Rendering through a typed handle would already lease Root. The frame
    // must acquire its own root lease, so use the untyped owning window.
    let witness: AnyWindowHandle = witness.into();
    witness
        .update(cx, |_, window, cx| {
            crate::native_test_pointer::prepare(window);
            window.render_frame(cx);
            window.activate_window();
        })
        .unwrap();
    // Losing main-window activation is not proof that the new witness has
    // received its asynchronous activation event. Establish both ends before
    // taking the no-focus-stealing baseline.
    let began = Instant::now();
    loop {
        let active = witness
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window.is_window_active()
            })
            .unwrap();
        let inactive = window
            .update(cx, |_, window, _| !window.is_window_active())
            .unwrap();
        if active && inactive {
            break;
        }
        if began.elapsed() >= Duration::from_secs(5) {
            use wry::raw_window_handle::{HasWindowHandle, RawWindowHandle};
            let native = |handle: AnyWindowHandle, cx: &mut AsyncApp| {
                handle
                    .update(cx, |_, window, _| {
                        let RawWindowHandle::Win32(raw) = window.window_handle().unwrap().as_raw()
                        else {
                            panic!("Win32 witness required")
                        };
                        raw.hwnd.get()
                    })
                    .unwrap()
            };
            panic!(
                "notification witness activation: witnessActive={active}, mainInactive={inactive}, foreground={}, witness={}, main={}",
                foreground(),
                native(witness, cx),
                native(window, cx)
            );
        }
        cx.background_executor()
            .timer(Duration::from_millis(50))
            .await;
    }
    assert!(
        witness
            .update(cx, |_, window, _| window.is_window_active())
            .unwrap()
    );
    let before = foreground();
    assert_ne!(before, 0);
    let event_id = window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.sync_notification_preferences(window, cx);
                let event = event();
                let id = event.id;
                assert_eq!(app.emit_notification(event, cx), Some(id));
                id
            })
        })
        .unwrap();
    settle(window, view, cx, |app, _, cx| {
        app.notification_window.is_some() && app.notifications.read(cx).desktop_visible()
    })
    .await;
    assert_eq!(
        foreground(),
        before,
        "G creation must not change the native foreground window"
    );
    let (desktop, numeric_id) = window
        .update(cx, |_, _, cx| {
            let app = view.read(cx);
            let entries = app.notifications.read(cx).desktop_items();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].0.event.as_ref().unwrap().id, event_id);
            assert_eq!(app.notifications.read(cx).visible_count(), 0);
            (app.notification_window.unwrap(), entries[0].0.id)
        })
        .unwrap();
    let compact_geometry = fitted_desktop(desktop, &[numeric_id], database, "compact", cx).await;
    assert_eq!(foreground(), before, "fitting G must not activate it");
    desktop
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(!window.is_window_active());
            assert!(
                window
                    .try_find(format!("desktop-notice-{numeric_id}"))
                    .is_some()
            );
            crate::smoke::capture(
                window,
                database.with_file_name("notification-desktop-G.png"),
            )
            .unwrap();
            crate::native_test_pointer::capture_diagnostic(
                window,
                &database.with_file_name("notification-desktop-G-native.png"),
            );
        })
        .unwrap();
    assert_eq!(foreground(), before, "painting G must not activate it");
    let secondary_id = window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.preferences_ui
                    .saved
                    .as_mut()
                    .unwrap()
                    .settings
                    .appearance
                    .notifications
                    .max_visible = 2;
                app.preferences_ui
                    .draft
                    .as_mut()
                    .unwrap()
                    .appearance
                    .notifications
                    .max_visible = 2;
                app.sync_notification_preferences(window, cx);
                let event = NoticeEvent::new(
                    NoticeCategory::OperationResult,
                    NoticeSeverity::Success,
                    "桌面小组件已显示。可拖动卡片，或在设置中调整外观。",
                );
                let event_id = app.emit_notification(event, cx).unwrap();
                app.notifications
                    .read(cx)
                    .desktop_items()
                    .iter()
                    .find(|(notice, _)| notice.event.as_ref().unwrap().id == event_id)
                    .unwrap()
                    .0
                    .id
            })
        })
        .unwrap();
    let stacked_geometry = fitted_desktop(
        desktop,
        &[numeric_id, secondary_id],
        database,
        "stacked",
        cx,
    )
    .await;
    assert_eq!(foreground(), before, "stacking G must not activate it");
    desktop
        .update(cx, |_, window, _| {
            crate::smoke::capture(
                window,
                database.with_file_name("notification-desktop-G-stacked.png"),
            )
            .unwrap();
            crate::native_test_pointer::capture_diagnostic(
                window,
                &database.with_file_name("notification-desktop-G-stacked-native.png"),
            );
        })
        .unwrap();
    window
        .update(cx, |_, _, cx| {
            view.read(cx)
                .notifications
                .clone()
                .update(cx, |center, cx| center.dismiss(secondary_id, cx))
        })
        .unwrap();
    let restored_geometry = fitted_desktop(desktop, &[numeric_id], database, "restored", cx).await;
    assert_eq!(foreground(), before, "shrinking G must not activate it");
    assert_eq!(compact_geometry["viewport"], restored_geometry["viewport"]);
    window
        .update(cx, |_, window, _| window.activate_window())
        .unwrap();
    settle(window, view, cx, |app, window, cx| {
        window.is_window_active()
            && app.notification_window.is_none()
            && app.notifications.read(cx).visible_count() == 1
    })
    .await;
    window
        .update(cx, |_, _, cx| {
            assert_eq!(
                view.read(cx).notifications.read(cx).test_state()[0].0,
                numeric_id
            )
        })
        .unwrap();
    witness
        .update(cx, |_, window, _| window.activate_window())
        .unwrap();
    settle(window, view, cx, |app, window, _| {
        !window.is_window_active() && app.notification_window.is_some()
    })
    .await;
    let before_close = foreground();
    let desktop = window
        .update(cx, |_, _, cx| view.read(cx).notification_window.unwrap())
        .unwrap();
    desktop
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click(format!("desktop-dismiss-{numeric_id}"), cx);
        })
        .unwrap();
    settle(window, view, cx, |app, _, cx| {
        app.notification_window.is_none() && !app.notifications.read(cx).desktop_visible()
    })
    .await;
    assert_eq!(
        foreground(),
        before_close,
        "closing G must not activate the main window"
    );
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.preferences_ui.saved = original.0;
                app.preferences_ui.draft = original.1;
                app.page = original.2;
                app.preferences_ui.section = original.3;
                app.preferences_ui.appearance_page = original.4;
                if let Some(value) = original.5
                    && let Some(search) = app.preferences_ui.search.clone()
                {
                    search.update(cx, |input, cx| input.set_value(value, window, cx));
                }
                app.clear_notices(cx);
                app.sync_notification_preferences(window, cx);
                window.activate_window();
                cx.notify();
            })
        })
        .unwrap();
    witness
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
    std::fs::write(database.with_file_name("notification-native-result.json"),serde_json::to_vec_pretty(&serde_json::json!({
        "foreground_styles":["A","B","C","D","E"],"desktop_style":"G","created_without_activation":true,
        "painted_without_activation":true,"closed_without_activation":true,"same_event_on_foreground_return":event_id.to_string(),
        "foreground_events":accepted,
        "content_fit":{"compact":compact_geometry,"stacked":stacked_geometry,"restored":restored_geometry},
        "fit_without_activation":true,"stack_without_activation":true,"shrink_without_activation":true,
    })).unwrap()).unwrap();
}
