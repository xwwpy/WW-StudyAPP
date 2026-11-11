use crate::settings_ui::{AppearancePage, SettingsSection, StoragePage};

#[derive(Clone)]
pub struct SettingResult {
    target: Option<&'static str>,
    row: usize,
    anchor: Option<&'static str>,
    component: Option<studyapp_core::NoticeStyle>,
    category: Option<studyapp_core::NoticeCategory>,
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
            target: None,
            row: 0,
            anchor: None,
            component: None,
            category: None,
            section,
            label,
            keywords,
            appearance: AppearancePage::Overview,
            storage: StoragePage::Overview,
            markdown_group: 0,
        }
    }
    const fn target(mut self, target: &'static str, row: usize) -> Self {
        self.target = Some(target);
        self.row = row;
        self
    }
    const fn anchor(mut self, anchor: &'static str) -> Self {
        self.anchor = Some(anchor);
        self
    }
    fn target_id(&self) -> String {
        if let Some(target) = self.target {
            return target.into();
        }
        if let Some(style) = self.component {
            return format!(
                "notices.component.{}",
                studyapp_core::NoticeStyle::ALL
                    .iter()
                    .position(|value| *value == style)
                    .unwrap_or(0)
            );
        }
        match self.section {
            SettingsSection::Learning => "learning",
            SettingsSection::Logging => "logging",
            SettingsSection::Notifications => "notices.rules",
            SettingsSection::Shortcuts => "shortcuts",
            SettingsSection::Sync => "sync",
            SettingsSection::AboutAccount => "about",
            SettingsSection::Storage => match self.storage {
                StoragePage::Overview => "storage.overview",
                StoragePage::Copies => "storage.copies",
                StoragePage::Images => "storage.images",
                StoragePage::Library => "storage.library",
            },
            SettingsSection::Appearance => match self.appearance {
                AppearancePage::Overview => "appearance.overview",
                AppearancePage::ThemeEditor => "appearance.theme",
                AppearancePage::Workspace => "appearance.workspace",
                AppearancePage::Widget => "appearance.widget",
                AppearancePage::Notifications => "notices.general",
            },
        }
        .into()
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
        SettingsSection::Appearance,
        "多级无序列表标记",
        "markdown md 无序 列表 嵌套 层级 标记 符号 圆 方块 三角 预设 自定义",
    ),
    SettingResult::new(
        SettingsSection::Appearance,
        "Markdown HTML 渲染开关",
        "md markdown html 行内 样式 安全 渲染 源文 关闭",
    ),
    SettingResult::new(
        SettingsSection::Logging,
        "日志等级与保存目录",
        "log 日志 诊断 error warn info debug trace 开启 关闭 位置 目录 路径",
    ),
    SettingResult::new(
        SettingsSection::Notifications,
        "通知分类与免打扰",
        "通知 提示 计时 复习 更新 红点 前台 后台 隐私 声音 免打扰",
    ),
    SettingResult::new(
        SettingsSection::Appearance,
        "通知样式与透明度",
        "通知 外观 气泡 卡片 横幅 锚点 圆角 透明度 动画",
    )
    .appearance(AppearancePage::Notifications, 0),
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

pub fn search(query: &str) -> Vec<SettingResult> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    let mut settings = SETTINGS.to_vec();
    settings.extend(extra_settings());
    settings
        .into_iter()
        .filter(|item| {
            let text = format!(
                "{} {} {} {}",
                item.label,
                item.keywords,
                item.component
                    .map(|style| style.label())
                    .unwrap_or_default(),
                crate::settings_navigation::target_title(&item.target_id())
            )
            .to_lowercase();
            query.split_whitespace().all(|word| text.contains(word))
        })
        .collect()
}

use crate::{app::StudyApp, components::*, controls::Button};
use gpui_kit::{
    component::{Disableable, Sizable, button::ButtonVariants},
    prelude::FluentBuilder,
    *,
};

impl StudyApp {
    pub(crate) fn open_settings_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.preferences_ui.settings_search_open = true;
        if let Some(input) = &self.preferences_ui.search {
            input.read(cx).focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }
    pub fn settings_search_active(&self, cx: &App) -> bool {
        self.preferences_ui.settings_search_open
            && self
                .preferences_ui
                .search
                .as_ref()
                .is_some_and(|input| !input.read(cx).value().trim().is_empty())
    }

