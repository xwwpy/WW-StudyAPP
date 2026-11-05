//! Timer configuration and widget appearance use the existing dialog/settings shell.
use crate::{
    app::StudyApp,
    components::*,
    controls::{Button, Checkbox},
    settings_components as form,
};
use gpui_kit::{
    component::{
        Disableable, Selectable, Sizable,
        button::ButtonVariants,
        input::{Input, InputEvent, InputState},
    },
    prelude::FluentBuilder,
    *,
};
use studyapp_core::{Command, TimerKind, TimerWidgetSettings, WidgetBackground, WidgetLayout};
use studyapp_ui::{
    modal_frame::{ModalFrame, ModalTone},
    timer_card::*,
};

impl StudyApp {
    pub fn timer_modal(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.timer_pending.is_some() {
            return Some(ModalFrame::new("重新开始计时？",muted("切换类型、时长或重新开始会清空当前这一段的计时进度。已有学习记录不会改变；如需保留本段收获，可先记录再切换。"))
                .tone(ModalTone::Warning).scroll_handle(self.modal_focus.scroll())
                .footer(row().justify_end()
                    .child(Button::new("timer-cancel-change").label("取消").on_click(cx.listener(|this,_,_,cx|{this.timer_pending=None;cx.notify();})))
                    .child(Button::new("timer-confirm-change").primary().label("确认重新开始").on_click(cx.listener(|this,_,_,cx|{
                        if let Some(command)=this.timer_pending.take(){this.send(command,cx);}cx.notify();
                    }))))
                .into_any_element());
        }
        let kind = self.timer_setup?;
        let mut kinds = row().flex_wrap();
        for item in [TimerKind::Study, TimerKind::CountUp, TimerKind::CountDown] {
            kinds = kinds.child(
                Button::new(SharedString::from(format!("timer-setup-{item:?}")))
                    .small()
                    .label(item.label())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.timer_setup = Some(item);
                        cx.notify();
                    })),
            );
        }
        Some(
            ModalFrame::new(
                "设置计时",
                column()
                    .gap_3()
                    .child(kinds)
                    .child(form::field(
                        "计时名称（可选，最多 80 字）",
                        Input::new(&self.timer_name),
                    ))
                    .when(kind == TimerKind::CountDown, |c| {
                        c.child(form::field(
                            "倒计时分钟数（正整数）",
                            Input::new(&self.timer_duration),
                        ))
                    })
                    .child(muted(if kind == TimerKind::Study {
                        "专注计时沿用学习设置的询问间隔，到点生成待确认记录。"
                    } else {
                        "通用计时不会自动生成学习记录，结束后可选择记录这段。"
                    })),
            )
            .scroll_handle(self.modal_focus.scroll())
            .footer(
                row()
                    .justify_end()
                    .child(
                        Button::new("timer-cancel-setup")
                            .label("取消")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.timer_setup = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("timer-start-setup")
                            .primary()
                            .action("开始计时")
                            .on_click(cx.listener(|this, _, _, cx| {
                                let Some(kind) = this.timer_setup else {
                                    return;
                                };
                                let label = this.timer_name.read(cx).value().trim().to_owned();
                                if label.chars().count() > 80 || label.chars().any(char::is_control)
                                {
                                    this.push_notice(
                                        Err("计时名称须为 80 字以内的单行文字".into()),
                                        cx,
                                    );
                                    return;
                                }
                                let target_seconds = if kind == TimerKind::CountDown {
                                    match studyapp_core::parse_countdown_minutes(
                                        &this.timer_duration.read(cx).value(),
                                    )
                                    .and_then(studyapp_core::countdown_seconds)
                                    {
                                        Ok(seconds) => Some(seconds),
                                        Err(error) => {
                                            this.push_notice(Err(error.to_string()), cx);
                                            return;
                                        }
                                    }
                                } else {
                                    None
                                };
                                this.timer_setup = None;
                                this.request_timer_change(
                                    Command::StartTimer {
                                        kind,
                                        target_seconds,
                                        label,
                                    },
                                    cx,
                                );
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element(),
        )
    }

    pub fn fill_widget_fields(
        &mut self,
        value: &TimerWidgetSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for (key, placeholder, text) in [
            (
                "widget-accent",
                "留空跟随主题；或 #RRGGBB / #RRGGBBAA",
                value.accent.clone().unwrap_or_default(),
            ),
            (
                "widget-presets",
                "正整数分钟，用逗号分隔",
                value
                    .countdown_presets
                    .iter()
                    .map(u64::to_string)
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
        ] {
            if !self.preferences_ui.widget_fields.contains_key(key) {
                let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
                cx.subscribe(&input, move |this, input, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        let text = input.read(cx).value();
                        if let Some(draft) = &mut this.preferences_ui.draft {
                            if key == "widget-accent" {
                                draft.timer_widget.accent =
                                    (!text.trim().is_empty()).then(|| text.trim().to_owned());
                            } else if let Ok(presets) = parse_presets(&text) {
                                draft.timer_widget.countdown_presets = presets;
                            }
                        }
                        cx.notify();
                    }
                })
                .detach();
                self.preferences_ui.widget_fields.insert(key, input);
            }
            self.preferences_ui.widget_fields[key]
                .update(cx, |input, cx| input.set_value(text, window, cx));
            if key == "widget-accent" {
                if let Some(color) = self.preferences_ui.appearance_colors.get(key) {
                    color.update(cx, |color, cx| color.dismiss(cx));
                } else {
                    let source = self.preferences_ui.widget_fields[key].clone();
                    let color = cx.new(|cx| {
                        crate::settings_color::ColorSetting::new(key, source, true, window, cx)
                    });
                    cx.subscribe(
                        &color,
                        |this, color, _: &crate::settings_color::ColorSettingChanged, cx| {
                            let text = color.read(cx).preview_value(cx);
                            if let Some(draft) = &mut this.preferences_ui.draft {
                                draft.timer_widget.accent =
                                    (!text.trim().is_empty()).then(|| text.trim().to_owned());
                            }
                            cx.notify();
                        },
                    )
                    .detach();
                    cx.subscribe(
                        &color,
                        |_, _, _: &crate::settings_color::ColorSettingPreviewChanged, cx| {
                            cx.notify()
                        },
                    )
                    .detach();
                    self.preferences_ui.appearance_colors.insert(key, color);
                }
            }
        }
    }
    pub fn widget_settings_value(
        &self,
        mut value: TimerWidgetSettings,
        cx: &App,
    ) -> anyhow::Result<TimerWidgetSettings> {
        if let Some(input) = self.preferences_ui.widget_fields.get("widget-accent") {
            let text = input.read(cx).value();
            value.accent = (!text.trim().is_empty()).then(|| text.trim().to_owned());
        }
        if let Some(input) = self.preferences_ui.widget_fields.get("widget-presets") {
            value.countdown_presets = parse_presets(&input.read(cx).value())?;
        }
        value.validate()?;
        Ok(value)
    }
    pub fn widget_fields_dirty(&self, baseline: &TimerWidgetSettings, cx: &App) -> bool {
        self.widget_settings_value(baseline.clone(), cx)
            .map_or(true, |value| value != *baseline)
    }
    fn widget_number(
        &self,
        key: &str,
        label: &str,
        value: u8,
        bounds: (u8, u8, i16),
        field: fn(&mut TimerWidgetSettings) -> &mut u8,
        cx: &mut Context<Self>,
    ) -> Div {
        let (min, max, step) = bounds;
        let busy = self.preferences_ui.busy || self.preferences_ui.replacing || self.storage_busy;
        form::settings_row(
            label.to_owned(),
            row()
                .gap_1()
                .child(
                    Button::new(SharedString::from(format!("{key}-less")))
                        .small()
                        .icon_action(gpui_kit::assets::IconName::Minus, format!("减少{label}"))
                        .disabled(busy || value <= min)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(draft) = &mut this.preferences_ui.draft {
                                *field(&mut draft.timer_widget) = (i16::from(value) - step)
                                    .clamp(i16::from(min), i16::from(max))
                                    as u8;
                            }
                            cx.notify();
                        })),
                )
                .child(div().min_w(px(48.)).text_center().child(value.to_string()))
                .child(
                    Button::new(SharedString::from(format!("{key}-more")))
                        .small()
                        .icon_action(gpui_kit::assets::IconName::Plus, format!("增加{label}"))
                        .disabled(busy || value >= max)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(draft) = &mut this.preferences_ui.draft {
                                *field(&mut draft.timer_widget) = (i16::from(value) + step)
                                    .clamp(i16::from(min), i16::from(max))
                                    as u8;
                            }
                            cx.notify();
                        })),
                ),
        )
    }
    pub fn widget_settings_controls(&self, cx: &mut Context<Self>) -> Div {
        let settings = self.preview_widget_settings(cx);
        let busy = self.preferences_ui.busy || self.preferences_ui.replacing || self.storage_busy;
        let timer = self
            .snapshot
            .as_ref()
            .map(|s| s.timer.clone())
            .unwrap_or_default();
        let mut layouts = row().flex_wrap();
        for (value, label) in [
            (WidgetLayout::Compact, "紧凑"),
            (WidgetLayout::Standard, "标准"),
            (WidgetLayout::Expanded, "展开"),
        ] {
            layouts = layouts.child(
                Button::new(SharedString::from(format!("widget-layout-{value:?}")))
                    .small()
                    .label(label)
                    .selected(settings.layout == value)
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(draft) = &mut this.preferences_ui.draft {
                            draft.timer_widget.layout = value;
                        }
                        cx.notify();
                    })),
            );
        }
        let mut backgrounds = row().flex_wrap();
        for (value, label) in [
            (WidgetBackground::Follow, "跟随主题"),
            (WidgetBackground::Solid, "不透明"),
            (WidgetBackground::Inset, "内嵌面色"),
        ] {
            backgrounds = backgrounds.child(
                Button::new(SharedString::from(format!("widget-background-{value:?}")))
                    .small()
                    .label(label)
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(draft) = &mut this.preferences_ui.draft {
                            draft.timer_widget.background = value;
                        }
                        cx.notify();
                    })),
            );
        }
        let mut toggles = column().gap_2();
        let mut behavior = column().gap_2();
        type BoolField = fn(&mut TimerWidgetSettings) -> &mut bool;
        let fields: [(&str, &str, bool, BoolField); 5] = [
            (
                "widget-pinned",
                "窗口置顶（Windows）",
                settings.pinned,
                |s| &mut s.pinned,
            ),
            (
                "widget-seconds",
                "显示秒数",
                settings.show_seconds,
                |s| &mut s.show_seconds,
            ),
            (
                "widget-progress",
                "显示进度",
                settings.show_progress,
                |s| &mut s.show_progress,
            ),
            (
                "widget-segment",
                "显示计时说明",
                settings.show_segment,
                |s| &mut s.show_segment,
            ),
            (
                "widget-controls",
                "显示计时按钮",
                settings.show_controls,
                |s| &mut s.show_controls,
            ),
        ];
        for (key, label, checked, field) in fields {
            let toggle = Checkbox::new(SharedString::from(key.to_owned()))
                .label(label)
                .checked(checked)
                .disabled(busy || (key == "widget-pinned" && !cfg!(windows)))
                .on_click(cx.listener(move |this, checked, _, cx| {
                    if let Some(draft) = &mut this.preferences_ui.draft {
                        *field(&mut draft.timer_widget) = *checked;
                    }
                    cx.notify();
                }));
            if key == "widget-pinned" {
                behavior = behavior.child(toggle);
            } else {
                toggles = toggles.child(toggle);
            }
        }
        let mut colors = form::section("颜色与透明度")
            .id("widget-settings-colors")
            .test_support()
            .child(self.widget_number(
                "widget-opacity",
                "背景不透明度（%）",
                settings.opacity,
                (30, 100, 5),
                |s| &mut s.opacity,
                cx,
            ));
        if let Some(color) = self.preferences_ui.appearance_colors.get("widget-accent") {
            color.update(cx, |color, cx| {
                color.configure(crate::theme::palette().accent, busy, cx)
            });
            colors = colors.child(form::field("强调色（留空跟随主题）", color.clone()));
        }
        let mut presets = form::section("计时预设")
            .id("widget-settings-presets")
            .test_support();
        if let Some(input) = self.preferences_ui.widget_fields.get("widget-presets") {
            presets = presets.child(form::field(
                "倒计时预设（正整数分钟，最多 8 项）",
                Input::new(input).disabled(busy),
            ));
        }
        column().w_full().min_w_0().flex_shrink_0().gap_4()
            .child(heading("桌面计时小组件"))
            .child(muted("窗口和今日页共享同一计时。外观修改可实时预览，保存后保留；窗口位置仅保存在当前设备。"))
            .child(form::section("布局与显示").id("widget-settings-display").test_support()
                .child(form::field("布局", layouts)).child(toggles))
            .child(form::section("尺寸与背景").id("widget-settings-size").test_support()
                .child(form::field("背景", backgrounds))
                .child(self.widget_number("widget-radius", "圆角", settings.corner_radius, (0,28,2), |s| &mut s.corner_radius, cx))
                .child(self.widget_number("widget-scale", "数字缩放（%）", settings.scale, (80,140,5), |s| &mut s.scale, cx)))
            .child(colors)
            .child(presets)
            .child(form::section("窗口行为").id("widget-settings-behavior").test_support()
                .child(behavior)
                .child(Button::new("settings-toggle-timer-widget")
                    .action(if self.timer_widget.is_some() { "隐藏桌面小组件" } else { "显示桌面小组件" }).disabled(busy)
                    .on_click(cx.listener(|this, _, window, cx| this.timer_action(TimerCardAction::ShowOnDesktop, window, cx)))))
            .child(form::section("实时预览").id("widget-settings-preview").test_support()
                .child(div().max_w(px(480.)).child(timer_card(TimerCardModel::from_timer(&timer,1), TimerCardStyle::from_widget_settings(&settings, crate::theme::palette()), TimerCardVariant::Preview, |_,_,_|{}))))
            .child(muted("小组件开启时，关闭主窗口只会最小化；需要真正退出，请使用卡片内的退出图标。背景透明度不会降低文字或按钮的不透明度。")
                .id("widget-settings-end").test_support().flex_shrink_0())
    }
}
fn parse_presets(text: &str) -> anyhow::Result<Vec<u64>> {
    let values = text
        .replace('，', ",")
        .split(',')
        .map(studyapp_core::parse_countdown_minutes)
        .collect::<Result<Vec<_>, _>>()?;
    anyhow::ensure!(
        !values.is_empty() && values.len() <= 8,
        "倒计时预设须为正整数分钟，共 1–8 项"
    );
    let unique: std::collections::BTreeSet<_> = values.iter().collect();
    anyhow::ensure!(unique.len() == values.len(), "倒计时预设不能重复");
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::parse_presets;
    #[test]
    fn presets_allow_long_durations_but_never_wrap_or_truncate() {
        assert_eq!(
            parse_presets("181，1500, 4294967296").unwrap(),
            vec![181, 1500, 4294967296]
        );
        assert_eq!(
            parse_presets(&(u64::MAX / 60).to_string()).unwrap(),
            vec![u64::MAX / 60]
        );
        for text in ["", "0", "1,1", "-1", "1.5", "1,2,3,4,5,6,7,8,9"] {
            assert!(parse_presets(text).is_err(), "{text}");
        }
        assert!(parse_presets(&(u64::MAX / 60 + 1).to_string()).is_err());
    }
}
