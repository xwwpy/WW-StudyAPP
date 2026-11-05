//! Passive desktop cards observe the same queue; no worker, separate clock or strong app owner.
use crate::{app::StudyApp, components::*, controls::Button, notifications::NotificationCenter};
use gpui_kit::{
    component::{Sizable, button::ButtonVariants},
    *,
};
use std::{cell::Cell, collections::BTreeMap, rc::Rc};
use studyapp_core::{NoticePosition, NoticeStyle, NotificationAppearance};
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
}
struct DesktopNotifications {
    center: Entity<NotificationCenter>,
    presentation: DesktopPresentation,
    scrolls: BTreeMap<u64, ScrollHandle>,
}
impl DesktopNotifications {
    fn new(
        center: Entity<NotificationCenter>,
        initial: DesktopPresentation,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&center, |this, center, cx| {
            this.presentation = DesktopPresentation::read(center.read(cx));
            cx.notify();
        })
        .detach();
        Self {
            center,
            presentation: initial,
            scrolls: BTreeMap::new(),
        }
    }
}
impl Render for DesktopNotifications {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let center = self.center.downgrade();
        self.scrolls.retain(|id, _| {
            self.presentation
                .entries
                .iter()
                .any(|(entry, _)| entry.id == *id)
        });
        let mut content = column().min_w_0().gap_2().p_2();
        for (entry, opacity) in &self.presentation.entries {
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
                .label("×")
                .accessibility_label("关闭通知")
                .on_click(move |_, _, cx| {
                    let _ = close_owner.update(cx, |center, cx| center.dismiss(id, cx));
                });
            let action_owner = center.clone();
            let action = event.action.map(|action| {
                Button::new(SharedString::from(format!("desktop-action-{id}")))
                    .small()
                    .label(action.label())
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

/// Only this newly-created window receives the nonactivation policy.
fn passive_window(window: &mut Window) -> anyhow::Result<()> {
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
        let width = px(f32::from(presentation.appearance.width) + 16.).min(screen.size.width);
        let height = px((presentation.entries.len().max(1)
            * if presentation.private { 150 } else { 270 }) as f32
            + 16.)
        .min(screen.size.height * 0.75);
        let margin = px(16.);
        let position = presentation.appearance.position;
        let x = if matches!(
            position,
            NoticePosition::TopLeft | NoticePosition::BottomLeft
        ) {
            screen.left() + margin
        } else {
            screen.right() - width - margin
        };
        let y = if matches!(position, NoticePosition::TopLeft | NoticePosition::TopRight) {
            screen.top() + margin
        } else {
            screen.bottom() - height - margin
        };
        let bounds = Bounds::new(
            point(x.max(screen.left()), y.max(screen.top())),
            size(width, height),
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
            window.on_window_should_close(cx, move |_, cx| {
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
            let view = cx.new(|cx| DesktopNotifications::new(center, presentation, cx));
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
