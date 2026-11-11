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
    use super::{SettingScroll, seek};
    use gpui_kit::{
        Context, InteractiveElement, IntoElement, ParentElement, Render, ScrollHandle, Styled,
        TestAppContext, TestSupportExt, Window, div, px,
    };
    struct Fixture {
        scroll: ScrollHandle,
        width: f32,
    }
    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            crate::components::scroll_body("anchor-fixture", &self.scroll)
                .w(px(self.width))
                .h(px(180.))
                .child(
                    crate::components::column().children((0_usize..20).map(|index| {
                        crate::settings_components::settings_row(
                            format!("field-{index}"),
                            div()
                                .id(("anchor-value", index))
                                .test_support()
                                .h(px(42.))
                                .child("unchanged"),
                        )
                    })),
                )
        }
    }
    #[gpui_kit::test]
    fn locating_a_field_scrolls_without_focusing_or_changing_values(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, visual) = cx.add_window_view(|_, _| Fixture {
            scroll: ScrollHandle::new(),
            width: 600.,
        });
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        visual.update(|window, cx| {
            let scroll = view.read(cx).scroll.clone();
            seek("field-17", SettingScroll::Normal(scroll), window, cx);
            window.refresh();
        });
        for _ in 0..3 {
            visual.run_until_parked();
            visual.update(|window, cx| {
                window.draw(cx).clear(cx);
            });
        }
        visual.update(|_, cx| assert!(view.read(cx).scroll.offset().y < px(-500.)));
        // A second seek is independent and brings an earlier field back.
        visual.update(|window, cx| {
            seek(
                "field-1",
                SettingScroll::Normal(view.read(cx).scroll.clone()),
                window,
                cx,
            );
            window.refresh();
        });
        for _ in 0..3 {
            visual.run_until_parked();
            visual.update(|window, cx| {
                window.draw(cx).clear(cx);
            });
        }
        visual.update(|_, cx| assert!(view.read(cx).scroll.offset().y > px(-300.)));
    }
}
