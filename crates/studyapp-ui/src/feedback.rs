use crate::components::*;
use gpui_kit::{
    base::{Dialog, DialogPopup},
    *,
};

/// Retains one focus scope across renders. Focus changes only when a blocking
/// dialog opens or closes; result notifications never enter this scope.
pub struct ModalFocus {
    handle: FocusHandle,
    previous: Option<WeakFocusHandle>,
    active: bool,
    scroll: ScrollHandle,
}

impl ModalFocus {
    pub fn new(cx: &mut App) -> Self {
        Self {
            handle: cx.focus_handle(),
            previous: None,
            active: false,
            scroll: ScrollHandle::new(),
        }
    }

    pub fn handle(&self) -> &FocusHandle {
        &self.handle
    }

    pub fn scroll(&self) -> &ScrollHandle {
        &self.scroll
    }

    #[cfg(all(test, feature = "ui-test"))]
    pub fn sync(&mut self, active: bool, window: &mut Window, cx: &mut App) {
        self.sync_with_initial(active, None, window, cx);
    }

    pub fn sync_with_initial(
        &mut self,
        active: bool,
        initial: Option<FocusHandle>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if active == self.active {
            // A menu or asynchronous input callback may restore an older
            // focus target after the dialog opens. Reclaim only escaped focus,
            // keeping every input and action already inside the dialog intact.
            if active && !self.handle.contains_focused(window, cx) {
                initial.as_ref().unwrap_or(&self.handle).focus(window, cx);
            }
            return;
        }
        self.active = active;
        if active {
            self.previous = window.focused(cx).map(|handle| handle.downgrade());
            initial.as_ref().unwrap_or(&self.handle).focus(window, cx);
        } else if let Some(previous) = self.previous.take().and_then(|handle| handle.upgrade()) {
            previous.focus(window, cx);
        }
    }
}

/// Uses the base dialog that is hosted by gpui-kit::open_window's Base Root.
/// Its focus trap and occluding popup provide keyboard and pointer containment.
pub fn modal_shell(
    content: impl IntoElement,
    focus: &ModalFocus,
    window: &Window,
    cx: &mut App,
) -> Dialog {
    let viewport = window.viewport_size();
    Dialog::new(cx)
        .focus_handle(focus.handle().clone())
        .close_on_backdrop_press(false)
        .backdrop(
            div()
                .size_full()
                .occlude()
                .bg(modal_backdrop())
                .on_scroll_wheel(|_, _, cx| cx.stop_propagation()),
        )
        .popup(
            DialogPopup::new()
                .w(px(560.).min((viewport.width - px(48.)).max(px(240.))))
                .p_5()
                .rounded_xl()
                .shadow_lg()
                .border_1()
                .border_color(crate::theme::color(BORDER))
                .bg(crate::theme::color(0xFFFFFF))
                .text_color(crate::theme::color(INK))
                .child(
                    scroll_body("modal-content-scroll", &focus.scroll)
                        .test_support()
                        .max_h((viewport.height - px(112.)).max(px(120.)))
                        .overflow_y_scroll()
                        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                        .child(content),
                ),
        )
}

fn modal_backdrop() -> Rgba {
    let palette = crate::theme::palette();
    // Neutral dimming keeps the hue of the user's chosen surfaces intact.
    rgb(0).opacity(if palette.dark { 0.64 } else { 0.48 })
}

/// The frame owns padding and its body viewport; no second scroll container.
pub fn modal_frame_shell(
    content: impl IntoElement,
    focus: &ModalFocus,
    window: &Window,
    cx: &mut App,
) -> Dialog {
    Dialog::new(cx)
        .focus_handle(focus.handle().clone())
        .close_on_backdrop_press(false)
        .backdrop(
            div()
                .size_full()
                .occlude()
                .bg(modal_backdrop())
                .on_scroll_wheel(|_, _, cx| cx.stop_propagation()),
        )
        .popup(
            DialogPopup::new()
                .max_w(window.viewport_size().width * 0.9)
                .rounded(px(16.))
                .shadow_lg()
                .border_1()
                .border_color(rgb(crate::theme::palette().border))
                .bg(rgb(crate::theme::palette().surface_raised))
                .text_color(rgb(crate::theme::palette().text))
                .child(content),
        )
}

#[cfg(all(test, feature = "ui-test"))]
mod tests {
    use super::ModalFocus;
    use crate::components::row;
    use crate::controls::Button;
    use gpui_kit::{
        AppContext, Context, FocusHandle, InteractiveElement, IntoElement, ParentElement, Render,
        Styled, TestAppContext, Window, WindowOptions, base::Root, div, test::TestWindowExt,
    };

    struct ModalFixture {
        focus: ModalFocus,
        open: bool,
        background_clicks: usize,
        outside: FocusHandle,
    }

