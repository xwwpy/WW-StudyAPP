//! Passive desktop cards observe the same queue; no worker, separate clock or strong app owner.
use crate::{app::StudyApp, components::*, controls::Button, notifications::NotificationCenter};
use gpui_kit::{
    component::{Sizable, button::ButtonVariants},
    *,
};
use std::{cell::Cell, collections::BTreeMap, rc::Rc};
use studyapp_core::{NoticeStyle, NotificationAppearance};
use studyapp_ui::{
    notice_queue::Notice,
    notification_card::{NotificationCardModel, notification_card},
};

#[derive(Clone)]
struct DesktopPresentation {
    entries: Vec<(Notice, f32)>,
    appearance: NotificationAppearance,
    private: bool,
}
impl DesktopPresentation {
    fn read(center: &NotificationCenter) -> Self {
        Self {
            entries: center.desktop_items(),
            appearance: center.appearance().clone(),
            private: center.private_desktop(),
        }
    }
    fn animating(&self) -> bool {
        self.entries.iter().any(|(_, opacity)| *opacity < 1.)
    }
}
struct DesktopNotifications {
    center: Entity<NotificationCenter>,
    presentation: DesktopPresentation,
    scrolls: BTreeMap<u64, ScrollHandle>,
    fit: Rc<Cell<Option<Bounds<Pixels>>>>,
    owner: WeakEntity<StudyApp>,
}
impl DesktopNotifications {
    fn new(
        center: Entity<NotificationCenter>,
        initial: DesktopPresentation,
        owner: WeakEntity<StudyApp>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&center, |_, _, cx| cx.notify()).detach();
        Self {
            center,
            presentation: initial,
            scrolls: BTreeMap::new(),
            fit: Rc::default(),
            owner,
        }
    }
}
impl Render for DesktopNotifications {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Opacity is derived from the center's shared clock, not immutable event
        // data. A model-change snapshot can retain the first nearly-clear frame.
        self.presentation = DesktopPresentation::read(self.center.read(cx));
        if self.presentation.animating() {
            window.request_animation_frame();
        }
        let center = self.center.downgrade();
        self.scrolls.retain(|id, _| {
            self.presentation
                .entries
                .iter()
                .any(|(entry, _)| entry.id == *id)
        });
        if self.presentation.appearance.stack == studyapp_core::NoticeStack::NewestFirst {
            self.presentation.entries.reverse();
        }
        let mut content = column()
            .relative()
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .gap(px(8.))
            .p(px(8.));
        for (index, (entry, opacity)) in self.presentation.entries.iter().enumerate() {
            let Some(event) = &entry.event else {
                continue;
            };
            let id = entry.id;
            let mut model = NotificationCardModel::from_event(
                event,
                entry.repetitions,
                NoticeStyle::CornerCard,
            );
            if self.presentation.private {
                model.summary = "请返回应用查看详情。".into();
                model.event_id = None;
            }
            let close_owner = center.clone();
            let close = Button::new(SharedString::from(format!("desktop-dismiss-{id}")))
                .small()
                .ghost()
                .action("关闭通知")
                .accessibility_label("关闭通知")
                .on_click(move |_, _, cx| {
                    let _ = close_owner.update(cx, |center, cx| center.dismiss(id, cx));
                });
            let action_owner = center.clone();
            let action = event.action.map(|action| {
                Button::new(SharedString::from(format!("desktop-action-{id}")))
                    .small()
                    .action(action.label())
                    .on_click(move |_, _, cx| {
                        let _ = action_owner.update(cx, |center, cx| center.activate(id, cx));
                    })
            });
            let hover_owner = center.clone();
            let presented_owner = center.clone();
            content = content.child(
                div()
                    .id(SharedString::from(format!("desktop-notice-{id}")))
                    .test_support()
                    .ml(px(
                        if self.presentation.appearance.stack
                            == studyapp_core::NoticeStack::Staggered
                        {
                            index as f32 * 8.
                        } else {
                            0.
                        },
                    ))
                    .opacity(*opacity)
                    .on_hover(move |hovered, _, cx| {
                        let _ = hover_owner.update(cx, |center, cx| center.hover(id, *hovered, cx));
                    })
                    .child(notification_card(
                        model,
                        &self.presentation.appearance,
                        self.scrolls.entry(id).or_default(),
                        close,
                        action,
                    ))
                    .relative()
                    .child(
                        canvas(
                            move |bounds, _, cx| {
                                if bounds.size.width > px(0.) && bounds.size.height > px(0.) {
                                    let _ = presented_owner.update(cx, |center, cx| {
                                        center.did_present(
                                            &[id],
                                            studyapp_core::NoticeRoute::DesktopCard,
                                            cx,
                                        )
                                    });
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    ),
            );
        }
        let fit = self.fit.clone();
        let owner = self.owner.clone();
        let width = px(f32::from(
            self.presentation
                .appearance
                .surface(NoticeStyle::CornerCard)
                .width,
        ) + 16.);
        let position = self.presentation.appearance.position;
        content = content.child(
            canvas(
                move |bounds, window, cx| {
                    let Some(display) = window.display(cx).or_else(|| cx.primary_display()) else {
                        return;
                    };
                    let desired = crate::card_window::notification_bounds(
                        display.visible_bounds(),
                        width,
                        bounds.size.height,
                        position,
                    );
                    if fit.get() != Some(desired) {
                        fit.set(Some(desired));
                        let owner = owner.clone();
                        // Layout only measures. Native geometry changes run after the
                        // draw and retain the same passive window and event identity.
                        window.defer(cx, move |window, cx| {
                            if crate::card_window::fit_passive(window, desired, cx).is_err() {
                                fail_desktop(owner, cx);
                            }
                        });
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0(),
        );
        div()
            .id("desktop-notifications")
            .test_support()
            .size_full()
            .font_family(crate::design::UI_FONT)
            .child(
                div()
                    .id("desktop-notifications-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .child(content),
            )
    }
}

/// Read the actual mounted view and painted primitives, never reconstruct a scene.
#[cfg(all(windows, feature = "ui-test"))]
pub(crate) fn test_paint_observation(window: &Window, cx: &App) -> serde_json::Value {
    let view = gpui_kit::base::Root::read(window, cx)
        .view()
        .clone()
        .downcast::<DesktopNotifications>()
        .expect("the owned notification window must contain DesktopNotifications");
    let desktop = view.read(cx);
    let quads = window.painted_quads();
    let rectangle = |bounds: Bounds<ScaledPixels>| {
        [
            bounds.left().as_f32(),
            bounds.top().as_f32(),
            bounds.size.width.as_f32(),
            bounds.size.height.as_f32(),
        ]
    };
    serde_json::json!({
        "snapshotOpacities": desktop.presentation.entries.iter().map(|(notice, opacity)| (notice.id, opacity)).collect::<Vec<_>>(),
        "viewport": [f32::from(window.viewport_size().width), f32::from(window.viewport_size().height)],
        "scale": window.scale_factor(),
        "surface": studyapp_ui::theme::palette().surface,
        "quadCount": quads.len(),
        "quads": quads.iter().take(128).map(|quad| serde_json::json!({
            "bounds": rectangle(quad.bounds),
            "contentMask": rectangle(quad.content_mask.bounds),
            "background": format!("{:?}", quad.background),
            "backgroundAlpha": quad.background.as_solid().map(|color| color.a),
        })).collect::<Vec<_>>(),
    })
}

/// Only this newly-created window receives the nonactivation policy.
fn passive_window(window: &mut Window) -> anyhow::Result<()> {
    crate::card_window::disable_native_frame(window)?;
    #[cfg(all(windows, not(test)))]
    {
        use windows::Win32::{Foundation::HWND, UI::WindowsAndMessaging::*};
        use wry::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let RawWindowHandle::Win32(raw) = window.window_handle()?.as_raw() else {
            anyhow::bail!("通知窗口句柄无效")
        };
        let hwnd = HWND(raw.hwnd.get() as *mut _);
        unsafe {
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(
                hwnd,
                GWL_EXSTYLE,
                (style | WS_EX_TOOLWINDOW.0 as isize | WS_EX_NOACTIVATE.0 as isize)
                    & !(WS_EX_APPWINDOW.0 as isize),
            );
            SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            )?;
            anyhow::ensure!(
                GetWindowLongPtrW(hwnd, GWL_EXSTYLE) & WS_EX_NOACTIVATE.0 as isize != 0,
                "通知窗口无法保持非激活状态"
            );
        }
    }
    #[cfg(any(not(windows), test))]
    let _ = window;
    Ok(())
}

fn fail_desktop(owner: WeakEntity<StudyApp>, cx: &mut App) {
    cx.defer(move |cx| {
        let _ = owner.update(cx, |app, cx| {
            app.close_notification_desktop(cx);
            app.notification_desktop_failed = true;
            let main = app.main_window;
            let owner = owner.clone();
            cx.defer(move |cx| {
                let _ = main.update(cx, |_, window, cx| {
                    let _ =
                        owner.update(cx, |app, cx| app.sync_notification_preferences(window, cx));
                });
            });
        });
    });
}

impl StudyApp {
    pub fn close_notification_desktop(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = self.notification_window.take() {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
    }
    pub fn sync_notification_desktop(&mut self, window: &Window, cx: &mut Context<Self>) {
        if !self.notifications.read(cx).desktop_visible()
            || self.close_requested
            || self.close_allowed
        {
            self.close_notification_desktop(cx);
            return;
        }
        if self.notification_window.is_some() || self.notification_desktop_failed {
            return;
        }
        let Some(display) = window.display(cx).or_else(|| cx.primary_display()) else {
            self.notification_desktop_failed = true;
            self.sync_notification_preferences(window, cx);
            return;
        };
        let presentation = DesktopPresentation::read(self.notifications.read(cx));
        let screen = display.visible_bounds();
        let bounds = crate::card_window::notification_bounds(
            screen,
            px(f32::from(
                presentation
                    .appearance
                    .surface(NoticeStyle::CornerCard)
                    .width,
            ) + 16.),
            px(1.),
            presentation.appearance.position,
        );
        let options = WindowOptions {
            kind: WindowKind::PopUp,
            focus: false,
            show: true,
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_background: WindowBackgroundAppearance::Transparent,
            is_resizable: false,
            is_minimizable: false,
            window_decorations: Some(WindowDecorations::Client),
            ..Default::default()
        };
        let center = self.notifications.clone();
        let owner = cx.weak_entity();
        let passive = Rc::new(Cell::new(false));
        let ready = passive.clone();
        let result = cx.open_window(options, move |window, cx| {
            // A failure closes the passive window and reroutes the same events below.
            ready.set(passive_window(window).is_ok());
            let close_owner = owner.clone();
            window.on_window_should_close(cx, move |_, cx| {
                let owner = close_owner.clone();
                let _ = owner.update(cx, |this, cx| {
                    this.notification_window = None;
                    this.notification_desktop_failed = true;
                    let main = this.main_window;
                    let owner = owner.clone();
                    cx.defer(move |cx| {
                        let _ = main.update(cx, |_, window, cx| {
                            let _ = owner.update(cx, |app, cx| {
                                app.sync_notification_preferences(window, cx)
                            });
                        });
                    });
                });
                true
            });
            let view = cx.new(|cx| DesktopNotifications::new(center, presentation, owner, cx));
            cx.new(|cx| gpui_kit::base::Root::new(view, window, cx).bg(rgba(0)))
        });
        match result {
            Ok(handle) => {
                if passive.get() {
                    self.notification_window = Some(handle.into());
                } else {
                    let _ = handle.update(cx, |_, window, _| window.remove_window());
                    self.notification_desktop_failed = true;
                    self.sync_notification_preferences(window, cx);
                }
            }
            Err(_) => {
                self.notification_desktop_failed = true;
                self.sync_notification_preferences(window, cx);
            }
        }
    }
}

#[cfg(all(test, feature = "ui-test"))]
mod tests {
    use super::{DesktopNotifications, DesktopPresentation, NotificationCenter, StudyApp};
    use gpui_kit::{
        AppContext, Styled, TestAppContext, VisualTestContext, base, px, size, test::TestWindowExt,
    };
    use studyapp_core::{
        NoticeCategory, NoticeEnvironment, NoticeEvent, NoticeSeverity, NotificationAppearance,
        NotificationSettings,
    };

    #[gpui_kit::test]
    fn desktop_cards_fit_natural_content_from_a_one_pixel_bootstrap(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::apply(&Default::default(), false, cx);
        });
        let data = tempfile::tempdir().unwrap().keep();
        let worker = crate::worker::Worker::start(data.join("studyapp.sqlite3"), None);
        let mut owner = None;
        let mut desktop = None;
        let handle = cx.open_window(size(px(376.), px(1.)), |window, cx| {
            let app = cx.new(|cx| StudyApp::new(worker, window, cx));
            let center = cx.new(|_| NotificationCenter::new());
            center.update(cx, |center, cx| {
                center.freeze_time();
                center.configure(&NotificationSettings::default(), &NotificationAppearance { reduce_motion: true, max_visible: 2, ..Default::default() }, NoticeEnvironment { foreground: false, desktop_available: true, anchor_available: false, minute: 0 }, false, cx);
                center.enqueue(NoticeEvent::new(NoticeCategory::SaveFailure, NoticeSeverity::Error, "这是通知显示回归样例。关闭提示不会丢弃尚未保存的内容。长消息仍在卡片内部滚动。"), cx).unwrap();
            });
            let presentation = DesktopPresentation::read(center.read(cx));
            assert_eq!(presentation.entries.len(), 1);
            let view = cx.new(|cx| DesktopNotifications::new(center, presentation, app.downgrade(), cx));
            owner = Some(app);
            desktop = Some(view.clone());
            base::Root::new(view, window, cx).bg(gpui_kit::rgba(0))
        });
        let desktop = desktop.unwrap();
        let visual = &mut VisualTestContext::from_window(handle.into(), cx);
        for _ in 0..8 {
            visual.update(|window, cx| window.render_frame(cx));
        }
        visual.update(|window, cx| {
            let card = window.find("desktop-notice-1").bounds();
            let viewport = window.viewport_size();
            let fit = desktop.read(cx).fit.get();
            assert!(card.size.height > px(32.));
            assert!(
                (f32::from(viewport.height - card.size.height) - 16.).abs() <= 2.,
                "viewport={viewport:?}, card={card:?}, requested={fit:?}"
            );
            let quads = window.painted_quads();
            assert!(
                quads.iter().any(|quad| {
                    !quad.background.is_transparent()
                        && quad.bounds.size.height > px(32.).scale(window.scale_factor())
                        && quad.content_mask.bounds.size.height
                            > px(32.).scale(window.scale_factor())
                }),
                "fitted cards must paint visible surfaces, not only layout rectangles: {quads:?}"
            );
        });
        let compact = visual.update(|window, _| window.viewport_size());
        let center = desktop.read_with(visual, |desktop, _| desktop.center.clone());
        center.update(visual, |center, cx| {
            center
                .enqueue(
                    NoticeEvent::new(
                        NoticeCategory::OperationResult,
                        NoticeSeverity::Success,
                        "桌面小组件已显示。",
                    ),
                    cx,
                )
                .unwrap();
        });
        for _ in 0..8 {
            visual.update(|window, cx| window.render_frame(cx));
        }
        visual.update(|window, _| {
            let first = window.find("desktop-notice-1").bounds();
            let second = window.find("desktop-notice-2").bounds();
            let viewport = window.viewport_size();
            assert!(viewport.height > compact.height);
            assert!(
                (f32::from(viewport.height - first.size.height - second.size.height) - 24.).abs()
                    <= 2.
            );
            assert!(first.top() >= px(7.) && second.bottom() <= viewport.height - px(7.));
        });
        center.update(visual, |center, cx| center.dismiss(2, cx));
        for _ in 0..8 {
            visual.update(|window, cx| window.render_frame(cx));
        }
        visual.update(|window, _| assert_eq!(window.viewport_size(), compact));
        assert!(owner.is_some());
    }

    #[gpui_kit::test]
    fn fitted_bootstrap_observes_the_real_zero_half_and_complete_animation(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::apply(&Default::default(), false, cx);
        });
        let data = tempfile::tempdir().unwrap().keep();
        let worker = crate::worker::Worker::start(data.join("studyapp.sqlite3"), None);
        let mut owner = None;
        let mut desktop = None;
        let handle = cx.open_window(size(px(376.), px(1.)), |window, cx| {
            let app = cx.new(|cx| StudyApp::new(worker, window, cx));
            let center = cx.new(|_| NotificationCenter::new());
            center.update(cx, |center, cx| {
                center.freeze_time();
                center.configure(
                    &NotificationSettings::default(),
                    &NotificationAppearance::default(),
                    NoticeEnvironment {
                        foreground: false,
                        desktop_available: true,
                        anchor_available: false,
                        minute: 0,
                    },
                    false,
                    cx,
                );
                center
                    .enqueue(
                        NoticeEvent::new(
                            NoticeCategory::SaveFailure,
                            NoticeSeverity::Error,
                            "动画仍未显示时，布局也可能已经完成。",
                        ),
                        cx,
                    )
                    .unwrap();
            });
            let presentation = DesktopPresentation::read(center.read(cx));
            let view =
                cx.new(|cx| DesktopNotifications::new(center, presentation, app.downgrade(), cx));
            desktop = Some(view.clone());
            owner = Some(app);
            base::Root::new(view, window, cx).bg(gpui_kit::rgba(0))
        });
        let desktop = desktop.unwrap();
        let visual = &mut VisualTestContext::from_window(handle.into(), cx);
        for _ in 0..8 {
            visual.update(|window, cx| window.render_frame(cx));
        }
        visual.update(|window, cx| {
            let card = window.find("desktop-notice-1").bounds();
            assert!(
                (f32::from(window.viewport_size().height - card.size.height) - 16.).abs() <= 2.
            );
            assert_eq!(desktop.read(cx).presentation.entries[0].1, 0.);
        });
        let center = desktop.read_with(visual, |view, _| view.center.clone());
        for expected in [0.5, 1.] {
            center.update(visual, |center, _| {
                center.advance_time_without_notification(std::time::Duration::from_millis(100))
            });
            for _ in 0..8 {
                visual.update(|window, cx| window.render_frame(cx));
            }
            desktop.read_with(visual, |view, _| {
                assert_eq!(view.presentation.entries[0].1, expected);
                assert_eq!(view.presentation.animating(), expected < 1.);
            });
            visual.update(|window, _| {
                assert!(
                    window.painted_quads().iter().any(|quad| {
                        quad.bounds.size.width == px(360.).scale(window.scale_factor())
                            && quad.bounds.size.height > px(32.).scale(window.scale_factor())
                            && quad
                                .background
                                .as_solid()
                                .is_some_and(|color| color.a == expected)
                    }),
                    "the card surface must paint the current shared-clock opacity"
                );
            });
        }
        assert!(owner.is_some());
    }
}
