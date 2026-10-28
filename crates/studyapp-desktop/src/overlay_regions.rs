//! Overlay ownership: one popup must never clear another surface's region.

use std::collections::BTreeMap;

pub struct OverlayRegions<T> {
    sources: BTreeMap<&'static str, Vec<T>>,
}

impl<T> Default for OverlayRegions<T> {
    fn default() -> Self {
        Self {
            sources: BTreeMap::new(),
        }
    }
}

impl<T: Clone + PartialEq> OverlayRegions<T> {
    pub fn set(&mut self, source: &'static str, bounds: Vec<T>) -> bool {
        if bounds.is_empty() {
            return self.sources.remove(source).is_some();
        }
        if self.sources.get(source) == Some(&bounds) {
            return false;
        }
        self.sources.insert(source, bounds);
        true
    }

    pub fn all(&self) -> Vec<T> {
        self.sources.values().flatten().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_a_menu_preserves_notification_regions() {
        let mut regions = OverlayRegions::default();
        assert!(regions.set("notices", vec![(10, 20, 80, 40)]));
        assert!(regions.set("menu", vec![(200, 20, 80, 160)]));
        assert_eq!(regions.all().len(), 2);
        assert!(regions.set("menu", vec![]));
        assert_eq!(regions.all(), vec![(10, 20, 80, 40)]);
        assert!(!regions.set("menu", vec![]));
    }

    #[test]
    fn redundant_layout_does_not_reapply_native_regions() {
        let mut regions = OverlayRegions::default();
        assert!(regions.set("menu", vec![1, 2]));
        assert!(!regions.set("menu", vec![1, 2]));
        assert!(regions.set("menu", vec![1, 3]));
        assert_eq!(regions.all(), vec![1, 3]);
    }

    #[test]
    fn tooltip_arrow_changes_update_union_and_close_preserves_other_sources() {
        use gpui_kit::{Bounds, point, px, size};
        use studyapp_ui::bubble::OverlayShape;
        let body = OverlayShape::card(Bounds::new(
            point(px(10.), px(20.)),
            size(px(100.), px(50.)),
        ));
        let arrow = OverlayShape::Triangle([
            point(px(50.), px(20.)),
            point(px(56.), px(14.)),
            point(px(62.), px(20.)),
        ]);
        let mut regions = OverlayRegions::default();
        regions.set("notices", vec![body]);
        regions.set("note-menu", vec![body]);
        assert!(regions.set("pointer-tooltip", vec![body, arrow]));
        assert!(!regions.set("pointer-tooltip", vec![body, arrow]));
        let changed = OverlayShape::Triangle([
            point(px(60.), px(20.)),
            point(px(66.), px(14.)),
            point(px(72.), px(20.)),
        ]);
        assert!(regions.set("pointer-tooltip", vec![body, changed]));
        assert!(regions.set("pointer-tooltip", vec![]));
        assert_eq!(regions.all(), vec![body, body]);
    }
}
#[cfg(all(test, feature = "ui-test"))]
mod popup_tests {
    use gpui_kit::component::menu::{PopupMenu, PopupMenuLayout};
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{
        Context, Entity, Focusable, IntoElement, ParentElement, Render, TestAppContext, Window, div,
    };
    use std::{cell::RefCell, rc::Rc};

    struct MenuSurface(Entity<PopupMenu>);
    impl Render for MenuSurface {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().child(self.0.clone())
        }
    }

    #[gpui_kit::test]
    fn submenu_layout_restores_on_reopening_at_same_position(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (surface, cx) = cx.add_window_view(|window, cx| {
            MenuSurface(PopupMenu::build(window, cx, |menu, window, cx| {
                menu.submenu("Move", window, cx, |child, _, _| {
                    child.link("Folder", "https://example.invalid")
                })
                .link("Other", "https://example.invalid")
            }))
        });
        let menu = cx.update(|_, cx| surface.read(cx).0.clone());
        let layouts = Rc::new(RefCell::new(Vec::new()));
        let recorded = layouts.clone();
        let _subscription = cx.update(|_, cx| {
            cx.subscribe(&menu, move |_, event: &PopupMenuLayout, _| {
                *recorded.borrow_mut() = event.0.clone();
            })
        });
        cx.update(|window, cx| {
            menu.read(cx).focus_handle(cx).focus(window, cx);
            window.render_frame(cx);
            window.press("down", cx);
            window.render_frame(cx);
            window.render_frame(cx);
        });
        let first = layouts.borrow().clone();
        assert_eq!(first.len(), 2, "submenu was not published");
        cx.update(|window, cx| {
            window.press("down", cx);
            window.render_frame(cx);
        });
        assert_eq!(
            layouts.borrow().len(),
            1,
            "closed submenu still clips the editor"
        );
        cx.update(|window, cx| {
            window.press("up", cx);
            window.render_frame(cx);
            window.render_frame(cx);
        });
        assert_eq!(
            *layouts.borrow(),
            first,
            "unchanged submenu bounds were omitted after reopening"
        );
    }
}
