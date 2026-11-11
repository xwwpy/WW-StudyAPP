//! Notification choices use the same editable preference transaction as other pages.
use crate::{
    app::StudyApp,
    components::*,
    controls::{Button, Checkbox},
    settings_components as form,
};
use gpui_kit::{
    component::{Disableable, Sizable, button::ButtonVariants, input::InputState},
    prelude::FluentBuilder,
    *,
};
use studyapp_core::{
    NoticeCategory, NoticeDensity, NoticePosition, NoticeStyle, NotificationAppearance,
    NotificationSettings, QuietHours,
};

#[derive(Clone, PartialEq)]
pub(crate) struct NotificationViewKey {
    settings: NotificationSettings,
    times: Vec<SharedString>,
    page: usize,
    appearance: NotificationAppearance,
    selected: Option<NoticeCategory>,
    theme: crate::theme::ResolvedTheme,
    busy: bool,
    revision: u64,
}

pub(crate) struct NotificationSettingsView {
    owner: WeakEntity<StudyApp>,
    key: Option<NotificationViewKey>,
}
impl Render for NotificationSettingsView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let owner = self.owner.clone();
        container_query(move |available, _, cx| {
            owner
                .update(cx, |app, cx| app.notification_workspace(available, cx))
                .unwrap_or_else(|_| column())
        })
        .min_w_0()
        .min_h_0()
    }
}

type Toggle<T> = (&'static str, &'static str, bool, fn(&mut T) -> &mut bool);

