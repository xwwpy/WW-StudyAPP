//! Shared presentation for the inline timer, desktop window and settings preview.
use crate::{
    components::*,
    controls::Button,
    design::{MONO_FONT, TypeRole},
    theme::ResolvedTheme,
};
use gpui_kit::{
    component::{Disableable, Icon, Selectable, Sizable, button::ButtonVariants, tooltip::Tooltip},
    prelude::FluentBuilder,
    *,
};
use std::{cell::Cell, rc::Rc};
use studyapp_core::{TimerKind, TimerState, TimerWidgetSettings, WidgetBackground, WidgetLayout};

pub const PIN_SUPPORTED: bool = cfg!(any(windows, target_os = "macos", target_os = "linux"));

#[derive(Clone, Debug)]
pub struct TimerCardModel {
    pub kind: TimerKind,
    pub label: String,
    pub elapsed: u64,
    pub target: u64,
    pub remaining: u64,
    pub ratio: f32,
    pub paused: bool,
    pub finished: bool,
    pub segment: usize,
}
impl TimerCardModel {
    pub fn from_timer(timer: &TimerState, segment: usize) -> Self {
        Self {
            kind: timer.kind,
            label: timer.label.clone(),
            elapsed: timer.active_seconds,
            target: timer.target_seconds,
            remaining: timer.target_seconds.saturating_sub(timer.active_seconds),
            ratio: if timer.target_seconds == 0 {
                0.
            } else {
                (timer.active_seconds as f32 / timer.target_seconds as f32).clamp(0., 1.)
            },
            paused: timer.paused,
            finished: timer.finished_at.is_some(),
            segment,
        }
    }
}
#[derive(Clone, Debug)]
pub struct TimerCardStyle {
    pub palette: ResolvedTheme,
    pub settings: TimerWidgetSettings,
    pub surface: u32,
    pub accent: Hsla,
    pub pin_applied: Option<bool>,
    pub pin_pending: bool,
    pub pin_failed: bool,
}
impl TimerCardStyle {
    pub fn from_widget_settings(settings: &TimerWidgetSettings, palette: ResolvedTheme) -> Self {
        let surface = match settings.background {
            WidgetBackground::Inset => palette.inset,
            _ => palette.surface,
        };
        let accent = settings
            .accent
            .as_deref()
            .and_then(studyapp_core::parse_rgba_color)
            .map(|color| Hsla::from(rgba(color.packed_rgba())))
            .unwrap_or_else(|| rgb(palette.accent).into());
        Self {
            palette,
            settings: settings.clone(),
            surface,
            accent,
            pin_applied: None,
            pin_pending: false,
            pin_failed: false,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimerCardVariant {
    Inline,
    Dashboard { desktop_open: bool },
    Widget,
    Preview,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimerCardAction {
    Toggle,
    Finish,
    Reset,
    Record,
    ChooseKind(TimerKind),
    Countdown(u64),
    Configure,
    ShowOnDesktop,
    OpenSettings,
    OpenMain,
    TogglePinned,
    RetryPinned,
    CloseWidget,
    ExitApp,
}
fn clock(seconds: u64, show_seconds: bool) -> String {
    if show_seconds {
        if seconds >= 3600 {
            format!(
                "{}:{:02}:{:02}",
                seconds / 3600,
                seconds / 60 % 60,
                seconds % 60
            )
        } else {
            format!("{:02}:{:02}", seconds / 60, seconds % 60)
        }
    } else {
        format!("{} 分钟", seconds.div_ceil(60))
    }
}

pub fn timer_card(
    model: TimerCardModel,
    style: TimerCardStyle,
    variant: TimerCardVariant,
    on_action: impl Fn(TimerCardAction, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    render_card(model, style, variant, on_action, None)
}

/// Reports natural client height, including the measured padding and tools.
/// The host may defer a resize; settings previews never resize their window.
pub fn timer_widget_card(
    model: TimerCardModel,
    style: TimerCardStyle,
    on_action: impl Fn(TimerCardAction, &mut Window, &mut App) + 'static,
    on_content_height: impl Fn(Pixels, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    render_card(
        model,
        style,
        TimerCardVariant::Widget,
        on_action,
        Some(Rc::new(on_content_height)),
    )
}

type HeightListener = Rc<dyn Fn(Pixels, &mut Window, &mut App)>;

fn widget_control(
    id: &str,
    label: &str,
    icon: Option<gpui_kit::assets::IconName>,
    zoom: f32,
) -> Button {
    let mut button = Button::new(SharedString::from(id.to_owned()))
        .small()
        .ghost()
        .target_scale(zoom)
        .rounded(px(6. * zoom))
        .h(px(32. * zoom))
        .min_w(px(32. * zoom))
        .px(px(if icon.is_some() { 0. } else { 8. * zoom }))
        .accessibility_label(label.to_owned())
        .tooltip(label.to_owned());
    button = if let Some(icon) = icon {
        button.child(Icon::new(icon).with_size(px(14. * zoom)))
    } else {
        button.child(div().text_size(px(14. * zoom)).child(label.to_owned()))
    };
    button
}

fn render_card(
    model: TimerCardModel,
    style: TimerCardStyle,
    variant: TimerCardVariant,
    on_action: impl Fn(TimerCardAction, &mut Window, &mut App) + 'static,
    on_content_height: Option<HeightListener>,
) -> impl IntoElement {
    let natural_height = Rc::new(Cell::new(None::<Pixels>));
    let action = Rc::new(on_action);
    let widget = matches!(
        variant,
        TimerCardVariant::Widget | TimerCardVariant::Preview
    );
    let dashboard = matches!(variant, TimerCardVariant::Dashboard { .. });
    let preview = variant == TimerCardVariant::Preview;
    let s = &style.settings;
    let zoom = if widget {
        f32::from(s.scale) / 100.
    } else {
        1.
    };
    let alpha = if widget && s.background != WidgetBackground::Solid {
        f32::from(s.opacity) / 100.
    } else {
        1.
    };
    let compact = widget && s.layout == WidgetLayout::Compact;
    let palette = style.palette;
    let status = if model.finished {
        "计时已结束"
    } else if model.paused {
        "计时已暂停"
    } else {
        "正在计时"
    };
    let accent = if model.paused && !model.finished {
        Hsla::from(rgb(palette.warning))
    } else {
        style.accent
    };
    let button_style = |button: Button| {
        button.when(widget, |button| {
            button
                .bg(rgba(0))
                .border_color(rgba(0))
                .transparent_surface(
                    rgb(palette.text).into(),
                    accent.opacity(accent.a * alpha * 0.12),
                    accent.opacity(accent.a * alpha * 0.2),
                )
        })
    };
    let button = |id: &str, label: &str, value: TimerCardAction| {
        let action = action.clone();
        let base = if widget {
            widget_control(id, label, crate::action_icons::action_icon(label), zoom)
        } else {
            Button::new(SharedString::from(id.to_owned()))
                .small()
                .ghost()
                .action(label.to_owned())
        };
        button_style(base)
            .disabled(preview)
            .on_click(move |_, window, cx| action(value, window, cx))
    };
    let value = if model.kind == TimerKind::CountUp {
        model.elapsed
    } else {
        model.remaining
    };
    let digits = || {
        typography(TypeRole::Display)
            .id(if widget {
                "widget-timer-value"
            } else {
                "today-timer-value"
            })
            .aria_label(format!(
                "{} {}",
                if model.kind == TimerKind::CountUp {
                    "已用"
                } else {
                    "剩余"
                },
                clock(value, !widget || s.show_seconds)
            ))
            .test_support()
            .font_family(MONO_FONT)
            .line_height(relative(1.2))
            .text_size(px((if compact || dashboard { 32. } else { 42. })
                * if widget {
                    f32::from(s.scale) / 100.
                } else {
                    1.
                }))
            .font_features(FontFeatures(std::sync::Arc::new(vec![("tnum".into(), 1)])))
            .text_color(rgb(palette.text))
            .child(clock(value, !widget || s.show_seconds))
    };
    let mut header = row().flex_wrap().gap(px(8. * zoom)).child(
        column()
            .flex_1()
            .min_w_0()
            .gap(px(4. * zoom))
            .child(
                heading(if model.label.is_empty() {
                    model.kind.label().to_owned()
                } else {
                    model.label.clone()
                })
                .text_size(crate::design::type_size(palette.font_size, TypeRole::Section) * zoom)
                .line_height(relative(crate::design::line_height(TypeRole::Section)))
                .truncate(),
            )
            .child(
                div()
                    .id(if widget {
                        "widget-timer-status"
                    } else {
                        "inline-timer-status"
                    })
                    .aria_label(status)
                    .test_support()
                    .tooltip(move |window, cx| Tooltip::new(status).build(window, cx))
                    .text_color(rgb(palette.muted))
                    .line_height(relative(crate::design::line_height(TypeRole::Caption)))
                    .text_size(
                        crate::design::type_size(palette.font_size, TypeRole::Caption) * zoom,
                    )
                    .flex()
                    .items_center()
                    .gap(px(4. * zoom))
                    .when(dashboard, |view| {
                        view.child(muted(format!("{} · {status}", model.kind.label())))
                    })
                    .child(
                        Icon::new(if model.finished {
                            gpui_kit::assets::IconName::CircleCheck
                        } else if model.paused {
                            gpui_kit::assets::IconName::Pause
                        } else {
                            gpui_kit::assets::IconName::Play
                        })
                        .with_size(px(14. * zoom)),
                    ),
            ),
    );
    if dashboard {
        header = header.child(digits());
    }
    if !widget {
        let desktop_open = matches!(variant, TimerCardVariant::Dashboard { desktop_open: true });
        header = header.child(
            button(
                "timer-show-desktop",
                if desktop_open {
                    "隐藏桌面小组件"
                } else {
                    "显示桌面小组件"
                },
                TimerCardAction::ShowOnDesktop,
            )
            .selected(desktop_open)
            .toggled(desktop_open),
        );
    }
    let root_id = match variant {
        TimerCardVariant::Inline | TimerCardVariant::Dashboard { .. } => "today-timer",
        TimerCardVariant::Widget => "timer-widget-card",
        TimerCardVariant::Preview => "timer-widget-preview",
    };
    let progress_id = if widget {
        "timer-widget-progress"
    } else {
        "today-timer-progress"
    };
    // Surface alpha never fades text, icons or progress indicators.
    let measured = natural_height.clone();
    let mut surface = card()
        .on_children_prepainted(move |children, window, cx| {
            if let (Some(listener), Some(natural), Some(viewport)) =
                (on_content_height.as_ref(), measured.get(), children.last())
            {
                // Replace only allocated body height with natural height.
                // Border, padding, gaps and tools come from actual layout.
                listener(
                    window.viewport_size().height - viewport.size.height + natural,
                    window,
                    cx,
                );
            }
        })
        .id(root_id)
        .test_support()
        .w_full()
        .min_w_0()
        .rounded(px(if widget {
            f32::from(s.corner_radius) * zoom
        } else {
            crate::design::CARD_RADIUS
        }))
        .bg(Hsla::from(rgb(style.surface)).opacity(alpha))
        .border_color(rgb(palette.border).opacity(alpha))
        .text_color(rgb(palette.text))
        .p(px((if compact { 12. } else { 16. }) * zoom))
        .gap(px(8. * zoom))
        .overflow_hidden()
        .when(variant == TimerCardVariant::Widget, |card| card.h_full());

    if widget {
        use gpui_kit::assets::IconName;
        let mut drag = div()
            .id(if preview {
                "timer-preview-drag"
            } else {
                "timer-widget-drag"
            })
            .test_support()
            .flex()
            .items_center()
            .flex_1()
            .min_w_0()
            .h(px(32. * zoom))
            .aria_label("拖动小组件")
            .tooltip(|window, cx| Tooltip::new("拖动小组件").build(window, cx))
            .child(Icon::new(IconName::GripVertical).with_size(px(14. * zoom)));
        if !preview {
            drag = drag
                .cursor_move()
                .window_control_area(WindowControlArea::Drag);
            if !cfg!(windows) {
                drag = drag
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move());
            }
        }
        let applied = style.pin_applied.unwrap_or(s.pinned);
        let pin_label = if style.pin_failed {
            if s.pinned {
                "重试置顶"
            } else {
                "重试取消置顶"
            }
        } else if style.pin_pending {
            if s.pinned {
                "正在置顶"
            } else {
                "正在取消置顶"
            }
        } else if applied {
            "取消置顶"
        } else {
            "置顶"
        };
        let pin_action = action.clone();
        let retry_pin = style.pin_failed;
        let pin = button_style(widget_control(
            "widget-pin",
            pin_label,
            Some(if applied {
                IconName::PinOff
            } else {
                IconName::Pin
            }),
            zoom,
        ))
        .selected(applied)
        .toggled(applied)
        .loading(style.pin_pending)
        .disabled(preview || style.pin_pending || !PIN_SUPPORTED)
        .on_click(move |_, window, cx| {
            pin_action(
                if retry_pin {
                    TimerCardAction::RetryPinned
                } else {
                    TimerCardAction::TogglePinned
                },
                window,
                cx,
            )
        });
        surface = surface.child(
            row()
                .id("widget-tools")
                .test_support()
                .flex_shrink_0()
                .gap(px(4. * zoom))
                .child(drag)
                .child(pin)
                .child(button(
                    "widget-open-main",
                    "主界面",
                    TimerCardAction::OpenMain,
                ))
                .child(button(
                    "widget-settings",
                    "外观",
                    TimerCardAction::OpenSettings,
                ))
                .child(button("widget-exit", "退出应用", TimerCardAction::ExitApp))
                .child(button("widget-close", "关闭", TimerCardAction::CloseWidget)),
        );
    }
    let mut card = column()
        .id(if widget {
            "timer-widget-content"
        } else {
            "timer-inline-body"
        })
        .test_support()
        .w_full()
        .min_w_0()
        .flex_shrink_0()
        .gap(px(8. * zoom))
        .child(header)
        .when(!dashboard, |card| card.child(digits()))
        .when(!compact && (!widget || s.show_segment), |c| {
            c.child(
                muted(if dashboard && model.kind != TimerKind::CountUp {
                    format!(
                        "{} · 已用 {} · 共 {}",
                        if model.kind == TimerKind::Study {
                            format!("第 {} 段", model.segment)
                        } else {
                            "倒计时".into()
                        },
                        clock(model.elapsed, true),
                        clock(model.target, true)
                    )
                } else if model.kind == TimerKind::Study {
                    format!(
                        "第 {} 段 · 已用 {}",
                        model.segment,
                        clock(model.elapsed, true)
                    )
                } else {
                    format!("已用 {} · 不会自动计入学习记录", clock(model.elapsed, true))
                })
                .id("timer-details")
                .test_support()
                .text_color(rgb(palette.muted))
                .text_size(crate::design::type_size(palette.font_size, TypeRole::Caption) * zoom)
                .line_height(relative(crate::design::line_height(TypeRole::Caption))),
            )
        });
    if model.kind != TimerKind::CountUp && (!widget || s.show_progress) {
        card = card.child(
            div()
                .id(progress_id)
                .test_support()
                .w_full()
                .h(px(6. * zoom))
                .rounded_full()
                .bg(accent.opacity(accent.a * 0.16))
                .child(
                    div()
                        .w(relative(model.ratio))
                        .h_full()
                        .rounded_full()
                        .bg(accent),
                ),
        );
    }
    if !widget || s.show_controls {
        let mut actions = row().flex_wrap().gap(px(4. * zoom)).child(button(
            if widget { "widget-pause" } else { "pause" },
            if model.finished {
                "重新开始"
            } else if model.paused {
                "继续计时"
            } else {
                "暂停"
            },
            if model.finished {
                TimerCardAction::Reset
            } else {
                TimerCardAction::Toggle
            },
        ));
        if model.kind != TimerKind::Study && !model.finished {
            actions = actions.child(button("timer-finish", "结束", TimerCardAction::Finish));
        }
        if !compact {
            actions = actions
                .child(button(
                    if widget {
                        "widget-timer-record"
                    } else {
                        "manual-top"
                    },
                    "记录这段",
                    TimerCardAction::Record,
                ))
                .child(button(
                    "timer-configure",
                    "设置计时",
                    TimerCardAction::Configure,
                ));
        }
        card = card.child(actions);
    }
    if !dashboard && (!widget || s.layout == WidgetLayout::Expanded) {
        let mut kinds = row().flex_wrap().gap(px(4. * zoom));
        for kind in [TimerKind::Study, TimerKind::CountUp, TimerKind::CountDown] {
            kinds = kinds.child(
                button(
                    &format!("timer-kind-{kind:?}"),
                    kind.label(),
                    TimerCardAction::ChooseKind(kind),
                )
                .selected(model.kind == kind)
                .when(widget && model.kind == kind, |button| {
                    button.text_color(accent).toggled(true)
                }),
            );
        }
        card = card.child(kinds);
        if model.kind == TimerKind::CountDown {
            let mut presets = row().flex_wrap().gap(px(4. * zoom));
            for &minutes in &s.countdown_presets {
                presets = presets.child(button(
                    &format!("timer-preset-{minutes}"),
                    &format!("{minutes} 分钟"),
                    TimerCardAction::Countdown(minutes),
                ));
            }
            card = card.child(presets);
        }
    }
    if !compact && !dashboard && model.kind == TimerKind::Study {
        card = card.child(
            muted(format!(
                "本段 {} 分钟 · 到时将邀请你确认收获",
                model.target / 60
            ))
            .text_size(crate::design::type_size(palette.font_size, TypeRole::Caption) * zoom)
            .line_height(relative(crate::design::line_height(TypeRole::Caption))),
        );
    }
    if variant == TimerCardVariant::Widget {
        surface
            .child(
                column()
                    .on_children_prepainted(move |children, _, _| {
                        natural_height.set(children.first().map(|bounds| bounds.size.height));
                    })
                    .id("timer-widget-scroll")
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_y_scroll()
                    .child(card),
            )
            .into_any_element()
    } else {
        surface.child(card).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn widget_accent_alpha_does_not_change_surface_or_foreground_opacity() {
        let palette = crate::theme::palette();
        let settings = TimerWidgetSettings {
            accent: Some("#FF000080".into()),
            ..Default::default()
        };
        let style = TimerCardStyle::from_widget_settings(&settings, palette);
        assert!((style.accent.a - 128. / 255.).abs() < 0.0001);
        assert_eq!(style.surface, palette.surface);
        assert_eq!(style.palette.text, palette.text);
        assert_eq!(style.settings.opacity, settings.opacity);
    }
    #[::core::prelude::v1::test]
    fn progress_and_remaining_are_bounded_and_countup_never_divides_by_zero() {
        for kind in [TimerKind::Study, TimerKind::CountUp, TimerKind::CountDown] {
            let t = TimerState {
                kind,
                target_seconds: if kind == TimerKind::CountUp { 0 } else { 300 },
                active_seconds: 250,
                ..Default::default()
            };
            let m = TimerCardModel::from_timer(&t, 2);
            assert!(m.ratio.is_finite() && (0. ..=1.).contains(&m.ratio));
            assert_eq!(m.remaining, if kind == TimerKind::CountUp { 0 } else { 50 });
        }
        assert_eq!(clock(3661, true), "1:01:01");
        assert_eq!(clock(90061, true), "25:01:01");
        assert_eq!(clock(u64::MAX, true), "5124095576030431:00:15");
        assert_eq!(clock(u64::MAX, false), "307445734561825861 分钟");
        assert_eq!(clock(61, false), "2 分钟");
    }
}

#[cfg(all(test, feature = "ui-test"))]
mod widget_geometry_tests {
    use super::{TimerCardModel, TimerCardStyle, TimerCardVariant, timer_card};
    use gpui_kit::{
        Context, IntoElement, ParentElement, Render, Styled, TestAppContext, Window, div, px, size,
        test::TestWindowExt,
    };
    use studyapp_core::{TimerState, TimerWidgetSettings, WidgetLayout};
    struct Fixture(TimerWidgetSettings);
    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(timer_card(
                TimerCardModel::from_timer(&TimerState::default(), 1),
                TimerCardStyle::from_widget_settings(&self.0, crate::theme::palette()),
                TimerCardVariant::Widget,
                |_, _, _| {},
            ))
        }
    }
    struct MeasuredFixture {
        settings: TimerWidgetSettings,
        height: std::rc::Rc<std::cell::Cell<gpui_kit::Pixels>>,
    }
    impl Render for MeasuredFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let height = self.height.clone();
            div().size_full().child(super::timer_widget_card(
                TimerCardModel::from_timer(&TimerState::default(), 1),
                TimerCardStyle::from_widget_settings(&self.settings, crate::theme::palette()),
                |_, _, _| {},
                move |measured, _, _| height.set(measured),
            ))
        }
    }

    #[gpui_kit::test]
    fn widget_natural_height_is_independent_of_old_window_height_and_scroll(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let measured = std::rc::Rc::new(std::cell::Cell::new(px(0.)));
        let (view, visual) = cx.add_window_view(|_, _| MeasuredFixture {
            settings: TimerWidgetSettings::default(),
            height: measured.clone(),
        });
        for (width, font) in [(280., 14), (280., 22), (480., 22)] {
            for layout in [
                WidgetLayout::Compact,
                WidgetLayout::Standard,
                WidgetLayout::Expanded,
            ] {
                view.update(visual, |view, cx| {
                    view.settings.layout = layout;
                    cx.notify();
                });
                visual.simulate_resize(size(px(width), px(900.)));
                visual.update(|window, cx| {
                    crate::theme::apply(
                        &studyapp_core::AppearanceSettings {
                            font_size: font,
                            ..Default::default()
                        },
                        false,
                        cx,
                    );
                    window.render_frame(cx);
                });
                let natural = measured.get();
                assert!(
                    natural > px(100.) && natural < px(850.),
                    "{layout:?}: {natural:?}"
                );
                let fitted = px(f32::from(natural).ceil().max(200.));
                visual.simulate_resize(size(px(width), fitted));
                visual.update(|window, cx| {
                    window.render_frame(cx);
                    assert!((measured.get() - natural).abs() < px(1.));
                    let content = window.find("timer-widget-content").bounds();
                    let viewport = window.find("timer-widget-scroll").bounds();
                    assert!(content.size.height <= viewport.size.height + px(1.));
                    assert!(viewport.size.height - content.size.height < px(70.));
                });
                visual.simulate_resize(size(px(width), px(200.)));
                visual.update(|window, cx| {
                    window.render_frame(cx);
                    assert!((measured.get() - natural).abs() < px(1.));
                    let before = window.find("timer-widget-content").bounds();
                    let viewport = window.find("timer-widget-scroll").bounds();
                    window.scroll(
                        "timer-widget-scroll",
                        gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-2000.))),
                        cx,
                    );
                    window.render_frame(cx);
                    assert!((measured.get() - natural).abs() < px(1.));
                    let after = window.find("timer-widget-content").bounds();
                    if before.size.height > viewport.size.height + px(1.) {
                        assert!(
                            after.top() < before.top(),
                            "short widget must really scroll"
                        );
                        assert!(after.bottom() <= viewport.bottom() + px(1.));
                    }
                });
            }
        }
    }

    #[gpui_kit::test]
    fn window_is_exactly_one_card_and_all_chrome_is_inside_it(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, visual) = cx.add_window_view(|_, _| Fixture(TimerWidgetSettings::default()));
        for (width, height, font) in [
            (280., 200., 14),
            (280., 200., 22),
            (380., 300., 14),
            (480., 360., 22),
        ] {
            visual.simulate_resize(size(px(width), px(height)));
            for layout in [
                WidgetLayout::Compact,
                WidgetLayout::Standard,
                WidgetLayout::Expanded,
            ] {
                view.update(visual, |view, cx| {
                    view.0.layout = layout;
                    cx.notify();
                });
                visual.update(|window, cx| {
                    crate::theme::apply(
                        &studyapp_core::AppearanceSettings {
                            font_size: font,
                            ..Default::default()
                        },
                        false,
                        cx,
                    );
                    window.render_frame(cx);
                    let card = window.find("timer-widget-card").bounds();
                    assert_eq!(card.origin, gpui_kit::point(px(0.), px(0.)));
                    assert_eq!(card.size, size(px(width), px(height)));
                    for id in [
                        "timer-widget-drag",
                        "widget-pin",
                        "widget-open-main",
                        "widget-settings",
                        "widget-close",
                        "widget-exit",
                    ] {
                        let bounds = window.find(id).bounds();
                        assert!(
                            bounds.size.width > px(0.) && bounds.size.height > px(0.),
                            "{id}: {bounds:?}"
                        );
                        assert!(
                            bounds.left() >= card.left() && bounds.right() <= card.right(),
                            "{id}: {bounds:?}"
                        );
                        assert!(
                            bounds.top() >= card.top() && bounds.bottom() <= card.bottom(),
                            "{id}: {bounds:?}"
                        );
                    }
                    assert!(
                        window.find("widget-tools").bounds().bottom()
                            <= window.find("timer-widget-scroll").bounds().top()
                    );
                });
            }
        }
    }

    struct PinFixture {
        style: TimerCardStyle,
        variant: TimerCardVariant,
        actions: std::rc::Rc<std::cell::RefCell<Vec<super::TimerCardAction>>>,
    }
    impl Render for PinFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let actions = self.actions.clone();
            div().size_full().child(timer_card(
                TimerCardModel::from_timer(&TimerState::default(), 1),
                self.style.clone(),
                self.variant,
                move |action, _, _| actions.borrow_mut().push(action),
            ))
        }
    }
    #[gpui_kit::test]
    fn pin_control_reports_acknowledged_state_and_explicit_retry_without_activating_preview(
        cx: &mut TestAppContext,
    ) {
        use super::TimerCardAction::{RetryPinned, TogglePinned};
        cx.update(gpui_kit::init);
        let actions = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let recorded = actions.clone();
        let (view, visual) = cx.add_window_view(move |_, _| PinFixture {
            style: TimerCardStyle::from_widget_settings(
                &Default::default(),
                crate::theme::palette(),
            ),
            variant: TimerCardVariant::Widget,
            actions: recorded,
        });
        // desired, applied, pending, failed, preview, accessible label, dispatched action
        for (desired, applied, pending, failed, preview, label, expected) in [
            (true, false, true, false, false, "正在置顶", None),
            (false, true, true, false, false, "正在取消置顶", None),
            (
                true,
                false,
                false,
                true,
                false,
                "重试置顶",
                Some(RetryPinned),
            ),
            (
                false,
                true,
                false,
                true,
                false,
                "重试取消置顶",
                Some(RetryPinned),
            ),
            (
                true,
                true,
                false,
                false,
                false,
                "取消置顶",
                Some(TogglePinned),
            ),
            (
                false,
                false,
                false,
                false,
                false,
                "置顶",
                Some(TogglePinned),
            ),
            (true, true, false, false, true, "取消置顶", None),
            (true, false, false, true, true, "重试置顶", None),
        ] {
            actions.borrow_mut().clear();
            view.update(visual, |fixture, cx| {
                fixture.style.settings.pinned = desired;
                fixture.style.pin_applied = Some(applied);
                fixture.style.pin_pending = pending;
                fixture.style.pin_failed = failed;
                fixture.variant = if preview {
                    TimerCardVariant::Preview
                } else {
                    TimerCardVariant::Widget
                };
                cx.notify();
            });
            visual.update(|window, cx| {
                window.render_frame(cx);
                let pin = window.find("widget-pin");
                assert_eq!(pin.label(), Some(label));
                assert_eq!(
                    pin.checked(),
                    Some(applied),
                    "never substitute saved intent for acknowledgement"
                );
                // The upstream button does not expose a disabled flag to the
                // observer. Prove blocking by dispatching a real virtual click
                // below and asserting that no action escapes pending/preview.
                window.click("widget-pin", cx);
            });
            assert_eq!(
                &*actions.borrow(),
                &expected.into_iter().collect::<Vec<_>>()
            );
        }
    }
}

