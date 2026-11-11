use crate::components::*;
use gpui_kit::{
    base::{Dialog, DialogPopup, ElementExt, TextSelection, TextSelectionScopeId},
    *,
};

/// Retains one focus scope across renders. Focus changes only when a blocking
/// dialog opens or closes; result notifications never enter this scope.
pub struct ModalFocus {
    handle: FocusHandle,
    previous: Option<WeakFocusHandle>,
    active: bool,
    scroll: ScrollHandle,
    background_scope: TextSelectionScopeId,
}

impl ModalFocus {
    pub fn new(cx: &mut App) -> Self {
        Self {
            handle: cx.focus_handle(),
            previous: None,
            active: false,
            scroll: ScrollHandle::new(),
            background_scope: TextSelectionScopeId::new(),
        }
    }

    pub fn handle(&self) -> &FocusHandle {
        &self.handle
    }

    pub fn scroll(&self) -> &ScrollHandle {
        &self.scroll
    }

    /// Background renderers are ineligible while a blocking app dialog is
    /// open. Keep the modal in the root's normal scope, so component root
    /// redraws cannot reset an ongoing drag-selection.
    pub fn background(&self, content: impl IntoElement) -> impl IntoElement {
        div()
            // A distinct semantic boundary also invalidates cached prepaint
            // registrations once per transition, not on every modal frame.
            .id(("modal-background", usize::from(self.active)))
            .size_full()
            .child(content)
            .text_selection_scope(if self.active {
                self.background_scope
            } else {
                TextSelectionScopeId::default()
            })
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
        // Drop stale page/drag selection on modal transitions only.
        TextSelection::clear(window, cx);
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
                        .child(content)
                        .text_selection_scope(TextSelectionScopeId::default()),
                ),
        )
}

pub fn modal_backdrop_opacity(dark: bool) -> f32 {
    if dark { 0.64 } else { 0.48 }
}

fn modal_backdrop() -> Rgba {
    let palette = crate::theme::palette();
    // Neutral dimming keeps the hue of the user's chosen surfaces intact.
    rgb(0).opacity(modal_backdrop_opacity(palette.dark))
}

// Framed content paints its own outline and rounded surface. A second opaque
// popup (even with no padding) leaks through cards with a larger corner radius.
fn framed_popup(max_width: Pixels) -> DialogPopup {
    DialogPopup::new()
        .max_w(max_width)
        .bg(rgba(0))
        .text_color(rgb(crate::theme::palette().text))
}

/// The frame owns padding and its body viewport; no second scroll container.
pub fn modal_frame_shell(
    content: impl IntoElement,
    focus: &ModalFocus,
    window: &Window,
    cx: &mut App,
) -> Dialog {
    modal_frame_shell_observed(content, focus, window, cx, |_, _, _| {})
}

/// Native backgrounds stay visible, leaving the real popup rectangle clear.
pub fn modal_frame_shell_observed(
    content: impl IntoElement,
    focus: &ModalFocus,
    window: &Window,
    cx: &mut App,
    observe: impl Fn(Bounds<Pixels>, &mut Window, &mut App) + 'static,
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
            framed_popup(window.viewport_size().width * 0.96).child(
                div()
                    .id("modal-native-occlusion")
                    .test_support()
                    .relative()
                    .child(content)
                    .child(
                        canvas(
                            move |bounds, window, cx| observe(bounds, window, cx),
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .text_selection_scope(TextSelectionScopeId::default()),
            ),
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
    fn native_modal_occlusion_observes_the_card_not_its_following_static_position(
        cx: &mut TestAppContext,
    ) {
        use gpui_kit::{Bounds, Pixels, WindowBounds, point, px, size};
        use std::{cell::RefCell, rc::Rc};
        struct ObservedModal {
            focus: ModalFocus,
            observed: Rc<RefCell<Option<Bounds<Pixels>>>>,
        }
        impl Render for ObservedModal {
            fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                self.focus.sync(true, window, cx);
                let observed = self.observed.clone();
                super::modal_frame_shell_observed(
                    crate::modal_frame::ModalFrame::new(
                        "通知裁剪",
                        div().h(px(160.)).child("说明"),
                    )
                    .footer(Button::new("occlusion-dismiss").label("关闭")),
                    &self.focus,
                    window,
                    cx,
                    move |bounds, _, _| *observed.borrow_mut() = Some(bounds),
                )
            }
        }
        cx.update(gpui_kit::init);
        let observed = Rc::new(RefCell::new(None));
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
                    cx.new(|cx| ObservedModal {
                        focus: ModalFocus::new(cx),
                        observed: observed.clone(),
                    })
                },
            )
            .unwrap()
        });
        cx.run_until_parked();
        for viewport in [size(px(1000.), px(700.)), size(px(640.), px(550.))] {
            cx.update_window(handle, |_, window, cx| {
                window.resize(viewport);
                window.bounds_changed(cx);
                view.update(cx, |_, cx| cx.notify());
                window.render_frame(cx);
                assert_eq!(
                    observed.borrow().as_ref().copied(),
                    Some(window.find("modal-native-occlusion").bounds()),
                    "native cutout must exactly track the visible card, including its origin"
                );
            })
            .unwrap();
            cx.run_until_parked();
        }
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

#[cfg(test)]
mod framed_surface_tests {
    use super::framed_popup;
    use gpui_kit::{StyleRefinement, Styled, px, rgba};
    #[test]
    fn framed_host_is_transparent_and_does_not_add_a_second_card_outline() {
        let mut popup = framed_popup(px(480.));
        let style = popup.style();
        let empty = StyleRefinement::default();
        assert_eq!(style.background, Some(rgba(0).into()));
        assert_eq!(style.border_widths, empty.border_widths);
        assert_eq!(style.corner_radii, empty.corner_radii);
        assert_eq!(style.padding, empty.padding);
        assert_eq!(style.box_shadow, empty.box_shadow);
    }
}
