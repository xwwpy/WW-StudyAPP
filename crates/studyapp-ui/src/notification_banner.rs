//! In-flow banner host. Its height belongs to layout, not an overlay.
use super::NotificationCenter;
use crate::components::column;
use gpui_kit::*;
use std::{cell::RefCell, rc::Rc};
use studyapp_core::{NoticeLayout, NoticeRoute, NoticeStyle};

pub struct NotificationBanner {
    owner: WeakEntity<NotificationCenter>,
}
impl NotificationCenter {
    pub fn banner_content(&mut self, cx: &mut Context<Self>) -> Entity<NotificationBanner> {
        if let Some(surface) = &self.banner_surface {
            return surface.clone();
        }
        let owner = cx.entity();
        let surface = cx.new(|cx| {
            cx.observe(&owner, |_, _, cx| cx.notify()).detach();
            NotificationBanner {
                owner: owner.downgrade(),
            }
        });
        self.banner_surface = Some(surface.clone());
        surface
    }
}
impl Render for NotificationBanner {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.owner
            .update(cx, |center, cx| {
                let mut view = column().gap_0().min_w_0().w_full().flex_shrink_0();
                if center.appearance.layout(NoticeStyle::Banner) != NoticeLayout::Primary {
                    return view;
                }
                let entries: Vec<_> = center
                    .queue
                    .visible()
                    .filter(|entry| entry.route == NoticeRoute::Foreground(NoticeStyle::Banner))
                    .cloned()
                    .collect();
                let frame = Rc::new(RefCell::new(Vec::new()));
                for entry in entries {
                    view = view.child(center.render_card(entry, frame.clone(), cx));
                }
                let owner = cx.weak_entity();
                view.child(
                    canvas(
                        move |_, _, cx| {
                            let records = frame.borrow().clone();
                            let _ = owner.update(cx, |center, cx| {
                                let mut changed = false;
                                for (id, bounds) in records {
                                    if center.card_sizes.get(&id) != Some(&bounds.size) {
                                        center.card_sizes.insert(id, bounds.size);
                                        changed = true;
                                    }
                                }
                                if changed {
                                    cx.notify();
                                }
                            });
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size(px(0.)),
                )
            })
            .unwrap_or_else(|_| div())
    }
}