#[cfg(all(test, feature = "ui-test"))]
mod transparent_widget_tests {
    use super::{TimerCardModel, TimerCardStyle, TimerCardVariant, timer_card};
    use gpui_kit::{
        Context, IntoElement, Render, TestAppContext, Window, px, size, test::TestWindowExt,
    };
    use studyapp_core::{TimerKind, TimerState, TimerWidgetSettings, WidgetLayout};
    struct Transparent;
    impl Render for Transparent {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            timer_card(
                TimerCardModel::from_timer(
                    &TimerState {
                        kind: TimerKind::CountUp,
                        ..Default::default()
                    },
                    1,
                ),
                TimerCardStyle::from_widget_settings(
                    &TimerWidgetSettings {
                        opacity: 0,
                        layout: WidgetLayout::Expanded,
                        ..Default::default()
                    },
                    crate::theme::palette(),
                ),
                TimerCardVariant::Widget,
                |_, _, _| {},
            )
        }
    }
    struct ZoomFixture(u8);
    impl Render for ZoomFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            timer_card(
                TimerCardModel::from_timer(&TimerState::default(), 1),
                TimerCardStyle::from_widget_settings(
                    &TimerWidgetSettings {
                        scale: self.0,
                        layout: WidgetLayout::Expanded,
                        ..Default::default()
                    },
                    crate::theme::palette(),
                ),
                TimerCardVariant::Widget,
                |_, _, _| {},
            )
        }
    }
    #[gpui_kit::test]
    fn custom_zoom_scales_digits_tool_targets_and_spacing_together(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, visual) = cx.add_window_view(|_, _| ZoomFixture(100));
        let mut baseline = None;
        for scale in [100, 50, 73, 140, 175, 200, 100] {
            let zoom = f32::from(scale) / 100.;
            view.update(visual, |view, cx| {
                view.0 = scale;
                cx.notify();
            });
            visual.simulate_resize(size(px(600. * zoom), px(900. * zoom)));
            visual.update(|window, cx| {
                window.render_frame(cx);
                let actual = [
                    "widget-timer-value",
                    "widget-tools",
                    "widget-pin",
                    "timer-kind-Study",
                ]
                .map(|id| window.find(id).bounds().size.height);
                if let Some(baseline) = baseline {
                    for (actual, expected) in actual.into_iter().zip(baseline) {
                        assert!(
                            (actual - expected * zoom).abs() < px(2.),
                            "scale={scale}: {actual:?} vs {expected:?}"
                        );
                    }
                } else {
                    baseline = Some(actual);
                }
            });
        }
    }

    #[gpui_kit::test]
    fn zero_opacity_leaves_no_opaque_digit_or_selected_button_backing(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (_, visual) = cx.add_window_view(|_, _| Transparent);
        visual.simulate_resize(size(px(600.), px(700.)));
        visual.update(|window, cx| {
            window.render_frame(cx);
            for hovered in [None, Some("timer-kind-Study"), Some("timer-kind-CountUp"), Some("widget-pin")] {
                if let Some(id) = hovered { window.hover(id, cx); }
                window.render_frame(cx);
                let backgrounds: Vec<_> = window
                    .painted_quads()
                    .into_iter()
                    .filter(|quad| !quad.background.is_transparent())
                    .collect();
                assert!(
                    backgrounds.is_empty(),
                    "transparent widget still paints opaque backgrounds when hovering {hovered:?}: {backgrounds:?}"
                );
            }
        });
    }
}
