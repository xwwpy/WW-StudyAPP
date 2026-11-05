//! Shared presentation for the inline timer, desktop window and settings preview.
use crate::{
    components::*,
    controls::Button,
    design::{MONO_FONT, TypeRole},
    theme::ResolvedTheme,
};
use gpui_kit::{
    component::{Disableable, Selectable, Sizable, button::ButtonVariants},
    prelude::FluentBuilder,
    *,
};
use std::rc::Rc;
use studyapp_core::{TimerKind, TimerState, TimerWidgetSettings, WidgetBackground, WidgetLayout};

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
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimerCardVariant {
    Inline,
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
    Countdown(u32),
    Configure,
    ShowOnDesktop,
    OpenSettings,
    OpenMain,
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
    let action = Rc::new(on_action);
    let widget = variant != TimerCardVariant::Inline;
    let preview = variant == TimerCardVariant::Preview;
    let s = &style.settings;
    let compact = widget && s.layout == WidgetLayout::Compact;
    let palette = style.palette;
    let status = if model.finished {
        "✓ 计时已结束"
    } else if model.paused {
        "Ⅱ 计时已暂停"
    } else {
        "▶ 正在计时"
    };
    let accent = if model.paused && !model.finished {
        Hsla::from(rgb(palette.warning))
    } else {
        style.accent
    };
    let button = |id: &str, label: &str, value: TimerCardAction| {
        let action = action.clone();
        Button::new(SharedString::from(id.to_owned()))
            .small()
            .ghost()
            .label(label.to_owned())
            .disabled(preview)
            .on_click(move |_, window, cx| action(value, window, cx))
    };
    let mut header = row().flex_wrap().gap_2().child(
        column()
            .flex_1()
            .min_w_0()
            .gap_1()
            .child(
                heading(if model.label.is_empty() {
                    model.kind.label().to_owned()
                } else {
                    model.label.clone()
                })
                .truncate(),
            )
            .child(
                muted(format!("{} · {status}", model.kind.label())).text_color(rgb(palette.muted)),
            ),
    );
    if !widget {
        header = header.child(button(
            "timer-show-desktop",
            "在桌面显示",
            TimerCardAction::ShowOnDesktop,
        ));
    }
    let value = if model.kind == TimerKind::CountUp {
        model.elapsed
    } else {
        model.remaining
    };
    let root_id = match variant {
        TimerCardVariant::Inline => "today-timer",
        TimerCardVariant::Widget => "timer-widget-card",
        TimerCardVariant::Preview => "timer-widget-preview",
    };
    let progress_id = if widget {
        "timer-widget-progress"
    } else {
        "today-timer-progress"
    };
    // Translucency applies only to the surface, never to text or controls.
    // An opaque number backing remains readable over an unknown desktop.
    let alpha = if widget && s.background != WidgetBackground::Solid {
        f32::from(s.opacity) / 100.
    } else {
        1.
    };
    let mut card = card()
        .id(root_id)
        .test_support()
        .w_full()
        .min_w_0()
        .rounded(px(if widget {
            f32::from(s.corner_radius)
        } else {
            crate::design::CARD_RADIUS
        }))
        .bg(Hsla::from(rgb(style.surface)).opacity(alpha))
        .border_color(rgb(palette.border))
        .text_color(rgb(palette.text))
        .p(px(if compact { 12. } else { 16. }))
        .gap_2()
        .child(header)
        .child(
            typography(TypeRole::Display)
                .font_family(MONO_FONT)
                .text_size(px((if compact { 32. } else { 42. })
                    * if widget {
                        f32::from(s.scale) / 100.
                    } else {
                        1.
                    }))
                .font_features(FontFeatures(std::sync::Arc::new(vec![("tnum".into(), 1)])))
                .text_color(rgb(palette.text))
                .when(widget && s.opacity < 80, |c| {
                    c.bg(rgb(style.surface)).rounded_md().px_2()
                })
                .child(clock(value, !widget || s.show_seconds)),
        )
        .when(!compact && (!widget || s.show_segment), |c| {
            c.child(
                muted(if model.kind == TimerKind::Study {
                    format!(
                        "第 {} 段 · 已用 {}",
                        model.segment,
                        clock(model.elapsed, true)
                    )
                } else {
                    format!("已用 {} · 不会自动计入学习记录", clock(model.elapsed, true))
                })
                .text_color(rgb(palette.muted)),
            )
        });
    if model.kind != TimerKind::CountUp && (!widget || s.show_progress) {
        card = card.child(
            div()
                .id(progress_id)
                .test_support()
                .w_full()
                .h(px(6.))
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
        let mut actions = row().flex_wrap().gap_1().child(button(
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
    if !widget || s.layout == WidgetLayout::Expanded {
        let mut kinds = row().flex_wrap().gap_1();
        for kind in [TimerKind::Study, TimerKind::CountUp, TimerKind::CountDown] {
            kinds = kinds.child(
                button(
                    &format!("timer-kind-{kind:?}"),
                    kind.label(),
                    TimerCardAction::ChooseKind(kind),
                )
                .selected(model.kind == kind),
            );
        }
        card = card.child(kinds);
        if model.kind == TimerKind::CountDown {
            let mut presets = row().flex_wrap().gap_1();
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
    if !compact && model.kind == TimerKind::Study {
        card = card.child(muted(format!(
            "本段 {} 分钟 · 到时将邀请你确认收获",
            model.target / 60
        )));
    }
    card
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
        assert_eq!(clock(61, false), "2 分钟");
    }
}