impl StudyApp {
    fn notification_settings(&self) -> NotificationSettings {
        self.preferences_ui
            .draft
            .as_ref()
            .map(|s| s.notifications.clone())
            .or_else(|| {
                self.snapshot
                    .as_ref()
                    .map(|s| s.settings.notifications.clone())
            })
            .unwrap_or_default()
    }
    fn notification_busy(&self) -> bool {
        self.preferences_ui.busy || self.preferences_ui.replacing || self.storage_busy
    }
    fn notification_view_key(&self, cx: &App) -> NotificationViewKey {
        NotificationViewKey {
            settings: self.notification_settings(),
            page: self.preferences_ui.notification_page,
            times: ["notification-start", "notification-end"]
                .into_iter()
                .filter_map(|key| {
                    self.preferences_ui
                        .notification_fields
                        .get(key)
                        .map(|input| input.read(cx).value())
                })
                .collect(),
            appearance: self
                .preferences_ui
                .draft
                .as_ref()
                .map(|draft| draft.appearance.notifications.clone())
                .unwrap_or_default(),
            selected: self.preferences_ui.notification_category,
            theme: crate::theme::palette(),
            busy: self.notification_busy(),
            revision: self.preferences_ui.section_revision,
        }
    }
    pub fn notification_surface(&self, cx: &mut Context<Self>) -> Div {
        let key = self.notification_view_key(cx);
        let Some(view) = self.preferences_ui.notification_view.as_ref() else {
            return column();
        };
        view.update(cx, |view, cx| {
            if view.key.as_ref() != Some(&key) {
                view.key = Some(key);
                cx.notify();
            }
        });
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .w_full()
            .child(
                div()
                    .id("settings-notifications-surface")
                    .test_support()
                    .size_full()
                    .child(view.clone().cached(StyleRefinement::default().size_full())),
            )
    }
    fn notification_workspace(&self, available: Size<Pixels>, cx: &mut Context<Self>) -> Div {
        let pages = NoticeCategory::ALL.len().div_ceil(5);
        let page = self
            .preferences_ui
            .notification_page
            .min(pages.saturating_sub(1));
        let rows: Vec<_> = [0, 1]
            .into_iter()
            .chain(
                (page * 5..((page + 1) * 5).min(NoticeCategory::ALL.len())).map(|index| index + 2),
            )
            .collect();
        self.preferences_ui.notification_rows.sync_with_height(
            &rows,
            self.notification_view_key(cx),
            px(64.),
        );
        self.apply_settings_list_offset(&self.preferences_ui.notification_rows.scroll);
        let owner = cx.entity();
        column().w(available.width).h(available.height).min_h_0().min_w_0()
            .child(form::virtual_rows(&self.preferences_ui.notification_rows.scroll, move |row, _, cx| {
                owner.update(cx, |app, cx| {
                    #[cfg(feature = "ui-test")]
                    app.preferences_ui.notification_row_builds.set(app.preferences_ui.notification_row_builds.get() + 1);
                    let row = rows[row];
                    div().py_2().child(match row {
                        0 => app.notification_general(cx),
                        1 => column().gap_2().child(heading("分别管理通知"))
                            .child(muted("关闭分类会同时关闭该类前台和后台提示。自动保存成功保持静默；主动保存成功默认关闭。各场景可分别覆盖其前台样式。")),
                        _ => app.notification_category_row(NoticeCategory::ALL[row - 2], cx),
                    })
                }).into_any_element()
            }))
            .child(row().flex_wrap().gap_2().child(muted(format!("场景第 {} / {} 页 · 每页 5 项", page + 1, pages)))
                .child(Button::new("notice-page-prev").small().ghost().label("上一页").disabled(page == 0 || self.notification_busy())
                    .on_click(cx.listener(move |this, _, _, cx| { this.preferences_ui.notification_page = page.saturating_sub(1); this.preferences_ui.notification_rows.scroll_to_top(); cx.notify(); })))
                .child(Button::new("notice-page-next").small().ghost().label("下一页").disabled(page + 1 >= pages || self.notification_busy())
                    .on_click(cx.listener(move |this, _, _, cx| { this.preferences_ui.notification_page = (page + 1).min(pages - 1); this.preferences_ui.notification_rows.scroll_to_top(); cx.notify(); }))))
    }
    #[cfg(feature = "ui-test")]
    pub fn notification_cache_matches(&self, cx: &App) -> bool {
        self.preferences_ui
            .notification_view
            .as_ref()
            .is_some_and(|view| view.read(cx).key.as_ref() == Some(&self.notification_view_key(cx)))
    }
    #[cfg(feature = "ui-test")]
    pub fn notification_render_work(&self) -> u64 {
        self.preferences_ui.notification_row_builds.get()
    }
    fn notification_general(&self, cx: &mut Context<Self>) -> Div {
        let settings = self.notification_settings();
        let busy = self.notification_busy();
        let globals: [Toggle<NotificationSettings>; 5] = [
            (
                "notification-update-badge",
                "显示新版本红点",
                settings.update_badge,
                |s| &mut s.update_badge,
            ),
            (
                "notification-dnd",
                "立即免打扰",
                settings.do_not_disturb,
                |s| &mut s.do_not_disturb,
            ),
            (
                "notification-critical",
                "免打扰时仍显示严重错误",
                settings.critical_during_quiet,
                |s| &mut s.critical_during_quiet,
            ),
            (
                "notification-private",
                "桌面卡片隐藏具体内容",
                settings.private_desktop,
                |s| &mut s.private_desktop,
            ),
            (
                "notification-sound",
                "播放提示音",
                settings.sound,
                |s| &mut s.sound,
            ),
        ];
        let mut general = form::section("通知方式")
            .child(muted("分类开关与前后台开关分别生效；删除、覆盖和未保存内容的必要确认不受这些开关影响。后台使用独立桌面卡片，不使用系统通知。"));
        for (key, label, checked, field) in globals {
            general = general.child(
                studyapp_ui::settings_switch::SettingSwitch::new(SharedString::from(
                    key.to_owned(),
                ))
                .default_value(*field(&mut NotificationSettings::default()))
                .label(label)
                .checked(checked)
                .disabled(busy)
                .on_click(cx.listener(move |this, checked, _, cx| {
                    if let Some(draft) = &mut this.preferences_ui.draft {
                        *field(&mut draft.notifications) = *checked;
                    }
                    cx.notify();
                })),
            );
        }
        general = general.child(
            studyapp_ui::settings_switch::SettingSwitch::new("notification-quiet")
                .default_value(false)
                .label("启用定时免打扰")
                .checked(settings.quiet_hours.is_some())
                .disabled(busy)
                .on_click(cx.listener(|this, checked: &bool, _, cx| {
                    if let Some(draft) = &mut this.preferences_ui.draft {
                        if !*checked {
                            this.preferences_ui.quiet_draft =
                                draft.notifications.quiet_hours.take();
                        } else {
                            draft.notifications.quiet_hours =
                                Some(this.preferences_ui.quiet_draft.clone().unwrap_or(
                                    QuietHours {
                                        start_minute: 1320,
                                        end_minute: 480,
                                    },
                                ));
                        }
                    }
                    cx.notify();
                })),
        );
        let enabled = settings.quiet_hours.is_some();
        let mut times = row().flex_wrap().gap_3();
        for (key, label, default) in [
            ("notification-start", "免打扰开始", 1320),
            ("notification-end", "免打扰结束", 480),
        ] {
            if let Some(input) = self.preferences_ui.notification_fields.get(key) {
                times = times.child(
                    form::field(
                        label,
                        studyapp_ui::settings_time::time_input(
                            key,
                            input,
                            default,
                            busy || !enabled,
                            cx,
                        ),
                    )
                    .flex_1()
                    .min_w(px(250.)),
                );
            }
        }
        let raw_time = |key| {
            self.preferences_ui
                .notification_fields
                .get(key)
                .map(|input| input.read(cx).value().to_string())
                .unwrap_or_default()
        };
        general
            .child(times)
            .child(
                muted(if enabled {
                    "已启用定时免打扰；保存全部设置后生效。"
                } else {
                    "启用定时免打扰后可调整起止时间；当前时间草稿保留。"
                })
                .id("quiet-hours-enabled-hint")
                .test_support(),
            )
            .child(
                muted(studyapp_ui::settings_time::range_description(
                    &raw_time("notification-start"),
                    &raw_time("notification-end"),
                ))
                .id("quiet-hours-range-hint")
                .test_support(),
            )
    }
    fn notification_category_row(&self, category: NoticeCategory, cx: &mut Context<Self>) -> Div {
        let settings = self.notification_settings();
        let busy = self.notification_busy();
        let rule = settings.rule(category);
        let key = format!("notification-rule-{category:?}");
        let mut item = column()
            .relative()
            .child(studyapp_ui::settings_anchor::probe(category.label()))
            .gap_2()
            .py_3()
            .border_b_1()
            .border_color(crate::theme::color(BORDER))
            .child(
                studyapp_ui::settings_switch::SettingSwitch::new(SharedString::from(format!(
                    "{key}-enabled"
                )))
                .label(category.label())
                .checked(rule.enabled)
                .default_value(category.default_rule().enabled)
                .disabled(busy)
                .on_click(cx.listener(move |this, checked, _, cx| {
                    if let Some(draft) = &mut this.preferences_ui.draft {
                        draft
                            .notifications
                            .rules
                            .entry(category)
                            .or_insert_with(|| category.default_rule())
                            .enabled = *checked;
                    }
                    cx.notify();
                })),
            );

        {
            let mut channels = row().flex_wrap();
            for (suffix, label, checked, foreground) in [
                ("foreground", "应用内", rule.foreground, true),
                ("background", "后台桌面卡片", rule.background, false),
            ] {
                channels = channels.child(
                    Checkbox::new(SharedString::from(format!("{key}-{suffix}")))
                        .relative()
                        .child(studyapp_ui::settings_anchor::probe(format!(
                            "{key}-{suffix}"
                        )))
                        .label(label)
                        .checked(checked)
                        .disabled(busy || !rule.enabled)
                        .on_click(cx.listener(move |this, checked, _, cx| {
                            if let Some(draft) = &mut this.preferences_ui.draft {
                                let rule = draft
                                    .notifications
                                    .rules
                                    .entry(category)
                                    .or_insert_with(|| category.default_rule());
                                if foreground {
                                    rule.foreground = *checked;
                                } else {
                                    rule.background = *checked;
                                }
                            }
                            cx.notify();
                        })),
                );
            }
            let styles = form::inherited_choice(
                SharedString::from(format!("{key}-style")),
                rule.style,
                (
                    category.default_style(studyapp_core::NoticeSeverity::Information),
                    format!(
                        "继承用途分配 · 普通事件：{}；错误：{}；严重错误保留必要确认。",
                        category
                            .default_style(studyapp_core::NoticeSeverity::Information)
                            .label(),
                        category
                            .default_style(studyapp_core::NoticeSeverity::Error)
                            .label(),
                    ),
                ),
                category
                    .notification_styles()
                    .iter()
                    .copied()
                    .map(|style| (style, style.label().into()))
                    .collect(),
                busy || !rule.enabled,
                cx.weak_entity(),
                move |this, style, _, cx| {
                    if let Some(draft) = &mut this.preferences_ui.draft {
                        draft
                            .notifications
                            .rules
                            .entry(category)
                            .or_insert_with(|| category.default_rule())
                            .style = style;
                    }
                    cx.notify();
                },
            );
            item = item
                .child(form::checkbox_group("通知投递渠道", channels))
                .child(
                    div()
                        .relative()
                        .child(studyapp_ui::settings_anchor::probe(format!("{key}-style")))
                        .child(form::settings_row("前台样式", styles)),
                )
                .when(!rule.enabled, |item| {
                    item.child(muted(
                        "启用此通知分类后可调整投递渠道、前台样式与内容字段；当前草稿保留。",
                    ))
                })
                .child(self.notice_scenario_fields(category, cx));
        }
        item
    }
    pub fn fill_notification_fields(
        &mut self,
        value: &NotificationAppearance,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.preferences_ui.notification_view.is_none() {
            let owner = cx.weak_entity();
            self.preferences_ui.notification_view =
                Some(cx.new(|_| NotificationSettingsView { owner, key: None }));
        }
        for (key, text) in [
            (
                "notification-background",
                value.background.clone().unwrap_or_default(),
            ),
            (
                "notification-accent",
                value.accent.clone().unwrap_or_default(),
            ),
        ] {
            if let Some(input) = self.preferences_ui.notification_fields.get(key) {
                input.update(cx, |input, cx| input.set_value(text, window, cx));
            } else {
                let input = cx.new(|cx| InputState::new(window, cx).default_value(text));
                self.preferences_ui
                    .notification_fields
                    .insert(key, input.clone());
                let color = cx.new(|cx| {
                    crate::settings_color::ColorSetting::new(key, input, true, window, cx)
                });
                cx.subscribe(
                    &color,
                    move |this, color, _: &crate::settings_color::ColorSettingChanged, cx| {
                        let raw = color.read(cx).preview_value(cx);
                        let value = (!raw.is_empty()).then(|| raw.to_string());
                        if let Some(draft) = &mut this.preferences_ui.draft {
                            if key == "notification-background" {
                                draft.appearance.notifications.background = value;
                            } else {
                                draft.appearance.notifications.accent = value;
                            }
                        }
                        cx.notify();
                    },
                )
                .detach();
                cx.subscribe(
                    &color,
                    |_, _, _: &crate::settings_color::ColorSettingPreviewChanged, cx| cx.notify(),
                )
                .detach();
                self.preferences_ui.appearance_colors.insert(key, color);
            }
            if let Some(color) = self.preferences_ui.appearance_colors.get(key) {
                color.update(cx, |color, cx| color.dismiss(cx));
            }
        }
    }
    fn preview_notification_appearance(&self, cx: &App) -> NotificationAppearance {
        let mut value = self
            .preferences_ui
            .draft
            .as_ref()
            .map(|s| s.appearance.notifications.clone())
            .unwrap_or_default();
        for (key, background) in [
            ("notification-background", true),
            ("notification-accent", false),
        ] {
            if let Some(color) = self.preferences_ui.appearance_colors.get(key) {
                let raw = color.read(cx).preview_value(cx);
                if raw.is_empty() || studyapp_core::parse_rgba_color(&raw).is_some() {
                    let raw = (!raw.is_empty()).then(|| raw.to_string());
                    if background {
                        value.background = raw;
                    } else {
                        value.accent = raw;
                    }
                }
            }
        }
        value
    }
    fn notification_number(
        &self,
        key: &str,
        label: &str,
        value: u16,
        bounds: (u16, u16, u16),
        set: fn(&mut NotificationAppearance, u16),
        cx: &mut Context<Self>,
    ) -> Div {
        let defaults = NotificationAppearance::default();
        let default = match key {
            "notification-duration" => defaults.duration_seconds.into(),
            "notification-visible" => defaults.max_visible.into(),
            "notification-width" => defaults.width,
            _ => defaults.radius.into(),
        };
        self.setting_number(
            key.into(),
            label,
            if key == "notification-duration" {
                "秒"
            } else if key == "notification-visible" {
                "张"
            } else {
                "px"
            },
            (
                value.into(),
                default.into(),
                bounds.0.into(),
                bounds.1.into(),
                self.notification_busy(),
            ),
            move |this, value| {
                if let Some(draft) = &mut this.preferences_ui.draft {
                    set(&mut draft.appearance.notifications, value as u16);
                }
            },
            cx,
        )
    }
    pub fn notification_appearance_part(&self, part: usize, cx: &mut Context<Self>) -> Div {
        #[cfg(feature = "ui-test")]
        self.preferences_ui
            .notification_row_builds
            .set(self.preferences_ui.notification_row_builds.get() + 1);
        if part == 2 {
            return self.notification_preview(cx);
        }
        let appearance = self.preview_notification_appearance(cx);
        let busy = self.notification_busy();
        if part == 0 {
            let positions = form::choice_with_default(
                "notification-position",
                (appearance.position, NoticePosition::default()),
                vec![
                    (NoticePosition::TopLeft, "左上".into()),
                    (NoticePosition::TopRight, "右上".into()),
                    (NoticePosition::BottomLeft, "左下".into()),
                    (NoticePosition::BottomRight, "右下".into()),
                ],
                busy,
                cx.weak_entity(),
                |this, position, _, cx| {
                    if let Some(draft) = &mut this.preferences_ui.draft {
                        draft.appearance.notifications.position = position;
                    }
                    cx.notify();
                },
            );
            let controls=form::section("通知外观").child(muted("沿用应用字体、按钮和明暗风格。组件按真实业务匹配；在通知场景中仅可选择适用的替代组件。"))
            .child(form::settings_row("角落卡片位置",positions))
            .child(self.notification_number("notification-duration","普通提示停留（秒）",appearance.duration_seconds.into(),(2,30,1),|s,v|s.duration_seconds=v as u8,cx))
            .child(self.notification_number("notification-visible","最多叠放",appearance.max_visible.into(),(1,4,1),|s,v|s.max_visible=v as u8,cx))
            .child(self.notification_number("notification-width","卡片宽度",appearance.width,(240,480,20),|s,v|s.width=v,cx))
            .child(self.notification_number("notification-radius","卡片圆角",appearance.radius.into(),(0,24,2),|s,v|s.radius=v as u8,cx));
            return controls;
        }
        let mut controls = form::section("间距、动效与颜色");
        let toggles: [Toggle<NotificationAppearance>; 3] = [
            (
                "notification-animation",
                "启用过渡动画",
                appearance.animation,
                |s| &mut s.animation,
            ),
            (
                "notification-reduce-motion",
                "减少动态效果",
                appearance.reduce_motion,
                |s| &mut s.reduce_motion,
            ),
            (
                "notification-shadow",
                "显示卡片阴影",
                appearance.shadow,
                |s| &mut s.shadow,
            ),
        ];
        for (key, label, checked, field) in toggles {
            controls = controls.child(
                studyapp_ui::settings_switch::SettingSwitch::new(key)
                    .default_value(*field(&mut NotificationAppearance::default()))
                    .label(label)
                    .checked(checked)
                    .disabled(busy)
                    .on_click(cx.listener(move |this, value, _, cx| {
                        if let Some(draft) = &mut this.preferences_ui.draft {
                            *field(&mut draft.appearance.notifications) = *value;
                        }
                        cx.notify();
                    })),
            );
        }
        controls = controls.child(
            studyapp_ui::settings_switch::SettingSwitch::new("notification-compact")
                .default_value(false)
                .label("紧凑间距")
                .checked(appearance.density == NoticeDensity::Compact)
                .disabled(busy)
                .on_click(cx.listener(|this, value, _, cx| {
                    if let Some(draft) = &mut this.preferences_ui.draft {
                        draft.appearance.notifications.density = if *value {
                            NoticeDensity::Compact
                        } else {
                            NoticeDensity::Comfortable
                        };
                    }
                    cx.notify();
                })),
        );
        for (key, label) in [
            ("notification-background", "卡片背景（自动 / RGBA）"),
            ("notification-accent", "强调色（自动 / RGBA）"),
        ] {
            if let Some(color) = self.preferences_ui.appearance_colors.get(key) {
                color.update(cx, |color, cx| {
                    color.configure(crate::theme::palette().accent, busy, cx)
                });
                controls = controls.child(form::field(label, color.clone()));
            }
        }
        column()
            .gap_4()
            .child(controls)
            .child(self.hover_content_controls(cx))
    }
    fn notification_preview(&self, cx: &mut Context<Self>) -> Div {
        let appearance = self.preview_notification_appearance(cx);
        let model = studyapp_ui::notification_card::NotificationCardModel {
            category: None,
            key: "notification-preview".into(),
            title: "计时完成".into(),
            summary: "这一段计时已结束。可以记录收获，或稍后继续学习。".into(),
            severity: studyapp_core::NoticeSeverity::Success,
            event_id: None,
            repetitions: 1,
            style: self
                .preferences_ui
                .notice_component
                .unwrap_or(NoticeStyle::Bubble),
        };
        let scroll = self.preferences_ui.notification_preview_scroll.clone();
        let key = format!("notice-preview-{:?}", model.style);
        let collapsed = self.preferences_ui.collapsed_previews.contains(&key);
        form::section("即时预览")
            .child(Button::new("toggle-notice-preview").small().ghost().label(if collapsed { "展开预览" } else { "收起预览" })
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.preferences_ui.collapsed_previews.remove(&key) { this.preferences_ui.collapsed_previews.insert(key.clone()); }
                    this.preferences_ui.section_revision += 1; cx.notify();
                })))
            .children((!collapsed).then_some(studyapp_ui::notice_preview::ComponentPreview { model, appearance, scroll }))
            .child(muted("布局示例不产生真实通知、不执行文件操作、不播放声音。菜单、任务和表单使用各自的真实组件结构。"))
    }
}

