//! Scrollbar decoration for a virtual list whose viewport is measured in prepaint.
use gpui_kit::component::scroll::{Scrollbar, ScrollbarHandle};
use gpui_kit::{
    App, Bounds, ElementId, IntoElement, ListState, ParentElement, Pixels, Point, RenderOnce, Size,
    Styled, Window, div,
};

/// Append after the list so measurement and painting use the current frame.
/// `right_gutter` is only the space explicitly reserved beside the list.
pub fn virtual_list_scrollbar(
    id: impl Into<ElementId>,
    state: &ListState,
    right_gutter: Pixels,
) -> impl IntoElement {
    ListScrollbar {
        id: id.into(),
        handle: ListScrollbarHandle {
            state: state.clone(),
            right_gutter,
        },
    }
}

#[derive(Clone)]
struct ListScrollbarHandle {
    state: ListState,
    right_gutter: Pixels,
}

impl ScrollbarHandle for ListScrollbarHandle {
    fn viewport_bounds(&self) -> Bounds<Pixels> {
        let mut bounds = self.state.viewport_bounds();
        bounds.size.width += self.right_gutter;
        bounds
    }

    fn offset(&self) -> Point<Pixels> {
        ScrollbarHandle::offset(&self.state)
    }
    fn set_offset(&self, offset: Point<Pixels>) {
        ScrollbarHandle::set_offset(&self.state, offset);
    }
    fn content_size(&self) -> Size<Pixels> {
        let mut size = ScrollbarHandle::content_size(&self.state);
        size.width += self.right_gutter;
        size
    }
    fn start_drag(&self) {
        ScrollbarHandle::start_drag(&self.state);
    }
    fn end_drag(&self) {
        ScrollbarHandle::end_drag(&self.state);
    }
}

#[derive(IntoElement)]
struct ListScrollbar {
    id: ElementId,
    handle: ListScrollbarHandle,
}

impl RenderOnce for ListScrollbar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let scrollbar = (!window.is_inspector_picking(cx))
            .then(|| Scrollbar::vertical(&self.handle).id(self.id));
        div().absolute().size_0().children(scrollbar)
    }
}

#[cfg(all(test, feature = "ui-test"))]
mod tests {
    use super::virtual_list_scrollbar;
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{
        Context, InputEvent, InteractiveElement, IntoElement, ListAlignment, ListState,
        MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Render, Styled,
        TestAppContext, TestSupportExt, Window, div, list, point, px, rgb,
    };

    const ROW_COLOR: u32 = 0x1259A3;

    struct ListFixture {
        state: ListState,
        gutter: f32,
        width: f32,
        height: f32,
    }

