use crate::settings_ui::{AppearancePage, SettingsSection, StoragePage};

pub struct SettingResult {
    pub section: SettingsSection,
    pub label: &'static str,
    pub keywords: &'static str,
    pub appearance: AppearancePage,
    pub storage: StoragePage,
    pub markdown_group: u8,
}

impl SettingResult {
    const fn new(section: SettingsSection, label: &'static str, keywords: &'static str) -> Self {
        Self {
            section,
            label,
            keywords,
            appearance: AppearancePage::Overview,
            storage: StoragePage::Overview,
            markdown_group: 0,
        }
    }
    const fn appearance(mut self, page: AppearancePage, group: u8) -> Self {
        self.appearance = page;
        self.markdown_group = group;
        self
    }
    const fn storage(mut self, page: StoragePage) -> Self {
        self.storage = page;
        self
    }
}

static SETTINGS: &[SettingResult] = &[
    SettingResult::new(
        SettingsSection::Learning,
        "学习提醒与回答时限",
        "学习 间隔 计时 小时 分钟 提醒 答复",
    ),
    SettingResult::new(
        SettingsSection::Learning,
        "复习间隔",
        "学习 复习 遗忘 计划 天",
    ),
    SettingResult::new(
        SettingsSection::Appearance,
        "主题与明暗模式",
        "外观 系统 亮色 暗色 深色 浅色 主题",
    ),
    SettingResult::new(
        SettingsSection::Appearance,
        "界面字号与内容间距",
        "外观 字体 字号 大小 缩放 布局 间距",
    ),
    SettingResult::new(
        SettingsSection::Appearance,
        "自定义主题与颜色",
        "外观 颜色 强调色 背景 文字 对比度 预设",
    )
    .appearance(AppearancePage::ThemeEditor, 0),
    SettingResult::new(
        SettingsSection::Appearance,
        "Markdown 正文与阅读宽度",
        "md markdown 文字 标题 字号 行高 宽度 字体",
    )
    .appearance(AppearancePage::ThemeEditor, 0),
    SettingResult::new(
        SettingsSection::Appearance,
        "Markdown 引用与代码",
        "md markdown 代码 引用 圆角 背景",
    )
    .appearance(AppearancePage::ThemeEditor, 1),
    SettingResult::new(
        SettingsSection::Appearance,
        "Markdown 表格与列表",
        "md markdown 表格 条纹 列表 边线",
    )
    .appearance(AppearancePage::ThemeEditor, 2),
    SettingResult::new(
        SettingsSection::Appearance,
        "语法高亮",
        "md markdown 代码 语言 关键字 注释 函数 高亮",
    )
    .appearance(AppearancePage::ThemeEditor, 3),
    SettingResult::new(
        SettingsSection::Appearance,
        "侧栏与文件路径显示",
        "外观 工作区 导航 隐藏 路径 目录",
    )
    .appearance(AppearancePage::Workspace, 0),
    SettingResult::new(
        SettingsSection::Shortcuts,
        "快捷键与按键录制",
        "快捷键 按键 快捷操作 录制 冲突 键盘",
    ),
    SettingResult::new(
        SettingsSection::Storage,
        "空间管理与托管副本",
        "存储 空间 占用 清理 副本 删除",
    )
    .storage(StoragePage::Copies),
    SettingResult::new(
        SettingsSection::Storage,
        "图片位置与云存储",
        "存储 图片 粘贴 云端 s3 oss bucket endpoint 凭据 密钥",
    )
    .storage(StoragePage::Images),
    SettingResult::new(
        SettingsSection::Storage,
        "资料库位置与备份恢复",
        "存储 本地 目录 路径 迁移 备份 恢复",
    )
    .storage(StoragePage::Library),
    SettingResult::new(
        SettingsSection::Sync,
        "笔记同步与连接设置",
        "云端 同步 webdav nas 文件夹 账户 密码 自动",
    ),
    SettingResult::new(
        SettingsSection::AboutAccount,
        "版本更新与账户",
        "关于 版本 更新 下载 安装 账户 微信 登录",
    ),
];

