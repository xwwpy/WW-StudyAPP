//! Modal animation and message updates invalidate only their own surface.
use super::NotificationCenter;
use crate::{
    controls::Button,
    notice_queue::Notice,
    notification_card::{NotificationCardModel, notification_card},
};
use gpui_kit::{
    component::{Sizable, button::ButtonVariants},
    *,
};
use studyapp_core::{NoticeRoute, NoticeStyle, NotificationAppearance};

pub(super) struct NotificationModal {
    owner: WeakEntity<NotificationCenter>,
    entry: Option<(Notice, f32)>,
    appearance: NotificationAppearance,
    scroll: ScrollHandle,
}
impl NotificationModal {
    pub(super) fn new(
        owner: Entity<NotificationCenter>,
        entry: (Notice, f32),
        appearance: NotificationAppearance,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&owner, |this, owner, cx| {
            let owner = owner.read(cx);
            let next = owner
                .queue
                .visible()
                .find(|e| e.route == NoticeRoute::Foreground(NoticeStyle::ModalCard))
                .map(|e| (e.clone(), e.opacity(owner.now())));
            if this.entry.as_ref().map(|(e, _)| e.id) != next.as_ref().map(|(e, _)| e.id) {
                this.scroll = ScrollHandle::new();
            }
            this.entry = next;
            this.appearance = owner.appearance.clone();
            cx.notify();
        })
        .detach();
        Self {
            owner: owner.downgrade(),
            entry: Some(entry),
            appearance,
            scroll: ScrollHandle::new(),
        }
    }
}
impl Render for NotificationModal {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let Some((entry, opacity)) = &self.entry else {
            return div();
        };
        let Some(event) = &entry.event else {
            return div();
        };
        let id = entry.id;
        let close_owner = self.owner.clone();
        let action_owner = self.owner.clone();
        let hover_owner = self.owner.clone();
        let presented_owner = self.owner.clone();
        let close = Button::new(SharedString::from(format!("dismiss-notice-{id}")))
            .small()
            .ghost()
            .label("×")
            .accessibility_label("关闭通知")
            .on_click(move |_, _, cx| {
                let _ = close_owner.update(cx, |owner, cx| owner.dismiss(id, cx));
            });
        let action = event.action.map(|action| {
            Button::new(SharedString::from(format!("notice-action-{id}")))
                .small()
                .label(action.label())
                .on_click(move |_, _, cx| {
                    let _ = action_owner.update(cx, |owner, cx| owner.activate(id, cx));
                })
        });
        div().opacity(*opacity).child(
            div()
                .id(SharedString::from(format!("notice-{id}")))
                .test_support()
                .on_hover(move |hovered, _, cx| {
                    let _ = hover_owner.update(cx, |owner, cx| owner.hover(id, *hovered, cx));
                })
                .child(notification_card(
                    NotificationCardModel::from_event(
                        event,
                        entry.repetitions,
                        NoticeStyle::ModalCard,
                    ),
                    &self.appearance,
                    &self.scroll,
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
                                        NoticeRoute::Foreground(NoticeStyle::ModalCard),
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
        )
    }
}
