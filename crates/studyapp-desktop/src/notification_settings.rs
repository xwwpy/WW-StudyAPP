//! Notification choices use the same editable preference transaction as other pages.
use crate::{
    app::StudyApp,
    components::*,
    controls::{Button, Checkbox},
    settings_components as form,
};
use gpui_kit::{
    component::{Disableable, Selectable, Sizable, button::ButtonVariants, input::InputState},
    *,
};
use studyapp_core::{
    NoticeCategory, NoticeDensity, NoticePosition, NoticeStyle, NotificationAppearance,
    NotificationSettings, QuietHours,
};

#[derive(Clone, PartialEq)]
pub(crate) struct NotificationViewKey {
    settings: NotificationSettings,
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
    fn notification_view_key(&self) -> NotificationViewKey {
        NotificationViewKey {
            settings: self.notification_settings(),
            selected: self.preferences_ui.notification_category,
            theme: crate::theme::palette(),
            busy: self.notification_busy(),
            revision: self.preferences_ui.section_revision,
        }
    }
    pub fn notification_surface(&self, cx: &mut Context<Self>) -> Div {
        let key = self.notification_view_key();
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
        let rows: Vec<_> = (0..NoticeCategory::ALL.len() + 2).collect();
        self.preferences_ui.notification_rows.sync_with_height(
            &rows,
            self.notification_view_key(),
            px(64.),
        );
        let owner = cx.entity();
        column().w(available.width).h(available.height).min_h_0().min_w_0()
            .child(form::virtual_rows(&self.preferences_ui.notification_rows.scroll, move |row, _, cx| {
                owner.update(cx, |app, cx| {
                    #[cfg(feature = "ui-test")]
                    app.preferences_ui.notification_row_builds.set(app.preferences_ui.notification_row_builds.get() + 1);
                    div().py_2().child(match row {
                        0 => app.notification_general(cx),
                        1 => column().gap_2().child(heading("分别管理通知"))
                            .child(muted("关闭分类会同时关闭该类前台和后台提示。自动保存成功保持静默；主动保存成功默认关闭。展开某一项可覆盖其前台样式。")),
                        _ => app.notification_category_row(NoticeCategory::ALL[row - 2], cx),
                    })
                }).into_any_element()
            }))
    }
    #[cfg(feature = "ui-test")]
    pub fn notification_cache_matches(&self, cx: &App) -> bool {
        self.preferences_ui
            .notification_view
            .as_ref()
            .is_some_and(|view| view.read(cx).key.as_ref() == Some(&self.notification_view_key()))
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
                Checkbox::new(SharedString::from(key.to_owned()))
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
            Checkbox::new("notification-quiet")
                .label("启用定时免打扰")
                .checked(settings.quiet_hours.is_some())
                .disabled(busy)
                .on_click(cx.listener(|this, checked: &bool, _, cx| {
                    if let Some(draft) = &mut this.preferences_ui.draft {
                        draft.notifications.quiet_hours = checked.then_some(QuietHours {
                            start_minute: 1320,
                            end_minute: 480,
                        });
                    }
                    cx.notify();
                })),
        );
        if let Some(hours) = &settings.quiet_hours {
            general = general
                .child(self.notification_time(
                    "notification-start",
                    "免打扰开始",
                    hours.start_minute,
                    hours.end_minute,
                    true,
                    cx,
                ))
                .child(self.notification_time(
                    "notification-end",
                    "免打扰结束",
                    hours.end_minute,
                    hours.start_minute,
                    false,
                    cx,
                ));
        }
        general
    }
    fn notification_category_row(&self, category: NoticeCategory, cx: &mut Context<Self>) -> Div {
        let settings = self.notification_settings();
        let busy = self.notification_busy();
        let rule = settings.rule(category);
        let key = format!("notification-rule-{category:?}");
        let selected = self.preferences_ui.notification_category == Some(category);
        let mut item = column()
            .gap_2()
            .py_2()
            .border_b_1()
            .border_color(crate::theme::color(BORDER))
            .child(
                row()
                    .min_w_0()
                    .justify_between()
                    .gap_2()
                    .child(
                        div().flex_1().min_w_0().child(
                            Checkbox::new(SharedString::from(format!("{key}-enabled")))
                                .label(category.label())
                                .checked(rule.enabled)
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
                        ),
                    )
                    .child(
                        Button::new(SharedString::from(format!("{key}-options")))
                            .small()
                            .ghost()
                            .action(if selected { "收起" } else { "设置" })
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.preferences_ui.notification_category =
                                    (!selected).then_some(category);
                                cx.notify();
                            })),
                    ),
            );
        if selected {
            let mut channels = row().flex_wrap();
            for (suffix, label, checked, foreground) in [
                ("foreground", "应用内", rule.foreground, true),
                ("background", "后台桌面卡片", rule.background, false),
            ] {
                channels = channels.child(
                    Checkbox::new(SharedString::from(format!("{key}-{suffix}")))
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
            let mut styles = row().flex_wrap().gap_1();
            for style in std::iter::once(None).chain(NoticeStyle::ALL.into_iter().map(Some)) {
                styles = styles.child(
                    Button::new(SharedString::from(format!("{key}-style-{style:?}")))
                        .small()
                        .ghost()
                        .label(style.map_or("跟随默认", NoticeStyle::label))
                        .disabled(busy || !rule.enabled)
                        .selected(rule.style == style)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(draft) = &mut this.preferences_ui.draft {
                                draft
                                    .notifications
                                    .rules
                                    .entry(category)
                                    .or_insert_with(|| category.default_rule())
                                    .style = style;
                            }
                            cx.notify();
                        })),
                );
            }
            item = item.child(channels).child(form::field("前台样式", styles));
        }
        item
    }
    fn notification_time(
        &self,
        key: &str,
        label: &str,
        value: u16,
        other: u16,
        start: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut controls = row()
            .flex_wrap()
            .gap_1()
            .child(
                div()
                    .min_w(px(55.))
                    .child(format!("{:02}:{:02}", value / 60, value % 60)),
            );
        for (step, text) in [(-60, "−1时"), (-1, "−1分"), (1, "+1分"), (60, "+1时")] {
            let next = (i32::from(value) + step).rem_euclid(1440) as u16;
            controls = controls.child(
                Button::new(SharedString::from(format!("{key}-{step}")))
                    .small()
                    .ghost()
                    .label(text)
                    .disabled(self.notification_busy() || next == other)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(hours) = this
                            .preferences_ui
                            .draft
                            .as_mut()
                            .and_then(|s| s.notifications.quiet_hours.as_mut())
                        {
                            if start {
                                hours.start_minute = next;
                            } else {
                                hours.end_minute = next;
                            }
                        }
                        cx.notify();
                    })),
            );
        }
        form::settings_row(label.to_owned(), controls)
    }
    pub fn fill_notification_fields(
        &mut self,
        value: &NotificationAppearance,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.fill_surface_fields(value, window, cx);
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
        let (min, max, step) = bounds;
        let mut controls = row().gap_1();
        for (suffix, text, next, limit) in [
            (
                "less",
                "−",
                value.saturating_sub(step).max(min),
                value <= min,
            ),
            (
                "more",
                "+",
                value.saturating_add(step).min(max),
                value >= max,
            ),
        ] {
            controls = controls.child(
                Button::new(SharedString::from(format!("{key}-{suffix}")))
                    .small()
                    .action(text)
                    .disabled(self.notification_busy() || limit)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(draft) = &mut this.preferences_ui.draft {
                            set(&mut draft.appearance.notifications, next);
                        }
                        cx.notify();
                    })),
            );
            if suffix == "less" {
                controls =
                    controls.child(div().min_w(px(48.)).text_center().child(value.to_string()));
            }
        }
        form::settings_row(label.to_owned(), controls)
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
            let mut styles = row().flex_wrap().gap_1();
            for style in NoticeStyle::ALL {
                styles = styles.child(
                    Button::new(SharedString::from(format!("notification-style-{style:?}")))
                        .small()
                        .ghost()
                        .label(style.label())
                        .disabled(busy)
                        .selected(appearance.style == style)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(draft) = &mut this.preferences_ui.draft {
                                draft.appearance.notifications.style = style;
                            }
                            cx.notify();
                        })),
                );
            }
            let mut positions = row().flex_wrap().gap_1();
            for (value, label) in [
                (NoticePosition::TopLeft, "左上"),
                (NoticePosition::TopRight, "右上"),
                (NoticePosition::BottomLeft, "左下"),
                (NoticePosition::BottomRight, "右下"),
            ] {
                positions = positions.child(
                    Button::new(SharedString::from(format!(
                        "notification-position-{value:?}"
                    )))
                    .small()
                    .ghost()
                    .label(label)
                    .disabled(busy)
                    .selected(appearance.position == value)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(draft) = &mut this.preferences_ui.draft {
                            draft.appearance.notifications.position = value;
                        }
                        cx.notify();
                    })),
                );
            }
            let controls=form::section("通知外观").child(muted("沿用应用字体、按钮和明暗风格。分类可在“通知”页单独覆盖样式；控件定位气泡缺少有效控件时使用角落卡片。"))
            .child(form::field("前台默认样式",styles)).child(form::field("气泡与角落卡片位置",positions))
            .child(self.notification_number("notification-duration","普通提示停留（秒）",appearance.duration_seconds.into(),(2,30,1),|s,v|s.duration_seconds=v as u8,cx))
            .child(self.notification_number("notification-visible","最多叠放",appearance.max_visible.into(),(1,4,1),|s,v|s.max_visible=v as u8,cx))
            .child(self.notification_number("notification-width","卡片宽度",appearance.width,(240,480,20),|s,v|s.width=v,cx))
            .child(self.notification_number("notification-radius","卡片圆角",appearance.radius.into(),(0,24,2),|s,v|s.radius=v as u8,cx));
            return column().gap_4().child(controls)
                .child(form::section("同时出现时")
                    .child(form::settings_row("堆叠方式",form::choice("notification-stack",appearance.stack,
                        studyapp_core::NoticeStack::ALL.into_iter().map(|v|(v,v.label().into())).collect(),busy,cx.weak_entity(),|this,value,_,cx| {
                            if let Some(draft)=&mut this.preferences_ui.draft { draft.appearance.notifications.stack=value; } cx.notify();
                        })))
                    .child(muted("最多展示指定数量，超出的通知排队；悬停暂停计时。阻塞对话框打开时，普通通知让位。")))
                .child(self.notice_surface_controls(cx));
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
                Checkbox::new(key)
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
            Checkbox::new("notification-compact")
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
            .child(self.notice_content_controls(cx))
            .child(self.hover_content_controls(cx))
    }
    fn notification_preview(&self, cx: &App) -> Div {
        let appearance = self.preview_notification_appearance(cx);
        let model = studyapp_ui::notification_card::NotificationCardModel {
            key: "notification-preview".into(),
            title: "计时完成".into(),
            summary: "这一段计时已结束。可以记录收获，或稍后继续学习。".into(),
            severity: studyapp_core::NoticeSeverity::Success,
            event_id: None,
            repetitions: 1,
            style: appearance.style,
        };
        form::section("即时预览")
            .child(studyapp_ui::notification_card::notification_card(model,&appearance,&self.preferences_ui.notification_preview_scroll,
                Button::new("notification-preview-close").small().ghost().action("关闭通知").accessibility_label("示例关闭按钮").disabled(true),
                Some(Button::new("notification-preview-action").small().action("记录这段").disabled(true))))
            .child(muted("预览不产生真实通知、不播放声音，也不改变分类开关。严重错误会保留到关闭或处理；停留时间只影响普通提示。修改后请保存全部设置。"))
    }
}
