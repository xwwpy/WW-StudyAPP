//! Settings browsing state contains no draft values or credentials.
use crate::{
    app::StudyApp,
    components::*,
    controls::Button,
    settings_ui::{AppearancePage, SettingsSection, StoragePage},
};
use gpui_kit::{
    component::{Disableable, Sizable, button::ButtonVariants},
    prelude::FluentBuilder,
    *,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use studyapp_core::{NoticeStyle, SelectionCategory};

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct SettingsBrowsing {
    pub width: u16,
    pub notification_page: usize,
    pub list_levels_page: usize,
    pub expanded: BTreeSet<String>,
    pub selected: String,
    pub offsets: BTreeMap<String, (usize, f32)>,
}
impl Default for SettingsBrowsing {
    fn default() -> Self {
        Self {
            width: 224,
            notification_page: 0,
            list_levels_page: 0,
            expanded: ["notes", "appearance", "notices"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            selected: "learning".into(),
            offsets: BTreeMap::new(),
        }
    }
}
#[derive(Clone)]
struct Target {
    id: String,
    label: String,
    group: &'static str,
}
fn targets() -> Vec<Target> {
    let mut result: Vec<_> = [
        ("learning", "学习与复习", "learning"),
        ("notes.snapshots", "快照策略", "notes"),
        ("notes.editing", "编辑行为", "notes"),
        ("appearance.overview", "主题与阅读", "appearance"),
        ("appearance.theme", "主题编辑", "appearance"),
        ("appearance.workspace", "工作区", "appearance"),
        ("appearance.selection", "选中样式", "appearance"),
        ("appearance.widget", "计时小组件", "appearance"),
        ("notices.rules", "通知规则与场景", "notices"),
        ("notices.general", "默认外观与动效", "notices"),
        ("notices.regions", "分区与堆叠", "notices"),
    ]
    .into_iter()
    .map(|(id, label, group)| Target {
        id: id.into(),
        label: label.into(),
        group,
    })
    .collect();
    result.extend(
        NoticeStyle::ALL
            .into_iter()
            .enumerate()
            .map(|(index, style)| Target {
                id: format!("notices.component.{index}"),
                label: style.label().into(),
                group: "notices",
            }),
    );
    result.extend(
        [
            ("shortcuts", "快捷键", "shortcuts"),
            ("storage.overview", "空间概览", "storage"),
            ("storage.copies", "托管副本", "storage"),
            ("storage.images", "图片存储", "storage"),
            ("storage.library", "资料库与备份", "storage"),
            ("sync", "同步", "sync"),
            ("logging", "日志", "logging"),
            ("about", "关于与账户", "about"),
        ]
        .into_iter()
        .map(|(id, label, group)| Target {
            id: id.into(),
            label: label.into(),
            group,
        }),
    );
    result
}
pub(crate) fn virtual_target(id: &str) -> bool {
    matches!(
        id,
        "appearance.overview"
            | "appearance.theme"
            | "appearance.workspace"
            | "notices.rules"
            | "shortcuts"
    )
}
pub(crate) fn target_title(id: &str) -> String {
    targets()
        .into_iter()
        .find(|target| target.id == id)
        .map(|target| target.label)
        .unwrap_or_else(|| "偏好设置".into())
}
impl StudyApp {
    pub fn select_settings_section(&mut self, section: SettingsSection, cx: &mut Context<Self>) {
        let target = match section {
            SettingsSection::Learning => "learning",
            SettingsSection::Appearance => "appearance.overview",
            SettingsSection::Notifications => "notices.rules",
            SettingsSection::Shortcuts => "shortcuts",
            SettingsSection::Storage => "storage.overview",
            SettingsSection::Sync => "sync",
            SettingsSection::Logging => "logging",
            SettingsSection::AboutAccount => "about",
        };
        self.select_settings_target(target, cx);
    }
    pub(crate) fn settings_target_title(&self) -> String {
        targets()
            .into_iter()
            .find(|target| target.id == self.preferences_ui.browsing.selected)
            .map(|target| target.label)
            .unwrap_or_else(|| "偏好设置".into())
    }
    pub(crate) fn persist_settings_browsing(&self) {
        if self.last_library.is_empty() {
            return;
        }
        let path =
            std::path::Path::new(&self.last_library).with_extension("settings-browsing.json");
        let mut state = self.preferences_ui.browsing.clone();
        self.remember_settings_offset(&mut state);
        state.notification_page = self.preferences_ui.notification_page;
        state.list_levels_page = self.preferences_ui.list_levels_page;
        let Ok(bytes) = serde_json::to_vec(&state) else {
            return;
        };
        // Only a tiny convenience record, never source documents or settings drafts.
        if path
            .symlink_metadata()
            .is_ok_and(|meta| !meta.file_type().is_file())
        {
            return;
        }
        let temporary = path.with_extension("settings-browsing.next");
        if temporary
            .symlink_metadata()
            .is_ok_and(|meta| !meta.file_type().is_file())
        {
            return;
        }
        if std::fs::write(&temporary, bytes).is_ok() {
            let _ = std::fs::rename(temporary, path);
        }
    }
    fn remember_settings_offset(&self, state: &mut SettingsBrowsing) {
        if state.selected.is_empty()
            || self
                .preferences_ui
                .pending_settings_scroll
                .borrow()
                .as_ref()
                .is_some_and(|(target, _)| target == &state.selected)
        {
            return;
        }
        let (item, offset) = match self.preferences_ui.section {
            SettingsSection::Appearance
                if !matches!(
                    state.selected.as_str(),
                    "appearance.selection" | "appearance.widget"
                ) && !state.selected.starts_with("notices.") =>
            {
                let top = self.settings_appearance_scroll().logical_scroll_top();
                (top.item_ix, f32::from(top.offset_in_item))
            }
            SettingsSection::Notifications if state.selected == "notices.rules" => {
                let top = self
                    .preferences_ui
                    .notification_rows
                    .scroll
                    .logical_scroll_top();
                (top.item_ix, f32::from(top.offset_in_item))
            }
            SettingsSection::Shortcuts => {
                let top = self.settings_shortcut_scroll().logical_scroll_top();
                (top.item_ix, f32::from(top.offset_in_item))
            }
            _ => (0, f32::from(self.settings_scroll.offset().y)),
        };
        state.offsets.insert(state.selected.clone(), (item, offset));
    }
    pub(crate) fn restore_settings_browsing(&mut self, cx: &mut Context<Self>) {
        if self.preferences_ui.browsing_library == self.last_library {
            return;
        }
        self.preferences_ui.browsing_library = self.last_library.clone();
        let path =
            std::path::Path::new(&self.last_library).with_extension("settings-browsing.json");
        let state = if path
            .metadata()
            .is_ok_and(|metadata| metadata.len() <= 65536)
        {
            std::fs::read(path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<SettingsBrowsing>(&bytes).ok())
        } else {
            None
        };
        let mut state = state.unwrap_or_default();
        state.width = state.width.clamp(176, 340);
        let targets = targets();
        state.offsets.retain(|key, (_, offset)| {
            offset.is_finite()
                && offset.abs() <= 1_000_000.
                && targets.iter().any(|target| &target.id == key)
        });
        let selected = if targets.iter().any(|target| target.id == state.selected) {
            state.selected.clone()
        } else {
            "learning".into()
        };
        self.preferences_ui.notification_page = state.notification_page.min(
            studyapp_core::NoticeCategory::ALL
                .len()
                .div_ceil(5)
                .saturating_sub(1),
        );
        self.preferences_ui.list_levels_page = state
            .list_levels_page
            .min(studyapp_core::LIST_LEVELS.div_ceil(4) - 1);
        self.preferences_ui.browsing = state;
        self.preferences_ui.browsing.selected.clear();
        self.select_settings_target(&selected, cx);
    }
    pub(crate) fn select_settings_target(&mut self, target: &str, cx: &mut Context<Self>) {
        if !targets().iter().any(|item| item.id == target) {
            return;
        }
        self.preferences_ui.image_cloud_search_reveal = false;
        self.sync_panel.update(cx, |panel, cx| {
            if panel.search_reveal {
                panel.search_reveal = false;
                cx.notify();
            }
        });
        let mut state = self.preferences_ui.browsing.clone();
        self.remember_settings_offset(&mut state);
        let section = match target.split('.').next().unwrap_or_default() {
            "learning" | "notes" => SettingsSection::Learning,
            "appearance" => SettingsSection::Appearance,
            "notices" if target == "notices.rules" || target == "notices.regions" => {
                SettingsSection::Notifications
            }
            "notices" => SettingsSection::Appearance,
            "shortcuts" => SettingsSection::Shortcuts,
            "storage" => SettingsSection::Storage,
            "sync" => SettingsSection::Sync,
            "logging" => SettingsSection::Logging,
            _ => SettingsSection::AboutAccount,
        };
        self.set_settings_section(section, cx);
        if target.starts_with("appearance.") || target.starts_with("notices.component.") {
            let page = match target {
                "appearance.theme" => AppearancePage::ThemeEditor,
                "appearance.workspace" => AppearancePage::Workspace,
                "appearance.widget" => AppearancePage::Widget,
                _ if target.starts_with("notices.component.") => AppearancePage::Notifications,
                _ => AppearancePage::Overview,
            };
            self.select_appearance_page(page, cx);
        }
        if let Some(index) = target
            .strip_prefix("notices.component.")
            .and_then(|value| value.parse::<usize>().ok())
        {
            self.preferences_ui.notice_component = NoticeStyle::ALL.get(index).copied();
        }
        if target.starts_with("storage.") {
            self.select_storage_page(
                match target {
                    "storage.copies" => StoragePage::Copies,
                    "storage.images" => StoragePage::Images,
                    "storage.library" => StoragePage::Library,
                    _ => StoragePage::Overview,
                },
                cx,
            );
        }
        state.selected = target.into();
        let (item_ix, offset) = state.offsets.get(target).copied().unwrap_or_default();
        self.settings_scroll
            .set_offset(point(px(0.), px(offset.min(0.))));
        *self.preferences_ui.pending_settings_scroll.borrow_mut() =
            virtual_target(target).then(|| {
                (
                    target.into(),
                    ListOffset {
                        item_ix,
                        offset_in_item: px(offset),
                    },
                )
            });
        self.preferences_ui.browsing = state;
        self.preferences_ui.settings_drawer = false;
        self.persist_settings_browsing();
        cx.notify();
    }
    pub(crate) fn apply_settings_list_offset(&self, scroll: &ListState) {
        let mut pending = self.preferences_ui.pending_settings_scroll.borrow_mut();
        if pending
            .as_ref()
            .is_some_and(|(target, _)| target == &self.preferences_ui.browsing.selected)
            && let Some((_, top)) = pending.take()
        {
            scroll.scroll_to(top);
        }
    }
    pub(crate) fn settings_tree(&self, cx: &mut Context<Self>) -> Div {
        use gpui_kit::{assets::IconName, component::Icon};
        use studyapp_ui::settings_components::navigation_button;
        let busy = self.preferences_ui.busy || self.preferences_ui.replacing || self.storage_busy;
        let state = &self.preferences_ui.browsing;
        let colors = crate::theme::selection(SelectionCategory::Navigation);
        let mut tree = column().gap_0().w_full().min_w_0().flex_shrink_0();
        for (group, label, icon) in [
            ("learning", "学习与复习", IconName::BookOpen),
            ("notes", "笔记与编辑", IconName::FileText),
            ("appearance", "外观", IconName::Palette),
            ("notices", "通知与浮层", IconName::Bell),
            ("shortcuts", "快捷键", IconName::Keyboard),
            ("storage", "存储与备份", IconName::HardDrive),
            ("sync", "同步", IconName::RefreshCw),
            ("logging", "日志", IconName::ScrollText),
            ("about", "关于与账户", IconName::Info),
        ] {
            let children: Vec<_> = targets()
                .into_iter()
                .filter(|target| target.group == group)
                .collect();
            let expandable = children.len() > 1;
            let expanded = state.expanded.contains(group);
            let selected = children.iter().any(|target| target.id == state.selected);
            let group_button = navigation_button(
                SharedString::from(format!("settings-group-{group}")),
                label,
                selected && !expandable,
            )
            .icon(
                Icon::new(if expandable {
                    if expanded {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    }
                } else {
                    icon
                })
                .with_size(px(14.)),
            )
            .font_weight(if selected {
                FontWeight::SEMIBOLD
            } else {
                FontWeight::NORMAL
            })
            .disabled(busy)
            .on_click(cx.listener(move |this, _, _, cx| {
                if expandable {
                    if !this.preferences_ui.browsing.expanded.remove(group) {
                        this.preferences_ui.browsing.expanded.insert(group.into());
                    }
                    this.persist_settings_browsing();
                } else {
                    this.select_settings_target(group, cx);
                }
                cx.notify();
            }));
            let mut group_view = column()
                .gap_0()
                .child(navigation_item(group_button, selected && !expandable));
            if expandable && expanded {
                let mut branch = column()
                    .gap_0()
                    .ml(px(14.))
                    .pl_2()
                    .border_l_1()
                    .border_color(crate::theme::color(BORDER));
                for target in children {
                    if target.id == "notices.component.0" {
                        branch = branch.child(muted("组件外观").text_xs().px_3().pt_3().pb_1());
                    }
                    let id = target.id.clone();
                    let current = target.id == state.selected;
                    branch = branch.child(navigation_item(
                        navigation_button(
                            SharedString::from(format!("settings-target-{}", target.id)),
                            target.label,
                            current,
                        )
                        .disabled(busy)
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.select_settings_target(&id, cx)),
                        ),
                        current,
                    ));
                }
                group_view = group_view.child(branch);
            }
            tree = tree.child(group_view);
        }
        column()
            .w(px(f32::from(state.width)))
            .h_full()
            .min_h_0()
            .flex_shrink_0()
            .gap_0()
            .bg(rgb(colors.idle_background))
            .child(
                row()
                    .min_h(px(54.))
                    .px_3()
                    .border_b_1()
                    .border_color(crate::theme::color(BORDER))
                    .child(heading("设置")),
            )
            .child(
                scroll_body(
                    "settings-tree-scroll",
                    &self.preferences_ui.settings_tree_scroll,
                )
                .test_support()
                .flex_1()
                .min_h_0()
                .px_1()
                .py_2()
                .child(tree),
            )
    }
    pub(crate) fn settings_tree_separator(&self, cx: &Context<Self>) -> Div {
        div()
            .id("settings-tree-resizer")
            .test_support()
            .w(px(6.))
            .h_full()
            .cursor_col_resize()
            .child(
                div()
                    .h_full()
                    .w(px(1.))
                    .mx_auto()
                    .bg(crate::theme::color(BORDER)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    this.preferences_ui.settings_resize =
                        Some((event.position.x, this.preferences_ui.browsing.width));
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .into_any_element()
            .map(|element| div().w(px(6.)).h_full().child(element))
    }
    pub(crate) fn settings_resize_events(&self, cx: &Context<Self>) -> impl IntoElement {
        let owner = cx.weak_entity();
        canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let moving = owner.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if !phase.capture() {
                        return;
                    }
                    let _ = moving.update(cx, |this, cx| {
                        if let Some((start, width)) = this.preferences_ui.settings_resize {
                            // A release outside the window may not deliver MouseUp.
                            // End the drag before using a later unpressed hover's position.
                            if event.pressed_button == Some(MouseButton::Left) {
                                this.preferences_ui.browsing.width = (f32::from(width)
                                    + f32::from(event.position.x - start))
                                .round()
                                .clamp(176., 340.)
                                    as u16;
                            } else {
                                this.preferences_ui.settings_resize = None;
                                this.persist_settings_browsing();
                            }
                            cx.stop_propagation();
                            cx.notify();
                        }
                    });
                });
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase.capture() && event.button == MouseButton::Left {
                        let _ = owner.update(cx, |this, cx| {
                            if this.preferences_ui.settings_resize.take().is_some() {
                                this.persist_settings_browsing();
                                cx.stop_propagation();
                                cx.notify();
                            }
                        });
                    }
                });
            },
        )
        .absolute()
        .size_full()
    }
}