    pub fn settings_search_bar(&self, cx: &mut Context<Self>) -> Div {
        if !self.preferences_ui.settings_search_open {
            return row();
        }
        let active = self.settings_search_active(cx);
        row()
            .relative()
            .min_w_0()
            .gap_2()
            .children(self.preferences_ui.search.as_ref().map(|input| {
                div()
                    .id("settings-search")
                    .test_support()
                    .flex_1()
                    .min_w_0()
                    .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        if event.keystroke.key == "escape" {
                            this.preferences_ui.settings_search_open = false;
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
                    .action("清除")
                    .on_click(cx.listener(|this, _, window, cx| {
                        if let Some(input) = &this.preferences_ui.search {
                            input.update(cx, |input, cx| input.set_value("", window, cx));
                        }
                        cx.notify();
                    }))
            }))
            .when(active, |view| {
                view.child(
                    div()
                        .id("settings-search-popup")
                        .test_support()
                        .absolute()
                        .top_full()
                        .left_0()
                        .right_0()
                        .occlude()
                        .p_3()
                        .rounded_md()
                        .shadow_lg()
                        .bg(crate::theme::color(BG))
                        .border_1()
                        .border_color(crate::theme::color(BORDER))
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.preferences_ui.settings_search_open = false;
                            cx.notify();
                        }))
                        .child(
                            scroll_body(
                                "settings-search-results",
                                &self.preferences_ui.settings_search_scroll,
                            )
                            .max_h(px(360.))
                            .child(self.settings_search_results(cx)),
                        ),
                )
            })
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
                column()
                    .gap_1()
                    .py_2()
                    .border_b_1()
                    .border_color(crate::theme::color(BORDER))
                    .child(muted(format!(
                        "{category} / {}",
                        crate::settings_navigation::target_title(&result.target_id())
                    )))
                    .child(muted(format!("匹配说明：{}", result.keywords)))
                    .child(
                        Button::new(("settings-search-result", index))
                            .ghost()
                            .w_full()
                            .justify_start()
                            .label(result.label)
                            .tooltip(result.keywords)
                            .disabled(self.preferences_ui.busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select_settings_target(&result.target_id(), cx);
                                this.preferences_ui.notification_category = result.category;
                                let category_index = result.category.and_then(|category| {
                                    studyapp_core::NoticeCategory::ALL
                                        .iter()
                                        .position(|value| *value == category)
                                });
                                if let Some(index) = category_index {
                                    this.preferences_ui.notification_page = index / 5;
                                }
                                let row = category_index.map_or(result.row, |row| row % 5 + 2);
                                *this.preferences_ui.pending_settings_scroll.borrow_mut() =
                                    crate::settings_navigation::virtual_target(&result.target_id())
                                        .then(|| {
                                            (
                                                result.target_id(),
                                                ListOffset {
                                                    item_ix: row,
                                                    offset_in_item: px(0.),
                                                },
                                            )
                                        });
                                if result.target_id() == "appearance.selection" {
                                    this.preferences_ui.selection_target = None;
                                }
                                this.preferences_ui.appearance_group = result.markdown_group;
                                let scroll = match this.preferences_ui.section {
                                    SettingsSection::Appearance
                                        if result.target_id() == "appearance.theme"
                                            || result.target_id() == "appearance.overview"
                                            || result.target_id() == "appearance.workspace" =>
                                    {
                                        studyapp_ui::settings_anchor::SettingScroll::Virtual(
                                            this.settings_appearance_scroll(),
                                        )
                                    }
                                    SettingsSection::Notifications
                                        if result.target_id() == "notices.rules" =>
                                    {
                                        studyapp_ui::settings_anchor::SettingScroll::Virtual(
                                            this.preferences_ui.notification_rows.scroll.clone(),
                                        )
                                    }
                                    SettingsSection::Shortcuts => {
                                        studyapp_ui::settings_anchor::SettingScroll::Virtual(
                                            this.settings_shortcut_scroll(),
                                        )
                                    }
                                    _ => studyapp_ui::settings_anchor::SettingScroll::Normal(
                                        this.settings_scroll.clone(),
                                    ),
                                };
                                studyapp_ui::settings_anchor::seek(
                                    result.anchor.unwrap_or(result.label),
                                    scroll,
                                    window,
                                    cx,
                                );

                                if let Some(input) = &this.preferences_ui.search {
                                    input.update(cx, |input, cx| input.set_value("", window, cx));
                                }
                                this.preferences_ui.settings_search_open = false;
                                cx.notify();
                            })),
                    ),
            );
        }
        content
    }
}

