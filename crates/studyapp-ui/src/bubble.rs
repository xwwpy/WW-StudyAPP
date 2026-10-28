//! Shared painted geometry and mounted ownership for floating information surfaces.
use gpui_kit::*;
use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OverlayShape {
    RoundedRect {
        bounds: Bounds<Pixels>,
        radius: Pixels,
    },
    Triangle([Point<Pixels>; 3]),
}
impl OverlayShape {
    pub fn card(bounds: Bounds<Pixels>) -> Self {
        Self::RoundedRect {
            bounds,
            radius: px(crate::design::OVERLAY_RADIUS),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BubbleLayout {
    pub body: Bounds<Pixels>,
    pub arrow: [Point<Pixels>; 3],
    pub radius: Pixels,
    pub above: bool,
}
impl BubbleLayout {
    pub fn shapes(self) -> Vec<OverlayShape> {
        vec![
            OverlayShape::RoundedRect {
                bounds: self.body,
                radius: self.radius,
            },
            OverlayShape::Triangle(self.arrow),
        ]
    }
}

#[derive(Clone, Copy)]
pub struct BubbleMetrics {
    pub gap: Pixels,
    pub margin: Pixels,
    pub depth: Pixels,
    pub half_base: Pixels,
    pub radius: Pixels,
}
impl Default for BubbleMetrics {
    fn default() -> Self {
        Self {
            gap: px(8.),
            margin: px(8.),
            depth: px(6.),
            half_base: px(6.),
            radius: px(6.),
        }
    }
}
fn finite(value: Pixels) -> Pixels {
    let value = f32::from(value);
    px(if value.is_finite() { value.max(0.) } else { 0. })
}

/// Raw window pointer in, absolute painted body and triangle out. No framework offset.
pub fn bubble_layout(
    anchor: Point<Pixels>,
    measured: Size<Pixels>,
    viewport: Size<Pixels>,
    metrics: BubbleMetrics,
) -> BubbleLayout {
    let width = finite(viewport.width);
    let height = finite(viewport.height);
    let margin = finite(metrics.margin).min(width * 0.5).min(height * 0.5);
    let anchor = point(finite(anchor.x).min(width), finite(anchor.y).min(height));
    let depth = finite(metrics.depth).min(height * 0.25);
    let gap = finite(metrics.gap).min(height * 0.25);
    let below = (height - margin - anchor.y - gap - depth).max(px(0.));
    let above = (anchor.y - margin - gap - depth).max(px(0.));
    let above = finite(measured.height) > below && above > below;
    let body_size = size(
        finite(measured.width).min((width - margin * 2.).max(px(0.))),
        finite(measured.height).min(if above {
            (anchor.y - margin - gap - depth).max(px(0.))
        } else {
            below
        }),
    );
    let x = (anchor.x - body_size.width * 0.5)
        .clamp(margin, (width - margin - body_size.width).max(margin));
    let y = if above {
        anchor.y - gap - depth - body_size.height
    } else {
        anchor.y + gap + depth
    };
    let y = y.clamp(margin, (height - margin - body_size.height).max(margin));
    let body = Bounds::new(point(x, y), body_size);
    let radius = finite(metrics.radius)
        .min(body_size.width * 0.5)
        .min(body_size.height * 0.5);
    let half = finite(metrics.half_base).min((body_size.width * 0.5 - radius).max(px(0.)));
    let center = anchor
        .x
        .clamp(body.left() + radius + half, body.right() - radius - half);
    let edge = if above { body.bottom() } else { body.top() };
    let tip = if above {
        (edge + depth).min(height)
    } else {
        (edge - depth).max(px(0.))
    };
    BubbleLayout {
        body,
        arrow: [
            point(center - half, edge),
            point(anchor.x, tip),
            point(center + half, edge),
        ],
        radius,
        above,
    }
}

#[derive(Clone, Debug)]
pub struct BubbleRegionsChanged(pub Vec<OverlayShape>);
#[derive(Default)]
pub struct BubbleRegistry {
    active: Option<(u64, Vec<OverlayShape>)>,
}
impl EventEmitter<BubbleRegionsChanged> for BubbleRegistry {}
impl BubbleRegistry {
    pub fn shapes(&self) -> Vec<OverlayShape> {
        self.active
            .as_ref()
            .map(|(_, shapes)| shapes.clone())
            .unwrap_or_default()
    }
    fn publish(&mut self, owner: u64, shapes: Vec<OverlayShape>, cx: &mut Context<Self>) {
        let changed = self.active.as_ref().is_none_or(|(_, old)| *old != shapes);
        self.active = Some((owner, shapes));
        if changed {
            cx.emit(BubbleRegionsChanged(self.shapes()));
        }
    }
    fn release(&mut self, owner: u64, cx: &mut Context<Self>) {
        if self
            .active
            .as_ref()
            .is_some_and(|(active, _)| *active == owner)
        {
            self.active = None;
            cx.emit(BubbleRegionsChanged(Vec::new()));
        }
    }
}
#[derive(Default)]
struct WindowRegistries(HashMap<WindowId, WeakEntity<BubbleRegistry>>);
impl Global for WindowRegistries {}
pub fn window_bubble_registry(window: &Window, cx: &mut App) -> Entity<BubbleRegistry> {
    let id = window.window_handle().window_id();
    if let Some(registry) = cx
        .default_global::<WindowRegistries>()
        .0
        .get(&id)
        .and_then(WeakEntity::upgrade)
    {
        return registry;
    }
    let registry = cx.new(|_| BubbleRegistry::default());
    cx.default_global::<WindowRegistries>()
        .0
        .insert(id, registry.downgrade());
    registry
}
static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);
struct MountedBubble {
    owner: u64,
    registry: WeakEntity<BubbleRegistry>,
    app: AsyncApp,
}
impl Drop for MountedBubble {
    fn drop(&mut self) {
        let owner = self.owner;
        let registry = self.registry.clone();
        self.app
            .spawn(async move |cx| {
                let _ = registry.update(cx, |registry, cx| registry.release(owner, cx));
            })
            .detach();
    }
}

pub(crate) struct PointerBubble {
    pub(crate) text: String,
    pub(crate) position: Point<Pixels>,
    pub(crate) registry: Entity<BubbleRegistry>,
}
impl Render for PointerBubble {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let owner = cx.weak_entity();
        let metrics = BubbleMetrics::default();
        let viewport = window.viewport_size();
        let available_width = (viewport.width - metrics.margin * 2.).max(px(0.));
        let available_height = bubble_layout(
            self.position,
            size(available_width, px(f32::MAX)),
            viewport,
            metrics,
        )
        .body
        .size
        .height;
        let padding = px(8.)
            .min(available_width * 0.25)
            .min(available_height * 0.25);
        let border = px(1.)
            .min(available_width * 0.25)
            .min(available_height * 0.25);
        let body = div()
            .id("sidebar-tooltip-content")
            .test_support()
            .min_w(px(120.).min(available_width))
            .max_w(px(320.).min(available_width))
            .max_h(available_height)
            .overflow_hidden()
            .overflow_y_scroll()
            .whitespace_normal()
            .p(padding)
            .rounded(metrics.radius)
            .border(border)
            .border_color(crate::theme::color(crate::components::BORDER))
            .bg(rgb(crate::theme::palette().surface_raised))
            .text_color(crate::theme::color(crate::components::INK))
            .text_sm()
            .shadow_md()
            .child(self.text.clone())
            .into_any_element();
        BubbleElement {
            body,
            anchor: self.position,
            registry: self.registry.clone(),
            metrics,
            owner,
        }
    }
}

struct BubbleElement {
    body: AnyElement,
    anchor: Point<Pixels>,
    registry: Entity<BubbleRegistry>,
    metrics: BubbleMetrics,
    owner: WeakEntity<PointerBubble>,
}
impl IntoElement for BubbleElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for BubbleElement {
    type RequestLayoutState = ();
    type PrepaintState = BubbleLayout;
    fn id(&self) -> Option<ElementId> {
        Some("pointer-bubble-surface".into())
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (window.request_layout(Style::default(), [], cx), ())
    }
    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> BubbleLayout {
        let viewport = window.viewport_size();
        let measured = self.body.layout_as_root(
            size(AvailableSpace::MaxContent, AvailableSpace::MinContent),
            window,
            cx,
        );
        let mut layout = bubble_layout(self.anchor, measured, viewport, self.metrics);
        // layout_bounds snaps absolute element offsets to the device grid.
        // Publish that same body and join the arrow to its actual edge, while
        // retaining the raw-pointer tip and gap from the mathematical layout.
        layout.body = window.pixel_snap_bounds(layout.body);
        let half = (layout.arrow[2].x - layout.arrow[0].x) * 0.5;
        let inset = (layout.radius + half).min(layout.body.size.width * 0.5);
        let center = layout.arrow[1]
            .x
            .clamp(layout.body.left() + inset, layout.body.right() - inset);
        let edge = if layout.above {
            layout.body.bottom()
        } else {
            layout.body.top()
        };
        layout.arrow[0] = point(center - half, edge);
        layout.arrow[2] = point(center + half, edge);
        self.body.prepaint_at(layout.body.origin, window, cx);
        window.with_element_state(
            id.expect("bubble surface id"),
            |lease: Option<MountedBubble>, _| {
                let lease = lease.unwrap_or_else(|| MountedBubble {
                    owner: NEXT_OWNER.fetch_add(1, Ordering::Relaxed),
                    registry: self.registry.downgrade(),
                    app: cx.to_async(),
                });
                self.registry.update(cx, |registry, cx| {
                    registry.publish(lease.owner, layout.shapes(), cx)
                });
                ((), lease)
            },
        );
        layout
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        layout: &mut BubbleLayout,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.body.paint(window, cx);
        let mut path = PathBuilder::fill();
        path.move_to(layout.arrow[0]);
        path.line_to(layout.arrow[1]);
        path.line_to(layout.arrow[2]);
        path.close();
        if let Ok(path) = path.build() {
            // Blur tails extend beyond the shadow bounds used for scene
            // ordering. Force the arrow above every body shadow, including
            // when its triangle only touches the body's recorded rectangle.
            let layer = layout
                .body
                .union(&Bounds::new(layout.arrow[1], Size::default()));
            window.paint_layer(layer, |window| {
                window.paint_path(path, rgb(crate::theme::palette().surface_raised));
            });
        }
        let owner = self.owner.clone();
        window.on_mouse_event::<MouseMoveEvent>(move |event, phase, _, cx| {
            if phase == DispatchPhase::Capture {
                let _ = owner.update(cx, |this, cx| {
                    if this.position != event.position {
                        this.position = event.position;
                        cx.notify();
                    }
                });
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{BubbleMetrics, bubble_layout};
    use gpui_kit::{point, px, size};
    #[::core::prelude::v1::test]
    fn centered_raw_pointer_and_flip_use_one_geometry() {
        let m = BubbleMetrics::default();
        let layout = bubble_layout(
            point(px(300.), px(100.)),
            size(px(200.), px(40.)),
            size(px(600.), px(400.)),
            m,
        );
        assert_eq!(layout.body.center().x, px(300.));
        assert_eq!(layout.body.top(), px(114.));
        assert_eq!(layout.arrow[1], point(px(300.), px(108.)));
        let flipped = bubble_layout(
            point(px(300.), px(380.)),
            size(px(200.), px(40.)),
            size(px(600.), px(400.)),
            m,
        );
        assert!(flipped.above);
        assert_eq!(flipped.body.bottom(), px(366.));
        assert_eq!(flipped.arrow[1], point(px(300.), px(372.)));
    }
    #[::core::prelude::v1::test]
    fn corners_and_small_viewports_bound_the_body_and_preserve_pointer_tip() {
        for viewport in [
            size(px(600.), px(400.)),
            size(px(30.), px(24.)),
            size(px(0.), px(0.)),
        ] {
            for anchor in [
                point(px(0.), px(0.)),
                point(viewport.width, px(0.)),
                point(px(0.), viewport.height),
                point(viewport.width, viewport.height),
            ] {
                let layout = bubble_layout(
                    anchor,
                    size(px(500.), px(800.)),
                    viewport,
                    BubbleMetrics::default(),
                );
                assert!(layout.body.left() >= px(0.) && layout.body.right() <= viewport.width);
                assert!(layout.body.top() >= px(0.) && layout.body.bottom() <= viewport.height);
                assert_eq!(layout.arrow[1].x, anchor.x);
                assert!(
                    layout.arrow[0].x >= layout.body.left()
                        && layout.arrow[2].x <= layout.body.right()
                );
            }
        }
    }
    #[::core::prelude::v1::test]
    fn invalid_measurement_is_finite() {
        let layout = bubble_layout(
            point(px(f32::NAN), px(f32::INFINITY)),
            size(px(f32::NAN), px(-1.)),
            size(px(30.), px(20.)),
            BubbleMetrics::default(),
        );
        assert!(f32::from(layout.body.left()).is_finite());
        assert_eq!(layout.body.size, size(px(0.), px(0.)));
    }
    #[cfg(feature = "ui-test")]
    mod integration {
        use super::super::{
            BubbleRegionsChanged, BubbleRegistry, OverlayShape, PointerBubble,
            window_bubble_registry,
        };
        use gpui_kit::test::TestWindowExt;
        use gpui_kit::{
            AppContext, Context, Entity, IntoElement, MouseMoveEvent, ParentElement, Render,
            StyleRefinement, Styled, TestAppContext, Window, div, point, px, size,
        };

        #[gpui_kit::test]
        fn odd_body_and_arrow_publish_the_actual_snapped_prepaint_geometry(
            cx: &mut TestAppContext,
        ) {
            use super::super::{BubbleElement, BubbleMetrics};
            use gpui_kit::{InteractiveElement, TestSupportExt};
            struct OddBody(Entity<PointerBubble>);
            impl Render for OddBody {
                fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                    let bubble = self.0.read(cx);
                    BubbleElement {
                        body: div()
                            .id("odd-bubble-body")
                            .test_support()
                            .w(px(223.))
                            .h(px(38.))
                            .into_any_element(),
                        anchor: bubble.position,
                        registry: bubble.registry.clone(),
                        metrics: BubbleMetrics::default(),
                        owner: self.0.downgrade(),
                    }
                }
            }
            cx.update(gpui_kit::init);
            let pointer = point(px(595.), px(427.75));
            let (host, cx) = cx.add_window_view(|window, cx| {
                let registry = window_bubble_registry(window, cx);
                OddBody(cx.new(|_| PointerBubble {
                    text: String::new(),
                    position: pointer,
                    registry,
                }))
            });
            cx.simulate_resize(size(px(1000.), px(700.)));
            for scale in [1., 1.25, 1.5, 2.] {
                cx.simulate_scale_factor_change(scale);
                cx.update(|window, cx| {
                    window.render_frame(cx);
                    let actual = window.find("odd-bubble-body").bounds();
                    if scale == 1. {
                        assert_eq!(actual.size, size(px(223.), px(38.)));
                    }
                    let shapes = host.read(cx).0.read(cx).registry.read(cx).shapes();
                    let OverlayShape::RoundedRect {
                        bounds: published, ..
                    } = shapes[0]
                    else {
                        panic!()
                    };
                    assert_eq!(
                        published, actual,
                        "native coverage used unsnapped mathematical body at scale={scale}"
                    );
                    assert_eq!(
                        actual.origin,
                        window.pixel_snap_point(point(
                            pointer.x - actual.size.width * 0.5,
                            pointer.y + px(14.)
                        ))
                    );
                    let OverlayShape::Triangle(arrow) = shapes[1] else {
                        panic!()
                    };
                    assert_eq!(
                        arrow[0].y,
                        actual.top(),
                        "arrow base must join the actual body"
                    );
                    assert_eq!(arrow[2].y, actual.top());
                    assert_eq!(
                        arrow[1],
                        point(pointer.x, pointer.y + px(8.)),
                        "snapping body must preserve raw-pointer tip/gap"
                    );
                });
            }
        }
        #[gpui_kit::test]
        fn registry_rejects_stale_release_and_deduplicates_geometry(cx: &mut TestAppContext) {
            let registry = cx.new(|_| BubbleRegistry::default());
            let events = std::rc::Rc::new(std::cell::Cell::new(0));
            let observed = events.clone();
            let _subscription = cx.update(|cx| {
                cx.subscribe(&registry, move |_, _: &BubbleRegionsChanged, _| {
                    observed.set(observed.get() + 1)
                })
            });
            let notifications = std::rc::Rc::new(std::cell::Cell::new(0));
            let observed = notifications.clone();
            let _observer =
                cx.update(|cx| cx.observe(&registry, move |_, _| observed.set(observed.get() + 1)));
            let shapes = vec![OverlayShape::Triangle([point(px(0.), px(0.)); 3])];
            registry.update(cx, |r, cx| {
                r.publish(1, shapes.clone(), cx);
                r.publish(2, shapes.clone(), cx);
                r.release(1, cx);
            });
            cx.run_until_parked();
            assert_eq!(events.get(), 1);
            assert_eq!(cx.update(|cx| registry.read(cx).shapes()), shapes);
            registry.update(cx, |r, cx| r.release(2, cx));
            cx.run_until_parked();
            assert!(cx.update(|cx| registry.read(cx).shapes()).is_empty());
            assert_eq!(events.get(), 2);
            assert_eq!(
                notifications.get(),
                0,
                "geometry events notified view observers"
            );
        }

        struct CachedContent(std::rc::Rc<std::cell::Cell<usize>>);
        impl Render for CachedContent {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                self.0.set(self.0.get() + 1);
                div().size_full()
            }
        }
        struct BubbleHarness {
            bubble: Entity<PointerBubble>,
            mounted: bool,
            renders: std::rc::Rc<std::cell::Cell<usize>>,
            cached: Entity<CachedContent>,
        }
        impl Render for BubbleHarness {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                self.renders.set(self.renders.get() + 1);
                div()
                    .size_full()
                    .child(
                        self.cached
                            .clone()
                            .cached(StyleRefinement::default().size_full()),
                    )
                    .children(self.mounted.then(|| self.bubble.clone()))
            }
        }
        #[gpui_kit::test]
        fn measured_bubble_clamps_flips_resizes_and_unmounts_without_registry_notify(
            cx: &mut TestAppContext,
        ) {
            cx.update(gpui_kit::init);
            let (host, cx) = cx.add_window_view(|window, cx| {
                let registry = window_bubble_registry(window, cx);
                let bubble = cx.new(|_| PointerBubble {
                    text: "Long mixed 内容 and averylongunbrokentoken".repeat(70),
                    position: point(px(200.), px(100.)),
                    registry,
                });
                BubbleHarness {
                    bubble,
                    mounted: true,
                    renders: Default::default(),
                    cached: cx.new(|_| CachedContent(Default::default())),
                }
            });
            cx.simulate_resize(size(px(420.), px(300.)));
            let (bubble, renders) =
                host.read_with(cx, |host, _| (host.bubble.clone(), host.renders.clone()));
            cx.update(|window, cx| window.render_frame(cx));
            let mut count = renders.get();
            let cached = host.read_with(cx, |host, cx| host.cached.read(cx).0.clone());
            let cached_count = cached.get();
            let id = bubble.entity_id();
            for position in [point(px(410.), px(270.)), point(px(20.), px(30.))] {
                cx.simulate_event(MouseMoveEvent {
                    position,
                    ..Default::default()
                });
                cx.run_until_parked();
                cx.update(|window, cx| {
                    window.draw(cx).clear(cx);
                    let bounds = window.find("sidebar-tooltip-content").bounds();
                    let shapes = bubble.read(cx).registry.read(cx).shapes();
                    let OverlayShape::RoundedRect { bounds: body, .. } = shapes[0] else {
                        panic!()
                    };
                    assert_eq!(bounds, body, "paint and native body geometry differ");
                    assert!(bounds.left() >= px(8.) && bounds.right() <= px(412.));
                    assert!(bounds.top() >= px(8.) && bounds.bottom() <= px(292.));
                    let OverlayShape::Triangle(points) = shapes[1] else {
                        panic!()
                    };
                    assert_eq!(points[1].x, position.x);
                });
                assert_eq!(bubble.entity_id(), id);
                // Window::draw_root always renders the root, and simulate_event plus
                // explicit render_frame each draw once. Publishing must not enqueue
                // another draw through registry notify/observe feedback.
                assert_eq!(
                    renders.get(),
                    count + 2,
                    "registry added a redraw feedback loop"
                );
                count = renders.get();
                assert_eq!(
                    cached.get(),
                    cached_count,
                    "pointer geometry rebuilt cached content"
                );
            }
            cx.simulate_resize(size(px(280.), px(200.)));
            cx.update(|window, cx| {
                window.render_frame(cx);
                let bounds = window.find("sidebar-tooltip-content").bounds();
                assert!(bounds.right() <= px(272.) && bounds.bottom() <= px(192.));
            });
            cx.simulate_resize(size(px(30.), px(24.)));
            cx.update(|window, cx| {
                window.render_frame(cx);
                let bounds = window.find("sidebar-tooltip-content").bounds();
                let shapes = bubble.read(cx).registry.read(cx).shapes();
                let OverlayShape::RoundedRect { bounds: body, .. } = shapes[0] else {
                    panic!()
                };
                assert_eq!(bounds, body, "tiny viewport paint differed from occlusion");
                assert!(bounds.right() <= px(30.) && bounds.bottom() <= px(24.));
            });
            host.update(cx, |host, cx| {
                host.mounted = false;
                cx.notify();
            });
            for _ in 0..3 {
                cx.update(|window, cx| window.render_frame(cx));
                cx.run_until_parked();
            }
            assert!(
                bubble
                    .read_with(cx, |bubble, cx| bubble.registry.read(cx).shapes())
                    .is_empty(),
                "retained view handle kept an unmounted native hole"
            );
            host.update(cx, |host, cx| {
                host.mounted = true;
                cx.notify();
            });
            cx.update(|window, cx| window.render_frame(cx));
            assert_eq!(
                bubble
                    .read_with(cx, |bubble, cx| bubble.registry.read(cx).shapes())
                    .len(),
                2
            );
        }

        #[gpui_kit::test]
        fn registries_are_window_local(cx: &mut TestAppContext) {
            let (first_host, first_id) = {
                let (first_host, first) = cx.add_window_view(|window, cx| {
                    let registry = window_bubble_registry(window, cx);
                    assert_eq!(
                        registry.entity_id(),
                        window_bubble_registry(window, cx).entity_id()
                    );
                    BubbleHarness {
                        bubble: cx.new(|_| PointerBubble {
                            text: "first".into(),
                            position: point(px(100.), px(100.)),
                            registry,
                        }),
                        mounted: true,
                        renders: Default::default(),
                        cached: cx.new(|_| CachedContent(Default::default())),
                    }
                });
                first.update(|window, cx| window.render_frame(cx));
                let first_id =
                    first.update(|window, cx| window_bubble_registry(window, cx).entity_id());
                (first_host, first_id)
            };
            let (_, second) = cx.add_window_view(|window, cx| {
                let registry = window_bubble_registry(window, cx);
                assert_ne!(registry.entity_id(), first_id);
                assert!(registry.read(cx).shapes().is_empty());
                BubbleHarness {
                    bubble: cx.new(|_| PointerBubble {
                        text: "second".into(),
                        position: point(px(200.), px(100.)),
                        registry,
                    }),
                    mounted: true,
                    renders: Default::default(),
                    cached: cx.new(|_| CachedContent(Default::default())),
                }
            });
            second.update(|window, cx| window.render_frame(cx));
            assert_eq!(
                first_host.read_with(second, |host, cx| host.bubble.read(cx).registry.entity_id()),
                first_id
            );
        }

        struct PopoverHarness {
            mounted: bool,
            origin: gpui_kit::Point<gpui_kit::Pixels>,
            body: std::rc::Rc<std::cell::Cell<Option<gpui_kit::Bounds<gpui_kit::Pixels>>>>,
            geometry: std::rc::Rc<
                std::cell::Cell<Option<gpui_kit::component::popover::PopoverSurfaceGeometry>>,
            >,
        }
        impl Render for PopoverHarness {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let body = self.body.clone();
                let geometry = self.geometry.clone();
                div().size_full().children(self.mounted.then(|| {
                    div()
                        .absolute()
                        .left(self.origin.x)
                        .top(self.origin.y)
                        .child(
                            gpui_kit::component::popover::Popover::new("geometry-popup")
                                .default_open(true)
                                .arrow(true)
                                .rounded(px(12.))
                                .on_surface_layout(move |bounds, _| body.set(bounds))
                                .on_surface_geometry(move |shape, _| geometry.set(shape))
                                .trigger(
                                    gpui_kit::component::button::Button::new("geometry-trigger")
                                        .size(px(40.)),
                                )
                                .child(div().size(px(60.))),
                        )
                }))
            }
        }
        #[gpui_kit::test]
        fn popover_geometry_preserves_body_callback_and_current_frame_arrow(
            cx: &mut TestAppContext,
        ) {
            cx.update(gpui_kit::init);
            let body = std::rc::Rc::new(std::cell::Cell::new(None));
            let geometry = std::rc::Rc::new(std::cell::Cell::new(None));
            let (view, cx) = cx.add_window_view(|_, _| PopoverHarness {
                body: body.clone(),
                geometry: geometry.clone(),
                origin: point(px(200.), px(100.)),
                mounted: true,
            });
            for origin in [
                point(px(200.), px(100.)),
                point(px(1900.), px(100.)),
                point(px(1880.), px(100.)),
            ] {
                view.update(cx, |view, cx| {
                    view.origin = origin;
                    cx.notify();
                });
                cx.update(|window, cx| window.render_frame(cx));
                let geometry = geometry.get().expect("missing painted geometry");
                assert_eq!(
                    body.get(),
                    Some(geometry.body),
                    "body callback included an arrow envelope"
                );
                assert_eq!(
                    geometry.radius,
                    px(12.),
                    "callback omitted the actual surface radius"
                );
                let points = geometry.arrow.expect("missing current-frame arrow");
                let half = px(6.).min(geometry.body.size.width * 0.5);
                let inset = (geometry.radius + half).min(geometry.body.size.width * 0.5);
                let target = origin
                    .x
                    .clamp(geometry.body.left() + inset, geometry.body.right() - inset);
                assert_eq!(
                    points,
                    [
                        point(target - half, geometry.body.top()),
                        point(target, geometry.body.top() - px(6.)),
                        point(target + half, geometry.body.top())
                    ]
                );
            }
            view.update(cx, |view, cx| {
                view.mounted = false;
                cx.notify();
            });
            for _ in 0..3 {
                cx.update(|window, cx| window.render_frame(cx));
                cx.run_until_parked();
            }
            assert!(body.get().is_none());
            assert!(geometry.get().is_none());
        }
    }
}
