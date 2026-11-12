//! Independently invalidated notification surface; animation never notifies StudyApp.
use crate::{components::*, controls::Button, notice_queue::NoticeQueue};
use gpui_kit::{
    component::{IconName, Sizable, button::ButtonVariants},
    *,
};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    rc::Rc,
    time::{Duration, Instant},
};

/// Opaque card rectangles in the parent window's logical pixels, without shadows.
#[derive(Clone, Debug)]
pub struct NotificationOcclusions(pub Vec<Bounds<Pixels>>);

#[path = "notification_banner.rs"]
mod banner;
#[path = "notification_modal.rs"]
mod modal;
#[path = "notification_presenter.rs"]
mod presenter;
pub use presenter::{
    NotificationActionRequested, NotificationActivityChanged, NotificationDelivered,
    NotificationPresented, NotificationRouted,
};

pub struct NotificationCenter {
    policy: studyapp_core::NotificationSettings,
    appearance: studyapp_core::NotificationAppearance,
    environment: studyapp_core::NoticeEnvironment,
    anchors: BTreeMap<String, Bounds<Pixels>>,
    anchor_owners: BTreeMap<String, u64>,
    card_sizes: BTreeMap<u64, Size<Pixels>>,
    expanded: std::collections::BTreeSet<u64>,
    modal_blocked: bool,
    modal_surface: Option<Entity<modal::NotificationModal>>,
    banner_surface: Option<Entity<banner::NotificationBanner>>,
    last_activity: (bool, bool),
    queue: NoticeQueue,
    epoch: Instant,
    timer: Option<Task<()>>,
    occlusions: Vec<Bounds<Pixels>>,
    scrolls: BTreeMap<u64, ScrollHandle>,
    #[cfg(feature = "ui-test")]
    test_time: Option<Duration>,
}

impl EventEmitter<NotificationOcclusions> for NotificationCenter {}

impl Default for NotificationCenter {
    fn default() -> Self {
        Self::new()
    }
}

impl NotificationCenter {
    pub fn occlusions(&self) -> &[Bounds<Pixels>] {
        &self.occlusions
    }
    pub fn new() -> Self {
        Self {
            policy: Default::default(),
            appearance: Default::default(),
            environment: studyapp_core::NoticeEnvironment {
                foreground: true,
                desktop_available: false,
                anchor_available: false,
                minute: 0,
            },
            anchors: Default::default(),
            anchor_owners: Default::default(),
            card_sizes: Default::default(),
            expanded: Default::default(),
            modal_blocked: false,
            modal_surface: None,
            banner_surface: None,
            last_activity: (false, false),
            queue: NoticeQueue::default(),
            epoch: Instant::now(),
            timer: None,
            occlusions: Vec::new(),
            scrolls: BTreeMap::new(),
            #[cfg(feature = "ui-test")]
            test_time: None,
        }
    }

    fn now(&self) -> Duration {
        #[cfg(feature = "ui-test")]
        if let Some(now) = self.test_time {
            return now;
        }
        self.epoch.elapsed()
    }

    #[cfg(feature = "ui-test")]
    pub fn freeze_time(&mut self) {
        assert!(
            self.test_time.is_none(),
            "notification test clock already frozen"
        );
        self.test_time = Some(self.now());
    }

    #[cfg(feature = "ui-test")]
    fn advance_clock(&mut self, delta: Duration) -> Duration {
        let now = self
            .test_time
            .as_mut()
            .expect("notification test clock is not frozen");
        *now = now
            .checked_add(delta)
            .expect("notification test clock overflow");
        *now
    }

    #[cfg(feature = "ui-test")]
    pub fn advance_time(&mut self, delta: Duration, cx: &mut Context<Self>) {
        let now = self.advance_clock(delta);
        self.queue.tick(now);
        cx.notify();
    }

    /// Advance the shared clock without creating a model-change notification.
    /// A frame-driven surface must still sample time rather than reuse old alpha.
    #[cfg(feature = "ui-test")]
    pub fn advance_time_without_notification(&mut self, delta: Duration) {
        self.advance_clock(delta);
    }