fn extra_settings() -> Vec<SettingResult> {
    use SettingsSection as S;
    let mut entries: Vec<_> = [
        (
            S::Learning,
            "notes.snapshots",
            "快照模式",
            "正文 保存 版本 历史 自动 仅扫描 手动 稳定等待 扫描间隔 离开收尾 Ctrl+S",
        ),
        (
            S::Learning,
            "notes.snapshots",
            "扫描变化与选择保存快照",
            "重新扫描 diff 差异 勾选 变化清单 手动 快照 过期预览",
        ),
        (
            S::Learning,
            "notes.editing",
            "Tab 同时调整未选中的子项",
            "列表 编辑 缩进 子树 续行 选中 多光标 shift tab",
        ),
        (
            S::Appearance,
            "appearance.selection",
            "全局与分类选中样式",
            "导航 选项 笔记 文件 目录 选中 取消选中 背景 文字 边框 标记 圆角 继承",
        ),
        (
            S::Appearance,
            "appearance.widget",
            "计时小组件",
            "透明度 字号 大小 宽度 高度 按钮 置顶 位置 拖动 计时 小组件",
        ),
        (
            S::Notifications,
            "notices.regions",
            "通知分区与堆叠",
            "轻提示 进度 角落 横幅 底部 单张 排队 数量 新到 靠前 错层 空间 等待",
        ),
        (
            S::Notifications,
            "notices.rules",
            "免打扰时间",
            "24 小时 HH:MM 开始 结束 次日 跨天 提示音 红点 隐私 严重错误",
        ),
        (
            S::Appearance,
            "notices.general",
            "提示动效与默认外观",
            "停留 秒 宽度 圆角 透明度 背景 强调色 边框 过渡 动画 减少动态 阴影 紧凑 间距",
        ),
        (
            S::Storage,
            "storage.images",
            "图片云端凭据",
            "密钥 secret 更换 清除 本次输入 隐藏 加密 S3 对象存储",
        ),
        (
            S::Sync,
            "sync",
            "同步凭据",
            "密码 WebDAV 应用密码 更换 清除 保存 确认 本次输入 隐藏 账户",
        ),
        (
            S::Storage,
            "storage.library",
            "资料库备份与迁移",
            "备份 创建 恢复 目录 位置 迁移 应用资料 完整性",
        ),
    ]
    .into_iter()
    .map(|(section, target, label, keywords)| {
        SettingResult::new(section, label, keywords).target(target, 0)
    })
    .collect();
    entries.extend(studyapp_core::NoticeStyle::ALL.into_iter().map(|style| {
        let mut entry = SettingResult::new(S::Appearance, style.label(), "通知 浮层 组件 独立 布局 备选 外观 不透明度 边框 颜色 宽度 高度 圆角 字段 默认 继承 简略 详细 预览");
        entry.component = Some(style); entry
    }));
    entries.extend(
        studyapp_core::NoticeCategory::ALL
            .into_iter()
            .map(|category| {
                let mut entry = SettingResult::new(
                    S::Notifications,
                    category.label(),
                    "通知 场景 分类 前台 后台 桌面 声音 开关 内容 字段 继承 自定义 必要 风险",
                );
                entry.category = Some(category);
                entry
            }),
    );
    entries.extend(field_settings());
    entries
}

