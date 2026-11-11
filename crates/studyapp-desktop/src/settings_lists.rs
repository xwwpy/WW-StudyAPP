//! Global list presentation settings, separate from theme preset colors.
use crate::{app::StudyApp, components::*, controls::Button, settings_components as form};
use gpui_kit::{
    component::{
        Disableable, Selectable, Sizable,
        button::ButtonVariants,
        menu::{DropdownMenu, PopupMenuItem},
    },
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
            .child(
                row()
                    .flex_wrap()
                    .children(ListPreset::ALL.into_iter().enumerate().map(|(i, preset)| {
                        let button = Button::new(("list-preset", i))
                            .small()
                            .label(preset.label())
                            .disabled(busy)
                            .tooltip("应用预设并清除逐级覆盖")
                            .on_click(cx.listener(move |this, _, window, cx| {
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
                            }));
                        button.selected(
                            style.preset == preset && style.levels.iter().all(Option::is_none),
                        )
                    })),
            )
            .child(muted(
                "选择预设会替换逐级自定义。每级可单独选择标记，或恢复沿用预设。",
            ))
            .children((page * 4..((page + 1) * 4).min(LIST_LEVELS)).map(|level| {
                let owner = cx.weak_entity();
                let marker = style.marker(level + 1);
                form::settings_row(
                    format!("第 {} 级", level + 1),
                    Button::new(("list-level", level))
                        .small()
                        .disabled(busy)
                        .label(format!(
                            "{} {}{}",
                            marker.symbol(),
                            marker.label(),
                            if style.levels[level].is_none() {
                                " · 预设"
                            } else {
                                ""
                            }
                        ))
                        .dropdown_menu(move |mut menu, _, cx| {
                            let current = owner
                                .update(cx, |this, _| {
                                    if this.preferences_ui.busy
                                        || this.preferences_ui.replacing
                                        || this.storage_busy
                                    {
                                        return None;
                                    }
                                    this.preferences_ui
                                        .draft
                                        .as_ref()
                                        .map(|s| s.appearance.list_style.levels[level])
                                })
                                .ok()
                                .flatten();
                            let Some(current) = current else {
                                return menu;
                            };
                            for value in
                                std::iter::once(None).chain(ListMarker::ALL.into_iter().map(Some))
                            {
                                let owner = owner.clone();
                                let label = value
                                    .map(|m| format!("{} {}", m.symbol(), m.label()))
                                    .unwrap_or_else(|| "沿用预设".into());
                                menu = menu.item(
                                    PopupMenuItem::new(label)
                                        .checked(current == value)
                                        .on_click(move |_, window, cx| {
                                            let _ = owner.update(cx, |this, cx| {
                                                if this.preferences_ui.busy
                                                    || this.preferences_ui.replacing
                                                    || this.storage_busy
                                                {
                                                    return;
                                                }
                                                if let Some(draft) = &mut this.preferences_ui.draft
                                                {
                                                    draft.appearance.list_style.levels[level] =
                                                        value;
                                                }
                                                this.refresh_appearance(window, cx);
                                                cx.notify();
                                            });
                                        }),
                                );
                            }
                            menu
                        }),
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
