//! Window-scoped, one-shot field navigation; never edits or focuses an input.
use gpui_kit::*;
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
#[derive(Clone)]
pub enum SettingScroll {
    Normal(ScrollHandle),
    Virtual(ListState),
}
struct Request {
    label: String,
    scroll: SettingScroll,
    created: Instant,
}
#[derive(Default)]
struct Requests(HashMap<WindowId, Request>);
impl Global for Requests {}
pub fn seek(label: &str, scroll: SettingScroll, window: &Window, cx: &mut App) {
    let requests = &mut cx.default_global::<Requests>().0;
    requests.retain(|_, value| value.created.elapsed() < Duration::from_secs(3));
    requests.insert(
        window.window_handle().window_id(),
        Request {
            label: label.into(),
            scroll,
            created: Instant::now(),
        },
    );
}
pub fn probe(label: impl Into<SharedString>) -> impl IntoElement {
    let label = label.into();
    canvas(
        move |bounds, window, cx| {
            let id = window.window_handle().window_id();
            let requests = &mut cx.default_global::<Requests>().0;
            if !requests.get(&id).is_some_and(|request| {
                request.label == label.as_ref()
                    && request.created.elapsed() < Duration::from_secs(3)
            }) {
                return;
            }
            let Some(request) = requests.remove(&id) else {
                return;
            };
            window.on_next_frame(move |window, _| {
                match request.scroll {
                    SettingScroll::Normal(scroll) => {
                        let delta = bounds.top() - scroll.bounds().top() - px(8.);
                        let offset = scroll.offset();
                        scroll.set_offset(point(offset.x, offset.y - delta));
                    }
                    SettingScroll::Virtual(scroll) => {
                        let delta = bounds.top() - scroll.viewport_bounds().top() - px(8.);
                        scroll.scroll_by(delta);
                    }
                }
                window.refresh();
            });
        },
        |_, _, _, _| {},
    )
    .absolute()
    .inset_0()
}

#[cfg(all(test, feature = "ui-test"))]
mod tests {
    use super::{Requests, SettingScroll, seek};
    use gpui_kit::component::input::InputState;
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{
        AppContext, Context, Entity, Focusable, InteractiveElement, IntoElement, ParentElement,
        Render, ScrollHandle, Styled, TestAppContext, TestSupportExt, VisualTestContext, Window,
        div, point, px,
    };
    struct Fixture {
        scroll: ScrollHandle,
        inputs: Vec<Entity<InputState>>,
        width: f32,
    }
    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            crate::components::scroll_body("anchor-fixture", &self.scroll)
                .w(px(self.width))
                .h(px(180.))
                .child(crate::components::column().flex_shrink_0().children(
                    self.inputs.iter().enumerate().map(|(index, input)| {
                        crate::settings_components::settings_row(
                            format!("field-{index}"),
                            div()
                                .id(("anchor-value", index))
                                .test_support()
                                .h(px(42.))
                                .child(crate::settings_components::input(input, false)),
                        )
                    }),
                ))
        }
    }

    fn deliver_frame(visual: &mut VisualTestContext) {
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
            // Drawing and parking tasks do not deliver platform animation frames.
            // Run the callback that seek's probe schedules after its layout pass.
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        });
        visual.run_until_parked();
    }

    #[gpui_kit::test]
    fn locating_a_field_scrolls_without_focusing_or_changing_values(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, visual) = cx.add_window_view(|window, cx| Fixture {
            scroll: ScrollHandle::new(),
            inputs: (0..20)
                .map(|index| {
                    cx.new(|cx| {
                        InputState::new(window, cx).default_value(format!("unchanged-{index}"))
                    })
                })
                .collect(),
            width: 600.,
        });
        for width in [600., 340.] {
            view.update(visual, |view, cx| {
                view.width = width;
                view.scroll.set_offset(point(px(0.), px(0.)));
                cx.notify();
            });
            deliver_frame(visual);
            let focus = visual.update(|window, cx| {
                let fixture = view.read(cx);
                let focus = fixture.inputs[0].read(cx).focus_handle(cx);
                assert!(
                    fixture.scroll.max_offset().y > px(500.),
                    "fixture must contain genuinely overflowing settings rows"
                );
                let scroll = fixture.scroll.clone();
                focus.focus(window, cx);
                seek("field-17", SettingScroll::Normal(scroll), window, cx);
                window.refresh();
                focus
            });
            deliver_frame(visual);
            visual.update(|window, cx| {
                let fixture = view.read(cx);
                assert!(fixture.scroll.offset().y < px(-500.));
                let target = window.find(("anchor-value", 17_usize)).bounds();
                assert!(target.top() >= fixture.scroll.bounds().top());
                assert!(target.bottom() <= fixture.scroll.bounds().bottom());
                assert_eq!(window.focused(cx), Some(focus.clone()));
                let scroll = fixture.scroll.clone();
                assert!(cx.default_global::<Requests>().0.is_empty());
                // A second seek is independent and brings an earlier field back.
                seek("field-1", SettingScroll::Normal(scroll), window, cx);
                window.refresh();
            });
            deliver_frame(visual);
            visual.update(|window, cx| {
                let fixture = view.read(cx);
                assert!(fixture.scroll.offset().y > px(-300.));
                let target = window.find(("anchor-value", 1_usize)).bounds();
                assert!(target.top() >= fixture.scroll.bounds().top());
                assert!(target.bottom() <= fixture.scroll.bounds().bottom());
                assert_eq!(window.focused(cx), Some(focus.clone()));
                for (index, input) in fixture.inputs.iter().enumerate() {
                    assert_eq!(
                        input.read(cx).value().as_str(),
                        format!("unchanged-{index}")
                    );
                }
                // Consumed requests must not reapply after manual scrolling.
                fixture.scroll.set_offset(point(px(0.), px(0.)));
                window.refresh();
            });
            deliver_frame(visual);
            visual.update(|_, cx| assert_eq!(view.read(cx).scroll.offset().y, px(0.)));
        }
    }
}
