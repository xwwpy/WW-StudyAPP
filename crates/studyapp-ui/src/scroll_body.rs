//! A bounded viewport whose scrollbar is finalized after all content children.
use gpui_kit::component::{InteractiveElementExt, scroll::Scrollbar};
use gpui_kit::*;

/// Keeps the caller's ID, scroll handle, clipping and interaction on one node.
/// Finalizing the decoration at layout time makes it the last child, without
/// globally lifting it over menus, popups or modal layers.
pub struct ScrollBody {
    inner: Stateful<Div>,
    scrollbar: Option<ScrollHandle>,
}

pub fn scroll_body(id: impl Into<ElementId>, handle: &ScrollHandle) -> ScrollBody {
    let wheel_handle = handle.clone();
    ScrollBody {
        inner: crate::components::column()
            .id(id)
            .relative()
            .min_h_0()
            .min_w_0()
            .overflow_y_scroll()
            .track_scroll(handle)
            .lock_scroll_axis()
            .on_scroll_wheel(move |event, window, cx| {
                // Child inputs and horizontal tables run first. An overflowing
                // pane remains independent, including at its first/last row.
                if event.delta.pixel_delta(window.line_height()).y != px(0.)
                    && wheel_handle.max_offset().y > px(0.)
                {
                    cx.stop_propagation();
                }
            }),
        scrollbar: Some(handle.clone()),
    }
}

impl Styled for ScrollBody {
    fn style(&mut self) -> &mut StyleRefinement {
        self.inner.style()
    }
}
impl InteractiveElement for ScrollBody {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.inner.interactivity()
    }
}
impl StatefulInteractiveElement for ScrollBody {}
impl ParentElement for ScrollBody {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.inner.extend(elements);
    }
}
impl IntoElement for ScrollBody {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for ScrollBody {
    type RequestLayoutState = <Stateful<Div> as Element>::RequestLayoutState;
    type PrepaintState = <Stateful<Div> as Element>::PrepaintState;

    fn id(&self) -> Option<ElementId> {
        Element::id(&self.inner)
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        self.inner.source_location()
    }
    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        if let Some(handle) = self.scrollbar.take()
            && !window.is_inspector_picking(cx)
        {
            // No footprint: a viewport-sized absolute decoration would create
            // overflow from padding and could move with the scrolled content.
            self.inner.extend([div()
                .absolute()
                .size_0()
                .child(Scrollbar::vertical(&handle).id("scroll-body-scrollbar"))
                .into_any_element()]);
        }
        self.inner.request_layout(id, inspector_id, window, cx)
    }
    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.inner
            .prepaint(id, inspector_id, bounds, state, window, cx)
    }
    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.inner
            .paint(id, inspector_id, bounds, state, prepaint, window, cx);
    }
    fn a11y_role(&self) -> Option<accesskit::Role> {
        self.inner.a11y_role()
    }
    fn write_a11y_info(&self, node: &mut accesskit::Node) {
        self.inner.write_a11y_info(node);
    }
    fn a11y_synthetic_children(
        &mut self,
        prepaint: &mut Self::PrepaintState,
        builder: &mut A11ySubtreeBuilder,
    ) {
        Element::a11y_synthetic_children(&mut self.inner, prepaint, builder);
    }
}
