use crate::{
    app::StudyApp,
    components::*,
    controls::{Button, Checkbox},
    settings_components as form,
};
use gpui_kit::{
    component::{Disableable, input::InputState},
    *,
};
use studyapp_core::{NoticeContent, NoticeStyle, NoticeSurfacePatch, NotificationAppearance};
const COLORS: [(NoticeStyle, &str, &str); 14] = [
    (
        NoticeStyle::Bubble,
        "surface-toast-bg",
        "surface-toast-border",
    ),
    (
        NoticeStyle::CornerCard,
        "surface-corner-bg",
        "surface-corner-border",
    ),
    (
        NoticeStyle::Banner,
        "surface-banner-bg",
        "surface-banner-border",
    ),
    (
        NoticeStyle::ModalCard,
        "surface-dialog-bg",
        "surface-dialog-border",
    ),
    (
        NoticeStyle::Anchored,
        "surface-popover-bg",
        "surface-popover-border",
    ),
    (
        NoticeStyle::ActionSheet,
        "surface-sheet-bg",
        "surface-sheet-border",
    ),
    (
        NoticeStyle::Snackbar,
        "surface-snackbar-bg",
        "surface-snackbar-border",
    ),
    (
        NoticeStyle::ActivityView,
        "surface-activity-bg",
        "surface-activity-border",
    ),
    (NoticeStyle::Hud, "surface-hud-bg", "surface-hud-border"),
    (
        NoticeStyle::Share,
        "surface-share-bg",
        "surface-share-border",
    ),
    (
        NoticeStyle::Dialog,
        "surface-form-bg",
        "surface-form-border",
    ),
    (
        NoticeStyle::Actionbar,
        "surface-actionbar-bg",
        "surface-actionbar-border",
    ),
    (
        NoticeStyle::PopupMenu,
        "surface-menu-bg",
        "surface-menu-border",
    ),
    (
        NoticeStyle::Tooltip,
        "surface-tooltip-bg",
        "surface-tooltip-border",
    ),
];
impl StudyApp {
    pub(crate) fn fill_surface_fields(
        &mut self,
        appearance: &NotificationAppearance,
        only: Option<NoticeStyle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for (style, background, border) in COLORS
            .into_iter()
            .filter(|(style, _, _)| only.is_none_or(|selected| selected == *style))
        {
            let surface = appearance.surface(style);
            for (key, value, is_background) in [
                (background, surface.background, true),
                (border, surface.border_color, false),
            ] {
                let text = value.unwrap_or_default();
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
                            if let Some(draft) = &mut this.preferences_ui.draft {
                                let surface =
                                    draft.appearance.notifications.surface_patch_mut(style);
                                let field = if is_background {
                                    &mut surface.background
                                } else {
                                    &mut surface.border_color
                                };
                                *field = Some(raw.to_string());
                            }
                            cx.notify();
                        },
                    )
                    .detach();
                    self.preferences_ui.appearance_colors.insert(key, color);
                }
            }
        }
    }
    fn surface_number(
        &self,
        label: &str,
        key: &'static str,
        value: u16,
        values: &[u16],
        set: fn(&mut NoticeSurfacePatch, u16),
        cx: &mut Context<Self>,
    ) -> Div {
        let style = self
            .preferences_ui
            .notice_component
            .unwrap_or(NoticeStyle::Bubble);
        let busy = self.preferences_ui.busy || self.preferences_ui.replacing || self.storage_busy;
        let defaults = NotificationAppearance::default().surface(style);
        let default = match key {
            "surface-opacity" => defaults.opacity.into(),
            "surface-border" => defaults.border_width.into(),
            "surface-width" => defaults.width,
            "surface-height" => defaults.max_height,
            _ => defaults.radius.into(),
        };
        let min = values.iter().copied().min().unwrap_or(0) as u64;
        let max = values.iter().copied().max().unwrap_or(u16::MAX) as u64;
        let appearance = self
            .preferences_ui
            .draft
            .as_ref()
            .map(|d| d.appearance.notifications.clone())
            .unwrap_or_default();
        let overridden = appearance.surfaces.contains_key(&style)
            || appearance
                .surface_patches
                .get(&style)
                .is_some_and(|patch| match key {
                    "surface-opacity" => patch.opacity.is_some(),
                    "surface-border" => patch.border_width.is_some(),
                    "surface-width" => patch.width.is_some(),
                    "surface-height" => patch.max_height.is_some(),
                    _ => patch.radius.is_some(),
                });
        let inherited = !overridden;
        let mut row = column()
            .relative()
            .child(studyapp_ui::settings_anchor::probe(label.to_owned()))
            .gap_1();
        if inherited {
            row = row.child(muted(format!(
                "{label}：继承全局／组件默认 · 当前有效值 {value}{}",
                if key == "surface-opacity" { "%" } else { " px" }
            )));
        } else {
            row = row.child(self.setting_number(
                format!("{key}-{style:?}"),
                label,
                if key == "surface-opacity" { "%" } else { "px" },
                (value.into(), default.into(), min, max, busy),
                move |this, value| {
                    if let Some(draft) = &mut this.preferences_ui.draft {
                        set(
                            draft.appearance.notifications.surface_patch_mut(style),
                            value as u16,
                        );
                    }
                },
                cx,
            ));
        }
        row.child(
            Button::new(SharedString::from(format!("{key}-{style:?}-inherit")))
                .label(if inherited {
                    "自定义"
                } else {
                    "恢复继承"
                })
                .disabled(busy)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(draft) = &mut this.preferences_ui.draft {
                        let appearance = &mut draft.appearance.notifications;
                        let patch = appearance.surface_patch_mut(style);
                        if inherited {
                            set(patch, value);
                        } else {
                            match key {
                                "surface-opacity" => patch.opacity = None,
                                "surface-border" => patch.border_width = None,
                                "surface-width" => patch.width = None,
                                "surface-height" => patch.max_height = None,
                                _ => patch.radius = None,
                            }
                        }
                    }
                    this.preferences_ui
                        .number_fields
                        .borrow_mut()
                        .remove(&format!("{key}-{style:?}"));
                    cx.notify();
                })),
        )
    }

    pub(crate) fn notice_surface_controls(&self, cx: &mut Context<Self>) -> Div {
        let appearance = self
            .preferences_ui
            .draft
            .as_ref()
            .map(|d| d.appearance.notifications.clone())
            .unwrap_or_default();
        let style = self
            .preferences_ui
            .notice_component
            .unwrap_or(NoticeStyle::Bubble);
        let surface = appearance.surface(style);
        let busy = self.preferences_ui.busy || self.preferences_ui.replacing || self.storage_busy;
        let mut view = form::section(format!("独立组件样式 · {}", style.label()))
            .child(muted(
                "当前子页仅编辑这一组件；未自定义的属性持续继承全局／组件默认，颜色可单独选择主题自动。",
            ))
            .child(form::settings_row(
                "组件布局",
                form::choice_with_default(
                    "notice-component-layout",
                    (appearance.layout(style), NotificationAppearance::default().layout(style)),
                    [
                        studyapp_core::NoticeLayout::Primary,
                        studyapp_core::NoticeLayout::Alternative,
                    ]
                    .into_iter()
                    .map(|layout| (layout, style.layout_label(layout).to_owned()))
                    .collect(),
                    busy,
                    cx.weak_entity(),
                    move |this, layout, _, cx| {
                        if let Some(draft) = &mut this.preferences_ui.draft {
                            draft.appearance.notifications.layouts.insert(style, layout);
                        }
                        cx.notify();
                    },
                ),
            ))
            .child(self.surface_number(
                "不透明度（%）",
                "surface-opacity",
                surface.opacity.into(),
                &[20, 40, 60, 80, 90, 100],
                |s, v| s.opacity = Some(v as u8),
                cx,
            ))
            .child(self.surface_number(
                "边框宽度（px）",
                "surface-border",
                surface.border_width.into(),
                &[0, 1, 2, 3, 4],
                |s, v| s.border_width = Some(v as u8),
                cx,
            ))
            .child(self.surface_number(
                "宽度（px）",
                "surface-width",
                surface.width,
                &[200, 240, 280, 320, 360, 420, 480, 560, 640],
                |s, v| s.width = Some(v),
                cx,
            ))
            .child(self.surface_number(
                "内容最大高度（px）",
                "surface-height",
                surface.max_height,
                &[48, 80, 120, 164, 240, 320, 480],
                |s, v| s.max_height = Some(v),
                cx,
            ))
            .child(self.surface_number(
                "圆角（px）",
                "surface-radius",
                surface.radius.into(),
                &[0, 4, 8, 12, 16, 24, 32],
                |s, v| s.radius = Some(v as u8),
                cx,
            ));
        if style == NoticeStyle::Tooltip {
            view = view.child(form::settings_row(
                "说明定位",
                form::choice_with_default(
                    "tooltip-position",
                    (
                        appearance.tooltip_position,
                        studyapp_core::TooltipPosition::default(),
                    ),
                    vec![
                        (
                            studyapp_core::TooltipPosition::FollowPointer,
                            "跟随鼠标".into(),
                        ),
                        (
                            studyapp_core::TooltipPosition::BesideControl,
                            "固定在控件旁".into(),
                        ),
                    ],
                    busy,
                    cx.weak_entity(),
                    |this, value, _, cx| {
                        if let Some(draft) = &mut this.preferences_ui.draft {
                            draft.appearance.notifications.tooltip_position = value;
                        }
                        cx.notify();
                    },
                ),
            ));
        }
        if let Some((_, background, border)) = COLORS.iter().find(|(kind, _, _)| *kind == style) {
            for (key, label) in [
                (*background, "此组件背景（自动 / RGBA）"),
                (*border, "此组件边框（自动 / RGBA）"),
            ] {
                if let Some(color) = self.preferences_ui.appearance_colors.get(key) {
                    let is_background = key == *background;
                    let overridden = appearance.surfaces.contains_key(&style)
                        || appearance.surface_patches.get(&style).is_some_and(|patch| {
                            if is_background {
                                patch.background.is_some()
                            } else {
                                patch.border_color.is_some()
                            }
                        });
                    let value = if is_background {
                        surface.background.clone()
                    } else {
                        surface.border_color.clone()
                    };
                    let effective = value.as_deref().unwrap_or("主题自动");
                    color.update(cx, |color, cx| {
                        let palette = crate::theme::palette();
                        color.configure(
                            if is_background {
                                palette.surface_raised
                            } else {
                                palette.border
                            },
                            busy,
                            cx,
                        );
                        color.automatic_label("主题自动", cx);
                    });
                    let content = column()
                        .gap_1()
                        .child(muted(if overridden {
                            format!("自定义 · 当前有效值 {effective}")
                        } else {
                            format!("继承全局／组件默认 · 当前有效值 {effective}")
                        }))
                        .children(overridden.then(|| color.clone()))
                        .child(
                            Button::new(SharedString::from(format!("{key}-inherit")))
                                .label(if overridden {
                                    "恢复继承"
                                } else {
                                    "自定义"
                                })
                                .disabled(busy)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    let effective =
                                        if let Some(draft) = &mut this.preferences_ui.draft {
                                            let appearance = &mut draft.appearance.notifications;
                                            let value = if is_background {
                                                appearance.surface(style).background
                                            } else {
                                                appearance.surface(style).border_color
                                            };
                                            let patch = appearance.surface_patch_mut(style);
                                            let field = if is_background {
                                                &mut patch.background
                                            } else {
                                                &mut patch.border_color
                                            };
                                            *field = if overridden {
                                                None
                                            } else {
                                                Some(value.unwrap_or_default())
                                            };
                                            let next = appearance.surface(style);
                                            if is_background {
                                                next.background
                                            } else {
                                                next.border_color
                                            }
                                        } else {
                                            None
                                        };
                                    if let Some(input) =
                                        this.preferences_ui.notification_fields.get(key)
                                    {
                                        input.update(cx, |input, cx| {
                                            input.set_value(
                                                effective.unwrap_or_default(),
                                                window,
                                                cx,
                                            )
                                        });
                                    }
                                    cx.notify();
                                })),
                        );
                    view = view.child(form::field(label, content));
                }
            }
        }
        view.child(
            Button::new("surface-reset")
                .label("恢复此组件默认样式")
                .disabled(busy)
                .on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(draft) = &mut this.preferences_ui.draft {
                        draft.appearance.notifications.surfaces.remove(&style);
                        draft
                            .appearance
                            .notifications
                            .surface_patches
                            .remove(&style);
                    }
                    let value = this
                        .preferences_ui
                        .draft
                        .as_ref()
                        .map(|d| d.appearance.notifications.clone())
                        .unwrap_or_default();
                    this.fill_surface_fields(&value, Some(style), window, cx);
                    cx.notify();
                })),
        )
    }
    pub(crate) fn notice_content_controls(&self, cx: &Context<Self>) -> Div {
        let style = self
            .preferences_ui
            .notice_component
            .unwrap_or(NoticeStyle::Bubble);
        let content = self
            .preferences_ui
            .draft
            .as_ref()
            .map(|d| d.appearance.notifications.component_content(style))
            .unwrap_or_default();
        let busy = self.preferences_ui.busy || self.preferences_ui.replacing || self.storage_busy;
        let mut controls = row().flex_wrap().gap_3();
        for field in style.configurable_fields().iter().copied() {
            let checked = field.get(&content);
            controls = controls.child(
                Checkbox::new(SharedString::from(format!(
                    "notice-field-{style:?}-{field:?}"
                )))
                .relative()
                .child(studyapp_ui::settings_anchor::probe(field.label()))
                .label(field.label())
                .checked(checked)
                .disabled(busy)
                .on_click(cx.listener(move |this, value, _, cx| {
                    if let Some(draft) = &mut this.preferences_ui.draft {
                        let appearance = &mut draft.appearance.notifications;
                        let defaults = appearance.component_content(style);
                        let next = appearance.component_fields.entry(style).or_insert(defaults);
                        field.set(next, *value);
                        if !next.title && !next.summary {
                            next.summary = true;
                        }
                    }
                    cx.notify();
                })),
            );
        }
        let mut presets = row().flex_wrap();
        for (id, label, detailed) in [
            ("brief", "简略预设", false),
            ("detailed", "详细预设", true),
            ("inherit", "恢复默认字段", false),
        ] {
            presets = presets.child(
                Button::new(SharedString::from(format!("notice-fields-{id}")))
                    .label(label)
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(draft) = &mut this.preferences_ui.draft {
                            if id == "inherit" {
                                draft
                                    .appearance
                                    .notifications
                                    .component_fields
                                    .remove(&style);
                            } else {
                                draft.appearance.notifications.component_fields.insert(
                                    style,
                                    if detailed {
                                        NoticeContent::detailed()
                                    } else {
                                        NoticeContent::brief()
                                    },
                                );
                            }
                        }
                        cx.notify();
                    })),
            );
        }
        form::section("组件默认内容").child(muted("仅列出此组件真正可隐藏的字段。风险说明和必要操作不会被隐藏；任务、菜单和表单由各自的真实业务提供内容。"))
            .child(form::checkbox_group("此组件可选字段", controls)).child(presets)
    }
}

