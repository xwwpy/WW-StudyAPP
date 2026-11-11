//! Workspace preferences belong to the global, discardable settings draft.
use crate::{app::StudyApp, components::*, settings_components as form};
use gpui_kit::*;
use studyapp_core::{LayoutSettings, LibrarySort, OutlinePosition};

impl StudyApp {
    fn stage_workspace_layout(&mut self, change: impl FnOnce(&mut LayoutSettings)) {
        if let Some(draft) = &mut self.preferences_ui.draft {
            change(&mut draft.layout);
            self.layout = draft.layout.clone();
        }
    }
    pub(crate) fn workspace_settings(&self, cx: &mut Context<Self>) -> Div {
        let defaults = studyapp_core::Settings::default();
        let draft = self.preferences_ui.draft.as_ref().unwrap_or(&defaults);
        let layout = &draft.layout;
        let busy = self.preferences_ui.busy || self.preferences_ui.replacing || self.storage_busy;
        let panels = form::section("面板与显示")
            .child(studyapp_ui::settings_switch::SettingSwitch::new("show-file-paths")
                .default_value(false).label("显示笔记文件的存储路径")
                .checked(draft.show_file_paths).disabled(busy)
                .on_click(cx.listener(|this, checked, _, cx| {
                    if let Some(draft) = &mut this.preferences_ui.draft { draft.show_file_paths = *checked; }
                    cx.notify();
                })))
            .child(studyapp_ui::settings_switch::SettingSwitch::new("settings-navigation-visible")
                .default_value(true).label("显示主导航").checked(layout.navigation_visible).disabled(busy)
                .on_click(cx.listener(|this, checked, _, cx| {
                    this.stage_workspace_layout(|layout| layout.navigation_visible = *checked); cx.notify();
                })))
            .child(self.setting_number(
                "workspace-navigation-width".into(), "主导航宽度", "px",
                (layout.navigation_width.into(), 160, 128, 320, busy || !layout.navigation_visible),
                |this, value| this.stage_workspace_layout(|layout| layout.navigation_width = value as u32), cx))
            .children((!layout.navigation_visible).then(|| muted("启用显示主导航后可调整宽度；当前宽度草稿保留。")))
            .child(studyapp_ui::settings_switch::SettingSwitch::new("settings-folders-visible")
                .default_value(true).label("显示资料库").checked(layout.folders_visible).disabled(busy)
                .on_click(cx.listener(|this, checked, _, cx| {
                    this.stage_workspace_layout(|layout| layout.folders_visible = *checked); cx.notify();
                })))
            .child(self.setting_number(
                "workspace-notes-width".into(), "资料库与笔记列表宽度", "px",
                (layout.notes_width.into(), 280, 160, 440, busy || !layout.folders_visible),
                |this, value| this.stage_workspace_layout(|layout| layout.notes_width = value as u32), cx))
            .children((!layout.folders_visible).then(|| muted("启用显示资料库后可调整列表宽度；当前宽度草稿保留。")))
            .child(muted("宽度为首选值，窄窗口会自动为正文让出空间，不改写首选值。设置内的调整统一保存；放弃修改恢复所有面板。"));
        let ordering = form::section("资料库排序").child(form::settings_row(
            "笔记排序",
            form::choice_with_default(
                "workspace-library-sort",
                (layout.library_sort, LibrarySort::default()),
                vec![
                    (LibrarySort::NameAscending, "名称升序".into()),
                    (LibrarySort::NameDescending, "名称降序".into()),
                    (LibrarySort::Newest, "最近更新优先".into()),
                    (LibrarySort::Oldest, "最早更新优先".into()),
                ],
                busy,
                cx.weak_entity(),
                |this, value, _, cx| {
                    this.stage_workspace_layout(|layout| layout.library_sort = value);
                    cx.notify();
                },
            ),
        ));
        let outline = layout.outline.unwrap_or_default();
        let directory = form::section("阅读目录")
            .child(
                studyapp_ui::settings_switch::SettingSwitch::new("workspace-outline-shown")
                    .default_value(false)
                    .label("显示阅读目录")
                    .checked(outline.shown)
                    .disabled(busy)
                    .on_click(cx.listener(|this, checked, _, cx| {
                        this.stage_workspace_layout(|layout| {
                            layout.outline.get_or_insert_default().shown = *checked
                        });
                        cx.notify();
                    })),
            )
            .child(form::settings_row(
                "目录位置",
                form::choice_with_default(
                    "workspace-outline-position",
                    (outline.position, OutlinePosition::default()),
                    vec![
                        (OutlinePosition::Auto, "自动适应".into()),
                        (OutlinePosition::Left, "左侧".into()),
                        (OutlinePosition::Right, "右侧".into()),
                        (OutlinePosition::Top, "顶部".into()),
                        (OutlinePosition::Floating, "悬浮".into()),
                    ],
                    busy || !outline.shown,
                    cx.weak_entity(),
                    |this, value, _, cx| {
                        this.stage_workspace_layout(|layout| {
                            layout.outline.get_or_insert_default().position = value
                        });
                        cx.notify();
                    },
                ),
            ))
            .children(
                (!outline.shown)
                    .then(|| muted("启用显示阅读目录后可调整位置；关闭目录不清空位置草稿。")),
            )
            .child(muted(
                "目录偏好与阅读页共用，保存后重开仍生效；不改变 Markdown 源文或撤销历史。",
            ));
        column()
            .gap_4()
            .child(panels)
            .child(ordering)
            .child(directory)
    }
}
