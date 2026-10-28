//! Compact update affordance, sharing the application's button activation/focus behavior.
use crate::{controls::Button, theme::ResolvedTheme};
use gpui_kit::{
    App, ClickEvent, ElementId, IntoElement, RenderOnce, Styled, Window,
    assets::IconName,
    component::{
        Disableable, FocusableExt, Sizable,
        button::{ButtonCustomVariant, ButtonVariants},
    },
    px, rgb,
};

#[derive(IntoElement)]
pub struct AvailableUpdateButton {
    inner: Button,
    disabled: bool,
}

impl AvailableUpdateButton {
    pub fn new(id: impl Into<ElementId>, version: impl AsRef<str>) -> Self {
        Self {
            inner: Button::new(id)
                .small()
                .w_full()
                .justify_start()
                .rounded(px(crate::design::CONTROL_RADIUS))
                .focus_ring(true)
                .icon(IconName::ArrowUp)
                .label(format!("更新至 {}", version.as_ref())),
            disabled: false,
        }
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.inner = self.inner.on_click(handler);
        self
    }
}

impl Disableable for AvailableUpdateButton {
    fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self.inner = self.inner.disabled(disabled);
        self
    }
}

impl RenderOnce for AvailableUpdateButton {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = UpdateColors::resolve(crate::theme::palette());
        let variant = ButtonCustomVariant::new(cx)
            .color(rgb(colors.normal).into())
            .foreground(rgb(colors.text).into())
            .hover(rgb(colors.hover).into())
            .active(rgb(colors.pressed).into());
        let button = self.inner.custom(variant);
        if self.disabled {
            button
                .bg(rgb(colors.disabled))
                .text_color(rgb(colors.disabled_text))
        } else {
            button
        }
    }
}

struct UpdateColors {
    normal: u32,
    hover: u32,
    pressed: u32,
    text: u32,
    disabled: u32,
    disabled_text: u32,
}

impl UpdateColors {
    fn resolve(theme: ResolvedTheme) -> Self {
        let tint = |weight: f32| {
            let channel = |shift: u32| {
                let base = ((theme.accent_soft >> shift) & 255u32) as f32;
                let accent = ((theme.accent >> shift) & 255u32) as f32;
                ((base * (1. - weight) + accent * weight).round() as u32) << shift
            };
            channel(16) | channel(8) | channel(0)
        };
        let normal = theme.accent_soft;
        let hover = tint(0.10);
        let pressed = tint(0.20);
        Self {
            normal,
            hover,
            pressed,
            text: studyapp_core::readable_theme_color(theme.accent, &[normal, hover, pressed]),
            disabled: theme.inset,
            disabled_text: theme.muted,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studyapp_core::{AppearanceSettings, ThemeMode, theme_contrast};

    #[::core::prelude::v1::test]
    fn update_available_states_remain_distinct_and_readable_in_light_and_dark_themes() {
        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            let theme = crate::theme::resolve(
                &AppearanceSettings {
                    mode,
                    ..Default::default()
                },
                false,
            );
            let colors = UpdateColors::resolve(theme);
            assert_ne!(colors.normal, colors.hover);
            assert_ne!(colors.hover, colors.pressed);
            for background in [colors.normal, colors.hover, colors.pressed] {
                assert!(theme_contrast(colors.text, background) >= 4.5);
            }
            assert!(theme_contrast(colors.disabled_text, colors.disabled) >= 4.5);
        }
    }

    #[cfg(feature = "ui-test")]
    #[gpui_kit::test]
    fn update_available_preserves_keyboard_mouse_and_disabled_semantics(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::{test::TestWindowExt, *};
        use std::{cell::Cell, rc::Rc};
        struct Fixture(Rc<Cell<usize>>, Rc<Cell<usize>>, f32);
        impl Render for Fixture {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let clicks = self.0.clone();
                let disabled_clicks = self.1.clone();
                div()
                    .tab_group()
                    .flex()
                    .flex_col()
                    .w(px(self.2))
                    .child(
                        AvailableUpdateButton::new("available", "0.8.0")
                            .on_click(move |_, _, _| clicks.set(clicks.get() + 1)),
                    )
                    .child(
                        AvailableUpdateButton::new("disabled", "0.8.0")
                            .disabled(true)
                            .on_click(move |_, _, _| {
                                disabled_clicks.set(disabled_clicks.get() + 1)
                            }),
                    )
            }
        }
        cx.update(gpui_kit::init);
        let clicks = Rc::new(Cell::new(0));
        let disabled_clicks = Rc::new(Cell::new(0));
        let (view, cx) = cx.add_window_view({
            let clicks = clicks.clone();
            let disabled_clicks = disabled_clicks.clone();
            move |_, _| Fixture(clicks, disabled_clicks, 180.)
        });
        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            for font_size in [12, 22] {
                for width in [160., 240.] {
                    view.update(cx, |view, cx| {
                        view.2 = width;
                        cx.notify();
                    });
                    cx.update(|window, cx| {
                        crate::theme::apply(
                            &AppearanceSettings {
                                mode,
                                font_size,
                                ..Default::default()
                            },
                            false,
                            cx,
                        );
                        window.render_frame(cx);
                        let button = window.find("available");
                        assert_eq!(button.bounds().size.width, px(width));
                        assert!(
                            button.bounds().size.height
                                >= px((font_size as f32 * 1.25 + 8.).max(32.))
                        );
                        assert_eq!(button.label(), Some("更新至 0.8.0"));
                        // The current GPUI base button does not expose an
                        // aria-disabled flag. Verify its actual inertness.
                        window.click("disabled", cx);
                        assert_eq!(disabled_clicks.get(), 0);
                        assert!(window.focused(cx).is_none());
                        window.hover("available", cx);
                        assert_eq!(
                            window.find("available").bounds(),
                            button.bounds(),
                            "hover has no layout shift"
                        );
                    });
                }
            }
        }
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.click("available", cx);
            window.click("disabled", cx);
            window.focus_next(cx);
            window.render_frame(cx);
            assert_eq!(window.find("available").focused(), Some(true));
        });
        for key in ["enter", "space"] {
            let keystroke = Keystroke::parse(key).unwrap();
            cx.simulate_event(KeyDownEvent {
                keystroke: keystroke.clone(),
                is_held: false,
                prefer_character_input: false,
            });
            cx.simulate_event(KeyUpEvent { keystroke });
        }
        assert_eq!(clicks.get(), 3);
        assert_eq!(disabled_clicks.get(), 0);
    }
}