fn field_settings() -> Vec<SettingResult> {
    use SettingsSection as S;
    let mut fields: Vec<_> = [
        (
            S::Learning,
            "learning",
            "询问间隔",
            "学习节奏 分钟 下一时间段",
        ),
        (S::Learning, "learning", "回答期限", "回答时限 分钟 超时"),
        (
            S::Learning,
            "learning",
            "复习间隔",
            "阶段 天 新建计划 添加 删除 分页",
        ),
        (
            S::Learning,
            "notes.snapshots",
            "保存方式",
            "快照 自动保存 仅扫描 手动",
        ),
        (
            S::Learning,
            "notes.snapshots",
            "扫描间隔",
            "快照 秒 定时 后台",
        ),
        (
            S::Learning,
            "notes.snapshots",
            "内容稳定等待",
            "快照 稳定窗口 秒 连续修改 合并",
        ),
        (
            S::Learning,
            "notes.editing",
            "Tab 同时调整未选中的子项",
            "缩进 列表 子树 续行 多光标",
        ),
        (
            S::Appearance,
            "appearance.overview",
            "主题与阅读",
            "明暗 跟随系统 浅色 深色 Markdown HTML",
        ),
        (
            S::Appearance,
            "appearance.overview",
            "界面字号",
            "字体 大小 像素 12 22 px",
        ),
        (
            S::Appearance,
            "appearance.overview",
            "内容间距",
            "紧凑 宽松 像素 布局 px",
        ),
        (
            S::Appearance,
            "appearance.overview",
            "多级无序列表",
            "标记 预设 固定层级 自定义",
        ),
        (
            S::Appearance,
            "appearance.selection",
            "选中背景",
            "全局 分类 继承 导航 选项 笔记 目录 颜色",
        ),
        (
            S::Appearance,
            "appearance.selection",
            "选中文字",
            "全局 分类 继承 颜色",
        ),
        (
            S::Appearance,
            "appearance.selection",
            "选中边框与标记",
            "全局 分类 继承 颜色",
        ),
        (
            S::Appearance,
            "appearance.selection",
            "未选中背景",
            "全局 分类 继承 颜色",
        ),
        (
            S::Appearance,
            "appearance.selection",
            "未选中文字",
            "全局 分类 继承 颜色",
        ),
        (
            S::Appearance,
            "appearance.widget",
            "布局与显示",
            "小组件 紧凑 标准 展开 显示按钮",
        ),
        (
            S::Appearance,
            "appearance.widget",
            "圆角",
            "小组件 尺寸 背景 px",
        ),
        (
            S::Appearance,
            "appearance.widget",
            "整体缩放（50–200%）",
            "小组件 百分比 大小",
        ),
        (
            S::Appearance,
            "appearance.widget",
            "背景不透明度（%）",
            "小组件 透明度 颜色",
        ),
        (
            S::Appearance,
            "appearance.widget",
            "强调色（留空跟随主题）",
            "小组件 RGBA 颜色 继承",
        ),
        (
            S::Appearance,
            "appearance.widget",
            "计时预设",
            "倒计时 分钟 正整数",
        ),
        (
            S::Appearance,
            "appearance.widget",
            "窗口行为",
            "小组件 窗口置顶 显示 隐藏",
        ),
        (
            S::Notifications,
            "notices.rules",
            "免打扰",
            "开始 结束 24小时 HH:MM 次日 跨天",
        ),
        (
            S::Storage,
            "storage.images",
            "本地图片",
            "路径 图片目录 选择文件夹",
        ),
        (
            S::Storage,
            "storage.library",
            "当前资料库",
            "路径 更改位置 迁移 验证 原资料保留",
        ),
        (
            S::Storage,
            "storage.library",
            "备份与恢复",
            "导出备份 从备份恢复 当前设置保留",
        ),
        (
            S::Sync,
            "sync",
            "同步文件夹",
            "文件夹 路径 根目录 选择文件夹",
        ),
        (S::Sync, "sync", "WebDAV 地址", "https nas 服务 连接"),
        (S::Sync, "sync", "账户", "用户名 WebDAV"),
        (
            S::Sync,
            "sync",
            "凭据",
            "密码 应用密码 WebDAV 更换 清除 隐藏",
        ),
        (S::Sync, "sync", "自动同步", "启用 开关 分钟 周期 间隔"),
        (S::Logging, "logging", "启用日志", "开启 关闭 日志 本机"),
        (
            S::Logging,
            "logging",
            "输出等级",
            "error warn info debug trace 详细程度",
        ),
        (
            S::Logging,
            "logging",
            "日志目录",
            "文件夹 路径 默认位置 logs",
        ),
    ]
    .into_iter()
    .map(|(section, target, label, keywords)| {
        SettingResult::new(section, label, keywords)
            .target(target, 0)
            .anchor(label)
    })
    .collect();
    fields.extend(
        crate::settings_ui::MARKDOWN_FIELDS
            .iter()
            .map(|(_, _, label, group)| {
                SettingResult::new(
                    S::Appearance,
                    label,
                    "Markdown 颜色 阅读 编辑器 高亮 对比度 RGBA 继承",
                )
                .target("appearance.theme", 1)
                .appearance(AppearancePage::ThemeEditor, *group)
                .anchor(label)
            }),
    );
    for (label, group) in [
        ("正文字号", 0),
        ("行高", 0),
        ("阅读宽度", 0),
        ("代码字号", 1),
        ("代码圆角", 1),
    ] {
        fields.push(
            SettingResult::new(
                S::Appearance,
                label,
                "Markdown 阅读 版式 字体 大小 宽度 自动 继承",
            )
            .target("appearance.theme", 1)
            .appearance(AppearancePage::ThemeEditor, group)
            .anchor(label),
        );
    }
    for (target, label, keywords, row) in [
        (
            "appearance.overview",
            "界面主题",
            "跟随系统 浅色 深色 明暗 模式",
            0,
        ),
        (
            "appearance.overview",
            "渲染 Markdown 中的安全 HTML 与行内样式",
            "html 行内样式 开关 源文 安全",
            0,
        ),
        (
            "appearance.overview",
            "列宽",
            "表格 等宽 内容 自适应 默认",
            0,
        ),
        (
            "appearance.overview",
            "列对齐",
            "表格 源文 左对齐 居中 右对齐",
            0,
        ),
        ("appearance.theme", "主题名称", "自定义 主题 命名", 0),
        ("appearance.theme", "强调色", "主题 颜色 RGBA", 0),
        ("appearance.theme", "窗口背景", "主题 颜色 RGBA", 0),
        ("appearance.theme", "卡片背景", "主题 颜色 RGBA", 0),
        ("appearance.theme", "文字颜色", "主题 颜色 RGBA", 0),
        (
            "appearance.workspace",
            "显示笔记文件的存储路径",
            "工作区 路径 隐私 文件",
            0,
        ),
        (
            "appearance.workspace",
            "显示主导航",
            "工作区 侧栏 导航 隐藏",
            0,
        ),
        (
            "appearance.workspace",
            "显示资料库",
            "工作区 侧栏 资料库 隐藏",
            0,
        ),
        (
            "appearance.selection",
            "编辑范围",
            "选中 全局 分类 导航 选项 笔记 目录",
            0,
        ),
        (
            "appearance.selection",
            "显示选中标记",
            "选中 标记 全局 显示 隐藏",
            0,
        ),
        (
            "appearance.selection",
            "选中项圆角",
            "选中 全局 类别 圆角 像素",
            0,
        ),
        ("appearance.widget", "布局", "计时小组件 紧凑 标准 展开", 0),
        (
            "appearance.widget",
            "背景",
            "计时小组件 跟随主题 不透明 内嵌面色",
            0,
        ),
        (
            "appearance.widget",
            "窗口置顶",
            "计时小组件 窗口 固定 置顶",
            0,
        ),
        (
            "appearance.widget",
            "显示秒数",
            "计时小组件 秒 显示 隐藏",
            0,
        ),
        (
            "appearance.widget",
            "显示进度",
            "计时小组件 进度 显示 隐藏",
            0,
        ),
        (
            "appearance.widget",
            "显示计时说明",
            "计时小组件 说明 阶段 显示 隐藏",
            0,
        ),
        (
            "appearance.widget",
            "显示计时按钮",
            "计时小组件 操作 按钮 显示 隐藏",
            0,
        ),
    ] {
        fields.push(
            SettingResult::new(S::Appearance, label, keywords)
                .target(target, row)
                .anchor(label),
        );
    }
    for (label, keywords) in [
        ("显示新版本红点", "更新 红点 版本"),
        ("立即免打扰", "通知 安静 暂停 免打扰"),
        ("免打扰时仍显示严重错误", "通知 严重 错误 免打扰 例外"),
        ("桌面卡片隐藏具体内容", "通知 隐私 后台 桌面 卡片"),
        ("播放提示音", "通知 声音 提示音"),
        ("启用定时免打扰", "通知 定时 时间段 开始 结束"),
        ("免打扰开始", "通知 时间 HH:MM 24 小时 起止 跨天 次日"),
        ("免打扰结束", "通知 时间 HH:MM 24 小时 起止 跨天 次日"),
    ] {
        fields.push(
            SettingResult::new(S::Notifications, label, keywords)
                .target("notices.rules", 0)
                .anchor(label),
        );
    }
    for (label, group) in [
        ("正文字体", 0),
        ("代码字体", 1),
        ("显示代码边框", 1),
        ("显示表格网格", 2),
        ("表格交替行背景", 2),
    ] {
        fields.push(
            SettingResult::new(
                S::Appearance,
                label,
                "Markdown 字体 代码 表格 网格 条纹 边框 显示 隐藏",
            )
            .target("appearance.theme", 1)
            .appearance(AppearancePage::ThemeEditor, group)
            .anchor(label),
        );
    }
    for label in [
        "保存方式",
        "云端服务",
        "寻址方式",
        "Endpoint",
        "Bucket",
        "Region",
        "Access Key ID",
        "对象前缀",
    ] {
        fields.push(
            SettingResult::new(
                S::Storage,
                label,
                "图片 云端 存储 S3 对象存储 本地 服务 地址 桶 区域 公开账户 路径",
            )
            .target("storage.images", 0)
            .anchor(label),
        );
    }
    fields.push(
        SettingResult::new(S::Sync, "同步目标", "关闭 文件夹 NAS WebDAV 同步目标")
            .target("sync", 0)
            .anchor("同步目标"),
    );
    for style in studyapp_core::NoticeStyle::ALL {
        for label in [
            "组件布局",
            "不透明度（%）",
            "边框宽度（px）",
            "宽度（px）",
            "内容最大高度（px）",
            "圆角（px）",
            "此组件背景（自动 / RGBA）",
            "此组件边框（自动 / RGBA）",
        ] {
            let mut entry = SettingResult::new(
                S::Appearance,
                label,
                "通知 浮层 组件 独立 外观 继承 有效值 默认 颜色 尺寸 布局 透明度",
            )
            .anchor(label);
            entry.component = Some(style);
            fields.push(entry);
        }
    }
    fields
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
    fn concrete_fields_include_their_real_subpage_and_markdown_row() {
        let width = search("阅读宽度")
            .into_iter()
            .find(|entry| entry.label == "阅读宽度")
            .unwrap();
        assert_eq!(width.target_id(), "appearance.theme");
        assert_eq!(width.row, 1);
        assert_eq!(width.markdown_group, 0);
        let code = search("代码背景")
            .into_iter()
            .find(|entry| entry.label == "代码背景")
            .unwrap();
        assert_eq!(code.target_id(), "appearance.theme");
        assert_eq!(code.markdown_group, 1);
        let corner = search("角落 圆角");
        assert!(corner.iter().any(|entry| entry.component
            == Some(studyapp_core::NoticeStyle::CornerCard)
            && entry.label == "圆角（px）"));
        assert!(
            search("日志目录")
                .iter()
                .any(|entry| entry.target_id() == "logging" && entry.anchor == Some("日志目录"))
        );
    }

    #[test]
    fn individual_enum_boolean_and_secretless_cloud_fields_have_exact_destinations() {
        for (label, target, row, group) in [
            ("显示秒数", "appearance.widget", 0, 0),
            ("界面主题", "appearance.overview", 0, 0),
            ("代码字体", "appearance.theme", 1, 1),
            ("表格交替行背景", "appearance.theme", 1, 2),
            ("免打扰开始", "notices.rules", 0, 0),
            ("寻址方式", "storage.images", 0, 0),
            ("同步目标", "sync", 0, 0),
        ] {
            let entry = search(label)
                .into_iter()
                .find(|entry| entry.label == label)
                .unwrap();
            assert_eq!(entry.target_id(), target);
            assert_eq!(entry.row, row);
            assert_eq!(entry.markdown_group, group);
            assert_eq!(entry.anchor, Some(label));
        }
    }

    #[test]
    fn empty_and_unmatched_queries_do_not_create_results() {
        assert!(search(" \t ").is_empty());
        assert!(search("absolutely-not-a-setting").is_empty());
    }
}