pub fn search(query: &str) -> Vec<&'static SettingResult> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    SETTINGS
        .iter()
        .filter(|item| {
            query
                .split_whitespace()
                .all(|word| item.label.contains(word) || item.keywords.contains(word))
        })
        .collect()
}

use crate::{app::StudyApp, components::*, controls::Button};
use gpui_kit::{
    component::{Disableable, Sizable, button::ButtonVariants},
    *,
};

impl StudyApp {
    pub fn settings_search_active(&self, cx: &App) -> bool {
        self.preferences_ui
            .search
            .as_ref()
            .is_some_and(|input| !input.read(cx).value().trim().is_empty())
    }

    pub fn settings_search_bar(&self, cx: &mut Context<Self>) -> Div {
        let active = self.settings_search_active(cx);
        row()
            .min_w_0()
            .gap_2()
            .children(self.preferences_ui.search.as_ref().map(|input| {
                div()
                    .id("settings-search")
                    .test_support()
                    .flex_1()
                    .min_w_0()
                    .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        if event.keystroke.key == "escape" && this.settings_search_active(cx) {
                            if let Some(input) = &this.preferences_ui.search {
                                input.update(cx, |input, cx| input.set_value("", window, cx));
                            }
                            cx.stop_propagation();
                            cx.notify();
                        }
                    }))
                    .child(crate::settings_components::input(
                        input,
                        self.preferences_ui.busy,
                    ))
            }))
            .children(active.then(|| {
                Button::new("settings-clear-search")
                    .small()
                    .ghost()
                    .label("清除")
                    .on_click(cx.listener(|this, _, window, cx| {
                        if let Some(input) = &this.preferences_ui.search {
                            input.update(cx, |input, cx| input.set_value("", window, cx));
                        }
                        cx.notify();
                    }))
            }))
    }

    pub fn settings_search_results(&self, cx: &mut Context<Self>) -> Div {
        let query = self
            .preferences_ui
            .search
            .as_ref()
            .map(|input| input.read(cx).value())
            .unwrap_or_default();
        let results = search(&query);
        let mut content = column()
            .gap_2()
            .child(muted(format!("找到 {} 项设置", results.len())));
        if results.is_empty() {
            return content.child(empty(
                "没有找到相关设置",
                "试试“字号”“主题”“同步”“备份”或“快捷键”。",
            ));
        }
        for (index, result) in results.into_iter().enumerate() {
            let category = SettingsSection::ALL
                .into_iter()
                .find(|(section, _, _)| *section == result.section)
                .map(|(_, _, label)| label)
                .unwrap_or_default();
            content = content.child(
                card().gap_1().child(muted(category)).child(
                    Button::new(("settings-search-result", index))
                        .ghost()
                        .w_full()
                        .justify_start()
                        .label(result.label)
                        .disabled(self.preferences_ui.busy)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.select_settings_section(result.section, cx);
                            this.select_appearance_page(result.appearance, cx);
                            this.select_storage_page(result.storage, cx);
                            this.preferences_ui.appearance_group = result.markdown_group;
                            if let Some(input) = &this.preferences_ui.search {
                                input.update(cx, |input, cx| input.set_value("", window, cx));
                            }
                            cx.notify();
                        })),
                ),
            );
        }
        content
    }
}

#[cfg(test)]
mod tests {
    use super::{AppearancePage, SettingsSection, StoragePage, search};

    #[test]
    fn searches_labels_and_synonyms_across_categories() {
        assert!(
            search("字号")
                .iter()
                .any(|item| item.label == "界面字号与内容间距")
        );
        assert!(
            search("  S3  ")
                .iter()
                .any(|item| matches!(item.storage, StoragePage::Images))
        );
        assert!(
            search("颜色 对比度")
                .iter()
                .any(|item| matches!(item.appearance, AppearancePage::ThemeEditor))
        );
        assert!(
            search("快捷键")
                .iter()
                .any(|item| matches!(item.section, SettingsSection::Shortcuts))
        );
    }

    #[test]
    fn empty_and_unmatched_queries_do_not_create_results() {
        assert!(search(" \t ").is_empty());
        assert!(search("absolutely-not-a-setting").is_empty());
    }
}