impl StudyApp {
    pub(crate) fn hover_content_controls(&self, cx: &Context<Self>) -> Div {
        let content = self
            .preferences_ui
            .draft
            .as_ref()
            .map(|d| d.appearance.notifications.hover_content)
            .unwrap_or_default();
        let busy = self.preferences_ui.busy || self.preferences_ui.replacing || self.storage_busy;
        let mut view=form::section("悬停提示内容")
            .child(muted("笔记、学习记录与复习卡片的元数据可分别开关。外观使用“Tooltip / 控件说明”的独立样式；错误详情和普通控件帮助不隐藏。"));
        for (id, label, detailed) in [
            ("hover-brief", "简略", false),
            ("hover-detailed", "详细", true),
        ] {
            view = view.child(
                Button::new(id)
                    .label(label)
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(draft) = &mut this.preferences_ui.draft {
                            draft.appearance.notifications.hover_content = if detailed {
                                Default::default()
                            } else {
                                studyapp_core::HoverContent::brief()
                            };
                        }
                        cx.notify();
                    })),
            );
        }
        type Field = (
            &'static str,
            &'static str,
            fn(&mut studyapp_core::HoverContent) -> &mut bool,
        );
        let fields: [Field; 5] = [
            ("hover-source", "笔记来源", |s| &mut s.source),
            ("hover-status", "记录状态", |s| &mut s.status),
            ("hover-learned", "学习时间", |s| &mut s.learned),
            ("hover-reviewed", "上次复习", |s| &mut s.reviewed),
            ("hover-next-review", "下次复习", |s| &mut s.next_review),
        ];
        let mut controls = row().flex_wrap().gap_3();
        for (id, label, field) in fields {
            let mut displayed = content;
            controls = controls.child(
                Checkbox::new(id)
                    .relative()
                    .child(studyapp_ui::settings_anchor::probe(label))
                    .label(label)
                    .checked(*field(&mut displayed))
                    .disabled(busy)
                    .on_click(cx.listener(move |this, value, _, cx| {
                        if let Some(draft) = &mut this.preferences_ui.draft {
                            let next = &mut draft.appearance.notifications.hover_content;
                            *field(next) = *value;
                            if !next.source
                                && !next.status
                                && !next.learned
                                && !next.reviewed
                                && !next.next_review
                            {
                                next.next_review = true;
                            }
                        }
                        cx.notify();
                    })),
            );
        }
        view.child(form::checkbox_group("悬停元数据字段", controls))
    }
}

