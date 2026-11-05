//! Compact update affordance, sharing the application's button activation/focus behavior.
use crate::{controls::Button, theme::ResolvedTheme};
use gpui_kit::{
    App, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    SharedString, Styled, TestSupportExt, Window,
    assets::IconName,
    component::{
        Disableable, FocusableExt, Sizable,
        button::{ButtonCustomVariant, ButtonVariants},
    },
    div, px, rgb,
};

/// A readable version button with a supplementary, never color-only unread dot.
pub fn version_entry(
    id: &'static str,
    current: &str,
    available: Option<&str>,
    unread: bool,
) -> Button {
    let description = match available {
        Some(version) if unread => {
            format!("当前版本 v{current}；新版本 v{version} 有未读更新内容，点击查看")
        }
        Some(version) => format!("当前版本 v{current}；可更新至 v{version}，点击查看"),
        None => format!("当前版本 v{current}；查看应用信息"),
    };
    Button::new(id)
        .small()
        .ghost()
        .label(format!("v{current}"))
        .accessibility_label(description.clone())
        .tooltip(description)
        .children((unread && available.is_some()).then(|| {
            div()
                .id(SharedString::from(format!("{id}-unread")))
                .test_support()
                .size(px(7.))
                .flex_shrink_0()
                .rounded_full()
                .bg(rgb(crate::theme::palette().danger))
        }))
}

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
    fn version_entry_read_dot_does_not_replace_installed_version_or_click_semantics(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::{test::TestWindowExt, *};
        use std::{cell::Cell, rc::Rc};
        struct Fixture(bool, Rc<Cell<usize>>);
        impl Render for Fixture {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let count = self.1.clone();
                version_entry("version", "0.2.5", Some("0.3.0"), self.0)
                    .on_click(move |_, _, _| count.set(count.get() + 1))
            }
        }
        cx.update(gpui_kit::init);
        let count = Rc::new(Cell::new(0));
        let (view, visual) = cx.add_window_view({
            let count = count.clone();
            move |_, _| Fixture(true, count)
        });
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("version-unread").is_some());
            assert_eq!(
                window.find("version").label(),
                Some("当前版本 v0.2.5；新版本 v0.3.0 有未读更新内容，点击查看")
            );
            window.click("version", cx);
        });
        assert_eq!(count.get(), 1);
        view.update(visual, |view, cx| {
            view.0 = false;
            cx.notify();
        });
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("version-unread").is_none());
            assert_eq!(
                window.find("version").label(),
                Some("当前版本 v0.2.5；可更新至 v0.3.0，点击查看")
            );
            window.click("version", cx);
        });
        assert_eq!(count.get(), 2);
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