    #[cfg(feature = "ui-test")]
    pub fn resume_time(&mut self) {
        let now = self
            .test_time
            .take()
            .expect("notification test clock is not frozen");
        // Preserve the queue's logical timeline rather than counting wall time
        // spent capturing screenshots or delivering native input while frozen.
        self.epoch = Instant::now()
            .checked_sub(now)
            .expect("notification test clock is out of range");
    }

    pub fn push(&mut self, message: Result<String, String>, cx: &mut Context<Self>) {
        self.queue.push(message, self.now());
        self.start_timer(cx);
    }

    fn start_timer(&mut self, cx: &mut Context<Self>) {
        if self.timer.is_none() && self.queue.needs_timer(self.now()) {
            self.timer = Some(cx.spawn(async move |owner, cx| {
                loop {
                    let delay = owner
                        .update(cx, |this, _| {
                            if this.queue.animating(this.now()) {
                                16
                            } else {
                                100
                            }
                        })
                        .unwrap_or(100);
                    cx.background_executor()
                        .timer(Duration::from_millis(delay))
                        .await;
                    let keep = owner
                        .update(cx, |this, cx| {
                            if this.queue.tick(this.now()) {
                                this.notify_activity(cx);
                            }
                            if !this.queue.needs_timer(this.now()) {
                                this.timer = None;
                                false
                            } else {
                                true
                            }
                        })
                        .unwrap_or(false);
                    if !keep {
                        break;
                    }
                }
            }));
        }
        self.notify_activity(cx);
    }

    #[cfg(feature = "ui-test")]
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.queue.clear();
        self.timer = None;
        self.notify_activity(cx);
    }

    #[cfg(feature = "ui-test")]
    pub fn visible_count(&self) -> usize {
        self.queue.visible().count()
    }

    /// Test observation only: bind an assertion to an event, not the latest
    /// unrelated diagnostic message or an arbitrary visible notification.
    #[cfg(feature = "ui-test")]
    pub fn test_visible_events(&self) -> Vec<(u64, studyapp_core::NoticeEvent)> {
        self.queue
            .visible()
            .filter_map(|entry| entry.event.clone().map(|event| (entry.id, event)))
            .collect()
    }

    /// Observe an event even while layout has queued it for lack of space.
    #[cfg(feature = "ui-test")]
    pub fn test_event_route(
        &self,
        event_id: studyapp_core::NoticeEventId,
    ) -> Option<studyapp_core::NoticeRoute> {
        self.queue
            .routes()
            .find_map(|(id, route)| (id == event_id).then_some(route))
    }

    #[cfg(feature = "ui-test")]
    pub fn test_state(&self) -> Vec<(u64, f32)> {
        self.queue
            .visible()
            .map(|entry| (entry.id, entry.opacity(self.now())))
            .collect()
    }

    #[cfg(feature = "ui-test")]
    pub fn test_is_closing(&self, id: u64) -> bool {
        self.queue
            .visible()
            .any(|entry| entry.id == id && entry.is_closing())
    }
}

#[cfg(all(test, feature = "ui-test"))]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn controlled_time_ignores_wall_delay_and_resumes_on_the_same_logical_timeline() {
        let mut center = NotificationCenter::new();
        center.freeze_time();
        let frozen = center.now();
        std::thread::sleep(Duration::from_millis(10));
        assert_eq!(center.now(), frozen);
        let advanced = center.advance_clock(Duration::from_millis(70));
        assert_eq!(advanced, frozen + Duration::from_millis(70));
        assert_eq!(center.now(), advanced);
        let before_resume = Instant::now();
        center.resume_time();
        let after_resume = Instant::now();
        assert!(center.test_time.is_none());
        assert!(before_resume.saturating_duration_since(center.epoch) <= advanced);
        assert!(after_resume.duration_since(center.epoch) >= advanced);
        assert!(center.now() >= advanced);
    }
}