impl StudyApp {
    pub(crate) fn notice_region_controls(&self, cx: &mut Context<Self>) -> Div {
        let appearance = self
            .preferences_ui
            .draft
            .as_ref()
            .map(|d| d.appearance.notifications.clone())
            .unwrap_or_default();
        let busy = self.preferences_ui.busy || self.preferences_ui.replacing || self.storage_busy;
        let mut view = form::section("分区与堆叠").child(muted("各用途区域独立排队；空间不足时等待，互不遮住操作入口。确认警告优先；主动菜单、详情、任务抽屉不进入通知队列。"));
        for region in studyapp_core::NoticeRegion::ALL {
            let settings = appearance.region_settings(region);
            view = view.child(
                column()
                    .relative()
                    .child(studyapp_ui::settings_anchor::probe(region.label()))
                    .gap_2()
                    .py_2()
                    .child(heading(region.label()))
                    .child(form::settings_row(
                        "排列",
                        form::choice_with_default(
                            SharedString::from(format!("region-stack-{region:?}")),
                            (
                                settings.stack,
                                NotificationAppearance::default()
                                    .region_settings(region)
                                    .stack,
                            ),
                            studyapp_core::NoticeStack::ALL
                                .into_iter()
                                .map(|value| (value, value.label().into()))
                                .collect(),
                            busy,
                            cx.weak_entity(),
                            move |this, stack, _, cx| {
                                if let Some(draft) = &mut this.preferences_ui.draft {
                                    let defaults =
                                        draft.appearance.notifications.region_settings(region);
                                    draft
                                        .appearance
                                        .notifications
                                        .regions
                                        .entry(region)
                                        .or_insert(defaults)
                                        .stack = stack;
                                }
                                cx.notify();
                            },
                        ),
                    ))
                    .child(
                        self.setting_number(
                            format!("region-count-{region:?}"),
                            "可见数量",
                            "条",
                            (
                                settings.max_visible.into(),
                                NotificationAppearance::default()
                                    .region_settings(region)
                                    .max_visible
                                    .into(),
                                1,
                                4,
                                busy,
                            ),
                            move |this, count| {
                                if let Some(draft) = &mut this.preferences_ui.draft {
                                    let defaults =
                                        draft.appearance.notifications.region_settings(region);
                                    draft
                                        .appearance
                                        .notifications
                                        .regions
                                        .entry(region)
                                        .or_insert(defaults)
                                        .max_visible = count as u8;
                                }
                            },
                            cx,
                        ),
                    )
                    .child(
                        Button::new(SharedString::from(format!("region-reset-{region:?}")))
                            .label("恢复此区域默认")
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(draft) = &mut this.preferences_ui.draft {
                                    draft.appearance.notifications.regions.remove(&region);
                                }
                                cx.notify();
                            })),
                    ),
            );
        }
        view
    }
}

