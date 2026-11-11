//! Structured views share the original center, queue and invalidation boundary.
use super::{NotificationCenter, NotificationOcclusions};
use crate::{
    components::column, controls::Button, notice_queue::Notice,
    notification_card::NotificationCardModel,
};
use gpui_kit::{
    component::{Sizable, button::ButtonVariants},
    prelude::FluentBuilder,
    *,
};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
use studyapp_core::{
    NoticeAction, NoticeEnvironment, NoticeEvent, NoticeEventId, NoticeRoute, NoticeSeverity,
    NoticeStyle, NotificationAppearance, NotificationSettings,
};

type FrameBounds = Rc<RefCell<Vec<(u64, Bounds<Pixels>)>>>;

#[derive(Clone, Copy, Debug)]
pub struct NotificationActionRequested {
    pub event_id: NoticeEventId,
    pub action: NoticeAction,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotificationActivityChanged {
    pub modal: bool,
    pub desktop: bool,
}
#[derive(Clone, Copy, Debug)]
pub struct NotificationPresented {
    pub event_id: NoticeEventId,
    pub category: studyapp_core::NoticeCategory,
    pub severity: NoticeSeverity,
}
#[derive(Clone, Copy, Debug)]
pub struct NotificationDelivered {
    pub event_id: NoticeEventId,
}
#[derive(Clone, Copy, Debug)]
pub struct NotificationRouted {
    pub event_id: NoticeEventId,
    pub route: NoticeRoute,
}
impl EventEmitter<NotificationDelivered> for NotificationCenter {}
impl EventEmitter<NotificationRouted> for NotificationCenter {}
impl EventEmitter<NotificationPresented> for NotificationCenter {}
impl EventEmitter<NotificationActionRequested> for NotificationCenter {}
impl EventEmitter<NotificationActivityChanged> for NotificationCenter {}

impl NotificationCenter {
    pub fn configure(
        &mut self,
        policy: &NotificationSettings,
        appearance: &NotificationAppearance,
        environment: NoticeEnvironment,
        modal_blocked: bool,
        cx: &mut Context<Self>,
    ) {
        if self.policy == *policy
            && self.appearance == *appearance
            && self.environment == environment
            && self.modal_blocked == modal_blocked
        {
            return;
        }
        if policy.validate().is_err() || appearance.validate().is_err() {
            return;
        }
        self.policy = policy.clone();
        self.appearance = appearance.clone();
        self.environment = environment;
        self.modal_blocked = modal_blocked;
        self.reroute_current(cx);
    }
    /// Replace a complete mounted-control snapshot. Unmounted anchors never survive.
    pub fn set_anchors(
        &mut self,
        mut anchors: BTreeMap<String, Bounds<Pixels>>,
        cx: &mut Context<Self>,
    ) {
        anchors.retain(|key, bounds| {
            !key.is_empty()
                && key.len() <= 80
                && key
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
                && [
                    bounds.origin.x,
                    bounds.origin.y,
                    bounds.size.width,
                    bounds.size.height,
                ]
                .into_iter()
                .all(|v| f32::from(v).is_finite())
                && bounds.size.width > px(0.)
                && bounds.size.height > px(0.)
        });
        if self.anchors != anchors {
            self.anchors = anchors;
            self.reroute_current(cx);
        }
    }
    pub(crate) fn publish_anchor(
        &mut self,
        key: &'static str,
        owner: u64,
        bounds: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if bounds.size.width <= px(0.) || bounds.size.height <= px(0.) {
            self.release_anchor(key, owner, cx);
            return;
        }
        self.anchor_owners.insert(key.into(), owner);
        if self.anchors.get(key) != Some(&bounds) {
            self.anchors.insert(key.into(), bounds);
            self.reroute_current(cx);
        }
    }
    pub(crate) fn release_anchor(&mut self, key: &str, owner: u64, cx: &mut Context<Self>) {
        if self.anchor_owners.get(key) == Some(&owner) {
            self.anchor_owners.remove(key);
            self.anchors.remove(key);
            self.reroute_current(cx);
        }
    }
    fn reroute_current(&mut self, cx: &mut Context<Self>) {
        let previous: BTreeMap<_, _> = self.queue.routes().collect();
        let now = self.now();
        let policy = &self.policy;
        let appearance = &self.appearance;
        let environment = self.environment;
        let anchors = &self.anchors;
        let blocked = self.modal_blocked;
        self.queue.reroute_with(appearance, now, |event| {
            let route = policy.route_event(
                event,
                appearance,
                NoticeEnvironment {
                    anchor_available: event
                        .anchor
                        .as_ref()
                        .is_some_and(|key| anchors.contains_key(key)),
                    ..environment
                },
            );
            if blocked && route == NoticeRoute::Foreground(NoticeStyle::ModalCard) {
                NoticeRoute::Deferred
            } else {
                route
            }
        });
        for (event_id, route) in self.queue.routes() {
            if previous.get(&event_id) != Some(&route) {
                cx.emit(NotificationRouted { event_id, route });
            }
        }
        self.start_timer(cx);
    }
    pub fn enqueue(&mut self, event: NoticeEvent, cx: &mut Context<Self>) -> Option<NoticeEventId> {
        let route = self.policy.route_event(
            &event,
            &self.appearance,
            NoticeEnvironment {
                anchor_available: event
                    .anchor
                    .as_ref()
                    .is_some_and(|key| self.anchors.contains_key(key)),
                ..self.environment
            },
        );
        let route =
            if self.modal_blocked && route == NoticeRoute::Foreground(NoticeStyle::ModalCard) {
                NoticeRoute::Deferred
            } else {
                route
            };
        let source_id = event.id;
        let result = self
            .queue
            .push_event(event, route, &self.appearance, self.now());
        // Re-evaluate the merged severity/anchor, not the lower-severity arrival.
        self.reroute_current(cx);
        let event_id = result.unwrap_or(source_id);
        let route = self
            .queue
            .routes()
            .find(|(id, _)| *id == event_id)
            .map_or(NoticeRoute::Suppressed, |(_, route)| route);
        cx.emit(NotificationRouted { event_id, route });
        result
    }
    pub fn did_present(&mut self, ids: &[u64], route: NoticeRoute, cx: &mut Context<Self>) {
        for event_id in self.queue.did_present(ids, route) {
            cx.emit(NotificationDelivered { event_id });
            if self.policy.sound
                && let Some(event) = self
                    .queue
                    .visible()
                    .chain(self.queue.desktop_visible())
                    .filter_map(|notice| notice.event.as_ref())
                    .find(|event| event.id == event_id)
            {
                cx.emit(NotificationPresented {
                    event_id,
                    category: event.category,
                    severity: event.severity,
                });
            }
        }
    }
    pub fn modal_visible(&self) -> bool {
        self.queue
            .visible()
            .any(|e| e.route == NoticeRoute::Foreground(NoticeStyle::ModalCard))
    }
    pub fn desktop_visible(&self) -> bool {
        self.queue.desktop_visible().next().is_some()
    }
    pub fn desktop_items(&self) -> Vec<(Notice, f32)> {
        self.queue
            .desktop_visible()
            .map(|entry| (entry.clone(), entry.opacity(self.now())))
            .collect()
    }
    pub fn appearance(&self) -> &NotificationAppearance {
        &self.appearance
    }
    pub fn private_desktop(&self) -> bool {
        self.policy.private_desktop
    }
    pub(super) fn notify_activity(&mut self, cx: &mut Context<Self>) {
        let next = (self.modal_visible(), self.desktop_visible());
        if self.last_activity != next {
            self.last_activity = next;
            cx.emit(NotificationActivityChanged {
                modal: next.0,
                desktop: next.1,
            });
        }
        cx.notify();
    }
    pub fn dismiss_modal(&mut self, cx: &mut Context<Self>) {
        let id = self
            .queue
            .visible()
            .find(|e| e.route == NoticeRoute::Foreground(NoticeStyle::ModalCard))
            .map(|e| e.id);
        if let Some(id) = id {
            self.dismiss(id, cx);
        }
    }
    pub fn dismiss(&mut self, id: u64, cx: &mut Context<Self>) {
        self.queue.dismiss(id, self.now());
        self.queue.tick(self.now());
        self.start_timer(cx);
    }
    pub fn hover(&mut self, id: u64, hovered: bool, cx: &mut Context<Self>) {
        self.queue.hover(id, hovered, self.now());
        self.start_timer(cx);
    }
    pub fn activate(&mut self, id: u64, cx: &mut Context<Self>) {
        let event = self
            .queue
            .visible()
            .chain(self.queue.desktop_visible())
            .find(|e| e.id == id)
            .and_then(|e| e.event.clone());
        if let Some(event) = event
            && let Some(action) = event.action
        {
            self.queue.acknowledge(id, self.now());
            self.start_timer(cx);
            cx.emit(NotificationActionRequested {
                event_id: event.id,
                action,
            });
        }
    }
    pub(super) fn render_card(
        &mut self,
        entry: Notice,
        bounds: FrameBounds,
        cx: &mut Context<Self>,
    ) -> Div {
        let id = entry.id;
        let style = match entry.route {
            NoticeRoute::Foreground(style) => style,
            _ => NoticeStyle::CornerCard,
        };
        let route = entry.route;
        let surface_width = self.appearance.surface(style).width;
        let model = if let Some(event) = &entry.event {
            NotificationCardModel::from_event(event, entry.repetitions, style)
        } else {
            NotificationCardModel {
                key: format!("legacy-{id}").into(),
                category: None,
                title: if entry.message.is_ok() {
                    "提示"
                } else {
                    "操作未完成"
                }
                .into(),
                summary: entry.message.as_ref().unwrap_or_else(|e| e).clone().into(),
                severity: if entry.message.is_ok() {
                    NoticeSeverity::Success
                } else {
                    NoticeSeverity::Error
                },
                event_id: None,
                repetitions: entry.repetitions,
                style,
            }
        };
        let action = entry.event.as_ref().and_then(|e| e.action).map(|action| {
            Button::new(SharedString::from(format!("notice-action-{id}")))
                .small()
                .action(action.label())
                .on_click(cx.listener(move |this, _, _, cx| this.activate(id, cx)))
        });
        let close = Button::new(SharedString::from(format!("dismiss-notice-{id}")))
            .small()
            .ghost()
            .action("关闭通知")
            .accessibility_label("关闭通知")
            .on_click(cx.listener(move |this, _, _, cx| this.dismiss(id, cx)));
        let expanded = self.expanded.contains(&id);
        let toggle = (style == NoticeStyle::CornerCard
            && self.appearance.layout(style) == studyapp_core::NoticeLayout::Alternative
            && model.severity < NoticeSeverity::Error)
            .then(|| {
                Button::new(SharedString::from(format!("notice-expand-{id}")))
                    .small()
                    .ghost()
                    .label(if expanded {
                        "收起正文"
                    } else {
                        "展开完整正文"
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.expanded.remove(&id) {
                            this.expanded.insert(id);
                        }
                        this.card_sizes.remove(&id);
                        cx.notify();
                    }))
            });
        let scroll = self.scrolls.entry(id).or_default().clone();
        let presented_owner = cx.weak_entity();
        div().min_w_0().child(
            column()
                .id(SharedString::from(format!("notice-{id}")))
                .test_support()
                .occlude()
                .min_w_0()
                .w(px(f32::from(surface_width)))
                .when(
                    style == NoticeStyle::Banner
                        || style == NoticeStyle::Snackbar
                            && self.appearance.layout(style)
                                == studyapp_core::NoticeLayout::Alternative,
                    |view| view.w_full(),
                )
                .max_w_full()
                .flex_shrink_0()
                .opacity(entry.opacity(self.now()))
                .on_hover(cx.listener(move |this, hovered, _, cx| this.hover(id, *hovered, cx)))
                .child(crate::notification_card::notification_card_state(
                    model,
                    &self.appearance,
                    &scroll,
                    close,
                    action,
                    expanded,
                    toggle,
                ))
                .relative()
                .child(
                    canvas(
                        move |rect, _, cx| {
                            bounds.borrow_mut().push((id, rect));
                            if rect.size.width > px(0.) && rect.size.height > px(0.) {
                                let _ = presented_owner
                                    .update(cx, |center, cx| center.did_present(&[id], route, cx));
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                ),
        )
    }
    /// The host supplies its existing blocking-dialog shell and keyboard scope.
    pub fn modal_content(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let entry = self
            .queue
            .visible()
            .find(|e| e.route == NoticeRoute::Foreground(NoticeStyle::ModalCard))
            .cloned()?;
        if self.modal_surface.is_none() {
            let owner = cx.entity();
            let initial = (entry.clone(), entry.opacity(self.now()));
            let appearance = self.appearance.clone();
            self.modal_surface =
                Some(cx.new(|cx| {
                    super::modal::NotificationModal::new(owner, initial, appearance, cx)
                }));
        }
        self.modal_surface
            .as_ref()
            .map(|surface| surface.clone().into_any_element())
    }
    pub(super) fn structured_surface(&mut self, window: &Window, cx: &mut Context<Self>) -> Div {
        use crate::notice_placement::{Candidate, place};
        let viewport = window.viewport_size();
        let entries: Vec<_> = self.queue.candidates().cloned().collect();
        let candidates: Vec<_> = entries
            .iter()
            .filter_map(|entry| {
                let NoticeRoute::Foreground(style) = entry.route else {
                    return None;
                };
                let surface = self.appearance.surface(style);
                let estimate = match style {
                    NoticeStyle::Bubble => 44.,
                    NoticeStyle::Snackbar => 68.,
                    NoticeStyle::Banner => 80.,
                    _ => f32::from(surface.max_height) + 96.,
                };
                Some(Candidate {
                    id: entry.id,
                    style,
                    width: f32::from(surface.width),
                    height: self
                        .card_sizes
                        .get(&entry.id)
                        .map_or(estimate, |size| f32::from(size.height)),
                })
            })
            .collect();
        let placement = place(
            &candidates,
            &self.appearance,
            viewport.width.into(),
            viewport.height.into(),
        );
        if self.queue.set_layout_visible(placement.ids(), self.now()) {
            cx.notify();
        }
        self.scrolls
            .retain(|id, _| entries.iter().any(|entry| entry.id == *id));
        self.card_sizes
            .retain(|id, _| entries.iter().any(|entry| entry.id == *id));
        self.expanded
            .retain(|id| entries.iter().any(|entry| entry.id == *id));
        let frame = Rc::new(RefCell::new(Vec::new()));
        let mut floating = div()
            .id("floating-notice")
            .test_support()
            .absolute()
            .inset_0();
        for (id, rect) in placement.floating {
            let Some(entry) = entries.iter().find(|entry| entry.id == id) else {
                continue;
            };
            floating = floating.child(
                div()
                    .absolute()
                    .left(px(rect.x))
                    .top(px(rect.y))
                    .w(px(rect.width))
                    .child(self.render_card(entry.clone(), frame.clone(), cx)),
            );
        }
        let weak = cx.weak_entity();
        div().absolute().inset_0().child(floating).child(
            canvas(
                move |_, _, cx| {
                    let records = frame.borrow().clone();
                    let _ = weak.update(cx, |this, cx| {
                        let occlusions: Vec<_> =
                            records.iter().map(|(_, bounds)| *bounds).collect();
                        let mut changed = false;
                        for (id, bounds) in records {
                            if this.card_sizes.get(&id) != Some(&bounds.size) {
                                this.card_sizes.insert(id, bounds.size);
                                changed = true;
                            }
                        }
                        if changed {
                            cx.notify();
                        }
                        if this.occlusions != occlusions {
                            this.occlusions = occlusions.clone();
                            cx.emit(NotificationOcclusions(occlusions));
                        }
                    });
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size(px(0.)),
        )
    }
}
