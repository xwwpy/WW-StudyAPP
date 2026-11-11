//! Global list presentation settings, separate from theme preset colors.
use crate::{app::StudyApp, components::*, controls::Button, settings_components as form};
use gpui_kit::{
    component::{Disableable, Sizable, button::ButtonVariants},
    *,
};
use studyapp_core::{LIST_LEVELS, ListMarker, ListPreset, ListStyle};

impl StudyApp {
    pub(crate) fn list_style_settings(&self, cx: &Context<Self>) -> Div {
        let style = self
            .preferences_ui
            .draft
            .as_ref()
            .map(|s| s.appearance.list_style)
            .unwrap_or_default();
        let busy = self.preferences_ui.busy || self.preferences_ui.replacing || self.storage_busy;
        let pages = LIST_LEVELS.div_ceil(4);
        let page = self.preferences_ui.list_levels_page.min(pages - 1);
        form::section("多级无序列表")
            .child(muted(
                "按实际列表嵌套层级显示；有序列表和任务框不变。第九级起重复这八级，不修改源文。",
            ))
            .child(form::settings_row(
                "标记预设",
                column()
                    .gap_2()
                    .child(form::choice(
                        "list-preset",
                        style.preset,
                        ListPreset::ALL
                            .into_iter()
                            .map(|preset| (preset, preset.label().into()))
                            .collect(),
                        busy,
                        cx.weak_entity(),
                        move |this, preset, window, cx| {
                            if this.preferences_ui.busy
                                || this.preferences_ui.replacing
                                || this.storage_busy
                            {
                                return;
                            }
                            if let Some(draft) = &mut this.preferences_ui.draft {
                                draft.appearance.list_style = ListStyle {
                                    preset,
                                    indent_subtree: style.indent_subtree,
                                    ..Default::default()
                                };
                            }
                            this.refresh_appearance(window, cx);
                            cx.notify();
                        },
                    ))
                    .child(
                        Button::new("list-preset-reset")
                            .small()
                            .ghost()
                            .label("恢复默认标记")
                            .disabled(
                                busy || (style.preset == ListPreset::default()
                                    && style.levels.iter().all(Option::is_none)),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if let Some(draft) = &mut this.preferences_ui.draft {
                                    draft.appearance.list_style = ListStyle {
                                        indent_subtree: style.indent_subtree,
                                        ..Default::default()
                                    };
                                }
                                this.refresh_appearance(window, cx);
                                cx.notify();
                            })),
                    ),
            ))
            .child(muted(
                "选择预设会替换逐级自定义。每级可单独选择标记，或恢复沿用预设。",
            ))
            .children((page * 4..((page + 1) * 4).min(LIST_LEVELS)).map(|level| {
                let marker = style.marker(level + 1);
                form::settings_row(
                    format!("第 {} 级", level + 1),
                    form::inherited_choice(
                        ("list-level", level),
                        style.levels[level],
                        (
                            marker,
                            format!(
                                "继承预设 {} · 当前有效值：{} {}",
                                style.preset.label(),
                                marker.symbol(),
                                marker.label()
                            ),
                        ),
                        ListMarker::ALL
                            .into_iter()
                            .map(|value| (value, format!("{} {}", value.symbol(), value.label())))
                            .collect(),
                        busy,
                        cx.weak_entity(),
                        move |this, value, window, cx| {
                            if this.preferences_ui.busy
                                || this.preferences_ui.replacing
                                || this.storage_busy
                            {
                                return;
                            }
                            if let Some(draft) = &mut this.preferences_ui.draft {
                                draft.appearance.list_style.levels[level] = value;
                            }
                            this.refresh_appearance(window, cx);
                            cx.notify();
                        },
                    ),
                )
            }))
            .child(
                row()
                    .flex_wrap()
                    .child(
                        Button::new("list-levels-previous")
                            .small()
                            .ghost()
                            .label("上一页")
                            .disabled(busy || page == 0)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.preferences_ui.list_levels_page = page.saturating_sub(1);
                                cx.notify();
                            })),
                    )
                    .child(muted(format!(
                        "第 {} / {} 页 · 共 {} 级",
                        page + 1,
                        pages,
                        LIST_LEVELS
                    )))
                    .child(
                        Button::new("list-levels-next")
                            .small()
                            .ghost()
                            .label("下一页")
                            .disabled(busy || page + 1 >= pages)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.preferences_ui.list_levels_page = (page + 1).min(pages - 1);
                                cx.notify();
                            })),
                    ),
            )
            .child(muted(
                "适用于所有主题、正文和原生摘要；即时预览，保存全部设置后写入资料库。",
            ))
    }
}
