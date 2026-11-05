//! Poll a known owning window, not the entity's last inferred render window.
use gpui_kit::{AnyWindowHandle, AsyncApp, AsyncWindowContext, Context, WeakEntity, Window};

/// False means the owner/window is gone. A native nested event may temporarily
/// borrow the App; defer that tick rather than permanently stopping the loop.
pub(crate) fn update<T: 'static>(
    owner: &WeakEntity<T>,
    cx: &mut AsyncWindowContext,
    poll: impl FnOnce(&mut T, &mut Window, &mut Context<T>),
) -> bool {
    keep_polling(
        owner,
        cx.update(|window, cx| owner.update(cx, |owner, cx| poll(owner, window, cx))),
    )
}

/// The same owning-window boundary for tasks created from an entity observer.
pub(crate) fn update_in_app<T: 'static>(
    owner: &WeakEntity<T>,
    window: AnyWindowHandle,
    cx: &mut AsyncApp,
    poll: impl FnOnce(&mut T, &mut Window, &mut Context<T>),
) -> bool {
    keep_polling(
        owner,
        window.update(cx, |_, window, cx| {
            owner.update(cx, |owner, cx| poll(owner, window, cx))
        }),
    )
}

fn keep_polling<T: 'static>(
    owner: &WeakEntity<T>,
    result: anyhow::Result<anyhow::Result<()>>,
) -> bool {
    match result {
        Ok(result) => result.is_ok(),
        Err(error) => {
            let deferred = error.is::<std::cell::BorrowMutError>() && owner.upgrade().is_some();
            #[cfg(feature = "ui-test")]
            if !deferred {
                eprintln!(
                    "native owning-window poll stopped: owner={}, alive={}, reason={error:#}",
                    std::any::type_name::<T>(),
                    owner.upgrade().is_some()
                );
            }
            deferred
        }
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
    fn pending_pin_acknowledgement_survives_a_temporary_app_borrow(cx: &mut TestAppContext) {
        use crate::timer_widget_pin::{POLL_INTERVAL, PinController, PinUpdate};
        let (owner, visual) = cx.add_window_view(|_, _| Probe(0));
        let weak = owner.downgrade();
        let (handle, mut async_cx) =
            visual.update(|window, cx| (window.window_handle(), cx.to_async()));
        let now = std::time::Instant::now();
        let mut pin = PinController::default();
        let mut requests = 0;
        assert!(matches!(
            pin.step(true, 0, now, |request| {
                requests += usize::from(request);
                Ok(false)
            }),
            PinUpdate::Pending
        ));
        let mut pending = true;
        visual.update(|_, _| {
            assert!(
                update_in_app(&weak, handle, &mut async_cx, |_, _, _| {
                    panic!("a borrowed App must defer this pin poll");
                }),
                "a temporary App borrow must not stop the pin task"
            );
        });
        assert!(pin.pending());
        assert!(update_in_app(
            &weak,
            handle,
            &mut async_cx,
            |owner, _, _| {
                assert!(matches!(
                    pin.step(true, 0, now + POLL_INTERVAL, |request| {
                        requests += usize::from(request);
                        Ok(true)
                    }),
                    PinUpdate::Applied
                ));
                pending = pin.pending();
                owner.0 += 1;
            }
        ));
        assert!(!pending && pin.applied());
        assert_eq!(
            requests, 1,
            "resuming must not send the native request twice"
        );
        assert_eq!(owner.read_with(visual, |owner, _| owner.0), 1);
        visual.update(|window, _| window.remove_window());
        assert!(!update_in_app(&weak, handle, &mut async_cx, |_, _, _| {
            panic!("a closed owning window must not be polled");
        }));
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
