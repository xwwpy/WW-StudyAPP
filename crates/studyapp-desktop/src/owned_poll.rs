//! Poll a known owning window, not the entity's last inferred render window.
use gpui_kit::{AsyncWindowContext, Context, WeakEntity, Window};

/// False means the owner/window is gone. A native nested event may temporarily
/// borrow the App; defer that tick rather than permanently stopping the loop.
pub(crate) fn update<T: 'static>(
    owner: &WeakEntity<T>,
    cx: &mut AsyncWindowContext,
    poll: impl FnOnce(&mut T, &mut Window, &mut Context<T>),
) -> bool {
    match cx.update(|window, cx| owner.update(cx, |owner, cx| poll(owner, window, cx))) {
        Ok(result) => result.is_ok(),
        Err(error) => error.is::<std::cell::BorrowMutError>() && owner.upgrade().is_some(),
    }
}

#[cfg(all(test, feature = "ui-test"))]
mod tests {
    use super::*;
    use gpui_kit::{IntoElement, Render, TestAppContext, div};
    struct Probe(usize);
    impl Render for Probe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }
    #[gpui_kit::test]
    fn temporary_app_borrow_defers_a_tick_and_the_next_tick_runs(cx: &mut TestAppContext) {
        let (owner, visual) = cx.add_window_view(|_, _| Probe(0));
        let weak = owner.downgrade();
        let mut async_cx = visual.update(|window, cx| window.to_async(cx));
        visual.update(|_, _| {
            assert!(update(&weak, &mut async_cx, |owner, _, _| owner.0 += 1));
        });
        assert_eq!(owner.read_with(visual, |owner, _| owner.0), 0);
        assert!(update(&weak, &mut async_cx, |owner, _, _| owner.0 += 1));
        assert_eq!(owner.read_with(visual, |owner, _| owner.0), 1);
    }
    #[gpui_kit::test]
    fn closed_owning_window_stops_even_if_an_entity_handle_is_retained(cx: &mut TestAppContext) {
        let (owner, visual) = cx.add_window_view(|_, _| Probe(0));
        let weak = owner.downgrade();
        let mut async_cx = visual.update(|window, cx| window.to_async(cx));
        visual.update(|window, _| window.remove_window());
        assert!(!update(&weak, &mut async_cx, |_, _, _| panic!(
            "closed window polled"
        )));
    }
}