    #[gpui_kit::test]
    fn long_modal_keeps_footer_visible_outside_the_body_scroll(cx: &mut TestAppContext) {
        use gpui_kit::{Bounds, ScrollDelta, WindowBounds, point, px, size};
        struct LongModal {
            focus: ModalFocus,
        }
        impl Render for LongModal {
            fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                self.focus.sync(true, window, cx);
                super::modal_frame_shell(
                    crate::modal_frame::ModalFrame::new(
                        "长内容确认",
                        div().h(px(900.)).flex_shrink_0().child("长说明"),
                    )
                    .scroll_handle(self.focus.scroll())
                    .footer(Button::new("long-modal-cancel").label("取消")),
                    &self.focus,
                    window,
                    cx,
                )
            }
        }
        cx.update(gpui_kit::init);
        let (handle, view) = cx.update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(1000.), px(700.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |_, cx| {
                    cx.new(|cx| LongModal {
                        focus: ModalFocus::new(cx),
                    })
                },
            )
            .unwrap()
        });
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            let footer = window.find("modal-footer").bounds();
            assert!(footer.bottom() <= window.viewport_size().height);
            assert!(footer.origin.y >= window.find("modal-content-scroll").bounds().bottom());
            window.scroll(
                "modal-content-scroll",
                ScrollDelta::Pixels(point(px(0.), px(-300.))),
                cx,
            );
            assert_eq!(window.find("modal-footer").bounds(), footer);
            assert!(view.read(cx).focus.scroll().offset().y < px(0.));
        })
        .unwrap();
    }

    impl Render for ModalFixture {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.focus.sync(self.open, window, cx);
            let cancel = cx.weak_entity();
            div()
                .size_full()
                .child(
                    div().track_focus(&self.outside).child(
                        Button::new("background-action")
                            .label("Background")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.background_clicks += 1;
                                cx.notify();
                            })),
                    ),
                )
                .children(self.open.then(|| {
                    super::modal_frame_shell(
                        crate::modal_frame::ModalFrame::new("确认操作", div().child("操作说明"))
                            .footer(
                                row()
                                    .child(Button::new("modal-first").label("First"))
                                    .child(Button::new("modal-last").label("Last")),
                            ),
                        &self.focus,
                        window,
                        cx,
                    )
                    .on_cancel(move |_, _, cx| {
                        let _ = cancel.update(cx, |this, cx| {
                            this.open = false;
                            cx.notify();
                        });
                        true
                    })
                }))
        }
    }

    #[gpui_kit::test]
    fn dialog_contains_tab_focus_blocks_background_and_restores_focus_on_escape(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (handle, view) = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
                cx.new(|cx| ModalFixture {
                    focus: ModalFocus::new(cx),
                    open: false,
                    background_clicks: 0,
                    outside: cx.focus_handle(),
                })
            })
            .unwrap()
        });
        let handle = handle.downcast::<Root>().unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("background-action", cx);
            // Component buttons intentionally prevent focus on mouse down.
            // Establish a real prior focus target instead of assuming a click
            // focused the button, then verify that exact target is restored.
            let outside = view.read(cx).outside.clone();
            outside.focus(window, cx);
        })
        .unwrap();
        cx.run_until_parked();
        view.update(cx, |this, cx| {
            this.open = true;
            cx.notify();
        });
        cx.run_until_parked();
        for (index, key) in ["tab", "tab", "tab", "shift-tab", "shift-tab", "shift-tab"]
            .into_iter()
            .enumerate()
        {
            cx.update_window(handle.into(), |_, window, cx| {
                window.press(key, cx);
                window.render_frame(cx);
                assert!(
                    view.read(cx).focus.handle().contains_focused(window, cx),
                    "focus escaped after {key}"
                );
                if index == 0 {
                    assert_eq!(
                        window.find("modal-first").focused(),
                        Some(true),
                        "the first Tab stop must be the cancel action"
                    );
                }
            })
            .unwrap();
            cx.run_until_parked();
        }
        cx.update_window(handle.into(), |_, window, cx| {
            let outside = view.read(cx).outside.clone();
            outside.focus(window, cx);
            window.render_frame(cx);
            assert!(
                view.read(cx).focus.handle().contains_focused(window, cx),
                "deferred menu focus restoration escaped the active dialog"
            );
        })
        .unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("background-action", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(
            cx.read(|cx| view.read(cx).background_clicks),
            1,
            "backdrop allowed background activation"
        );
        cx.update_window(handle.into(), |_, window, cx| {
            window.press("escape", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(!view.read(cx).open);
            assert!(view.read(cx).outside.is_focused(window));
        })
        .unwrap();
        view.update(cx, |this, cx| {
            this.open = true;
            cx.notify();
        });
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("close", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(!view.read(cx).open);
            assert!(view.read(cx).outside.is_focused(window));
        })
        .unwrap();
    }
}
