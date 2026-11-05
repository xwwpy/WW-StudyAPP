//! Mounted, clipped control geometry. A stale unmount cannot erase a newer owner.
use crate::notifications::NotificationCenter;
use gpui_kit::*;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);
struct MountedAnchor {
    owner: u64,
    key: &'static str,
    center: WeakEntity<NotificationCenter>,
    center_id: EntityId,
    app: AsyncApp,
}
impl Drop for MountedAnchor {
    fn drop(&mut self) {
        let (owner, key, center) = (self.owner, self.key, self.center.clone());
        self.app
            .spawn(async move |cx| {
                let _ = center.update(cx, |center, cx| center.release_anchor(key, owner, cx));
            })
            .detach();
    }
}
/// Wraps a real control without adding layout, focus or input handlers.
pub struct NotificationAnchor {
    key: &'static str,
    center: Entity<NotificationCenter>,
    body: AnyElement,
}
impl NotificationAnchor {
    pub fn new(
        key: &'static str,
        center: Entity<NotificationCenter>,
        body: impl IntoElement,
    ) -> Self {
        Self {
            key,
            center,
            body: body.into_any_element(),
        }
    }
}
impl IntoElement for NotificationAnchor {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for NotificationAnchor {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        Some(SharedString::from(format!("notification-anchor-{}", self.key)).into())
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
        (self.body.request_layout(window, cx), ())
    }
    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.body.prepaint(window, cx);
        let clipped = bounds
            .intersect(&window.content_mask().bounds)
            .intersect(&Bounds::new(Point::default(), window.viewport_size()));
        window.with_element_state(
            id.expect("notification anchor id"),
            |lease: Option<MountedAnchor>, _| {
                let lease = lease
                    .filter(|lease| lease.center_id == self.center.entity_id())
                    .unwrap_or_else(|| MountedAnchor {
                        owner: NEXT_OWNER.fetch_add(1, Ordering::Relaxed),
                        key: self.key,
                        center: self.center.downgrade(),
                        center_id: self.center.entity_id(),
                        app: cx.to_async(),
                    });
                self.center.update(cx, |center, cx| {
                    center.publish_anchor(self.key, lease.owner, clipped, cx)
                });
                ((), lease)
            },
        );
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.body.paint(window, cx);
    }
}