impl StudyApp {
    pub(crate) fn fill_notification_times(
        &mut self,
        settings: &NotificationSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let hours = settings.quiet_hours.clone().unwrap_or(QuietHours {
            start_minute: 1320,
            end_minute: 480,
        });
        self.preferences_ui.quiet_draft = Some(hours.clone());
        for (key, minute) in [
            ("notification-start", hours.start_minute),
            ("notification-end", hours.end_minute),
        ] {
            let text = studyapp_ui::settings_time::format_time(minute);
            if let Some(input) = self.preferences_ui.notification_fields.get(key) {
                input.update(cx, |input, cx| input.set_value(text, window, cx));
            } else {
                let input = cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder("HH:MM")
                        .default_value(text)
                });
                cx.subscribe(
                    &input,
                    |this, _, event: &gpui_kit::component::input::InputEvent, cx| {
                        if matches!(event, gpui_kit::component::input::InputEvent::Change) {
                            let value = this
                                .preferences_ui
                                .draft
                                .as_ref()
                                .map(|draft| draft.notifications.clone());
                            if let Some(settings) = value
                                .and_then(|value| this.notification_settings_value(value, cx).ok())
                                && let Some(hours) = settings.quiet_hours
                            {
                                this.preferences_ui.quiet_draft = Some(hours.clone());
                                if let Some(draft) = &mut this.preferences_ui.draft {
                                    draft.notifications.quiet_hours = Some(hours);
                                }
                            }
                            cx.notify();
                        }
                    },
                )
                .detach();
                self.preferences_ui.notification_fields.insert(key, input);
            }
        }
    }
    pub(crate) fn notification_settings_value(
        &self,
        mut value: NotificationSettings,
        cx: &App,
    ) -> anyhow::Result<NotificationSettings> {
        if value.quiet_hours.is_some() {
            let minute = |key| {
                self.preferences_ui
                    .notification_fields
                    .get(key)
                    .and_then(|input| {
                        studyapp_ui::settings_time::parse_time(input.read(cx).value().as_str())
                    })
                    .ok_or_else(|| anyhow::anyhow!("免打扰时间请填写 00:00–23:59，精确到分钟"))
            };
            value.quiet_hours = Some(QuietHours {
                start_minute: minute("notification-start")?,
                end_minute: minute("notification-end")?,
            });
        }
        value.validate()?;
        Ok(value)
    }
    pub(crate) fn notification_times_dirty(&self, cx: &App) -> bool {
        self.preferences_ui.draft.as_ref().is_some_and(|draft| {
            self.notification_settings_value(draft.notifications.clone(), cx)
                .is_err()
        })
    }
}