impl StudyApp {
    pub(crate) fn notice_scenario_fields(
        &self,
        category: studyapp_core::NoticeCategory,
        cx: &Context<Self>,
    ) -> Div {
        let appearance = self
            .preferences_ui
            .draft
            .as_ref()
            .map(|d| d.appearance.notifications.clone())
            .unwrap_or_default();
        let style = self
            .preferences_ui
            .draft
            .as_ref()
            .and_then(|d| d.notifications.rule(category).style)
            .unwrap_or_else(|| category.default_style(studyapp_core::NoticeSeverity::Information));
        let defaults = appearance.component_content(style);
        let mut patch = appearance
            .scenario_fields
            .get(&category)
            .cloned()
            .unwrap_or_default();
        let enabled = self
            .preferences_ui
            .draft
            .as_ref()
            .is_none_or(|draft| draft.notifications.rule(category).enabled);
        let busy = self.preferences_ui.busy
            || self.preferences_ui.replacing
            || self.storage_busy
            || !enabled;
        let mut view = column().gap_2().child(muted(format!(
            "场景字段 · 未覆盖时持续跟随{}的默认内容",
            style.label()
        )));
        if !enabled {
            view = view.child(muted("需启用此通知场景后才能调整；当前字段草稿仍保留。"));
        }
        let mut presets = row().flex_wrap().gap_2();
        for (id, label, preset) in [
            ("brief", "简略预设", Some(false)),
            ("detailed", "详细预设", Some(true)),
            ("inherit", "全部恢复继承", None),
        ] {
            presets = presets.child(
                Button::new(SharedString::from(format!(
                    "scenario-preset-{category:?}-{id}"
                )))
                .label(label)
                .disabled(busy)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(draft) = &mut this.preferences_ui.draft {
                        if let Some(detailed) = preset {
                            let content = if detailed {
                                NoticeContent::detailed()
                            } else {
                                NoticeContent::brief()
                            };
                            let patch = draft
                                .appearance
                                .notifications
                                .scenario_fields
                                .entry(category)
                                .or_default();
                            for field in style.configurable_fields().iter().copied() {
                                *field.patch(patch) = Some(field.get(&content));
                            }
                        } else {
                            draft
                                .appearance
                                .notifications
                                .scenario_fields
                                .remove(&category);
                        }
                    }
                    cx.notify();
                })),
            );
        }
        view = view.child(presets);
        for field in style.configurable_fields().iter().copied() {
            let stored = *field.patch(&mut patch);
            let checked = stored.unwrap_or_else(|| field.get(&defaults));
            let locked = !category
                .notification_styles()
                .contains(&NoticeStyle::Bubble)
                && matches!(
                    field,
                    studyapp_core::NoticeField::Summary
                        | studyapp_core::NoticeField::Action
                        | studyapp_core::NoticeField::Title
                );
            let control = if stored.is_some() {
                row().flex_wrap().child(
                    Checkbox::new(SharedString::from(format!(
                        "scenario-{category:?}-{field:?}"
                    )))
                    .label(field.label())
                    .checked(checked || locked)
                    .disabled(busy || locked)
                    .on_click(cx.listener(move |this, value, _, cx| {
                        if let Some(draft) = &mut this.preferences_ui.draft {
                            *field.patch(
                                draft
                                    .appearance
                                    .notifications
                                    .scenario_fields
                                    .entry(category)
                                    .or_default(),
                            ) = Some(*value);
                        }
                        cx.notify();
                    })),
                )
            } else {
                row().child(muted(format!(
                    "{}：继承组件 · {}",
                    field.label(),
                    if checked || locked {
                        "显示"
                    } else {
                        "隐藏"
                    }
                )))
            };
            view = view.child(
                control
                    .relative()
                    .child(studyapp_ui::settings_anchor::probe(format!(
                        "scenario-{category:?}-{field:?}"
                    )))
                    .child(
                        Button::new(SharedString::from(format!(
                            "scenario-inherit-{category:?}-{field:?}"
                        )))
                        .label(if stored.is_some() {
                            "恢复继承"
                        } else {
                            "自定义"
                        })
                        .disabled(busy || locked)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(draft) = &mut this.preferences_ui.draft {
                                *field.patch(
                                    draft
                                        .appearance
                                        .notifications
                                        .scenario_fields
                                        .entry(category)
                                        .or_default(),
                                ) = if stored.is_some() {
                                    None
                                } else {
                                    Some(checked)
                                };
                            }
                            cx.notify();
                        })),
                    ),
            );
        }
        view
    }
}