impl StudyApp {
    pub(crate) fn settings_header(
        &self,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let category = match self
            .preferences_ui
            .browsing
            .selected
            .split('.')
            .next()
            .unwrap_or("")
        {
            "learning" => "学习与复习",
            "notes" => "笔记与编辑",
            "appearance" => "外观",
            "notices" => "通知与浮层",
            "shortcuts" => "快捷键",
            "storage" => "存储与备份",
            "sync" => "同步",
            "logging" => "日志",
            _ => "关于与账户",
        };
        let title = self.settings_target_title();
        row()
            .id("settings-titlebar")
            .test_support()
            .w_full()
            .min_w_0()
            .min_h(px(54.))
            .flex_shrink_0()
            .flex_wrap()
            .gap_2()
            .px(px(studyapp_ui::settings_components::PANE_INSET))
            .py_2()
            .border_b_1()
            .border_color(crate::theme::color(BORDER))
            .children(compact.then(|| {
                Button::new("settings-drawer-toggle")
                    .small()
                    .ghost()
                    .icon(gpui_kit::assets::IconName::PanelLeft)
                    .label("分类")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.preferences_ui.settings_drawer = !this.preferences_ui.settings_drawer;
                        cx.notify();
                    }))
            }))
            .child(
                row()
                    .flex_1()
                    .min_w_0()
                    .flex_wrap()
                    .gap_2()
                    .children((category != title).then(|| muted(format!("{category}  ›"))))
                    .child(
                        typography(studyapp_ui::design::TypeRole::Body)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    ),
            )
            .child(
                Button::new("settings-show-search")
                    .small()
                    .ghost()
                    .icon(gpui_kit::assets::IconName::Search)
                    .label("搜索 · Ctrl+F")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.open_settings_search(window, cx)),
                    ),
            )
    }
    pub(crate) fn auxiliary_settings_page(
        &self,
        width: Pixels,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let page = self.preferences_ui.browsing.selected.as_str();
        Some(match page {
            "notes.snapshots" => self.snapshot_settings(cx),
            "notes.editing" => {
                let enabled = self
                    .preferences_ui
                    .draft
                    .as_ref()
                    .is_some_and(|draft| draft.appearance.list_style.indent_subtree);
                let busy =
                    self.preferences_ui.busy || self.preferences_ui.replacing || self.storage_busy;
                crate::settings_components::section("列表编辑行为")
                    .child(studyapp_ui::settings_switch::SettingSwitch::new("editor-indent-subtree").default_value(false).label("Tab 同时调整未选中的子项")
                        .checked(enabled).disabled(busy).on_click(cx.listener(|this, value, _, cx| {
                            if let Some(draft) = &mut this.preferences_ui.draft { draft.appearance.list_style.indent_subtree = *value; }
                            cx.notify();
                        })))
                    .child(muted("关闭时只调整当前项及其自身续行；明确选中的子项仍分别调整。开启后带动整棵子树。保存全部设置后生效。"))
                    .child(muted("列表之后不缩进的正文会退出列表；Enter 一次只新增一个物理行，显示解析不改写原文。"))
            }
            "appearance.selection" => self.selection_settings(cx),
            "notices.regions" => self.notice_region_controls(cx),
            "notices.general" => crate::settings_components::card_grid(width)
                .child(self.notification_appearance_part(0, cx))
                .child(self.notification_appearance_part(1, cx)),
            _ if page.starts_with("notices.component.") => {
                crate::settings_components::card_grid(width)
                    .child(self.notice_surface_controls(cx))
                    .child(self.notice_content_controls(cx))
                    .child(
                        div()
                            .col_span_full()
                            .child(self.notification_appearance_part(2, cx)),
                    )
            }
            _ => return None,
        })
    }
}