impl Render for NotificationCenter {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.queue.candidates().any(|entry| entry.event.is_some()) {
            return self.structured_surface(window, cx).into_any_element();
        }
        let now = self.now();
        self.scrolls
            .retain(|id, _| self.queue.visible().any(|notice| notice.id == *id));
        for notice in self.queue.visible() {
            self.scrolls.entry(notice.id).or_default();
        }
        let palette = crate::theme::palette();
        let available = (window.viewport_size().width - px(32.)).max(px(0.));
        let font_size = px((palette.font_size as f32 - 1.).max(12.));
        let font = Font {
            family: crate::design::UI_FONT.into(),
            ..Default::default()
        };
        let frame_bounds = Rc::new(RefCell::new(Vec::<Bounds<Pixels>>::new()));
        let weak = cx.weak_entity();
        column()
            .id("floating-notice")
            .test_support()
            .absolute()
            .top(px(crate::design::TITLE_BAR_HEIGHT + 12.))
            .left_0()
            .w_full()
            .items_center()
            .gap(px(8.))
            .children(self.queue.visible().enumerate().map(|(index, notice)| {
                let id = notice.id;
                let (message, accent, tint, icon) = match &notice.message {
                    Ok(message) => (message, palette.success, palette.background, "✓"),
                    Err(message) => (message, palette.danger, palette.background, "!"),
                };
                let opacity = notice.opacity(now);
                let text_width = message
                    .lines()
                    .map(|line| {
                        window
                            .text_system()
                            .shape_line(
                                line.to_owned().into(),
                                font_size,
                                &[TextRun {
                                    len: line.len(),
                                    font: font.clone(),
                                    color: rgb(palette.text).into(),
                                    background_color: None,
                                    underline: None,
                                    strikethrough: None,
                                }],
                                None,
                            )
                            .width
                    })
                    .fold(px(0.), Pixels::max);
                let width = (text_width + px(86.))
                    .max(px(180.))
                    .min(px(560.))
                    .min(available);
                let card_bounds = frame_bounds.clone();
                row()
                    .id(SharedString::from(format!("notice-{id}")))
                    .test_support()
                    .occlude()
                    .flex_shrink_0()
                    .w(width)
                    .min_h(px(40.))
                    .px_2()
                    .py(px(8.))
                    .gap_2()
                    .rounded(px(crate::design::OVERLAY_RADIUS))
                    .border_1()
                    .border_color(rgb(palette.border))
                    .bg(rgb(palette.surface))
                    .shadow_sm()
                    .min_w_0()
                    .opacity(opacity)
                    .relative()
                    .top(px(-6. * (1. - opacity)))
                    .child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(22.))
                            .rounded_full()
                            .bg(rgb(tint))
                            .text_color(rgb(accent))
                            .child(icon),
                    )
                    .child(
                        scroll_body(
                            if index == 0 {
                                SharedString::from("notice-message-scroll")
                            } else {
                                SharedString::from(format!("notice-message-{id}"))
                            },
                            &self.scrolls[&id],
                        )
                        .test_support()
                        .flex_1()
                        .min_w_0()
                        .max_h(px(164.))
                        .pr_2()
                        .text_size(font_size)
                        .line_height(px(20.))
                        .text_color(rgb(palette.text))
                        .child(message.clone())
                        .on_scroll_wheel(|_, _, cx| cx.stop_propagation()),
                    )
                    .child(
                        Button::new(if index == 0 {
                            SharedString::from("dismiss-notice")
                        } else {
                            SharedString::from(format!("dismiss-notice-{id}"))
                        })
                        .small()
                        .ghost()
                        .icon(IconName::Close)
                        .accessibility_label("关闭通知")
                        .tooltip("关闭通知")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.queue.dismiss(id, this.now());
                            this.start_timer(cx);
                        })),
                    )
                    .on_hover(cx.listener(move |this, hovered, _, cx| {
                        this.queue.hover(id, *hovered, this.now());
                        this.start_timer(cx);
                    }))
                    .child(
                        canvas(
                            move |bounds, _, _| {
                                // Reserve the real visual footprint, not the entire
                                // centered overlay row, so the rest of the editor remains
                                // visible and receives native pointer events.
                                card_bounds.borrow_mut().push(bounds);
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    )
            }))
            .child(
                canvas(
                    move |_, _, cx| {
                        let bounds = frame_bounds.borrow().clone();
                        let _ = weak.update(cx, |this, cx| {
                            if this.occlusions != bounds {
                                this.occlusions = bounds.clone();
                                cx.emit(NotificationOcclusions(bounds));
                            }
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size(px(0.)),
            )
            .into_any_element()
    }
}

#[cfg(all(test, feature = "ui-test"))]
#[path = "notification_presenter_tests.rs"]
mod presenter_tests;
