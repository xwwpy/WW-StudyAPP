use crate::{app::StudyApp, components::*, controls::Button, settings_components as form};
use gpui_kit::{
    component::{Disableable, Selectable, input::InputState},
    prelude::FluentBuilder,
    *,
};
use studyapp_core::{SelectionAppearance, SelectionCategory, SelectionOverride};

type ColorField = (
    &'static str,
    &'static str,
    fn(&mut SelectionAppearance) -> &mut Option<String>,
    fn(&mut SelectionOverride) -> &mut Option<String>,
);
const FIELDS: [ColorField; 5] = [
    (
        "selection-background",
        "选中背景",
        |s| &mut s.selected_background,
        |s| &mut s.selected_background,
    ),
    (
        "selection-text",
        "选中文字",
        |s| &mut s.selected_text,
        |s| &mut s.selected_text,
    ),
    (
        "selection-border",
        "选中边框与标记",
        |s| &mut s.selected_border,
        |s| &mut s.selected_border,
    ),
    (
        "selection-idle-background",
        "未选中背景",
        |s| &mut s.idle_background,
        |s| &mut s.idle_background,
    ),
    (
        "selection-idle-text",
        "未选中文字",
        |s| &mut s.idle_text,
        |s| &mut s.idle_text,
    ),
];
impl StudyApp {
    pub(crate) fn fill_selection_fields(
        &mut self,
        value: &SelectionAppearance,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Each category owns its raw text as well as its typed patch. Switching
        // the editor target never rewrites another category's invalid draft.
        for target in std::iter::once(None).chain(SelectionCategory::ALL.into_iter().map(Some)) {
            for (base_key, _, field, override_field) in FIELDS {
                let key = selection_color_key(base_key, target);
                let text = target
                    .map_or_else(
                        || field(&mut value.clone()).clone(),
                        |category| {
                            override_field(
                                &mut value.categories.get(&category).cloned().unwrap_or_default(),
                            )
                            .clone()
                        },
                    )
                    .unwrap_or_default();
                if let Some(input) = self.preferences_ui.appearance_fields.get(key) {
                    input.update(cx, |input, cx| input.set_value(text, window, cx));
                } else {
                    let input = cx.new(|cx| InputState::new(window, cx).default_value(text));
                    self.preferences_ui
                        .appearance_fields
                        .insert(key, input.clone());
                    let color = cx.new(|cx| {
                        crate::settings_color::ColorSetting::new(key, input, true, window, cx)
                    });
                    cx.subscribe(
                        &color,
                        move |this, color, _: &crate::settings_color::ColorSettingChanged, cx| {
                            let raw = color.read(cx).preview_value(cx);
                            if (raw.is_empty() || studyapp_core::parse_rgba_color(&raw).is_some())
                                && let Some(draft) = &mut this.preferences_ui.draft
                            {
                                let value = (!raw.is_empty()).then(|| raw.to_string());
                                let style = &mut draft.appearance.selection;
                                if let Some(category) = target {
                                    *override_field(
                                        style.categories.entry(category).or_default(),
                                    ) = value;
                                } else {
                                    *field(style) = value;
                                }
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
    pub(crate) fn selection_settings(&self, cx: &mut Context<Self>) -> Div {
        let global = self
            .preferences_ui
            .draft
            .as_ref()
            .map(|d| d.appearance.selection.clone())
            .unwrap_or_default();
        let target = self.preferences_ui.selection_target;
        let style = target.map_or_else(|| global.clone(), |category| global.effective(category));
        let busy = self.preferences_ui.busy || self.preferences_ui.replacing || self.storage_busy;
        let category = target.unwrap_or(SelectionCategory::Options);
        let mut targets = vec![(None, "全局默认".into())];
        targets.extend(
            SelectionCategory::ALL
                .into_iter()
                .map(|c| (Some(c), c.label().into())),
        );
        let mut view = form::section("选中项与未选中项")
            .child(form::settings_row("编辑范围", form::choice("selection-target", target, targets, busy, cx.weak_entity(), |this, value, window, cx| {
                this.preferences_ui.selection_target = value;
                let _ = window;
                cx.notify();
            })))
            .child(muted("类别未覆盖的属性持续跟随全局；改一个属性不会冻结其它属性。悬停、键盘焦点与正文文本选区保持独立。"))
            .child(row().child(Button::new("selection-preview-on").label("选中项").selection_category(category).map(|button| if target.is_none() { button.global_selection() } else { button }).selected(true))
                .child(Button::new("selection-preview-off").label("未选中项").selection_category(category).map(|button| if target.is_none() { button.global_selection() } else { button }).selected(false)));
        if let Some(category) = target {
            let patch = global
                .categories
                .get(&category)
                .cloned()
                .unwrap_or_default();
            view = view.child(studyapp_ui::settings_switch::inherited_switch(
                "selection-indicator-override",
                "选中标记",
                patch.indicator,
                (
                    global.indicator,
                    format!(
                        "继承全局 · 当前有效值：{}",
                        if global.indicator { "显示" } else { "隐藏" }
                    ),
                ),
                busy,
                cx.weak_entity(),
                move |this, value, window, cx| {
                    if let Some(draft) = &mut this.preferences_ui.draft {
                        draft
                            .appearance
                            .selection
                            .categories
                            .entry(category)
                            .or_default()
                            .indicator = value;
                    }
                    this.refresh_appearance(window, cx);
                    cx.notify();
                },
            ));
            if let Some(radius) = patch.radius {
                view = view.child(self.setting_number(
                    format!("selection-radius-{category:?}"),
                    "圆角",
                    "px",
                    (
                        radius.into(),
                        SelectionAppearance::default().radius.into(),
                        0,
                        24,
                        busy,
                    ),
                    move |this, value| {
                        if let Some(draft) = &mut this.preferences_ui.draft {
                            draft
                                .appearance
                                .selection
                                .categories
                                .entry(category)
                                .or_default()
                                .radius = Some(value as u8);
                        }
                    },
                    cx,
                ));
            } else {
                view = view.child(muted(format!(
                    "圆角：继承全局 · 当前有效值 {} px",
                    global.radius
                )));
            }
            let has_radius = patch.radius.is_some();
            let effective_radius = style.radius;
            view = view.child(
                Button::new("selection-radius-inheritance")
                    .label(if patch.radius.is_some() {
                        "恢复继承"
                    } else {
                        "自定义圆角"
                    })
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(draft) = &mut this.preferences_ui.draft {
                            draft
                                .appearance
                                .selection
                                .categories
                                .entry(category)
                                .or_default()
                                .radius = if has_radius {
                                None
                            } else {
                                Some(effective_radius)
                            };
                        }
                        this.preferences_ui
                            .number_fields
                            .borrow_mut()
                            .remove(&format!("selection-radius-{category:?}"));
                        cx.notify();
                    })),
            );
        } else {
            view = view
                .child(
                    studyapp_ui::settings_switch::SettingSwitch::new("selection-indicator")
                        .default_value(true)
                        .label("显示选中标记")
                        .checked(style.indicator)
                        .disabled(busy)
                        .on_click(cx.listener(|this, value, window, cx| {
                            if let Some(draft) = &mut this.preferences_ui.draft {
                                draft.appearance.selection.indicator = *value;
                            }
                            this.refresh_appearance(window, cx);
                            cx.notify();
                        })),
                )
                .child(self.setting_number(
                    "selection-radius".into(),
                    "选中项圆角",
                    "px",
                    (
                        style.radius.into(),
                        SelectionAppearance::default().radius.into(),
                        0,
                        24,
                        busy,
                    ),
                    |this, value| {
                        if let Some(draft) = &mut this.preferences_ui.draft {
                            draft.appearance.selection.radius = value as u8;
                        }
                    },
                    cx,
                ));
        }
        let palette = crate::theme::palette();
        for (base_key, label, global_field, override_field) in FIELDS {
            let key = selection_color_key(base_key, target);
            if let Some(color) = self.preferences_ui.appearance_colors.get(key) {
                let inherited = match base_key {
                    "selection-background" => palette.selected_background,
                    "selection-text" => palette.selected_text,
                    "selection-border" => palette.selected_border,
                    "selection-idle-background" => palette.idle_background,
                    _ => palette.idle_text,
                };
                color.update(cx, |color, cx| color.configure(inherited, busy, cx));
                let inherited = target.is_some_and(|category| {
                    override_field(
                        &mut global
                            .categories
                            .get(&category)
                            .cloned()
                            .unwrap_or_default(),
                    )
                    .is_none()
                });
                let effective = global_field(&mut style.clone())
                    .clone()
                    .unwrap_or_else(|| "当前主题".into());
                let mut content = column().gap_1();
                if inherited {
                    content = content.child(muted(format!("继承全局 · 当前有效值 {effective}")));
                } else {
                    content = content.child(color.clone());
                }
                if let Some(category) = target {
                    content = content.child(
                        Button::new(SharedString::from(format!("{key}-inherit")))
                            .label(if inherited {
                                "自定义"
                            } else {
                                "恢复继承"
                            })
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let raw = if inherited {
                                    let palette = crate::theme::palette();
                                    let effective = this
                                        .preferences_ui
                                        .draft
                                        .as_ref()
                                        .map(|d| d.appearance.selection.effective(category))
                                        .unwrap_or_default();
                                    global_field(&mut effective.clone()).clone().unwrap_or_else(
                                        || {
                                            format!(
                                                "#{:06X}",
                                                match base_key {
                                                    "selection-background" =>
                                                        palette.selected_background,
                                                    "selection-text" => palette.selected_text,
                                                    "selection-border" => palette.selected_border,
                                                    "selection-idle-background" =>
                                                        palette.idle_background,
                                                    _ => palette.idle_text,
                                                }
                                            )
                                        },
                                    )
                                } else {
                                    String::new()
                                };
                                if let Some(draft) = &mut this.preferences_ui.draft {
                                    *override_field(
                                        draft
                                            .appearance
                                            .selection
                                            .categories
                                            .entry(category)
                                            .or_default(),
                                    ) = (!raw.is_empty()).then(|| raw.clone());
                                }
                                if let Some(input) = this.preferences_ui.appearance_fields.get(key)
                                {
                                    input.update(cx, |input, cx| input.set_value(raw, window, cx));
                                }
                                cx.notify();
                            })),
                    );
                }
                view = view.child(form::settings_row(label, content));
            }
        }
        view
    }
}

fn selection_color_key(key: &'static str, target: Option<SelectionCategory>) -> &'static str {
    match (target, key) {
        (Some(SelectionCategory::Navigation), "selection-background") => {
            "selection-navigation-background"
        }
        (Some(SelectionCategory::Navigation), "selection-text") => "selection-navigation-text",
        (Some(SelectionCategory::Navigation), "selection-border") => "selection-navigation-border",
        (Some(SelectionCategory::Navigation), "selection-idle-background") => {
            "selection-navigation-idle-background"
        }
        (Some(SelectionCategory::Navigation), "selection-idle-text") => {
            "selection-navigation-idle-text"
        }
        (Some(SelectionCategory::Options), "selection-background") => {
            "selection-options-background"
        }
        (Some(SelectionCategory::Options), "selection-text") => "selection-options-text",
        (Some(SelectionCategory::Options), "selection-border") => "selection-options-border",
        (Some(SelectionCategory::Options), "selection-idle-background") => {
            "selection-options-idle-background"
        }
        (Some(SelectionCategory::Options), "selection-idle-text") => "selection-options-idle-text",
        (Some(SelectionCategory::Library), "selection-background") => {
            "selection-library-background"
        }
        (Some(SelectionCategory::Library), "selection-text") => "selection-library-text",
        (Some(SelectionCategory::Library), "selection-border") => "selection-library-border",
        (Some(SelectionCategory::Library), "selection-idle-background") => {
            "selection-library-idle-background"
        }
        (Some(SelectionCategory::Library), "selection-idle-text") => "selection-library-idle-text",
        (Some(SelectionCategory::Directory), "selection-background") => {
            "selection-directory-background"
        }
        (Some(SelectionCategory::Directory), "selection-text") => "selection-directory-text",
        (Some(SelectionCategory::Directory), "selection-border") => "selection-directory-border",
        (Some(SelectionCategory::Directory), "selection-idle-background") => {
            "selection-directory-idle-background"
        }
        (Some(SelectionCategory::Directory), "selection-idle-text") => {
            "selection-directory-idle-text"
        }
        _ => key,
    }
}