    impl Render for ListFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().p(px(24.)).child(
                div()
                    .id("list-viewport")
                    .test_support()
                    .relative()
                    .w(px(self.width))
                    .h(px(self.height))
                    .border_1()
                    .px(px(8.))
                    .pr(px(self.gutter))
                    .overflow_hidden()
                    .child(
                        list(self.state.clone(), |ix, _, _| {
                            div()
                                .id(("colored-row", ix))
                                .test_support()
                                .h(px(if ix % 2 == 0 { 32. } else { 48. }))
                                .bg(rgb(ROW_COLOR))
                                .into_any_element()
                        })
                        .size_full(),
                    )
                    .child(virtual_list_scrollbar(
                        "list-scrollbar",
                        &self.state,
                        px(self.gutter),
                    )),
            )
        }
    }

    #[gpui_kit::test]
    fn virtual_list_scrollbar_uses_current_bounds_and_paints_after_rows(cx: &mut TestAppContext) {
        check_list_scrollbar(cx, 0.);
    }

    #[gpui_kit::test]
    fn virtual_list_scrollbar_preserves_reserved_gutter(cx: &mut TestAppContext) {
        check_list_scrollbar(cx, 18.);
    }

    fn check_list_scrollbar(cx: &mut TestAppContext, gutter: f32) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::apply(&Default::default(), false, cx);
        });
        for count in [2, 24] {
            let state = ListState::new(count, ListAlignment::Top, px(0.));
            state.reset_with_uniform_height(count, px(40.));
            let (view, test_cx) = cx.add_window_view(|_, _| ListFixture {
                // add_window_view may draw while parking its executor. Keep
                // the real state fresh until the explicitly asserted frame.
                state: ListState::new(0, ListAlignment::Top, px(0.)),
                gutter,
                width: 360.,
                height: 160.,
            });
            assert_eq!(state.viewport_bounds().size.height, px(0.));
            for (width, height) in [(360., 160.), (280., 112.), (280., 400.)] {
                // Keep installation, drawing and inspection in one transaction:
                // separate context updates can flush a frame and hide stale bounds.
                test_cx.update(|window, cx| {
                    view.update(cx, |view, cx| {
                        view.state = state.clone();
                        view.width = width;
                        view.height = height;
                        cx.notify();
                    });
                    window.render_frame(cx);
                        let scale = window.scale_factor();
                        let viewport = state.viewport_bounds();
                        assert_eq!(viewport.size.height, px(height - 2.));
                        let viewport = window.pixel_snap_bounds(viewport).scale(scale);
                        let quads = window.painted_quads();
                        let rows: Vec<_> = quads.iter().enumerate()
                            .filter(|(_, quad)| quad.background == rgb(ROW_COLOR).into()).collect();
                        let thumbs: Vec<_> = quads.iter().enumerate()
                            .filter(|(_, quad)| !quad.background.is_transparent()
                                && quad.background != rgb(ROW_COLOR).into()).collect();
                        assert!(!rows.is_empty());
                        if count == 2 {
                            assert!(thumbs.is_empty(), "fitting virtual list has a thumb");
                            return;
                        }
                        assert_eq!(thumbs.len(), 1, "overflowing virtual list must paint a thumb on this frame");
                        let (index, thumb) = thumbs[0];
                        assert!(rows.iter().all(|(row_index, _)| *row_index < index),
                            "scrollbar must paint after colored row backgrounds");
                        assert!(thumb.bounds.top() >= viewport.top() && thumb.bounds.bottom() <= viewport.bottom(),
                            "scrollbar uses stale height: {:?}, viewport={viewport:?}", thumb.bounds);
                        let content_height = viewport.size.height + state.max_offset_for_scrollbar().y.scale(scale);
                        let inset = thumb.bounds.top() - viewport.top();
                        let expected_length = (viewport.size.height * (viewport.size.height / content_height))
                            .max(px(48.).scale(scale)) - inset * 2.;
                        assert!(thumb.bounds.size.height >= expected_length - px(1.).scale(scale) && thumb.bounds.size.height <= expected_length + px(1.).scale(scale),
                            "thumb length uses a previous frame or includes parent borders");
                        let right = viewport.right() + px(gutter).scale(scale);
                        assert!(thumb.bounds.right() <= right && thumb.bounds.right() >= right - px(4.).scale(scale),
                            "thumb misses reserved gutter: {:?}, viewport={viewport:?}, gutter={gutter}", thumb.bounds);
                        if gutter > 0. {
                            assert!(thumb.bounds.left() >= viewport.right(), "thumb overlaps colored selected rows");
                        }
                    });
            }
            if count > 2 {
                test_cx.update(|window, cx| {
                    let viewport = state.viewport_bounds();
                    let x = viewport.right() + px(gutter) - px(8.);
                    let from = point(x, viewport.top() + px(8.));
                    let to = point(x, viewport.bottom() - px(2.));
                    window.dispatch_event(
                        MouseDownEvent {
                            position: from,
                            button: MouseButton::Left,
                            click_count: 1,
                            ..Default::default()
                        }
                        .to_platform_input(),
                        cx,
                    );
                    assert!(
                        state.is_scrollbar_dragging(),
                        "start_drag must be forwarded"
                    );
                    for step in 1..=8 {
                        window.dispatch_event(
                            MouseMoveEvent {
                                position: from + (to - from) * (step as f32 / 8.),
                                pressed_button: Some(MouseButton::Left),
                                ..Default::default()
                            }
                            .to_platform_input(),
                            cx,
                        );
                        window.render_frame(cx);
                    }
                    window.dispatch_event(
                        MouseUpEvent {
                            position: to,
                            button: MouseButton::Left,
                            click_count: 1,
                            ..Default::default()
                        }
                        .to_platform_input(),
                        cx,
                    );
                    window.render_frame(cx);
                    let last = state
                        .bounds_for_item(count - 1)
                        .expect("drag must reveal the last row");
                    assert!(
                        last.bottom() <= viewport.bottom() + px(1.),
                        "last variable row is clipped after drag"
                    );
                    assert!(!state.is_scrollbar_dragging(), "end_drag must be forwarded");
                });
            }
        }
    }
}
