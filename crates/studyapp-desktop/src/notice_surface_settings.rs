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
use studyapp_core::{NoticeContent, NoticeStyle, NoticeSurface, NotificationAppearance};
const COLORS: [(NoticeStyle, &str, &str); 8] = [
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
];
impl StudyApp {
    pub(crate) fn fill_surface_fields(
        &mut self,
        appearance: &NotificationAppearance,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for (style, background, border) in COLORS {
            let surface = appearance.surfaces.get(&style).cloned().unwrap_or_default();
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
                                let surface = draft.appearance.notifications.surface_mut(style);
                                let field = if is_background {
                                    &mut surface.background
                                } else {
                                    &mut surface.border_color
                                };
                                *field = (!raw.is_empty()).then(|| raw.to_string());
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
        set: fn(&mut NoticeSurface, u16),
        cx: &Context<Self>,
    ) -> Div {
        let mut values = values.to_vec();
        values.push(value);
        values.sort();
        values.dedup();
        let style = self
            .preferences_ui
            .draft
            .as_ref()
            .map(|d| d.appearance.notifications.style)
            .unwrap_or(NoticeStyle::Bubble);
        let busy = self.preferences_ui.busy || self.preferences_ui.replacing || self.storage_busy;
        form::settings_row(
            label.to_owned(),
            form::choice(
                key,
                value,
                values.into_iter().map(|v| (v, v.to_string())).collect(),
                busy,
                cx.weak_entity(),
                move |this, value, _, cx| {
                    if this.preferences_ui.busy
                        || this.preferences_ui.replacing
                        || this.storage_busy
                    {
                        return;
                    }
                    if let Some(draft) = &mut this.preferences_ui.draft {
                        set(draft.appearance.notifications.surface_mut(style), value);
                    }
                    cx.notify();
                },
            ),
        )
    }
    pub(crate) fn notice_surface_controls(&self, cx: &mut Context<Self>) -> Div {
        let appearance = self
            .preferences_ui
            .draft
            .as_ref()
            .map(|d| d.appearance.notifications.clone())
            .unwrap_or_default();
        let style = appearance.style;
        let surface = appearance.surface(style);
        let busy = self.preferences_ui.busy || self.preferences_ui.replacing || self.storage_busy;
        let mut view = form::section(format!("独立组件样式 · {}", style.label()))
            .child(muted(
                "选择上方样式以编辑对应组件；每种组件独立保存，颜色留空时跟随主题。",
            ))
            .child(self.surface_number(
                "不透明度（%）",
                "surface-opacity",
                surface.opacity.into(),
                &[20, 40, 60, 80, 90, 100],
                |s, v| s.opacity = v as u8,
                cx,
            ))
            .child(self.surface_number(
                "边框宽度（px）",
                "surface-border",
                surface.border_width.into(),
                &[0, 1, 2, 3, 4],
                |s, v| s.border_width = v as u8,
                cx,
            ))
            .child(self.surface_number(
                "宽度（px）",
                "surface-width",
                surface.width,
                &[200, 240, 280, 320, 360, 420, 480, 560, 640],
                |s, v| s.width = v,
                cx,
            ))
            .child(self.surface_number(
                "内容最大高度（px）",
                "surface-height",
                surface.max_height,
                &[48, 80, 120, 164, 240, 320, 480],
                |s, v| s.max_height = v,
                cx,
            ))
            .child(self.surface_number(
                "圆角（px）",
                "surface-radius",
                surface.radius.into(),
                &[0, 4, 8, 12, 16, 24, 32],
                |s, v| s.radius = v as u8,
                cx,
            ));
        if let Some((_, background, border)) = COLORS.iter().find(|(kind, _, _)| *kind == style) {
            for (key, label) in [
                (*background, "此组件背景（自动 / RGBA）"),
                (*border, "此组件边框（自动 / RGBA）"),
            ] {
                if let Some(color) = self.preferences_ui.appearance_colors.get(key) {
                    color.update(cx, |color, cx| {
                        color.configure(crate::theme::palette().accent, busy, cx)
                    });
                    view = view.child(form::field(label, color.clone()));
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
                    }
                    let value = this
                        .preferences_ui
                        .draft
                        .as_ref()
                        .map(|d| d.appearance.notifications.clone())
                        .unwrap_or_default();
                    this.fill_surface_fields(&value, window, cx);
                    cx.notify();
                })),
        )
    }
    pub(crate) fn notice_content_controls(&self, cx: &Context<Self>) -> Div {
        let content = self
            .preferences_ui
            .draft
            .as_ref()
            .map(|d| d.appearance.notifications.content.clone())
            .unwrap_or_default();
        let busy = self.preferences_ui.busy || self.preferences_ui.replacing || self.storage_busy;
        let mut view = form::section("提示内容").child(
            row()
                .child(
                    Button::new("notice-content-brief")
                        .label("简略预设")
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(draft) = &mut this.preferences_ui.draft {
                                draft.appearance.notifications.content = NoticeContent::brief();
                            }
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("notice-content-detailed")
                        .label("详细预设")
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(draft) = &mut this.preferences_ui.draft {
                                draft.appearance.notifications.content = NoticeContent::detailed();
                            }
                            cx.notify();
                        })),
                ),
        );
        type Field = (
            &'static str,
            &'static str,
            fn(&mut NoticeContent) -> &mut bool,
        );
        let fields: [Field; 7] = [
            ("notice-show-title", "标题", |s| &mut s.title),
            ("notice-show-summary", "提示正文", |s| &mut s.summary),
            ("notice-show-icon", "状态图标", |s| &mut s.icon),
            ("notice-show-repetitions", "重复次数", |s| {
                &mut s.repetitions
            }),
            ("notice-show-event-id", "事件标识", |s| &mut s.event_id),
            ("notice-show-action", "操作入口", |s| &mut s.action),
            (
                "notice-show-details",
                "完整正文（关闭时截短）",
                |s| &mut s.detailed,
            ),
        ];
        for (key, label, field) in fields {
            let mut displayed = content.clone();
            let checked = *field(&mut displayed);
            view = view.child(
                Checkbox::new(key)
                    .label(label)
                    .checked(checked)
                    .disabled(busy)
                    .on_click(cx.listener(move |this, value, _, cx| {
                        if let Some(draft) = &mut this.preferences_ui.draft {
                            let next = &mut draft.appearance.notifications.content;
                            *field(next) = *value;
                            if !next.title && !next.summary {
                                next.title = true;
                            }
                        }
                        cx.notify();
                    })),
            );
        }
        view.child(muted(
            "错误和严重错误始终保留标题、完整正文和操作。后台隐私模式仍会隐藏正文细节和事件标识。",
        ))
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
            .child(muted("笔记、学习记录与复习卡片的元数据可分别开关。外观沿用“控件定位气泡 / Popover”的独立样式；错误详情和普通控件帮助不隐藏。"));
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
        for (id, label, field) in fields {
            let mut displayed = content;
            view = view.child(
                Checkbox::new(id)
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
        view
    }
}
