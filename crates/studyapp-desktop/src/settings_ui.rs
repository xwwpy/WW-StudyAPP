//! Settings are edited as a copy. Only a successful worker commit replaces the
//! baseline; background snapshots must not overwrite unfinished input.
use crate::{
    app::{Page, StudyApp},
    components::*,
    controls::Button,
    preferences::{PreferenceSnapshot, SavePreferences},
    settings_cache::InputCache,
    settings_components as form,
    worker::Work,
};
use gpui_kit::{
    component::{
        Disableable, Sizable,
        button::ButtonVariants,
        input::{InputEvent, InputState},
    },
    *,
};
use std::{collections::BTreeMap, rc::Rc, time::Instant};

pub(crate) const MARKDOWN_FIELDS: [(&str, &str, &str, u8); 26] = [
    ("md-body", "body", "正文", 0),
    ("md-heading", "heading", "标题", 0),
    ("md-link", "link", "链接", 0),
    ("md-muted", "muted", "辅助文字", 0),
    ("md-quote", "quote", "引用文字", 1),
    ("md-quote-background", "quote_background", "引用背景", 1),
    ("md-code", "code", "代码文字", 1),
    ("md-code-background", "code_background", "代码背景", 1),
    ("md-table", "table", "表格文字", 2),
    ("md-table-header", "table_header", "表头文字", 2),
    ("md-table-background", "table_background", "表格背景", 2),
    ("md-table-stripe", "table_stripe", "交替行背景", 2),
    ("md-list", "list", "列表标记", 2),
    ("md-grid", "grid", "表格边线", 2),
    ("md-code-border", "code_border", "代码边线", 1),
    ("md-keyword", "syntax_keyword", "关键字", 3),
    ("md-string", "syntax_string", "字符串", 3),
    ("md-number", "syntax_number", "数字", 3),
    ("md-comment", "syntax_comment", "注释", 3),
    ("md-type", "syntax_type", "类型", 3),
    ("md-property", "syntax_property", "属性", 3),
    ("md-tag", "syntax_tag", "标签", 3),
    ("md-function", "syntax_function", "函数", 3),
    ("md-punctuation", "syntax_punctuation", "标点", 3),
    (
        "md-selection-background",
        "selection_background",
        "选区背景",
        0,
    ),
    ("md-selection-text", "selection_text", "编辑器选中文字", 0),
];

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum SettingsSection {
    #[default]
    Learning,
    Appearance,
    Notifications,
    Logging,
    Shortcuts,
    Storage,
    Sync,
    AboutAccount,
}

impl SettingsSection {
    pub const ALL: [(Self, &'static str, &'static str); 8] = [
        (Self::Learning, "learning", "学习"),
        (Self::Appearance, "appearance", "外观"),
        (Self::Notifications, "notifications", "通知"),
        (Self::Logging, "logging", "日志"),
        (Self::Shortcuts, "shortcuts", "快捷键"),
        (Self::Storage, "storage", "存储"),
        (Self::Sync, "sync", "同步"),
        (Self::AboutAccount, "about-account", "关于 / 账户"),
    ];
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AppearancePage {
    #[default]
    Overview,
    ThemeEditor,
    Workspace,
    Widget,
    Notifications,
}

impl AppearancePage {
    fn parts(self) -> &'static [usize] {
        match self {
            Self::Overview => &[0],
            Self::ThemeEditor => &[1, 3],
            Self::Workspace => &[4],
            Self::Widget | Self::Notifications => &[],
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StoragePage {
    #[default]
    Overview,
    Copies,
    Images,
    Library,
}

#[derive(Clone, Copy)]
enum SettingsResetScope {
    All,
    NotesSnapshots,
    NotesEditing,
    Selection,
    NoticeRegions,
    NoticeComponent(studyapp_core::NoticeStyle),
    Learning,
    Notifications,
    Logging,
    NotificationAppearance,
    Appearance,
    ThemeEditor,
    Workspace,
    Widget,
    Shortcuts,
    Images,
    Sync,
}

impl SettingsResetScope {
    fn name(self) -> &'static str {
        match self {
            Self::All => "全部设置（所有子页）",
            Self::NotesSnapshots => "笔记与编辑 · 快照策略",
            Self::NotesEditing => "笔记与编辑 · 编辑行为",
            Self::Selection => "外观 · 选中样式",
            Self::NoticeRegions => "通知与浮层 · 分区与堆叠",
            Self::NoticeComponent(style) => style.label(),
            Self::Learning => "学习",
            Self::Notifications => "通知",
            Self::Logging => "日志",
            Self::NotificationAppearance => "外观 · 通知",
            Self::Appearance => "外观 · 主题与阅读",
            Self::ThemeEditor => "外观 · 主题编辑器",
            Self::Workspace => "外观 · 工作区",
            Self::Widget => "外观 · 计时小组件",
            Self::Shortcuts => "快捷键",
            Self::Images => "存储 · 图片存储",
            Self::Sync => "同步",
        }
    }
}

pub struct ShortcutRecording {
    pub action: String,
    pub append: bool,
}

#[derive(Default)]
struct ShortcutAnalysis {
    bindings: BTreeMap<String, String>,
    conflicts: BTreeMap<String, Vec<String>>,
    errors: BTreeMap<String, String>,
    search: BTreeMap<String, String>,
}

type AppearanceInputKey = (Vec<SharedString>, studyapp_core::MarkdownLayout);
type AppearanceViewKey = (
    AppearanceInputKey,
    u8,
    studyapp_core::Settings,
    crate::theme::ResolvedTheme,
    bool,
    bool,
    Option<String>,
    u64,
    AppearancePage,
    Option<studyapp_core::NoticeStyle>,
);

type NotificationTaskKey = (String, AppearanceViewKey);

struct NotificationTaskView {
    owner: WeakEntity<StudyApp>,
    key: Option<NotificationTaskKey>,
}
impl Render for NotificationTaskView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let owner = self.owner.clone();
        container_query(move |available, _, cx| {
            owner
                .update(cx, |app, cx| {
                    scroll_body("settings-section-scroll", &app.settings_scroll)
                        .test_support()
                        .w(available.width)
                        .h(available.height)
                        .min_w_0()
                        .px(px(form::PANE_INSET))
                        .pt_2()
                        .pb_4()
                        .children(app.auxiliary_settings_page(
                            (available.width - px(form::PANE_INSET * 2.)).max(px(0.)),
                            cx,
                        ))
                })
                .map(|element| element.into_any_element())
                .unwrap_or_else(|_| column().into_any_element())
        })
        .min_w_0()
        .min_h_0()
    }
}

pub struct AppearanceView {
    owner: WeakEntity<StudyApp>,
    key: Option<AppearanceViewKey>,
}

impl Render for AppearanceView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let owner = self.owner.clone();
        container_query(move |available, window, cx| {
            owner
                .update(cx, |owner, cx| {
                    owner.appearance_workspace(available, window, cx)
                })
                .unwrap_or_else(|_| column())
        })
        .min_w_0()
        .min_h_0()
    }
}

struct MarkdownPreview {
    preset: studyapp_core::ThemePreset,
    colors: [u32; 26],
    palette_warnings: Vec<String>,
    markdown_warnings: Vec<studyapp_core::ContrastWarning>,
}

#[derive(PartialEq)]
struct ShortcutLayout {
    analysis_revision: u64,
    recording: Option<String>,
    recording_error: Option<String>,
    font_size: u16,
    spacing: u16,
}

pub enum SettingsContinuation {
    Page(Page),
    NewNote,
    Picker(&'static str, Option<String>),
    Migrate,
    MigrateApp,
    Close,
    InstallUpdate,
    InstallPackage,
    ViewNote(String),
    ConfirmDeleteCopy(String),
    ConfirmPurge(String),
    Record(crate::app::EditorMode),
    PickedWork(String, Box<Work>),
}

#[derive(Default)]
pub struct PreferencesUi {
    pub(crate) browsing: crate::settings_navigation::SettingsBrowsing,
    pub(crate) browsing_library: String,
    pub(crate) settings_drawer: bool,
    pub(crate) settings_tree_scroll: ScrollHandle,
    pub(crate) settings_resize: Option<(Pixels, u16)>,
    pub(crate) settings_search_open: bool,
    pub(crate) number_fields:
        std::cell::RefCell<BTreeMap<String, crate::settings_numbers::NumberField>>,
    pub(crate) pending_settings_scroll: std::cell::RefCell<Option<(String, ListOffset)>>,
    pub(crate) settings_search_scroll: ScrollHandle,
    pub(crate) review_stages: Vec<Entity<InputState>>,
    pub(crate) review_page: usize,
    pub(crate) list_levels_page: usize,
    pub(crate) widget_preset_stages: Vec<Entity<InputState>>,
    pub(crate) widget_preset_page: usize,
    pub search: Option<Entity<InputState>>,
    pub section: SettingsSection,
    pub appearance_page: AppearancePage,
    pub appearance_preview: Option<Entity<crate::appearance_preview::AppearancePreview>>,
    pub storage_page: StoragePage,
    pub storage_search: Option<Entity<InputState>>,
    pub section_revision: u64,
    pub section_changed_at: Option<Instant>,
    pub recording: Option<ShortcutRecording>,
    pub recording_focus: Option<FocusHandle>,
    pub recording_error: Option<String>,
    pub saved: Option<PreferenceSnapshot>,
    pub baseline: Option<PreferenceSnapshot>,
    pub draft: Option<studyapp_core::Settings>,
    pub shortcuts: BTreeMap<String, Entity<InputState>>,
    shortcut_analysis: InputCache<Vec<SharedString>, ShortcutAnalysis>,
    shortcut_rows: form::VirtualRows<ShortcutLayout>,
    #[cfg(feature = "ui-test")]
    pub shortcut_eager_baseline: bool,
    #[cfg(feature = "ui-test")]
    shortcut_row_builds: std::cell::Cell<u64>,
    #[cfg(feature = "ui-test")]
    shortcut_frame_rows: std::cell::Cell<u64>,
    pub shortcut_search: Option<Entity<InputState>>,
    pub logging_directory: Option<Entity<InputState>>,
    pub logging_picker: crate::folder_picker::FolderPicker,
    pub(crate) notification_view:
        Option<Entity<crate::notification_settings::NotificationSettingsView>>,
    pub(crate) notification_rows:
        form::VirtualRows<crate::notification_settings::NotificationViewKey>,
    #[cfg(feature = "ui-test")]
    pub(crate) notification_row_builds: std::cell::Cell<u64>,
    pub notice_component: Option<studyapp_core::NoticeStyle>,
    pub(crate) quiet_draft: Option<studyapp_core::QuietHours>,
    pub(crate) notification_page: usize,
    pub notification_category: Option<studyapp_core::NoticeCategory>,
    pub selection_target: Option<studyapp_core::SelectionCategory>,
    pub snapshot_fields: BTreeMap<&'static str, Entity<InputState>>,
    pub notification_fields: BTreeMap<&'static str, Entity<InputState>>,
    pub notification_preview_scroll: ScrollHandle,
    pub widget_fields: BTreeMap<&'static str, Entity<InputState>>,
    pub appearance_fields: BTreeMap<&'static str, Entity<InputState>>,
    pub appearance_colors: BTreeMap<&'static str, Entity<crate::settings_color::ColorSetting>>,
    markdown_preview: InputCache<AppearanceInputKey, MarkdownPreview>,
    appearance_rows: form::VirtualRows<AppearanceViewKey>,
    appearance_focus: Vec<FocusHandle>,
    appearance_view: Option<Entity<AppearanceView>>,
    notification_task_view: Option<Entity<NotificationTaskView>>,
    #[cfg(feature = "ui-test")]
    pub appearance_eager_baseline: bool,
    #[cfg(feature = "ui-test")]
    appearance_part_builds: std::cell::Cell<u64>,
    #[cfg(feature = "ui-test")]
    appearance_frame_parts: std::cell::Cell<u64>,
    pub appearance_touched: bool,
    pub editing_preset: Option<String>,
    pub(crate) theme_delete: Option<String>,
    pub appearance_layout: studyapp_core::MarkdownLayout,
    pub appearance_group: u8,
    pub image_root: Option<Entity<InputState>>,
    pub image_cloud: Option<crate::image_cloud::ImageCloudDraft>,
    pub(crate) image_cloud_search_reveal: bool,
    pub busy: bool,
    pub replacing: bool,
    pub leave: Option<SettingsContinuation>,
    pub reset_confirmation: bool,
    pub(crate) reset_all: bool,
    pub(crate) collapsed_previews: std::collections::BTreeSet<String>,
    pub error: Option<String>,
}

fn select_custom_preset(
    settings: &mut studyapp_core::AppearanceSettings,
    pending: Option<studyapp_core::ThemePreset>,
    target: &str,
) -> anyhow::Result<()> {
    let mut next = settings.clone();
    if let Some(pending) = pending {
        if let Some(existing) = next
            .custom_presets
            .iter_mut()
            .find(|preset| preset.id == pending.id)
        {
            *existing = pending;
        } else {
            next.custom_presets.push(pending);
        }
    }
    next.mode = studyapp_core::ThemeMode::Custom;
    next.active_preset = Some(target.to_owned());
    next.validate()?;
    *settings = next;
    Ok(())
}

impl StudyApp {
    pub(crate) fn settings_appearance_scroll(&self) -> ListState {
        self.preferences_ui.appearance_rows.scroll.clone()
    }
    pub(crate) fn settings_shortcut_scroll(&self) -> ListState {
        self.preferences_ui.shortcut_rows.scroll.clone()
    }

    fn settings_reset_scope(&self) -> Option<SettingsResetScope> {
        match self.preferences_ui.browsing.selected.as_str() {
            "notes.snapshots" => return Some(SettingsResetScope::NotesSnapshots),
            "notes.editing" => return Some(SettingsResetScope::NotesEditing),
            "appearance.selection" => return Some(SettingsResetScope::Selection),
            "notices.regions" => return Some(SettingsResetScope::NoticeRegions),
            "notices.general" => return Some(SettingsResetScope::NotificationAppearance),
            page if page.starts_with("notices.component.") => {
                return self
                    .preferences_ui
                    .notice_component
                    .map(SettingsResetScope::NoticeComponent);
            }
            _ => {}
        }
        Some(match self.preferences_ui.section {
            SettingsSection::Learning => SettingsResetScope::Learning,
            SettingsSection::Notifications => SettingsResetScope::Notifications,
            SettingsSection::Logging => SettingsResetScope::Logging,
            SettingsSection::Appearance => match self.preferences_ui.appearance_page {
                AppearancePage::Overview => SettingsResetScope::Appearance,
                AppearancePage::ThemeEditor => SettingsResetScope::ThemeEditor,
                AppearancePage::Workspace => SettingsResetScope::Workspace,
                AppearancePage::Widget => SettingsResetScope::Widget,
                AppearancePage::Notifications => SettingsResetScope::NotificationAppearance,
            },
            SettingsSection::Shortcuts => SettingsResetScope::Shortcuts,
            SettingsSection::Storage if self.preferences_ui.storage_page == StoragePage::Images => {
                SettingsResetScope::Images
            }
            SettingsSection::Sync => SettingsResetScope::Sync,
            SettingsSection::Storage | SettingsSection::AboutAccount => return None,
        })
    }

    fn reset_settings_page(
        &mut self,
        scope: SettingsResetScope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.preferences_ui.busy
            || self.preferences_ui.replacing
            || self.storage_busy
            || self.sync_panel.read(cx).status.busy
        {
            return;
        }
        let defaults = PreferenceSnapshot::defaults();
        if matches!(scope, SettingsResetScope::All) {
            self.fill_settings(defaults, window, cx);
            return;
        }
        let Some(draft) = &mut self.preferences_ui.draft else {
            return;
        };
        match scope {
            SettingsResetScope::All => unreachable!("handled above"),
            SettingsResetScope::NotesSnapshots => {
                draft.snapshots = defaults.settings.snapshots;
                self.fill_snapshot_fields(defaults.settings.snapshots, window, cx);
            }
            SettingsResetScope::NotesEditing => draft.appearance.list_style.indent_subtree = false,
            SettingsResetScope::Selection => {
                self.preferences_ui
                    .number_fields
                    .borrow_mut()
                    .retain(|key, _| !key.starts_with("selection-"));
                draft.appearance.selection = defaults.settings.appearance.selection.clone();
                self.fill_selection_fields(&defaults.settings.appearance.selection, window, cx);
            }
            SettingsResetScope::NoticeRegions => {
                draft.appearance.notifications.regions.clear();
                self.preferences_ui
                    .number_fields
                    .borrow_mut()
                    .retain(|key, _| !key.starts_with("region-"));
            }
            SettingsResetScope::NoticeComponent(style) => {
                self.preferences_ui
                    .number_fields
                    .borrow_mut()
                    .retain(|key, _| !key.ends_with(&format!("-{style:?}")));
                let appearance = &mut draft.appearance.notifications;
                appearance.layouts.remove(&style);
                appearance.surfaces.remove(&style);
                appearance.surface_patches.remove(&style);
                appearance.component_fields.remove(&style);
                let value = appearance.clone();
                self.fill_surface_fields(&value, Some(style), window, cx);
            }

            SettingsResetScope::Learning => {
                draft.interval_minutes = defaults.settings.interval_minutes;
                draft.answer_minutes = defaults.settings.answer_minutes;
                draft.review_days = defaults.settings.review_days.clone();
                self.interval.update(cx, |input, cx| {
                    input.set_value(defaults.settings.interval_minutes.to_string(), window, cx)
                });
                self.deadline.update(cx, |input, cx| {
                    input.set_value(defaults.settings.answer_minutes.to_string(), window, cx)
                });
                self.review_days.update(cx, |input, cx| {
                    input.set_value(
                        defaults
                            .settings
                            .review_days
                            .iter()
                            .map(u32::to_string)
                            .collect::<Vec<_>>()
                            .join(", "),
                        window,
                        cx,
                    )
                });
                self.fill_review_stages(&defaults.settings.review_days, window, cx);
            }
            SettingsResetScope::Notifications => {
                draft.notifications = defaults.settings.notifications.clone();
                self.fill_notification_times(&defaults.settings.notifications, window, cx);
            }
            SettingsResetScope::Logging => {
                draft.logging = defaults.settings.logging.clone();
                self.fill_logging_fields(&defaults.settings.logging, window, cx);
            }
            SettingsResetScope::NotificationAppearance => {
                let previous = &draft.appearance.notifications;
                let mut value = defaults.settings.appearance.notifications;
                value.layouts = previous.layouts.clone();
                value.surfaces = previous.surfaces.clone();
                value.surface_patches = previous.surface_patches.clone();
                value.component_fields = previous.component_fields.clone();
                value.scenario_fields = previous.scenario_fields.clone();
                value.regions = previous.regions.clone();
                value.tooltip_position = previous.tooltip_position;
                draft.appearance.notifications = value.clone();
                self.preferences_ui
                    .number_fields
                    .borrow_mut()
                    .retain(|key, _| !key.starts_with("notification-"));
                self.fill_notification_fields(&value, window, cx);
            }
            SettingsResetScope::Appearance => {
                self.preferences_ui
                    .number_fields
                    .borrow_mut()
                    .retain(|key, _| !key.starts_with("appearance-"));
                // Presets and their editor fields belong to the theme editor.
                draft.appearance.mode = defaults.settings.appearance.mode;
                draft.appearance.render_html = defaults.settings.appearance.render_html;
                draft.appearance.table_defaults = defaults.settings.appearance.table_defaults;
                let behavior = draft.appearance.list_style.indent_subtree;
                draft.appearance.list_style = defaults.settings.appearance.list_style;
                draft.appearance.list_style.indent_subtree = behavior;
                draft.appearance.font_size = defaults.settings.appearance.font_size;
                draft.appearance.spacing = defaults.settings.appearance.spacing;
            }
            SettingsResetScope::ThemeEditor => {
                let editing = self.preferences_ui.editing_preset.clone();
                self.fill_appearance_fields(&defaults.settings.appearance, window, cx);
                self.preferences_ui.editing_preset = editing;
                self.preferences_ui.appearance_touched = true;
            }
            SettingsResetScope::Widget => {
                self.preferences_ui
                    .number_fields
                    .borrow_mut()
                    .retain(|key, _| !key.starts_with("widget-"));
                let value = defaults.settings.timer_widget;
                draft.timer_widget = value.clone();
                self.fill_widget_fields(&value, window, cx);
            }
            SettingsResetScope::Workspace => {
                self.preferences_ui
                    .number_fields
                    .borrow_mut()
                    .retain(|key, _| !key.starts_with("workspace-"));
                draft.show_file_paths = defaults.settings.show_file_paths;
                draft.layout = defaults.settings.layout.clone();
                self.layout = defaults.settings.layout;
            }
            SettingsResetScope::Shortcuts => {
                self.preferences_ui.recording = None;
                self.preferences_ui.recording_error = None;
                for (action, binding) in defaults.shortcuts {
                    if let Some(input) = self.preferences_ui.shortcuts.get(&action) {
                        input.update(cx, |input, cx| input.set_value(binding, window, cx));
                    }
                }
            }
            SettingsResetScope::Images => {
                draft.images = defaults.settings.images.clone();
                if let Some(input) = &self.preferences_ui.image_root {
                    input.update(cx, |input, cx| input.set_value("", window, cx));
                }
                self.preferences_ui.image_cloud = Some(crate::image_cloud::ImageCloudDraft::new(
                    &defaults.settings.images,
                    false,
                    window,
                    cx,
                ));
            }
            SettingsResetScope::Sync => self
                .sync_panel
                .update(cx, |panel, cx| panel.reset_draft(defaults.sync, window, cx)),
        }
        self.preferences_ui.error = None;
        self.refresh_appearance(window, cx);
        cx.notify();
    }

    fn shortcut_analysis(&self, cx: &App) -> Rc<ShortcutAnalysis> {
        // Values are cheap shared strings. Programmatic set_value suppresses
        // InputEvent::Change, so capture/clear must invalidate by actual content.
        let values: Vec<_> = crate::shortcuts::SHORTCUT_ACTIONS
            .iter()
            .map(|action| {
                self.preferences_ui
                    .shortcuts
                    .get(action.id)
                    .map(|input| input.read(cx).value())
                    .unwrap_or_default()
            })
            .collect();
        self.preferences_ui.shortcut_analysis.get(values, |values| {
            let bindings: BTreeMap<_, _> = crate::shortcuts::SHORTCUT_ACTIONS
                .iter()
                .zip(values)
                .map(|(action, value)| (action.id.to_owned(), value.to_string()))
                .collect();
            let conflicts = crate::shortcuts::binding_conflicts(&bindings);
            let errors = bindings
                .iter()
                .filter_map(|(id, binding)| {
                    crate::shortcuts::normalize_binding(binding)
                        .err()
                        .map(|error| (id.clone(), error.to_string()))
                })
                .collect();
            let search = crate::shortcuts::SHORTCUT_ACTIONS
                .iter()
                .map(|action| {
                    (
                        action.id.to_owned(),
                        format!(
                            "{} {} {} {}",
                            action.label,
                            action.id,
                            action.group,
                            bindings.get(action.id).map_or("", String::as_str)
                        )
                        .to_lowercase(),
                    )
                })
                .collect();
            ShortcutAnalysis {
                bindings,
                conflicts,
                errors,
                search,
            }
        })
    }

    #[cfg(feature = "ui-test")]
    pub fn shortcut_analysis_rebuilds(&self) -> u64 {
        self.preferences_ui.shortcut_analysis.rebuilds()
    }

    #[cfg(feature = "ui-test")]
    pub fn shortcut_render_work(&self) -> (u64, u32) {
        (
            self.preferences_ui.shortcut_row_builds.get(),
            self.preferences_ui.shortcut_frame_rows.get().count_ones(),
        )
    }

    #[cfg(feature = "ui-test")]
    pub fn reset_shortcut_scroll(&self) {
        self.preferences_ui.shortcut_rows.scroll_to_top();
        self.preferences_ui.appearance_rows.scroll_to_top();
        self.settings_scroll.set_offset(point(px(0.), px(0.)));
    }

    fn markdown_preview(&self, cx: &Context<Self>) -> Rc<MarkdownPreview> {
        let values = self
            .preferences_ui
            .appearance_fields
            .iter()
            .map(|(key, input)| {
                self.preferences_ui
                    .appearance_colors
                    .get(key)
                    .map(|color| color.read(cx).preview_value(cx))
                    .unwrap_or_else(|| input.read(cx).value())
            })
            .collect();
        self.preferences_ui.markdown_preview.get(
            (values, self.preferences_ui.appearance_layout),
            |_| {
                let preset = self.appearance_input(true, cx);
                let colors = preset.markdown.resolved_colors(&preset.palette);
                let palette_warnings = preset
                    .palette
                    .contrast_warnings()
                    .into_iter()
                    .map(|warning| warning.to_string())
                    .collect();
                let markdown_warnings = preset.markdown.contrast_warnings(&preset.palette);
                MarkdownPreview {
                    preset,
                    colors,
                    palette_warnings,
                    markdown_warnings,
                }
            },
        )
    }

    #[cfg(feature = "ui-test")]
    pub fn appearance_preview_rebuilds(&self) -> u64 {
        self.preferences_ui.markdown_preview.rebuilds()
    }

    pub(crate) fn set_settings_section(
        &mut self,
        section: SettingsSection,
        cx: &mut Context<Self>,
    ) {
        for color in self.preferences_ui.appearance_colors.values() {
            color.update(cx, |color, cx| color.dismiss(cx));
        }
        if self.preferences_ui.section != section {
            self.preferences_ui.logging_picker.invalidate();
            self.preferences_ui.section = section;
            self.preferences_ui.section_revision =
                self.preferences_ui.section_revision.wrapping_add(1);
            self.preferences_ui.section_changed_at = Some(Instant::now());
            self.settings_scroll.set_offset(point(px(0.), px(0.)));
            self.preferences_ui.notification_rows.scroll_to_top();
            self.preferences_ui.shortcut_rows.scroll_to_top();
            self.preferences_ui.appearance_rows.scroll_to_top();
        }
        self.preferences_ui.recording = None;
        self.preferences_ui.recording_error = None;
        cx.notify();
    }

    pub fn effective_appearance(&self) -> studyapp_core::AppearanceSettings {
        if self.page == Page::Settings
            && let Some(draft) = &self.preferences_ui.draft
        {
            let mut appearance = draft.appearance.clone();
            if let Some(saved) = &self.preferences_ui.saved {
                // Functional editing behavior is not an appearance preview.
                appearance.list_style.indent_subtree =
                    saved.settings.appearance.list_style.indent_subtree;
            }
            return appearance;
        }
        self.preferences_ui
            .saved
            .as_ref()
            .map(|saved| saved.settings.appearance.clone())
            .unwrap_or_default()
    }

    fn fill_appearance_fields(
        &mut self,
        settings: &studyapp_core::AppearanceSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.preferences_ui.appearance_view.is_none() {
            let owner = cx.weak_entity();
            self.preferences_ui.appearance_view =
                Some(cx.new(|_| AppearanceView { owner, key: None }));
            self.preferences_ui.appearance_focus = (0..5).map(|_| cx.focus_handle()).collect();
        }
        if self.preferences_ui.notification_task_view.is_none() {
            let owner = cx.weak_entity();
            self.preferences_ui.notification_task_view =
                Some(cx.new(|_| NotificationTaskView { owner, key: None }));
        }
        if self.preferences_ui.appearance_preview.is_none() {
            self.preferences_ui.appearance_preview =
                Some(cx.new(|_| crate::appearance_preview::AppearancePreview::new()));
        }
        let preset = settings
            .custom_presets
            .iter()
            .find(|preset| Some(&preset.id) == settings.active_preset.as_ref());
        let palette = preset
            .map(|preset| preset.palette.clone())
            .unwrap_or_default();
        self.preferences_ui.editing_preset = preset.map(|preset| preset.id.clone());
        let markdown = preset
            .map(|preset| preset.markdown.clone())
            .unwrap_or_default();
        self.preferences_ui.appearance_layout = markdown.layout;
        let mut fields = vec![
            (
                "name",
                preset
                    .map(|preset| preset.name.clone())
                    .unwrap_or_else(|| "我的主题".into()),
            ),
            ("accent", palette.accent),
            ("background", palette.background),
            ("surface", palette.surface),
            ("text", palette.text),
        ];
        fields.extend(MARKDOWN_FIELDS.iter().map(|(key, token, _, _)| {
            (
                *key,
                markdown.colors.get(*token).cloned().unwrap_or_default(),
            )
        }));
        fields.extend([
            ("md-body-size", markdown.layout.body_size.to_string()),
            ("md-code-size", markdown.layout.code_size.to_string()),
            ("md-line-height", markdown.layout.line_height.to_string()),
            (
                "md-reading-width",
                markdown.layout.reading_width.to_string(),
            ),
            ("md-code-radius", markdown.layout.code_radius.to_string()),
        ]);
        for (key, value) in fields {
            if let Some(input) = self.preferences_ui.appearance_fields.get(key) {
                input.update(cx, |input, cx| input.set_value(value, window, cx));
            } else {
                let input = cx.new(|cx| InputState::new(window, cx).default_value(value));
                cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.preferences_ui.appearance_touched = true;
                        cx.notify();
                    }
                })
                .detach();
                self.preferences_ui.appearance_fields.insert(key, input);
            }
            let color_field = ["accent", "background", "surface", "text"].contains(&key)
                || MARKDOWN_FIELDS
                    .iter()
                    .any(|(color_key, _, _, _)| *color_key == key);
            if color_field {
                if let Some(color) = self.preferences_ui.appearance_colors.get(key) {
                    color.update(cx, |color, cx| color.dismiss(cx));
                } else {
                    let source = self.preferences_ui.appearance_fields[key].clone();
                    let color = cx.new(|cx| {
                        crate::settings_color::ColorSetting::new(
                            key,
                            source,
                            key.starts_with("md-"),
                            window,
                            cx,
                        )
                        .with_reset_value(match key {
                            "accent" => Some(studyapp_core::ThemePalette::default().accent),
                            "background" => Some(studyapp_core::ThemePalette::default().background),
                            "surface" => Some(studyapp_core::ThemePalette::default().surface),
                            "text" => Some(studyapp_core::ThemePalette::default().text),
                            _ => None,
                        })
                    });
                    cx.subscribe(
                        &color,
                        |this, _, _: &crate::settings_color::ColorSettingChanged, cx| {
                            this.preferences_ui.appearance_touched = true;
                            cx.notify();
                        },
                    )
                    .detach();
                    cx.subscribe(
                        &color,
                        |_, _, _: &crate::settings_color::ColorSettingPreviewChanged, cx| {
                            cx.notify()
                        },
                    )
                    .detach();
                    cx.subscribe(
                        &color,
                        |this, _, _: &crate::settings_color::ColorSettingPopupChanged, cx| {
                            let bounds = this.appearance_color_popup_shapes(cx);
                            if let Some(preview) = &this.preferences_ui.appearance_preview {
                                preview.update(cx, |preview, cx| {
                                    preview.set_overlay_shapes(
                                        "appearance-color-picker",
                                        bounds,
                                        cx,
                                    )
                                });
                            }
                            cx.notify();
                        },
                    )
                    .detach();
                    self.preferences_ui.appearance_colors.insert(key, color);
                }
            }
        }
        self.preferences_ui.appearance_touched = false;
    }

    fn custom_appearance_input(&self, cx: &Context<Self>) -> studyapp_core::ThemePreset {
        self.appearance_input(false, cx)
    }
    fn appearance_input(&self, preview: bool, cx: &Context<Self>) -> studyapp_core::ThemePreset {
        let value = |key| {
            if preview && let Some(color) = self.preferences_ui.appearance_colors.get(key) {
                return color.read(cx).preview_value(cx).trim().to_owned();
            }
            self.preferences_ui
                .appearance_fields
                .get(key)
                .map(|input| input.read(cx).value().trim().to_owned())
                .unwrap_or_default()
        };
        let id = self
            .preferences_ui
            .editing_preset
            .clone()
            .unwrap_or_else(|| format!("theme-{}", crate::preferences::unique_revision()));
        let mut layout = self.preferences_ui.appearance_layout;
        layout.body_size = value("md-body-size").parse().unwrap_or(u16::MAX);
        layout.code_size = value("md-code-size").parse().unwrap_or(u16::MAX);
        layout.line_height = value("md-line-height").parse().unwrap_or(u16::MAX);
        layout.reading_width = value("md-reading-width").parse().unwrap_or(u16::MAX);
        layout.code_radius = value("md-code-radius").parse().unwrap_or(u16::MAX);
        studyapp_core::ThemePreset {
            id,
            name: value("name"),
            markdown: studyapp_core::MarkdownStyle {
                colors: MARKDOWN_FIELDS
                    .iter()
                    .filter_map(|(key, token, _, _)| {
                        let color = value(*key);
                        (!color.is_empty()).then(|| ((*token).to_owned(), color))
                    })
                    .collect(),
                layout,
            },
            palette: studyapp_core::ThemePalette {
                accent: value("accent"),
                background: value("background"),
                surface: value("surface"),
                text: value("text"),
            },
        }
    }

    fn stage_custom_appearance(&mut self, cx: &mut Context<Self>) -> bool {
        let preset = self.custom_appearance_input(cx);
        let id = preset.id.clone();
        let Some(draft) = self.preferences_ui.draft.as_mut() else {
            return false;
        };
        match select_custom_preset(&mut draft.appearance, Some(preset), &id) {
            Ok(()) => {
                self.preferences_ui.editing_preset = Some(id);
                self.preferences_ui.appearance_touched = false;
                self.preferences_ui.error = None;
                cx.notify();
                true
            }
            Err(error) => {
                self.preferences_ui.error = Some(error.to_string());
                self.settings_validation_notice(error.to_string(), cx);
                cx.notify();
                false
            }
        }
    }

    pub fn start_shortcut_recording(
        &mut self,
        action: &str,
        append: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.preferences_ui.busy
            || self.preferences_ui.replacing
            || self.storage_busy
            || self.has_modal(cx)
        {
            return;
        }
        let focus = self
            .preferences_ui
            .recording_focus
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        self.preferences_ui.recording = Some(ShortcutRecording {
            action: action.into(),
            append,
        });
        self.preferences_ui.recording_error = None;
        self.preferences_ui.error = None;
        focus.focus(window, cx);
        cx.notify();
    }

    /// Call before native editing, application dispatch, and focus actions.
    /// A true return consumes the keystroke even when it was invalid.
    pub fn capture_settings_keystroke(
        &mut self,
        keystroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(recording) = self.preferences_ui.recording.as_ref() else {
            return false;
        };
        if self.page != Page::Settings
            || self.has_modal(cx)
            || self.preferences_ui.busy
            || self.preferences_ui.replacing
            || !self
                .preferences_ui
                .recording_focus
                .as_ref()
                .is_some_and(|focus| focus.is_focused(window))
        {
            self.preferences_ui.recording = None;
            cx.notify();
            return false;
        }
        let action = recording.action.clone();
        let append = recording.append;
        let modifiers = keystroke.modifiers;
        match crate::shortcuts::capture_key(
            &keystroke.key,
            modifiers.control,
            modifiers.alt,
            modifiers.shift,
            modifiers.platform,
        ) {
            crate::shortcuts::CaptureKey::Pending => {}
            crate::shortcuts::CaptureKey::Cancel => {
                self.preferences_ui.recording = None;
                self.preferences_ui.recording_error = None;
            }
            crate::shortcuts::CaptureKey::Invalid(error) => {
                self.preferences_ui.recording_error = Some(error)
            }
            crate::shortcuts::CaptureKey::Binding(binding) => {
                if let Some(input) = self.preferences_ui.shortcuts.get(&action) {
                    match crate::shortcuts::record_binding(
                        input.read(cx).value().as_str(),
                        &binding,
                        append,
                    ) {
                        Ok(binding) => {
                            input.update(cx, |input, cx| input.set_value(binding, window, cx));
                            self.preferences_ui.recording = None;
                            self.preferences_ui.recording_error = None;
                        }
                        Err(error) => self.preferences_ui.recording_error = Some(error.to_string()),
                    }
                }
            }
        }
        if self.preferences_ui.recording.is_none()
            && let Some(input) = self.preferences_ui.shortcut_search.as_ref()
        {
            input.read(cx).focus_handle(cx).focus(window, cx);
        }
        cx.notify();
        true
    }

    pub fn handle_shortcut(
        &mut self,
        keystroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.capture_settings_keystroke(keystroke, window, cx) {
            return true;
        }
        if self.preferences_ui.busy || self.preferences_ui.replacing || keystroke.modifiers.platform
        {
            return false;
        }
        if self.page == Page::Settings
            && !self.has_modal(cx)
            && keystroke.modifiers.control
            && !keystroke.modifiers.alt
            && !keystroke.modifiers.shift
            && keystroke.key.eq_ignore_ascii_case("f")
        {
            self.open_settings_search(window, cx);
            return true;
        }
        let modifiers = keystroke.modifiers;
        let Ok(combination) = crate::shortcuts::key_combination(
            &keystroke.key,
            modifiers.control,
            modifiers.alt,
            modifiers.shift,
        ) else {
            return false;
        };
        let Some(saved) = &self.preferences_ui.saved else {
            return false;
        };
        if crate::native_shortcuts::handle_native_edit(&combination, &saved.shortcuts, window, cx) {
            return true;
        }
        if self.has_modal(cx) {
            return false;
        }
        let action = saved.shortcuts.iter().find_map(|(action, binding)| {
            binding
                .split(';')
                .any(|alias| {
                    crate::shortcuts::normalize_binding(alias)
                        .is_ok_and(|alias| alias == combination)
                })
                .then(|| action.clone())
        });
        action.is_some_and(|action| self.dispatch_shortcut(&action, window, cx))
    }

    pub fn dispatch_shortcut(
        &mut self,
        action: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.has_modal(cx)
            || self.preferences_ui.busy
            || self.preferences_ui.replacing
            || self.storage_busy
        {
            return false;
        }
        match action {
            "app.timer-widget" => self.timer_action(
                studyapp_ui::timer_card::TimerCardAction::ShowOnDesktop,
                window,
                cx,
            ),
            "app.new" => self.new_note(window, cx),
            "app.import" => self.settings_picker("import", None, cx),
            "app.settings" => self.navigate(Page::Settings, window, cx),
            "app.library" => self.navigate(Page::Notes, window, cx),
            "app.navigation-toggle" => self.toggle_navigation(cx),
            "app.folders-toggle" if self.page == Page::Notes => self.toggle_folders(cx),
            "app.search" => {
                self.navigate(Page::Notes, window, cx);
                if self.page == Page::Notes {
                    self.search.read(cx).focus_handle(cx).focus(window, cx);
                }
            }
            "editor.save" if self.page == Page::Notes && self.note_editor.is_some() => {
                self.save_note(window, cx)
            }
            "editor.save" if self.page == Page::Settings => self.save_preferences(cx),
            _ => return false,
        }
        true
    }

    pub fn settings_dirty(&self, cx: &App) -> bool {
        let Some(baseline) = &self.preferences_ui.baseline else {
            return false;
        };
        self.setting_numbers_invalid(cx)
            || self
                .preferences_ui
                .appearance_colors
                .values()
                .any(|color| !color.read(cx).source_valid(cx))
            || self.preferences_ui.appearance_touched
            || self
                .preferences_ui
                .image_cloud
                .as_ref()
                .is_some_and(|draft| draft.dirty(&baseline.settings.images, cx))
            || self
                .preferences_ui
                .image_root
                .as_ref()
                .is_some_and(|input| {
                    input.read(cx).value().trim()
                        != baseline.settings.images.root.as_deref().unwrap_or("")
                })
            || self
                .preferences_ui
                .logging_directory
                .as_ref()
                .is_some_and(|input| {
                    input.read(cx).value().trim()
                        != baseline.settings.logging.directory.as_deref().unwrap_or("")
                })
            || self.notification_times_dirty(cx)
            || self.snapshot_fields_dirty(baseline.settings.snapshots, cx)
            || self.widget_fields_dirty(&baseline.settings.timer_widget, cx)
            || self.preferences_ui.draft.as_ref() != Some(&baseline.settings)
            || self.interval.read(cx).value().trim()
                != baseline.settings.interval_minutes.to_string()
            || self.deadline.read(cx).value().trim() != baseline.settings.answer_minutes.to_string()
            || self
                .review_days
                .read(cx)
                .value()
                .replace([' ', '，'], ",")
                .split(',')
                .filter(|part| !part.is_empty())
                .map(str::parse::<u32>)
                .collect::<Result<Vec<_>, _>>()
                .ok()
                .as_ref()
                != Some(&baseline.settings.review_days)
            || self.preferences_ui.shortcuts.iter().any(|(id, input)| {
                Some(input.read(cx).value().as_str())
                    != baseline.shortcuts.get(id).map(String::as_str)
            })
            || self.sync_panel.read(cx).has_unsaved_changes(cx)
    }

    fn fill_settings(
        &mut self,
        value: PreferenceSnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preferences_ui.recording = None;
        self.preferences_ui.recording_error = None;
        self.fill_widget_fields(&value.settings.timer_widget, window, cx);
        self.fill_snapshot_fields(value.settings.snapshots, window, cx);
        self.fill_review_stages(&value.settings.review_days, window, cx);
        self.fill_logging_fields(&value.settings.logging, window, cx);
        if self.preferences_ui.search.is_none() {
            let input = cx
                .new(|cx| InputState::new(window, cx).placeholder("搜索设置，如字号、同步、备份…"));
            cx.subscribe(&input, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            })
            .detach();
            self.preferences_ui.search = Some(input);
        }
        self.interval.update(cx, |input, cx| {
            input.set_value(value.settings.interval_minutes.to_string(), window, cx)
        });
        self.deadline.update(cx, |input, cx| {
            input.set_value(value.settings.answer_minutes.to_string(), window, cx)
        });
        self.review_days.update(cx, |input, cx| {
            input.set_value(
                value
                    .settings
                    .review_days
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(", "),
                window,
                cx,
            )
        });
        self.preferences_ui.number_fields.borrow_mut().clear();
        self.layout = value.settings.layout.clone();
        self.preferences_ui.draft = Some(value.settings.clone());
        self.fill_selection_fields(&value.settings.appearance.selection, window, cx);
        self.fill_surface_fields(&value.settings.appearance.notifications, None, window, cx);
        self.fill_appearance_fields(&value.settings.appearance, window, cx);
        self.fill_notification_fields(&value.settings.appearance.notifications, window, cx);
        self.fill_notification_times(&value.settings.notifications, window, cx);
        self.preferences_ui.image_cloud = Some(crate::image_cloud::ImageCloudDraft::new(
            &value.settings.images,
            value.image_secret_saved,
            window,
            cx,
        ));
        let root = value.settings.images.root.clone().unwrap_or_default();
        if let Some(input) = &self.preferences_ui.image_root {
            input.update(cx, |input, cx| input.set_value(root, window, cx));
        } else {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("留空使用资料库 images 目录")
                    .default_value(root)
            });
            cx.subscribe(&input, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            })
            .detach();
            self.preferences_ui.image_root = Some(input);
        }
        if self.preferences_ui.shortcut_search.is_none() {
            let input =
                cx.new(|cx| InputState::new(window, cx).placeholder("搜索操作、分组或快捷键"));
            cx.subscribe(&input, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            })
            .detach();
            self.preferences_ui.shortcut_search = Some(input);
        }
        if self.preferences_ui.storage_search.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("搜索托管副本名称"));
            cx.subscribe(&input, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            })
            .detach();
            self.preferences_ui.storage_search = Some(input);
        }
        for action in crate::shortcuts::SHORTCUT_ACTIONS {
            let binding = value.shortcuts.get(action.id).cloned().unwrap_or_default();
            if let Some(input) = self.preferences_ui.shortcuts.get(action.id) {
                input.update(cx, |input, cx| input.set_value(binding, window, cx));
            } else {
                let input = cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder("未绑定；例如 Ctrl+Shift+S")
                        .default_value(binding)
                });
                cx.subscribe(&input, |_, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        cx.notify();
                    }
                })
                .detach();
                self.preferences_ui
                    .shortcuts
                    .insert(action.id.into(), input);
            }
        }
        self.sync_panel.update(cx, |panel, cx| {
            panel.reset_draft(value.sync.clone(), window, cx)
        });
        self.preferences_ui.error = None;
        self.refresh_appearance(window, cx);
        cx.notify();
    }

    pub fn begin_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.preferences_ui.theme_delete = None;
        if let Some(saved) = self.preferences_ui.saved.clone() {
            self.fill_settings(saved.clone(), window, cx);
            self.preferences_ui.baseline = Some(saved);
            self.restore_settings_browsing(cx);
        }
    }

    pub fn accept_preferences(
        &mut self,
        preferences: PreferenceSnapshot,
        replaced: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.note_content.update(cx, |editor, cx| {
            editor.set_shortcuts(preferences.shortcuts.clone(), cx)
        });
        self.preferences_ui.saved = Some(preferences);
        if replaced || self.preferences_ui.baseline.is_none() {
            self.preferences_ui.leave = None;
            self.preferences_ui.busy = false;
            self.preferences_ui.replacing = self.pending_library.is_some();
            self.preferences_ui.reset_confirmation = false;
            self.begin_settings(window, cx);
        }
    }

    pub fn stage_layout(&mut self, cx: &mut Context<Self>) {
        if let Some(draft) = &mut self.preferences_ui.draft {
            draft.layout = self.layout.clone();
        }
        cx.notify();
    }

    pub fn guard_settings(
        &mut self,
        continuation: SettingsContinuation,
        cx: &mut Context<Self>,
    ) -> bool {
        self.preferences_ui.recording = None;
        if self.preferences_ui.busy || self.preferences_ui.replacing {
            return true;
        }
        if self.page == Page::Settings && self.settings_dirty(cx) {
            if self.preferences_ui.leave.is_none() {
                self.preferences_ui.leave = Some(continuation);
                self.preferences_ui.error = None;
            }
            cx.notify();
            return true;
        }
        false
    }

    pub fn navigate(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        if self.media_busy() {
            self.push_notice(Err("正在保存图片，请稍候再切换页面。".into()), cx);
            return;
        }
        if page == self.page || self.guard_settings(SettingsContinuation::Page(page), cx) {
            return;
        }
        if self.note_editor.is_some() {
            self.request_note_action(
                crate::editor_lifecycle::NoteIntent::Navigate(page),
                window,
                cx,
            );
            return;
        }
        if page == Page::Settings {
            self.begin_settings(window, cx);
        }
        if self.page == Page::Settings {
            self.persist_settings_browsing();
        }
        self.page = page;
        cx.notify();
    }

    pub fn settings_picker(
        &mut self,
        action: &'static str,
        note: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.media_busy() {
            self.push_notice(Err("正在保存图片，请稍候再操作资料库。".into()), cx);
            return;
        }
        if !self.guard_settings(SettingsContinuation::Picker(action, note.clone()), cx) {
            self.picker(action, note);
        }
    }

    fn resume_settings_action(
        &mut self,
        action: SettingsContinuation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            SettingsContinuation::Page(page) => self.navigate(page, window, cx),
            SettingsContinuation::NewNote => self.new_note(window, cx),
            SettingsContinuation::Picker(action, note) => self.picker(action, note),
            SettingsContinuation::Migrate => self.choose_storage(window, cx),
            SettingsContinuation::MigrateApp => self.choose_application_storage(window, cx),
            SettingsContinuation::Close => {
                self.timer_exit_requested = true;
                if self.handle_close(window, cx) {
                    self.close_allowed = true;
                    window.remove_window();
                }
            }
            SettingsContinuation::InstallUpdate => self.request_install_update(window, cx),
            SettingsContinuation::InstallPackage => self.install_nsis_update(window, cx),
            SettingsContinuation::ViewNote(id) => {
                self.select_note_document(id, None, true, window, cx);
            }
            SettingsContinuation::ConfirmDeleteCopy(id) => {
                self.storage_ui.purge = None;
                self.storage_confirm = Some(id);
                cx.notify();
            }
            SettingsContinuation::ConfirmPurge(id) => self.open_purge(id, cx),
            SettingsContinuation::Record(mode) => self.open_editor(mode, window, cx),
            SettingsContinuation::PickedWork(library, work) => {
                self.accept_picker_result(library, *work, cx)
            }
        }
    }

    pub fn accept_picker_result(&mut self, library: String, work: Work, cx: &mut Context<Self>) {
        if self.snapshot.is_none() && matches!(&work, Work::SwitchLibrary(_)) {
            self.startup_work(work, cx);
            return;
        }
        if library != self.last_library {
            self.push_notice(Err("资料库已切换，已取消旧文件选择操作。".into()), cx);
        } else if self.storage_busy
            || self.preferences_ui.busy
            || self.preferences_ui.replacing
            || self.media_jobs_active()
        {
            // Native pickers can finish out of order. Preserve their scoped
            // result until the operation owning the current locks completes.
            let _ = self.picker_sender.send((library, work));
        } else if self.page == Page::Settings && self.settings_dirty(cx) {
            self.guard_settings(
                SettingsContinuation::PickedWork(library, Box::new(work)),
                cx,
            );
        } else if !self.draft_work_ready
            && matches!(
                &work,
                Work::SwitchLibrary(_)
                    | Work::MigrateLibrary(_)
                    | Work::MigrateApplication(_)
                    | Work::Apply(studyapp_core::Command::Restore(_))
            )
        {
            self.draft_deferred_work = Some((library, work));
        } else if matches!(&work, Work::MigrateLibrary(_) | Work::MigrateApplication(_)) {
            self.storage_work(work, cx);
        } else {
            let replacing = matches!(
                &work,
                Work::SwitchLibrary(_) | Work::Apply(studyapp_core::Command::Restore(_))
            );
            if self
                .worker
                .sender
                .send(Work::Scoped {
                    library,
                    work: Box::new(work),
                })
                .is_err()
            {
                self.push_notice(Err("后台服务已停止，请重新打开应用。".into()), cx);
            } else if replacing {
                self.preferences_ui.replacing = true;
                self.sync_panel.update(cx, |panel, cx| {
                    panel.set_editing_locked(true, cx);
                });
            }
        }
        cx.notify();
    }

    pub fn discard_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.preferences_ui.busy {
            return;
        }
        let action = self.preferences_ui.leave.take();
        self.begin_settings(window, cx);
        if let Some(action) = action {
            self.resume_settings_action(action, window, cx);
        }
    }

    pub fn save_preferences(&mut self, cx: &mut Context<Self>) {
        #[cfg(feature = "ui-test")]
        eprintln!(
            "settings save handler entered: value={}, dirty={}, preferences_busy={}, replacing={}, recording={}, storage_busy={}, sync_busy={}",
            self.interval.read(cx).value(),
            self.settings_dirty(cx),
            self.preferences_ui.busy,
            self.preferences_ui.replacing,
            self.preferences_ui.recording.is_some(),
            self.storage_busy,
            self.sync_panel.read(cx).status.busy,
        );
        if self.preferences_ui.busy
            || self.preferences_ui.replacing
            || self.preferences_ui.recording.is_some()
            || self.storage_busy
            || self.sync_panel.read(cx).status.busy
        {
            return;
        }
        if !self.settings_dirty(cx) && self.preferences_ui.leave.is_none() {
            return;
        }
        if self.preferences_ui.appearance_touched {
            let mode = self
                .preferences_ui
                .draft
                .as_ref()
                .map(|draft| draft.appearance.mode)
                .unwrap_or_default();
            if !self.stage_custom_appearance(cx) {
                return;
            }
            if mode != studyapp_core::ThemeMode::Custom
                && let Some(draft) = &mut self.preferences_ui.draft
            {
                draft.appearance.mode = mode;
            }
        }
        let prepared = (|| -> anyhow::Result<SavePreferences> {
            anyhow::ensure!(
                self.preferences_ui
                    .appearance_colors
                    .values()
                    .all(|color| color.read(cx).source_valid(cx)),
                "颜色设置含无效输入，请填写 HEX 色值或恢复继承"
            );
            anyhow::ensure!(
                !self.setting_numbers_invalid(cx),
                "数字设置含无效输入，请填写各行标注范围内的整数"
            );
            let baseline = self
                .preferences_ui
                .baseline
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("配置尚未加载"))?;
            let mut settings = self
                .preferences_ui
                .draft
                .clone()
                .ok_or_else(|| anyhow::anyhow!("配置尚未加载"))?;
            settings.notifications =
                self.notification_settings_value(settings.notifications, cx)?;
            settings.snapshots = self.snapshot_settings_value(settings.snapshots, cx)?;
            settings.timer_widget =
                Box::new(self.widget_settings_value((*settings.timer_widget).clone(), cx)?);
            settings.interval_minutes = self
                .interval
                .read(cx)
                .value()
                .trim()
                .parse()
                .map_err(|_| anyhow::anyhow!("询问间隔请填写可安全表示的正整数分钟"))?;
            settings.answer_minutes = self
                .deadline
                .read(cx)
                .value()
                .trim()
                .parse()
                .map_err(|_| anyhow::anyhow!("回答期限请填写可安全表示的正整数分钟"))?;
            settings.review_days = self
                .review_days
                .read(cx)
                .value()
                .replace('，', ",")
                .split(',')
                .map(|part| {
                    part.trim()
                        .parse::<u32>()
                        .map_err(|_| anyhow::anyhow!("复习间隔请填写逗号分隔的正整数"))
                })
                .collect::<anyhow::Result<_>>()?;
            settings.logging.directory = self
                .preferences_ui
                .logging_directory
                .as_ref()
                .map(|input| input.read(cx).value().trim().to_owned())
                .filter(|value| !value.is_empty());
            settings.images.root = self
                .preferences_ui
                .image_root
                .as_ref()
                .map(|input| input.read(cx).value().trim().to_owned())
                .filter(|root| !root.is_empty());
            let image_secret = if let Some(image) = &self.preferences_ui.image_cloud {
                settings.images.cloud = image.target(cx);
                image.secret_change(cx)
            } else {
                Default::default()
            };
            settings.validate()?;
            let shortcuts = self
                .preferences_ui
                .shortcuts
                .iter()
                .map(|(id, input)| (id.clone(), input.read(cx).value().trim().to_owned()))
                .collect::<BTreeMap<_, _>>();
            crate::shortcuts::validate_bindings(&shortcuts)?;
            let (sync, password) = self.sync_panel.read(cx).draft_settings(cx)?;
            Ok(SavePreferences {
                expected_revision: baseline.revision.clone(),
                settings,
                shortcuts,
                sync,
                password,
                image_secret,
            })
        })();
        match prepared {
            Ok(prepared) => {
                if self
                    .worker
                    .sender
                    .send(Work::Scoped {
                        library: self.last_library.clone(),
                        work: Box::new(Work::SavePreferences(Box::new(prepared))),
                    })
                    .is_ok()
                {
                    self.preferences_ui.busy = true;
                    self.sync_panel.update(cx, |panel, cx| {
                        panel.set_editing_locked(true, cx);
                    });
                    self.preferences_ui.error = None;
                } else {
                    self.preferences_ui.error = Some("后台服务已停止，修改仍保留".into());
                    self.push_preferences_notice(Err("后台服务已停止，修改仍保留".into()), cx);
                }
            }
            Err(error) => {
                self.preferences_ui.error = Some(error.to_string());
                self.settings_validation_notice(error.to_string(), cx);
            }
        }
        cx.notify();
    }

    pub fn preferences_finished(
        &mut self,
        result: Result<(), String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.preferences_ui.busy {
            return;
        }
        self.preferences_ui.busy = false;
        self.sync_panel.update(cx, |panel, cx| {
            panel.set_editing_locked(false, cx);
        });
        match result {
            Ok(()) => {
                let action = self.preferences_ui.leave.take();
                self.begin_settings(window, cx);
                self.push_preferences_notice(Ok("全部设置已保存".into()), cx);
                if let Some(action) = action {
                    self.resume_settings_action(action, window, cx);
                }
            }
            Err(error) => {
                self.preferences_ui.error = Some(error.clone());
                self.push_preferences_notice(
                    Err(format!("设置未保存：{error}；当前修改仍保留")),
                    cx,
                );
            }
        }
        cx.notify();
    }

    pub fn settings_modal(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        use studyapp_ui::modal_frame::{ModalFrame, ModalTone, ModalWidth};
        if let Some(id) = &self.preferences_ui.theme_delete {
            let name = self
                .preferences_ui
                .draft
                .as_ref()
                .and_then(|draft| {
                    draft
                        .appearance
                        .custom_presets
                        .iter()
                        .find(|preset| &preset.id == id)
                })
                .map(|preset| preset.name.as_str())
                .unwrap_or("所选主题");
            let busy =
                self.preferences_ui.busy || self.preferences_ui.replacing || self.storage_busy;
            return Some(ModalFrame::new(format!("删除主题「{name}」的草稿？"), muted("仅从本次设置草稿中移除此主题；若正在使用，将切换为跟随系统。其他主题、笔记及学习记录保持不变。点击保存全部设置后保留，放弃修改可撤销。"))
                .tone(ModalTone::Warning).width(ModalWidth::Regular).scroll_handle(self.modal_focus.scroll())
                .footer(row().justify_end()
                    .child(Button::new("cancel-delete-theme").label("取消").disabled(busy).on_click(cx.listener(|this, _, _, cx| {
                        this.preferences_ui.theme_delete = None; cx.notify();
                    })))
                    .child(Button::new("confirm-delete-theme").primary().label("删除主题草稿").disabled(busy)
                        .on_click(cx.listener(|this, _, window, cx| this.confirm_theme_delete(window, cx)))))
                .into_any_element());
        }
        if self.preferences_ui.reset_confirmation {
            let scope = if self.preferences_ui.reset_all {
                SettingsResetScope::All
            } else {
                self.settings_reset_scope()?
            };
            return Some(ModalFrame::new(format!("恢复「{}」默认？", scope.name()), muted(if matches!(scope, SettingsResetScope::All) { "将所有设置子页恢复默认草稿，包括通知、快捷键和连接设置；保存后生效。笔记、历史、学习记录及已有文件不会删除。放弃修改可撤销此次恢复。" } else { "仅恢复本页选项为默认草稿；其他页面的修改保留。保存后生效，笔记、学习记录和已有文件保留。" }))
                .tone(ModalTone::Warning).width(ModalWidth::Regular).scroll_handle(self.modal_focus.scroll())
                .footer(row().justify_end()
                    .child(Button::new("cancel-reset-settings").label("取消").on_click(cx.listener(|this, _, _, cx| {
                        this.preferences_ui.reset_confirmation = false; cx.notify();
                    })))
                    .child(Button::new("confirm-reset-settings").primary().action("恢复本页默认草稿").on_click(cx.listener(move |this, _, window, cx| {
                        this.preferences_ui.reset_confirmation = false;
                        this.reset_settings_page(scope, window, cx);
                    }))))
                .into_any_element());
        }
        self.preferences_ui.leave.as_ref()?;
        let busy = self.preferences_ui.busy;
        Some(
            ModalFrame::new(
                "设置尚未保存",
                muted("是否保存本次修改后继续？放弃只撤销当前配置草稿。"),
            )
            .tone(ModalTone::Warning)
            .width(ModalWidth::Wide)
            .scroll_handle(self.modal_focus.scroll())
            .footer(
                row()
                    .flex_wrap()
                    .justify_end()
                    .child(
                        Button::new("settings-continue-editing")
                            .action("继续编辑")
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if matches!(
                                    this.preferences_ui.leave,
                                    Some(SettingsContinuation::InstallPackage)
                                ) {
                                    this.update_panel
                                        .update(cx, |panel, _| panel.cancel_install_preparation());
                                }
                                this.preferences_ui.leave = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("settings-discard")
                            .label("放弃修改")
                            .disabled(busy)
                            .on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.discard_settings(window, cx)
                                }),
                            ),
                    )
                    .child(
                        Button::new("settings-save-and-continue")
                            .primary()
                            .label(if busy {
                                "正在保存…"
                            } else {
                                "保存并继续"
                            })
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| this.save_preferences(cx))),
                    ),
            )
            .into_any_element(),
        )
    }

    pub fn settings_footer(&self, cx: &mut Context<Self>) -> Div {
        let busy = self.preferences_ui.busy || self.preferences_ui.replacing || self.storage_busy;
        let dirty = self.settings_dirty(cx);
        row()
            .w_full()
            .min_w_0()
            .items_center()
            .justify_between()
            .flex_wrap()
            .gap_2()
            .px(px(form::PANE_INSET))
            .py_2()
            .child(
                column()
                    .min_w_0()
                    .gap_1()
                    .child(muted(if dirty {
                        "有未保存修改 · 作用于全部分类"
                    } else {
                        "所有修改已保存"
                    }))
                    .child(
                        row()
                            .flex_wrap()
                            .gap_1()
                            .children(self.settings_reset_scope().map(|_| {
                                Button::new("reset-settings")
                                    .small()
                                    .ghost()
                                    .label("恢复本页默认…")
                                    .disabled(busy || self.sync_panel.read(cx).status.busy)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.preferences_ui.recording = None;
                                        this.preferences_ui.reset_all = false;
                                        this.preferences_ui.reset_confirmation = true;
                                        cx.notify();
                                    }))
                            }))
                            .child(
                                Button::new("reset-all-settings")
                                    .small()
                                    .ghost()
                                    .label("恢复全部默认…")
                                    .disabled(busy || self.sync_panel.read(cx).status.busy)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.preferences_ui.reset_all = true;
                                        this.preferences_ui.reset_confirmation = true;
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .child(
                row()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("discard-settings")
                            .small()
                            .ghost()
                            .label("放弃修改")
                            .disabled(busy || !dirty)
                            .on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.discard_settings(window, cx)
                                }),
                            ),
                    )
                    .child(studyapp_ui::notification_anchor::NotificationAnchor::new(
                        "settings-save",
                        self.notifications.clone(),
                        Button::new("save-settings")
                            .primary()
                            .small()
                            .label(if self.preferences_ui.busy {
                                "正在保存…"
                            } else {
                                "保存全部设置"
                            })
                            .disabled(
                                !dirty
                                    || busy
                                    || self.preferences_ui.saved.is_none()
                                    || self.preferences_ui.recording.is_some(),
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.save_preferences(cx))),
                    )),
            )
    }

    pub fn select_appearance_page(&mut self, page: AppearancePage, cx: &mut Context<Self>) {
        if self.preferences_ui.appearance_page == page {
            return;
        }
        for control in self.preferences_ui.appearance_colors.values() {
            control.update(cx, |control, cx| control.dismiss(cx));
        }
        self.preferences_ui.appearance_page = page;
        self.preferences_ui.appearance_rows.scroll_to_top();
        self.preferences_ui.section_revision += 1;
        cx.notify();
    }

    pub fn select_storage_page(&mut self, page: StoragePage, cx: &mut Context<Self>) {
        if self.preferences_ui.storage_page == page {
            return;
        }
        self.preferences_ui.storage_page = page;
        self.settings_scroll.set_offset(point(px(0.), px(0.)));
        cx.notify();
    }

    pub fn library_storage_controls(&self, path: &str, cx: &mut Context<Self>) -> Div {
        let busy = self.storage_busy || self.note_saving || self.sync_panel.read(cx).status.busy;
        form::section("资料库位置")
            .child(
                form::DirectoryField::new(
                    "library-directory-detail",
                    "当前资料库",
                    studyapp_desktop::presentation::display_path(path),
                )
                .actions(
                    row().child(
                        Button::new("migrate-library")
                            .small()
                            .action("更改位置")
                            .disabled(busy)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.choose_storage(window, cx)),
                            ),
                    ),
                ),
            )
            .child(muted(
                "更改位置会复制资料到新的空文件夹，验证完成后切换；原资料保留。",
            ))
            .child(form::settings_row(
                "其他资料库",
                Button::new("switch-library")
                    .small()
                    .action("选择资料库文件夹")
                    .disabled(busy)
                    .on_click(
                        cx.listener(|this, _, _, cx| this.settings_picker("library", None, cx)),
                    ),
            ))
            .child(form::settings_row(
                "备份与恢复",
                row()
                    .child(
                        Button::new("backup")
                            .small()
                            .action("导出备份")
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.settings_picker("backup", None, cx)
                            })),
                    )
                    .child(
                        Button::new("restore")
                            .small()
                            .action("从备份恢复")
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.settings_picker("restore", None, cx)
                            })),
                    ),
            ))
            .child(muted("备份恢复只恢复学习资料，保留当前设置。"))
    }

    pub fn preference_controls(&self, cx: &mut Context<Self>) -> Div {
        let busy = self.preferences_ui.busy || self.preferences_ui.replacing || self.storage_busy;
        match self.preferences_ui.section {
            SettingsSection::Learning => column()
                .gap_4()
                .child(
                    form::section("学习节奏")
                        .child(form::settings_row(
                            "询问间隔",
                            form::NumericSetting::new(
                                "learning-interval",
                                &self.interval,
                                "分钟",
                                60,
                                1,
                                u32::MAX.into(),
                                busy,
                            ),
                        ))
                        .child(form::settings_row(
                            "回答期限",
                            form::NumericSetting::new(
                                "learning-deadline",
                                &self.deadline,
                                "分钟",
                                5,
                                1,
                                u32::MAX.into(),
                                busy,
                            ),
                        ))
                        .child(muted(
                            "保存后生效；询问间隔从下一时间段生效，复习间隔只影响新建计划。",
                        )),
                )
                .child(self.review_stages(cx)),
            SettingsSection::Appearance => column(),
            SettingsSection::Notifications => column(),
            SettingsSection::Logging => self.logging_controls(cx),
            // The shortcut list owns the bounded viewport, outside this
            // auto-height form used by the other categories.
            SettingsSection::Shortcuts => column(),
            SettingsSection::Storage if self.preferences_ui.storage_page == StoragePage::Images => {
                self.image_settings_controls(cx)
            }
            _ => column(),
        }
    }

    fn markdown_style_preview(&self, cx: &mut Context<Self>) -> Div {
        use studyapp_core::ReadingFont;
        let preview = self.markdown_preview(cx);
        let preset = &preview.preset;
        let colors = &preview.colors;
        let p = &preset.markdown.layout;
        let color = |index| gpui_kit::rgb(colors[index]);
        let size = if p.body_size == 0 {
            self.effective_appearance().font_size
        } else {
            p.body_size.clamp(12, 28)
        } as f32;
        let code_size = if p.code_size == 0 {
            (size - 1.).max(12.)
        } else {
            p.code_size.clamp(12, 26) as f32
        };
        let font = |value| match value {
            ReadingFont::Sans => crate::design::UI_FONT,
            ReadingFont::Serif => "SimSun",
            ReadingFont::Mono => crate::design::MONO_FONT,
        };
        let sample = column()
            .id("theme-markdown-preview")
            .test_support()
            .gap_2()
            .p_3()
            .bg(gpui_kit::rgb(
                preset
                    .palette
                    .composited(crate::theme::palette().background)[1],
            ))
            .text_color(color(0))
            .text_size(px(size))
            .font_family(font(p.body_font))
            .line_height(px(size
                * if p.line_height == 0 {
                    f32::from(crate::theme::inherited_line_height(
                        self.effective_appearance().spacing,
                    )) / 100.
                } else {
                    p.line_height.clamp(130, 240) as f32 / 100.
                }))
            .child(
                div()
                    .text_size(px(size * 1.85))
                    .font_weight(FontWeight::BOLD)
                    .text_color(color(1))
                    .child("阅读样例 · Markdown"),
            )
            .child(
                row()
                    .flex_wrap()
                    .child("正文清晰易读，")
                    .child(div().text_color(color(2)).child("链接指向延伸阅读"))
                    .child(div().text_color(color(3)).child(" · 辅助文字")),
            )
            .child(
                div()
                    .p_2()
                    .border_l_2()
                    .border_color(color(13))
                    .bg(color(5))
                    .text_color(color(4))
                    .child("引用：知识来自观察、验证与思考。"),
            )
            .child(
                column()
                    .child(div().text_color(color(12)).child("• 一级要点"))
                    .child(div().pl_5().text_color(color(12)).child("1. 二级编号"))
                    .child(div().pl_10().child("☑ 三级任务")),
            )
            .child(
                column()
                    .p_2()
                    .rounded(px(p.code_radius.min(24) as f32))
                    .border(if p.code_border { px(1.) } else { px(0.) })
                    .border_color(color(14))
                    .bg(color(7))
                    .text_color(color(6))
                    .font_family(font(p.mono_font))
                    .text_size(px(code_size))
                    .child(
                        row()
                            .child(div().text_color(color(15)).child("const "))
                            .child("answer = ")
                            .child(div().text_color(color(17)).child("42"))
                            .child(";"),
                    )
                    .child(
                        div()
                            .text_color(color(18))
                            .child("// 保持代码与注释的可读性"),
                    ),
            )
            .child(
                column()
                    .border(if p.table_grid { px(1.) } else { px(0.) })
                    .border_color(color(13))
                    .child(
                        row()
                            .p_2()
                            .bg(gpui_kit::rgb(
                                preset
                                    .palette
                                    .composited(crate::theme::palette().background)[1],
                            ))
                            .text_color(color(9))
                            .child(div().flex_1().child("概念"))
                            .child(div().flex_1().child("进度")),
                    )
                    .child(
                        row()
                            .p_2()
                            .bg(color(10))
                            .text_color(color(8))
                            .child(div().flex_1().child("阅读"))
                            .child(div().flex_1().child("完成")),
                    )
                    .child(
                        row()
                            .p_2()
                            .bg(if p.table_stripes {
                                color(11)
                            } else {
                                color(10)
                            })
                            .text_color(color(8))
                            .child(div().flex_1().child("实践"))
                            .child(div().flex_1().child("进行中")),
                    ),
            );
        column()
            .gap_3()
            .child(heading("Markdown 样式"))
            .child(muted(
                "颜色留空会按主题自动适配；样例随输入更新。保存主题后可在笔记中使用。",
            ))
            .child(sample)
    }

    fn markdown_style_controls(&self, busy: bool, cx: &mut Context<Self>) -> Div {
        use studyapp_core::ReadingFont;
        let preview = self.markdown_preview(cx);
        let preset = &preview.preset;
        let colors = &preview.colors;
        let p = &preset.markdown.layout;
        let mut panel = column().gap_3().child(
            row().flex_wrap().children(
                ["文字与版式", "引用与代码", "表格与列表", "语法高亮"]
                    .into_iter()
                    .enumerate()
                    .map(|(index, label)| {
                        Button::new(("markdown-style-group", index))
                            .small()
                            .label(format!(
                                "{}{}",
                                if self.preferences_ui.appearance_group as usize == index {
                                    "✓ "
                                } else {
                                    ""
                                },
                                label
                            ))
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.preferences_ui.appearance_group = index as u8;
                                cx.notify();
                            }))
                    }),
            ),
        );
        for (index, (key, token, label, group)) in MARKDOWN_FIELDS.iter().enumerate() {
            if *group != self.preferences_ui.appearance_group {
                continue;
            }
            if let Some(input) = self.preferences_ui.appearance_fields.get(key) {
                let value = input.read(cx).value().trim().to_owned();
                let invalid =
                    !value.is_empty() && studyapp_core::parse_rgba_color(&value).is_none();
                let mut field =
                    form::settings_row(*label, self.color_control(key, colors[index], busy, cx));
                if invalid {
                    field = field.child(
                        muted("请输入 #RRGGBB 或 #RRGGBBAA，例如 #586D8380")
                            .text_color(rgb(crate::theme::palette().danger)),
                    );
                }
                if let Some(warning) = preview
                    .markdown_warnings
                    .iter()
                    .filter(|warning| warning.foreground == *token)
                    .min_by(|a, b| a.ratio.total_cmp(&b.ratio))
                {
                    field = field.child(
                        muted(format!(
                            "{label}对比度 {:.1}:1，可能不易阅读；仍可使用并保存。",
                            warning.ratio
                        ))
                        .text_color(rgb(crate::theme::palette().warning)),
                    );
                }
                panel = panel.child(field);
            }
        }
        let group = self.preferences_ui.appearance_group;
        let body_size = if p.body_size == 0 {
            self.effective_appearance().font_size
        } else {
            p.body_size.clamp(12, 28)
        };
        for (key, label, unit, min, max, default, field_group, inheritance) in [
            (
                "md-body-size",
                "正文字号",
                "px",
                12,
                28,
                0,
                0,
                Some((
                    u64::from(self.effective_appearance().font_size),
                    format!(
                        "继承界面字号 · 当前有效值 {} px",
                        self.effective_appearance().font_size
                    ),
                )),
            ),
            (
                "md-line-height",
                "行高",
                "%",
                130,
                240,
                0,
                0,
                Some((
                    u64::from(crate::theme::inherited_line_height(
                        self.effective_appearance().spacing,
                    )),
                    format!(
                        "继承界面间距换算 · 当前有效值 {}%",
                        crate::theme::inherited_line_height(self.effective_appearance().spacing)
                    ),
                )),
            ),
            (
                "md-reading-width",
                "阅读宽度",
                "px",
                480,
                1600,
                0,
                0,
                Some((480, "跟随可用宽度 · 自适应剩余空间".into())),
            ),
            (
                "md-code-size",
                "代码字号",
                "px",
                12,
                26,
                0,
                1,
                Some((
                    u64::from(body_size.saturating_sub(1).max(12)),
                    format!(
                        "继承正文字号减 1 · 当前有效值 {} px",
                        body_size.saturating_sub(1).max(12)
                    ),
                )),
            ),
            ("md-code-radius", "代码圆角", "px", 0, 24, 8, 1, None),
        ] {
            if group != field_group {
                continue;
            }
            if let Some(input) = self.preferences_ui.appearance_fields.get(key) {
                let mut control =
                    form::NumericSetting::new(key, input, unit, default, min, max, busy);
                if let Some((custom, description)) = inheritance {
                    control = control.inherit_zero(custom, description);
                }
                panel = panel.child(form::settings_row(label, control));
            }
        }
        if group <= 1 {
            let selected = if group == 0 { p.body_font } else { p.mono_font };
            panel = panel.child(form::settings_row(
                if group == 0 {
                    "正文字体"
                } else {
                    "代码字体"
                },
                form::choice_with_default(
                    "markdown-font",
                    (
                        selected,
                        if group == 0 {
                            ReadingFont::Sans
                        } else {
                            ReadingFont::Mono
                        },
                    ),
                    vec![
                        (ReadingFont::Sans, "无衬线".into()),
                        (ReadingFont::Serif, "衬线".into()),
                        (ReadingFont::Mono, "等宽".into()),
                    ],
                    busy,
                    cx.weak_entity(),
                    move |this, font, _, cx| {
                        if group == 0 {
                            this.preferences_ui.appearance_layout.body_font = font;
                        } else {
                            this.preferences_ui.appearance_layout.mono_font = font;
                        }
                        this.preferences_ui.appearance_touched = true;
                        cx.notify();
                    },
                ),
            ));
        }
        for (key, label, checked, field_group) in [
            ("grid", "显示表格网格", p.table_grid, 2),
            ("stripes", "表格交替行背景", p.table_stripes, 2),
            ("border", "显示代码边框", p.code_border, 1),
        ] {
            if group == field_group {
                panel = panel.child(
                    studyapp_ui::settings_switch::SettingSwitch::new(SharedString::from(format!(
                        "markdown-{key}"
                    )))
                    .default_value(true)
                    .label(label)
                    .checked(checked)
                    .disabled(busy)
                    .on_click(cx.listener(move |this, value, _, cx| {
                        let style = &mut this.preferences_ui.appearance_layout;
                        match key {
                            "grid" => style.table_grid = *value,
                            "stripes" => style.table_stripes = *value,
                            _ => style.code_border = *value,
                        }
                        this.preferences_ui.appearance_touched = true;
                        cx.notify();
                    })),
                );
            }
        }
        panel.child(
            row()
                .flex_wrap()
                .child(
                    Button::new("reset-markdown-style")
                        .small()
                        .action("恢复 Markdown 默认样式")
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, window, cx| {
                            let mut preset = this.custom_appearance_input(cx);
                            preset.markdown = Default::default();
                            let settings = studyapp_core::AppearanceSettings {
                                active_preset: Some(preset.id.clone()),
                                custom_presets: vec![preset],
                                ..Default::default()
                            };
                            this.fill_appearance_fields(&settings, window, cx);
                            this.preferences_ui.appearance_touched = true;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("discard-theme-fields")
                        .small()
                        .label("放弃本次编辑")
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, window, cx| {
                            let settings = this
                                .preferences_ui
                                .draft
                                .as_ref()
                                .map(|draft| draft.appearance.clone())
                                .unwrap_or_default();
                            this.fill_appearance_fields(&settings, window, cx);
                            cx.notify();
                        })),
                ),
        )
    }

    fn color_control(
        &self,
        key: &str,
        inherited: u32,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(color) = self.preferences_ui.appearance_colors.get(key) else {
            return div().into_any_element();
        };
        color.update(cx, |color, cx| color.configure(inherited, busy, cx));
        color.clone().into_any_element()
    }

    pub fn appearance_color_popup_shapes(
        &self,
        cx: &App,
    ) -> Vec<studyapp_ui::bubble::OverlayShape> {
        self.preferences_ui
            .appearance_colors
            .values()
            .flat_map(|color| color.read(cx).popup_shapes())
            .collect()
    }

    #[cfg(feature = "ui-test")]
    pub fn appearance_color_popup_bounds(&self, cx: &App) -> Vec<Bounds<Pixels>> {
        self.preferences_ui
            .appearance_colors
            .values()
            .filter_map(|color| color.read(cx).popup_bounds())
            .collect()
    }

    pub fn appearance_preview_visible(&self, _: &Window, cx: &App) -> bool {
        self.page == Page::Settings
            && !self.settings_search_active(cx)
            && !self
                .preferences_ui
                .collapsed_previews
                .contains("reading-preview")
            && self.preferences_ui.section == SettingsSection::Appearance
            && self.preferences_ui.appearance_page == AppearancePage::ThemeEditor
    }

    pub fn appearance_preview_theme(&self, cx: &Context<Self>) -> crate::theme::ResolvedTheme {
        let mut appearance = self.effective_appearance();
        let preset = self.appearance_input(true, cx);
        appearance.mode = studyapp_core::ThemeMode::Custom;
        appearance.active_preset = Some(preset.id.clone());
        appearance.custom_presets = vec![preset];
        crate::theme::resolve(&appearance, crate::theme::palette().dark)
    }

    fn appearance_view_key(&self, cx: &App) -> AppearanceViewKey {
        #[cfg(not(feature = "ui-test"))]
        let eager = false;
        #[cfg(feature = "ui-test")]
        let eager = self.preferences_ui.appearance_eager_baseline;
        (
            (
                self.preferences_ui
                    .appearance_fields
                    .iter()
                    .map(|(key, input)| {
                        self.preferences_ui
                            .appearance_colors
                            .get(key)
                            .map(|color| color.read(cx).preview_value(cx))
                            .unwrap_or_else(|| input.read(cx).value())
                    })
                    .chain(
                        self.preferences_ui
                            .notification_fields
                            .iter()
                            .map(|(key, input)| {
                                self.preferences_ui
                                    .appearance_colors
                                    .get(key)
                                    .map(|color| color.read(cx).preview_value(cx))
                                    .unwrap_or_else(|| input.read(cx).value())
                            }),
                    )
                    .collect(),
                self.preferences_ui.appearance_layout,
            ),
            self.preferences_ui.appearance_group,
            self.preferences_ui.draft.clone().unwrap_or_default(),
            crate::theme::palette(),
            self.preferences_ui.busy || self.preferences_ui.replacing || self.storage_busy,
            eager,
            self.preferences_ui.editing_preset.clone(),
            self.preferences_ui.section_revision,
            self.preferences_ui.appearance_page,
            self.preferences_ui.notice_component,
        )
    }

    #[cfg(all(feature = "ui-test", test))]
    pub fn notification_appearance_cache_matches(&self, cx: &App) -> bool {
        self.preferences_ui
            .appearance_view
            .as_ref()
            .is_some_and(|view| view.read(cx).key.as_ref() == Some(&self.appearance_view_key(cx)))
    }

    fn notification_task_key(&self, cx: &App) -> NotificationTaskKey {
        (
            self.preferences_ui.browsing.selected.clone(),
            self.appearance_view_key(cx),
        )
    }

    #[cfg(all(feature = "ui-test", any(windows, test)))]
    pub(crate) fn notification_task_cache_matches(&self, cx: &App) -> bool {
        self.preferences_ui
            .notification_task_view
            .as_ref()
            .is_some_and(|view| view.read(cx).key.as_ref() == Some(&self.notification_task_key(cx)))
    }

    pub(crate) fn notification_task_surface(&self, cx: &mut Context<Self>) -> Div {
        let Some(view) = self.preferences_ui.notification_task_view.as_ref() else {
            return column();
        };
        let key = self.notification_task_key(cx);
        view.update(cx, |view, cx| {
            if view.key.as_ref() != Some(&key) {
                view.key = Some(key);
                cx.notify();
            }
        });
        // Bound the real task page, keeping its existing non-virtual scroll,
        // responsive card grid and fixed settings header/footer unchanged.
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .w_full()
            .child(
                div()
                    .id("settings-notification-task-surface")
                    .test_support()
                    .size_full()
                    .child(view.clone().cached(StyleRefinement::default().size_full())),
            )
    }

    /// A definite-size view boundary lets GPUI reuse unchanged appearance
    /// layout/paint when only the parent shell (timer, notices) is notified.
    pub fn appearance_surface(&self, cx: &mut Context<Self>) -> Div {
        let Some(view) = self.preferences_ui.appearance_view.as_ref() else {
            return column();
        };
        let key = self.appearance_view_key(cx);
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
                    .id("settings-appearance-surface")
                    .test_support()
                    .size_full()
                    .child(view.clone().cached(StyleRefinement::default().size_full())),
            )
    }

    fn appearance_workspace(
        &self,
        available: Size<Pixels>,
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        if self.preferences_ui.appearance_page == AppearancePage::Notifications {
            self.preferences_ui.appearance_rows.sync_with_height(
                &[0, 1, 2],
                self.appearance_view_key(cx),
                px(320.),
            );
            self.apply_settings_list_offset(&self.preferences_ui.appearance_rows.scroll);
            self.preferences_ui.appearance_rows.track_focuses(
                (0..3)
                    .map(|row| (row, self.preferences_ui.appearance_focus[row].clone()))
                    .collect(),
            );
            let owner = cx.entity();
            return column()
                .w(available.width)
                .h(available.height)
                .min_h_0()
                .min_w_0()
                .child(form::virtual_rows(
                    &self.preferences_ui.appearance_rows.scroll,
                    move |row, _, cx| {
                        owner
                            .update(cx, |app, cx| {
                                div()
                                    .py_2()
                                    .track_focus(&app.preferences_ui.appearance_focus[row])
                                    .child(app.notification_appearance_part(row, cx))
                            })
                            .into_any_element()
                    },
                ));
        }
        if self.preferences_ui.appearance_page == AppearancePage::Widget {
            return column()
                .w(available.width)
                .h(available.height)
                .min_h_0()
                .min_w_0()
                .overflow_hidden()
                .child(
                    scroll_body("timer-widget-settings-scroll", &self.settings_scroll)
                        .test_support()
                        .flex_1()
                        .min_h_0()
                        .pr_4()
                        .pb_4()
                        .child(self.widget_settings_controls(available.width, cx)),
                );
        }
        // Keep programmatic field changes reflected even when the sample is
        // outside the viewport; cached resolution is independent of row layout.
        let _ = self.markdown_preview(cx);
        let key = self.appearance_view_key(cx);
        let parts = self.preferences_ui.appearance_page.parts();
        self.preferences_ui
            .appearance_rows
            .sync_with_height(parts, key, px(320.));
        self.apply_settings_list_offset(&self.preferences_ui.appearance_rows.scroll);
        self.preferences_ui.appearance_rows.track_focuses(
            parts
                .iter()
                .enumerate()
                .map(|(row, &part)| (row, self.preferences_ui.appearance_focus[part].clone()))
                .collect(),
        );
        #[cfg(feature = "ui-test")]
        self.preferences_ui.appearance_frame_parts.set(0);
        let owner = cx.entity();
        let parameters = column()
            .id("appearance-parameters")
            .test_support()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(form::virtual_rows(
                &self.preferences_ui.appearance_rows.scroll,
                move |row, _, cx| {
                    let part = parts[row];
                    owner
                        .update(cx, |owner, cx| {
                            div()
                                .py_2()
                                .track_focus(&owner.preferences_ui.appearance_focus[part])
                                .child(owner.appearance_part(
                                    part,
                                    (available.width - px(32.)).max(px(0.)),
                                    cx,
                                ))
                        })
                        .into_any_element()
                },
            ));
        #[cfg(feature = "ui-test")]
        let parameters = if self.preferences_ui.appearance_eager_baseline {
            column()
                .id("appearance-parameters")
                .test_support()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .child(
                    scroll_body("settings-section-scroll", &self.settings_scroll)
                        .test_support()
                        .flex_1()
                        .min_h_0()
                        .gap_0()
                        .px_4()
                        .children(parts.iter().map(|&part| {
                            div().py_2().child(self.appearance_part(
                                part,
                                (available.width - px(32.)).max(px(0.)),
                                cx,
                            ))
                        })),
                )
        } else {
            parameters
        };
        column().size_full().min_w_0().min_h_0().child(parameters)
    }

    fn theme_preset_controls(&self, cx: &mut Context<Self>) -> Div {
        let fallback = studyapp_core::AppearanceSettings::default();
        let appearance = self
            .preferences_ui
            .draft
            .as_ref()
            .map(|draft| &draft.appearance)
            .unwrap_or(&fallback);
        let busy = self.preferences_ui.busy || self.preferences_ui.replacing || self.storage_busy;
        let mut presets = column().gap_3().child(heading("主题预设"));
        let builtins = form::settings_row(
            "内置主题",
            form::choice(
                "builtin-theme-preset",
                appearance.active_preset.clone().unwrap_or_default(),
                studyapp_core::builtin_theme_presets()
                    .into_iter()
                    .map(|preset| (preset.id, preset.name))
                    .collect(),
                busy,
                cx.weak_entity(),
                |this, id, window, cx| {
                    if this.preferences_ui.busy
                        || this.preferences_ui.replacing
                        || this.storage_busy
                    {
                        return;
                    }
                    let Some(preset) = studyapp_core::builtin_theme_presets()
                        .into_iter()
                        .find(|preset| preset.id == id)
                    else {
                        return;
                    };
                    if this.preferences_ui.appearance_touched && !this.stage_custom_appearance(cx) {
                        return;
                    }
                    let settings = if let Some(draft) = &mut this.preferences_ui.draft {
                        if let Err(error) =
                            select_custom_preset(&mut draft.appearance, Some(preset), &id)
                        {
                            this.preferences_ui.error = Some(error.to_string());
                            this.settings_validation_notice(error.to_string(), cx);
                            cx.notify();
                            return;
                        }
                        Some(draft.appearance.clone())
                    } else {
                        None
                    };
                    if let Some(settings) = settings {
                        this.fill_appearance_fields(&settings, window, cx);
                    }
                    this.refresh_appearance(window, cx);
                    cx.notify();
                },
            ),
        );
        presets = presets
            .child(builtins)
            .child(muted("从菜单选择后可继续自定义；保存全部设置后生效。"));
        if appearance.custom_presets.is_empty() {
            presets = presets.child(form::settings_row(
                "自定义主题",
                muted("尚无自定义主题；编辑后点击「应用到界面」即可保留在本次草稿中。"),
            ));
        } else {
            presets = presets.child(form::settings_row(
                "自定义主题",
                form::choice(
                    "custom-theme-preset",
                    appearance.active_preset.clone().unwrap_or_default(),
                    appearance
                        .custom_presets
                        .iter()
                        .map(|preset| (preset.id.clone(), preset.name.clone()))
                        .collect(),
                    busy,
                    cx.weak_entity(),
                    |this, id, window, cx| {
                        if this.preferences_ui.busy
                            || this.preferences_ui.replacing
                            || this.storage_busy
                        {
                            return;
                        }
                        let pending = this
                            .preferences_ui
                            .appearance_touched
                            .then(|| this.custom_appearance_input(cx));
                        let settings = if let Some(draft) = &mut this.preferences_ui.draft {
                            if let Err(error) =
                                select_custom_preset(&mut draft.appearance, pending, &id)
                            {
                                this.preferences_ui.error = Some(error.to_string());
                                this.settings_validation_notice(error.to_string(), cx);
                                cx.notify();
                                return;
                            }
                            this.preferences_ui.error = None;
                            Some(draft.appearance.clone())
                        } else {
                            None
                        };
                        if let Some(settings) = settings {
                            this.fill_appearance_fields(&settings, window, cx);
                        }
                        this.refresh_appearance(window, cx);
                        cx.notify();
                    },
                ),
            ));
        }
        presets = presets.child(
            row()
                .flex_wrap()
                .border_t_1()
                .border_color(crate::theme::color(BORDER))
                .pt_3()
                .relative()
                .child(studyapp_ui::settings_anchor::probe("主题草稿操作"))
                .child(
                    Button::new("preview-custom-theme")
                        .small()
                        .action("应用到界面")
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, window, cx| {
                            if this.stage_custom_appearance(cx) {
                                this.refresh_appearance(window, cx);
                            }
                        })),
                )
                .child(
                    Button::new("new-custom-theme")
                        .small()
                        .action("+ 新建")
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, window, cx| {
                            if this.preferences_ui.appearance_touched
                                && !this.stage_custom_appearance(cx)
                            {
                                return;
                            }
                            let mut settings = this
                                .preferences_ui
                                .draft
                                .as_ref()
                                .map(|draft| draft.appearance.clone())
                                .unwrap_or_default();
                            settings.active_preset = None;
                            this.fill_appearance_fields(&settings, window, cx);
                            cx.notify();
                        })),
                ),
        );
        let selected = appearance.active_preset.as_ref().and_then(|id| {
            appearance
                .custom_presets
                .iter()
                .find(|preset| &preset.id == id)
        });
        presets.child(
            row()
                .flex_wrap()
                .border_t_1()
                .border_color(crate::theme::color(BORDER))
                .pt_3()
                .relative()
                .child(studyapp_ui::settings_anchor::probe("删除主题草稿"))
                .child(muted(
                    "删除仅修改主题草稿，需单独确认；保存全部设置后保留。",
                ))
                .child(
                    Button::new("delete-custom-theme")
                        .small()
                        .icon_action(gpui_kit::assets::IconName::Trash, "删除所选主题草稿…")
                        .disabled(busy || selected.is_none())
                        .on_click({
                            let id = selected.map(|preset| preset.id.clone());
                            cx.listener(move |this, _, _, cx| {
                                if this.preferences_ui.busy
                                    || this.preferences_ui.replacing
                                    || this.storage_busy
                                    || this.has_required_modal(cx)
                                {
                                    return;
                                }
                                this.preferences_ui.theme_delete = id.clone();
                                cx.notify();
                            })
                        }),
                ),
        )
    }

    fn confirm_theme_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.preferences_ui.busy || self.preferences_ui.replacing || self.storage_busy {
            return;
        }
        let Some(id) = self.preferences_ui.theme_delete.take() else {
            return;
        };
        let Some(draft) = &mut self.preferences_ui.draft else {
            return;
        };
        if !draft
            .appearance
            .custom_presets
            .iter()
            .any(|preset| preset.id == id)
        {
            cx.notify();
            return;
        }
        draft
            .appearance
            .custom_presets
            .retain(|preset| preset.id != id);
        if draft.appearance.active_preset.as_ref() == Some(&id) {
            draft.appearance.active_preset = None;
            draft.appearance.mode = studyapp_core::ThemeMode::System;
        }
        let settings = draft.appearance.clone();
        if self.preferences_ui.editing_preset.as_ref() == Some(&id) {
            self.fill_appearance_fields(&settings, window, cx);
        }
        self.refresh_appearance(window, cx);
        cx.notify();
    }

    // Part of the theme card's scrollable content, never a fixed third pane or
    // a bottom region stealing height from the parameter editor.
    fn inline_reading_preview(&self, cx: &mut Context<Self>) -> Div {
        let collapsed = self
            .preferences_ui
            .collapsed_previews
            .contains("reading-preview");
        column()
            .gap_2()
            .child(heading("阅读预览"))
            .flex_shrink_0()
            .gap_2()
            .child(
                row()
                    .w_full()
                    .child(muted("使用真实正文渲染器，只预览当前草稿。"))
                    .child(div().flex_1())
                    .child(
                        Button::new("toggle-reading-preview")
                            .small()
                            .ghost()
                            .label(if collapsed {
                                "展开预览"
                            } else {
                                "收起预览"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                if !this
                                    .preferences_ui
                                    .collapsed_previews
                                    .remove("reading-preview")
                                {
                                    this.preferences_ui
                                        .collapsed_previews
                                        .insert("reading-preview".into());
                                }
                                this.preferences_ui.section_revision += 1;
                                cx.notify();
                            })),
                    ),
            )
            .children((!collapsed).then(|| {
                div()
                    .id("appearance-preview-pane")
                    .test_support()
                    .h(px(220.))
                    .flex_shrink_0()
                    .min_h_0()
                    .min_w_0()
                    .overflow_hidden()
                    .children(self.preferences_ui.appearance_preview.clone())
            }))
    }

    #[cfg(feature = "ui-test")]
    pub fn appearance_render_work(&self) -> (u64, u32) {
        (
            self.preferences_ui.appearance_part_builds.get(),
            self.preferences_ui
                .appearance_frame_parts
                .get()
                .count_ones(),
        )
    }

    fn appearance_part(&self, part: usize, width: Pixels, cx: &mut Context<Self>) -> Div {
        #[cfg(feature = "ui-test")]
        {
            self.preferences_ui
                .appearance_part_builds
                .set(self.preferences_ui.appearance_part_builds.get() + 1);
            self.preferences_ui
                .appearance_frame_parts
                .set(self.preferences_ui.appearance_frame_parts.get() | (1 << part));
        }
        use studyapp_core::ThemeMode;
        let busy = self.preferences_ui.busy || self.preferences_ui.replacing || self.storage_busy;
        let fallback;
        let draft = if let Some(draft) = self.preferences_ui.draft.as_ref() {
            draft
        } else {
            fallback = studyapp_core::Settings::default();
            &fallback
        };
        let appearance = &draft.appearance;
        if part == 0 {
            return form::card_grid(width).child(
                form::section("主题与阅读").id("appearance-general-card").test_support()
                    .child(
                        studyapp_ui::settings_switch::SettingSwitch::new("markdown-render-html").default_value(true)
                            .label("渲染 Markdown 中的安全 HTML 与行内样式")
                            .checked(appearance.render_html)
                            .disabled(busy)
                            .on_click(cx.listener(|this, checked, window, cx| {
                                if let Some(draft) = &mut this.preferences_ui.draft {
                                    draft.appearance.render_html = *checked;
                                }
                                this.refresh_appearance(window, cx);
                                cx.notify();
                            })),
                    )
                    .child(muted("仅支持安全排版，不执行脚本或加载 HTML 资源。关闭后显示原文；围栏中的 HTML 始终为代码。"))
                    .child(form::settings_row("界面主题", form::choice_with_default(
                        "theme-mode", (appearance.mode, ThemeMode::System),
                        [(ThemeMode::System, "跟随系统".into()), (ThemeMode::Light, "浅色".into()), (ThemeMode::Dark, "深色".into())]
                            .into_iter().chain((appearance.mode == ThemeMode::Custom || appearance.active_palette().is_some()).then_some((ThemeMode::Custom, "自定义主题".into()))).collect(),
                        busy, cx.weak_entity(), |this, mode, window, cx| {
                            if let Some(draft) = &mut this.preferences_ui.draft { draft.appearance.mode = mode; }
                            this.refresh_appearance(window, cx); cx.notify();
                        },
                    )))
                    .child(self.setting_number("appearance-font-size".into(), "界面字号", "px", (appearance.font_size.into(), 14, 12, 22, busy),
                        |this, value| { if let Some(draft) = &mut this.preferences_ui.draft { draft.appearance.font_size = value as u16; } }, cx))
                    .child(self.setting_number("appearance-spacing".into(), "内容间距", "px", (appearance.spacing.into(), 8, 4, 16, busy),
                        |this, value| { if let Some(draft) = &mut this.preferences_ui.draft { draft.appearance.spacing = value as u16; } }, cx))
            ).child(form::section("Markdown 表格默认显示").id("appearance-table-card").test_support()
                    .child(form::settings_row("列宽", form::choice_with_default(
                        "table-default", (appearance.table_defaults.sizing, studyapp_core::TableDefaults::default().sizing),
                        vec![(studyapp_core::TableSizing::Equal, "等宽".into()), (studyapp_core::TableSizing::Adaptive, "按内容自适应".into())],
                        busy, cx.weak_entity(), |this, value, window, cx| {
                            if let Some(draft) = &mut this.preferences_ui.draft { draft.appearance.table_defaults.sizing = value; }
                            this.refresh_appearance(window, cx); cx.notify();
                        },
                    )))
                    .child(form::settings_row("列对齐", form::choice_with_default(
                        "table-align", (appearance.table_defaults.alignment, studyapp_core::TableDefaults::default().alignment),
                        vec![(studyapp_core::TableAlignment::Source, "沿用源文".into()), (studyapp_core::TableAlignment::Left, "左对齐".into()),
                            (studyapp_core::TableAlignment::Center, "居中".into()), (studyapp_core::TableAlignment::Right, "右对齐".into())],
                        busy, cx.weak_entity(), |this, value, window, cx| {
                            if let Some(draft) = &mut this.preferences_ui.draft { draft.appearance.table_defaults.alignment = value; }
                            this.refresh_appearance(window, cx); cx.notify();
                        },
                    )))
                    .child(muted("适用于所有主题；正文中可为单张表格调整列宽与对齐，恢复默认后重新跟随此处。不会改写 Markdown。"))
                    .child(muted("修改可即时预览；保存全部设置后才会写入资料库。")),
            ).child(div().col_span_full().id("appearance-list-card").test_support().child(self.list_style_settings(cx)));
        }
        if part == 1 {
            let mut custom = form::section("自定义主题");
            custom = custom.child(muted(
                "颜色和版式会即时更新阅读预览；保存全部设置后才会写入资料库。",
            ));
            for (key, label) in [
                ("name", "主题名称"),
                ("accent", "强调色"),
                ("background", "窗口背景"),
                ("surface", "卡片背景"),
                ("text", "文字颜色"),
            ] {
                if let Some(input) = self.preferences_ui.appearance_fields.get(key) {
                    let control = if key == "name" {
                        form::input(input, busy).into_any_element()
                    } else {
                        self.color_control(key, crate::theme::palette().surface, busy, cx)
                    };
                    custom = custom.child(form::settings_row(label, control));
                }
            }
            let preview = self.markdown_preview(cx);
            if !preview.palette_warnings.is_empty() {
                custom = custom.child(
                    column()
                        .gap_1()
                        .child(muted("所选颜色可能不易阅读，仍可应用并保存。"))
                        .children(preview.palette_warnings.iter().map(|warning| {
                            muted(warning.clone()).text_color(rgb(crate::theme::palette().warning))
                        })),
                );
            }
            return custom
                .child(self.inline_reading_preview(cx))
                .child(self.theme_preset_controls(cx));
        }
        if part == 2 {
            return form::section("Markdown 预览").child(self.markdown_style_preview(cx));
        }
        if part == 3 {
            return form::section("Markdown 参数").child(self.markdown_style_controls(busy, cx));
        }
        self.workspace_settings(width, cx)
    }

    pub fn poll_image_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cloud) = &mut self.preferences_ui.image_cloud else {
            return;
        };
        if self.page != Page::Settings
            || self.preferences_ui.busy
            || self.preferences_ui.replacing
            || self.storage_busy
        {
            cloud.picker.invalidate();
            return;
        }
        if let Some(path) = cloud.picker.poll(&self.last_library)
            && let Some(input) = &self.preferences_ui.image_root
        {
            input.update(cx, |input, cx| {
                input.set_value(path.to_string_lossy().into_owned(), window, cx)
            });
            cx.notify();
        }
    }

    fn image_settings_controls(&self, cx: &mut Context<Self>) -> Div {
        use studyapp_core::ImageStorage;
        let draft = self.preferences_ui.draft.clone().unwrap_or_default();
        let busy = self.preferences_ui.busy || self.preferences_ui.replacing || self.storage_busy;
        let mut panel = form::section("图片存储").child(form::settings_row(
            "保存方式",
            form::choice_with_default(
                "images-storage",
                (draft.images.storage, ImageStorage::Local),
                vec![
                    (ImageStorage::Local, "本地".into()),
                    (ImageStorage::Cloud, "云端".into()),
                    (ImageStorage::Both, "本地与云端".into()),
                ],
                busy,
                cx.weak_entity(),
                |this, storage, _, cx| {
                    if let Some(draft) = &mut this.preferences_ui.draft {
                        if draft.images.storage == ImageStorage::Local
                            && storage != ImageStorage::Local
                            && matches!(
                                draft.images.cloud,
                                studyapp_core::media_settings::ImageCloudTarget::NoteSync
                            )
                            && let Some(cloud) = &mut this.preferences_ui.image_cloud
                        {
                            cloud.kind = 0;
                        }
                        draft.images.storage = storage;
                    }
                    cx.notify();
                },
            ),
        ));
        if let Some(input) = &self.preferences_ui.image_root {
            panel = panel.child(
                form::DirectoryField::new(
                    "image-directory-detail",
                    "本地图片",
                    if input.read(cx).value().trim().is_empty() {
                        std::path::Path::new(&self.last_library)
                            .parent()
                            .map(|path| {
                                studyapp_desktop::presentation::path_label(path.join("images"))
                            })
                            .unwrap_or_default()
                    } else {
                        input.read(cx).value().to_string()
                    },
                )
                .fallback("资料库内的图片文件夹")
                .editor(form::input(input, busy))
                .actions(
                    row()
                        .flex_wrap()
                        .child(
                            Button::new("reset-image-root")
                                .small()
                                .ghost()
                                .label("恢复默认")
                                .disabled(busy || input.read(cx).value().trim().is_empty())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    if let Some(cloud) = &mut this.preferences_ui.image_cloud {
                                        cloud.picker.invalidate();
                                    }
                                    if let Some(input) = &this.preferences_ui.image_root {
                                        input.update(cx, |input, cx| {
                                            input.set_value("", window, cx)
                                        });
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("pick-image-root")
                                .small()
                                .action("选择文件夹")
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let raw = this
                                        .preferences_ui
                                        .image_root
                                        .as_ref()
                                        .map(|input| input.read(cx).value().trim().to_owned())
                                        .unwrap_or_default();
                                    let current = if raw.is_empty() {
                                        std::path::Path::new(&this.last_library)
                                            .parent()
                                            .map(|path| path.join("images"))
                                    } else {
                                        Some(std::path::PathBuf::from(raw))
                                    };
                                    if let Some(cloud) = &mut this.preferences_ui.image_cloud {
                                        cloud.picker.choose(
                                            &this.last_library,
                                            current,
                                            "选择本地图片目录",
                                        );
                                    }
                                })),
                        ),
                ),
            );
        }
        panel = panel.child(muted(
            "留空使用当前资料库 images 目录。选择目录只修改草稿，不迁移或删除已有图片。",
        ));
        let cloud_enabled = draft.images.storage != ImageStorage::Local;
        let reveal_cloud = self.preferences_ui.image_cloud_search_reveal;
        if (cloud_enabled || reveal_cloud)
            && let Some(cloud) = &self.preferences_ui.image_cloud
        {
            panel = panel
                .child(heading("图片云端服务"))
                .child(form::settings_row(
                    "云端服务",
                    form::choice_with_default(
                        "image-provider",
                        (cloud.kind, 0u8),
                        vec![
                            (0u8, "通用对象存储（S3 兼容）".into()),
                            (1u8, "复用笔记同步（WebDAV / NAS）".into()),
                        ],
                        busy || !cloud_enabled,
                        cx.weak_entity(),
                        |this, kind, _, cx| {
                            if let Some(cloud) = &mut this.preferences_ui.image_cloud {
                                cloud.kind = kind;
                            }
                            cx.notify();
                        },
                    ),
                ));
            if !cloud_enabled {
                panel = panel.child(muted("当前使用本地图片存储。搜索仅展开相关字段，未改变保存方式；选择云端或本地与云端后才能编辑云端配置。"));
            }
            if cloud.kind == 0 || reveal_cloud {
                let s3_disabled = busy || !cloud_enabled || cloud.kind != 0;
                if cloud.kind != 0 {
                    panel = panel.child(muted("以下为 S3 专用字段；当前服务不是 S3，因此仅供查看。搜索不会切换服务或修改草稿。"));
                }
                panel = panel.child(muted("兼容协议：S3 · AWS Signature V4。请填写服务商的 S3 兼容入口，不是控制台登录地址。"));
                for (key, label) in [
                    ("endpoint", "Endpoint"),
                    ("bucket", "Bucket"),
                    ("region", "Region"),
                    ("access_key_id", "Access Key ID"),
                    ("prefix", "对象前缀"),
                ] {
                    panel = panel.child(form::settings_row(
                        label,
                        div()
                            .id(SharedString::from(format!("image-cloud-field-{key}")))
                            .test_support()
                            .min_w_0()
                            .child(form::input(&cloud.fields[key], s3_disabled)),
                    ));
                }
                panel = panel.child(form::settings_row("寻址方式", form::choice_with_default(
                    "image-addressing", (cloud.addressing, studyapp_core::media_settings::ObjectAddressing::default()),
                    vec![(studyapp_core::media_settings::ObjectAddressing::VirtualHosted, "虚拟主机".into()), (studyapp_core::media_settings::ObjectAddressing::Path, "路径".into())],
                    s3_disabled, cx.weak_entity(), |this, style, _, cx| {
                        if let Some(cloud) = &mut this.preferences_ui.image_cloud { cloud.addressing = style; }
                        cx.notify();
                    },
                )))
                    .child(muted("虚拟主机：bucket.endpoint/key；路径：endpoint/bucket/key。不同服务的兼容范围与要求不同，请按服务商文档选择。"))
                    .child(form::settings_row("图片云端凭据", { cloud.credential.update(cx, |secret, cx| secret.configure(cloud.secret_saved, s3_disabled, cx)); cloud.credential.clone() }));
            }
            if cloud.kind != 0 {
                let target = match &self.sync_panel.read(cx).status.settings.target {
                    crate::sync_runtime::SyncTarget::Disabled => {
                        "笔记同步尚未配置；请先在同步分类配置 WebDAV 或文件夹 / NAS".to_owned()
                    }
                    crate::sync_runtime::SyncTarget::Folder { .. } => {
                        "已明确复用笔记同步的文件夹 / NAS 目标".into()
                    }
                    crate::sync_runtime::SyncTarget::WebDav { .. } => {
                        "已明确复用笔记同步的 WebDAV 目标及受保护账户".into()
                    }
                };
                panel = panel.child(muted(target)).child(
                    Button::new("image-open-sync-settings")
                        .small()
                        .action("查看同步配置")
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.select_settings_section(SettingsSection::Sync, cx);
                        })),
                );
            }
            let dirty = self.settings_dirty(cx);
            let running = busy || self.image_cloud_busy || !cloud_enabled;
            panel = panel
                .child(
                    row()
                        .child(
                            Button::new("test-image-cloud")
                                .small()
                                .action("测试图片云端连接")
                                .disabled(running || dirty)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if this
                                        .worker
                                        .sender
                                        .send(Work::Scoped {
                                            library: this.last_library.clone(),
                                            work: Box::new(Work::TestImageCloud),
                                        })
                                        .is_err()
                                    {
                                        this.push_notice(Err("后台服务已停止".into()), cx);
                                    }
                                })),
                        )
                        .child(
                            Button::new("retry-image-cloud")
                                .small()
                                .action("重试图片上传")
                                .disabled(running || dirty)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if this
                                        .worker
                                        .sender
                                        .send(Work::Scoped {
                                            library: this.last_library.clone(),
                                            work: Box::new(Work::RetryImageCloud),
                                        })
                                        .is_err()
                                    {
                                        this.push_notice(Err("后台服务已停止".into()), cx);
                                    }
                                })),
                        ),
                )
                .child(muted(if !cloud_enabled {
                    "云端动作未启用；请先选择并保存云端存储方式。"
                } else if dirty {
                    "请先保存全部设置，再测试连接或上传已有图片。"
                } else if self.image_cloud_busy {
                    "正在处理图片云端任务…"
                } else {
                    "连接和上传使用已保存的配置，不会在输入时联网。"
                }));
        }
        panel.child(muted("云端模式保留本地离线缓存；上传失败时本地图片仍保留，可重试。更改存储方式不会自动补传旧图片，请保存后点击重试图片上传。"))
    }

    pub fn shortcut_controls(&self, cx: &mut Context<Self>) -> Div {
        let busy = self.preferences_ui.busy || self.preferences_ui.replacing || self.storage_busy;
        let analysis = self.shortcut_analysis(cx);
        let mut header = form::section("快捷键").child(muted(
            "点击当前绑定录制，Esc 取消；＋追加组合，×清空。冲突会显示在对应操作下。",
        ));
        if let Some(search) = &self.preferences_ui.shortcut_search {
            header = header.child(form::input(search, busy));
        }
        let query = self
            .preferences_ui
            .shortcut_search
            .as_ref()
            .map(|search| search.read(cx).value().trim().to_lowercase())
            .unwrap_or_default();
        let rows: Vec<usize> = crate::shortcuts::SHORTCUT_ACTIONS
            .iter()
            .enumerate()
            .filter(|(_, action)| {
                self.preferences_ui
                    .recording
                    .as_ref()
                    .is_some_and(|recording| recording.action == action.id)
                    || query.is_empty()
                    || analysis
                        .search
                        .get(action.id)
                        .is_some_and(|search| search.contains(&query))
            })
            .map(|(ix, _)| ix)
            .collect();
        let theme = crate::theme::palette();
        self.preferences_ui.shortcut_rows.sync(
            &rows,
            ShortcutLayout {
                analysis_revision: self.preferences_ui.shortcut_analysis.rebuilds(),
                recording: self
                    .preferences_ui
                    .recording
                    .as_ref()
                    .map(|recording| recording.action.clone()),
                recording_error: self.preferences_ui.recording_error.clone(),
                font_size: theme.font_size,
                spacing: theme.spacing,
            },
        );
        self.apply_settings_list_offset(&self.preferences_ui.shortcut_rows.scroll);
        self.preferences_ui.shortcut_rows.track_focus(
            self.preferences_ui
                .recording
                .as_ref()
                .and_then(|recording| {
                    let row = rows.iter().position(|&ix| {
                        crate::shortcuts::SHORTCUT_ACTIONS[ix].id == recording.action
                    })?;
                    self.preferences_ui
                        .recording_focus
                        .clone()
                        .map(|focus| (row, focus))
                }),
        );
        #[cfg(feature = "ui-test")]
        self.preferences_ui.shortcut_frame_rows.set(0);
        let workspace = column()
            .relative()
            .child(
                div()
                    .id("settings-shortcuts-surface")
                    .test_support()
                    .absolute()
                    .size_full(),
            )
            .flex_1()
            .min_h_0()
            .min_w_0()
            .w_full()
            .gap_3()
            .child(header);
        if rows.is_empty() {
            return workspace.child(
                div()
                    .id("settings-section-scroll")
                    .test_support()
                    .flex_1()
                    .child(muted("没有匹配的快捷键")),
            );
        }
        // Keep an exact eager control only in the CI binary. Both paths use the
        // same row builder, typography and variable-height contents for A/B timing.
        #[cfg(feature = "ui-test")]
        if self.preferences_ui.shortcut_eager_baseline {
            return workspace.child(
                scroll_body("settings-section-scroll", &self.settings_scroll)
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .gap_0()
                    .px_4()
                    .pb_4()
                    .children(rows.iter().enumerate().map(|(row, &ix)| {
                        let group = row == 0
                            || crate::shortcuts::SHORTCUT_ACTIONS[rows[row - 1]].group
                                != crate::shortcuts::SHORTCUT_ACTIONS[ix].group;
                        self.shortcut_row(ix, group, &analysis, cx)
                    })),
            );
        }
        let owner = cx.entity();
        workspace.child(form::virtual_rows(
            &self.preferences_ui.shortcut_rows.scroll,
            move |row, _, cx| {
                let ix = rows[row];
                let group = row == 0
                    || crate::shortcuts::SHORTCUT_ACTIONS[rows[row - 1]].group
                        != crate::shortcuts::SHORTCUT_ACTIONS[ix].group;
                owner
                    .update(cx, |this, cx| this.shortcut_row(ix, group, &analysis, cx))
                    .into_any_element()
            },
        ))
    }

    fn shortcut_row(
        &self,
        index: usize,
        show_group: bool,
        analysis: &ShortcutAnalysis,
        cx: &mut Context<Self>,
    ) -> Div {
        let busy = self.preferences_ui.busy || self.preferences_ui.replacing || self.storage_busy;
        let action = &crate::shortcuts::SHORTCUT_ACTIONS[index];
        let id = action.id;
        #[cfg(feature = "ui-test")]
        {
            self.preferences_ui
                .shortcut_row_builds
                .set(self.preferences_ui.shortcut_row_builds.get() + 1);
            self.preferences_ui
                .shortcut_frame_rows
                .set(self.preferences_ui.shortcut_frame_rows.get() | (1 << index));
        }
        let recording = self
            .preferences_ui
            .recording
            .as_ref()
            .is_some_and(|recording| recording.action == id);
        let binding = analysis.bindings.get(id).map_or("", String::as_str);
        let error = analysis.errors.get(id);
        let counterparts = analysis.conflicts.get(id).map(|others| {
            others
                .iter()
                .map(|other| {
                    crate::shortcuts::SHORTCUT_ACTIONS
                        .iter()
                        .find(|action| action.id == other)
                        .map_or(other.as_str(), |action| action.label)
                })
                .collect::<Vec<_>>()
                .join("、")
        });
        let invalid = error.is_some() || counterparts.is_some();
        let mut keys = form::binding_cell()
            .id(SharedString::from(format!("record-shortcut-{id}")))
            .test_support()
            .flex_1()
            .min_w(px(170.))
            .flex_wrap()
            .border_color(crate::theme::color(if recording {
                GREEN
            } else if invalid {
                0xBE3434
            } else {
                BORDER
            }));
        if !busy && !recording {
            keys = keys
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.start_shortcut_recording(id, false, window, cx)
                }));
        }
        if recording {
            let mut capture = row()
                .id("shortcut-recording-target")
                .test_support()
                .flex_1()
                .min_w_0()
                .gap_2()
                .flex_wrap()
                .child(
                    div()
                        .flex_1()
                        .text_sm()
                        .text_color(crate::theme::color(GREEN))
                        .child("请按组合键…"),
                )
                .child(
                    Button::new("cancel-shortcut-recording")
                        .small()
                        .ghost()
                        .label("取消 · Esc")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.preferences_ui.recording = None;
                            this.preferences_ui.recording_error = None;
                            if let Some(search) = &this.preferences_ui.shortcut_search {
                                search.read(cx).focus_handle(cx).focus(window, cx);
                            }
                            cx.notify();
                        })),
                );
            if let Some(focus) = &self.preferences_ui.recording_focus {
                capture = capture.track_focus(focus);
            }
            keys = keys.child(capture);
        } else if binding.trim().is_empty() {
            keys = keys.child(muted("点击设置"));
        } else {
            for (index, alias) in binding.split(';').enumerate() {
                if index > 0 {
                    keys = keys.child(muted(" / "));
                }
                for key in alias.split('+') {
                    keys = keys.child(
                        div()
                            .px_1()
                            .py_0p5()
                            .rounded_sm()
                            .border_1()
                            .border_color(crate::theme::color(BORDER))
                            .bg(crate::theme::color(0xFFFFFF))
                            .text_xs()
                            .text_color(crate::theme::color(if invalid { 0xBE3434 } else { INK }))
                            .child(form::keycap_label(key).to_owned()),
                    );
                }
            }
        }
        let mut entry = column()
            .relative()
            .child(studyapp_ui::settings_anchor::probe(action.label))
            .id(SharedString::from(format!("shortcut-row-{id}")))
            .test_support()
            .gap_1()
            .py_2()
            .border_b_1()
            .border_color(crate::theme::color(BORDER))
            .child(
                row()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        div()
                            .w(px(136.))
                            .text_sm()
                            .text_color(crate::theme::color(INK))
                            .child(action.label),
                    )
                    .child(keys)
                    .child(
                        Button::new(SharedString::from(format!("append-shortcut-{id}")))
                            .small()
                            .ghost()
                            .action("＋")
                            .tooltip("追加快捷键")
                            .disabled(busy || recording)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.start_shortcut_recording(id, true, window, cx)
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("clear-shortcut-{id}")))
                            .small()
                            .ghost()
                            .action("关闭")
                            .tooltip("清空快捷键")
                            .disabled(busy || recording)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.preferences_ui.recording = None;
                                this.preferences_ui.recording_error = None;
                                if let Some(input) = this.preferences_ui.shortcuts.get(id) {
                                    input.update(cx, |input, cx| input.set_value("", window, cx));
                                }
                                cx.notify();
                            })),
                    ),
            );
        if let Some(counterparts) = counterparts {
            entry = entry.child(
                muted(format!("冲突：{counterparts}"))
                    .id(SharedString::from(format!("shortcut-conflict-{id}")))
                    .test_support()
                    .text_color(crate::theme::color(0xBE3434)),
            );
        }
        if let Some(error) = error {
            entry = entry.child(muted(error.clone()).text_color(crate::theme::color(0xBE3434)));
        }
        if recording && let Some(error) = &self.preferences_ui.recording_error {
            entry = entry.child(
                muted(error.clone())
                    .id("shortcut-recording-error")
                    .test_support()
                    .text_color(crate::theme::color(0xBE3434)),
            );
        }

        column()
            .gap_0()
            .children(show_group.then(|| {
                div()
                    .pt_3()
                    .pb_1()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(action.group)
            }))
            .child(entry)
    }
}

#[cfg(test)]
mod appearance_tests {
    use super::select_custom_preset;
    use studyapp_core::{AppearanceSettings, ThemePalette, ThemePreset};

    #[test]
    fn appearance_subpages_mount_only_the_selected_task() {
        use super::AppearancePage;
        assert_eq!(AppearancePage::Overview.parts(), &[0]);
        assert_eq!(AppearancePage::ThemeEditor.parts(), &[1, 3]);
        assert_eq!(AppearancePage::Workspace.parts(), &[4]);
    }

    fn preset(id: &str) -> ThemePreset {
        ThemePreset {
            id: id.into(),
            name: id.into(),
            palette: ThemePalette::default(),
            markdown: Default::default(),
        }
    }

    #[test]
    fn selecting_another_or_same_preset_preserves_unsaved_fields() {
        let baseline = AppearanceSettings {
            active_preset: Some("a".into()),
            custom_presets: vec![preset("a"), preset("b")],
            ..Default::default()
        };
        let mut draft = baseline.clone();
        let mut edited = preset("a");
        edited.name = "edited a".into();
        edited.palette.accent = "#215F99".into();
        edited.markdown.layout.code_size = 18;
        edited.markdown.layout.table_stripes = false;
        edited
            .markdown
            .colors
            .insert("heading".into(), "#234567".into());
        select_custom_preset(&mut draft, Some(edited.clone()), "b").unwrap();
        assert_eq!(draft.custom_presets[0], edited);
        assert_ne!(
            draft, baseline,
            "switching must retain dirty/save/discard semantics"
        );
        select_custom_preset(&mut draft, None, "a").unwrap();
        assert_eq!(draft.active_palette(), Some(&edited.palette));
        edited.name = "reselected a".into();
        select_custom_preset(&mut draft, Some(edited.clone()), "a").unwrap();
        assert_eq!(draft.custom_presets[0], edited);
        assert_eq!(
            baseline.custom_presets[0].name, "a",
            "saved baseline remains available for discard"
        );
    }

    #[test]
    fn invalid_pending_fields_block_selection_without_changing_draft() {
        let mut draft = AppearanceSettings {
            active_preset: Some("a".into()),
            custom_presets: vec![preset("a"), preset("b")],
            ..Default::default()
        };
        let before = draft.clone();
        let mut invalid = preset("a");
        invalid.palette.text = "#not-a-color".into();
        assert!(select_custom_preset(&mut draft, Some(invalid), "b").is_err());
        assert_eq!(
            draft, before,
            "invalid input must stay on the current preset intact"
        );
    }

    #[test]
    fn low_contrast_pending_preset_can_be_selected_and_preserves_exact_values() {
        let mut draft = AppearanceSettings {
            active_preset: Some("a".into()),
            custom_presets: vec![preset("a"), preset("b")],
            ..Default::default()
        };
        let mut edited = preset("a");
        edited.palette.text = edited.palette.surface.clone();
        edited.palette.accent = "#FAFAFA".into();
        edited
            .markdown
            .colors
            .insert("body".into(), "#FFFFFF".into());
        assert!(!edited.palette.contrast_warnings().is_empty());
        select_custom_preset(&mut draft, Some(edited.clone()), "b").unwrap();
        assert_eq!(draft.custom_presets[0], edited);
        draft.validate().unwrap();
    }
}

#[cfg(all(test, feature = "ui-test"))]
mod scoped_tests {
    use super::{
        AppearancePage, Page, PreferenceSnapshot, SettingsResetScope, SettingsSection, StoragePage,
        StudyApp,
    };
    use gpui_kit::{
        AppContext, Context, Entity, Focusable, IntoElement, Keystroke, ParentElement, Render,
        Styled, TestAppContext, VisualTestContext, Window, div, test::TestWindowExt,
    };
    use gpui_kit::{InputEvent as _, VisualContext as _};

    struct SettingsFixture {
        app: Entity<StudyApp>,
    }

    // Render the real appearance surface without StudyApp::render, which would
    // configure and mount the native Markdown editor. The preview stays a model.
    struct AppearanceFixture {
        app: Entity<StudyApp>,
        width: f32,
        height: f32,
    }

    // Exercise real color events and the render-time preview configuration,
    // while keeping AppearancePreview itself out of the rendered tree.
    struct PreviewCommitFixture {
        app: Entity<StudyApp>,
    }
    impl Render for PreviewCommitFixture {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.app.update(cx, |app, cx| {
                let theme = app.appearance_preview_theme(cx);
                app.preferences_ui
                    .appearance_preview
                    .clone()
                    .unwrap()
                    .update(cx, |preview, cx| {
                        preview.configure(theme, true, window, cx);
                    });
                div()
                    .w(gpui_kit::px(600.))
                    .h(gpui_kit::px(700.))
                    .child(app.preferences_ui.appearance_colors["text"].clone())
            })
        }
    }

    #[gpui_kit::test]
    fn committed_text_color_reaches_preview_on_the_next_render(cx: &mut TestAppContext) {
        let (app, visual) = fixture(cx);
        visual.replace_root_view(|_, _| PreviewCommitFixture { app: app.clone() });
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let preview = app.read_with(visual, |app, _| {
            app.preferences_ui.appearance_preview.clone().unwrap()
        });
        let editor = preview.read_with(visual, |preview, _| preview.editor().unwrap());
        let generation = editor.read_with(visual, |editor, _| editor.generation());
        visual.update(|window, cx| window.click("color-open-text", cx));
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.click("color-swatch-text-3", cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            assert_ne!(
                app.read(cx).preferences_ui.appearance_fields["text"]
                    .read(cx)
                    .value()
                    .as_str(),
                "#EF4444"
            );
            app.update(cx, |app, cx| {
                assert_eq!(app.appearance_preview_theme(cx).text, 0xEF4444)
            });
            window.click("color-close-text", cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));
        app.read_with(visual, |app, cx| {
            assert_eq!(
                app.preferences_ui.appearance_fields["text"]
                    .read(cx)
                    .value()
                    .as_str(),
                "#EF4444"
            );
        });
        app.update(visual, |app, cx| {
            assert_eq!(app.appearance_preview_theme(cx).text, 0xEF4444)
        });
        preview.read_with(visual, |preview, _| {
            assert_eq!(preview.applied_theme().unwrap().text, 0xEF4444)
        });
        editor.read_with(visual, |editor, _| {
            assert_eq!(editor.generation(), generation);
            assert_eq!(editor.value(), crate::appearance_preview::PREVIEW_SAMPLE);
            assert!(editor.readonly());
            assert!(
                !editor.ready() && !editor.creating(),
                "safe fixture mounted a native editor"
            );
        });
    }
    impl Render for AppearanceFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.app.update(cx, |app, cx| {
                div()
                    .flex()
                    .w(gpui_kit::px(self.width))
                    .h(gpui_kit::px(self.height))
                    .child(app.appearance_surface(cx))
            })
        }
    }

    #[gpui_kit::test]
    fn picker_does_not_hide_appearance_preview_in_compact_space(cx: &mut TestAppContext) {
        let (app, cx) = fixture(cx);
        cx.simulate_resize(gpui_kit::size(gpui_kit::px(800.), gpui_kit::px(650.)));
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.select_settings_section(SettingsSection::Appearance, cx);
                app.select_appearance_page(AppearancePage::ThemeEditor, cx);

                assert!(
                    app.appearance_preview_visible(window, cx),
                    "compact edit mode hid the sample"
                );
            });
        });
    }

    #[gpui_kit::test]
    fn live_color_popup_hover_dismiss_and_selection_keep_preview_visible(cx: &mut TestAppContext) {
        let (app, cx) = fixture(cx);
        app.update(cx, |app, _| {
            app.preferences_ui.section = SettingsSection::Appearance;
            app.preferences_ui.appearance_page = AppearancePage::ThemeEditor;
        });
        let surface = cx.replace_root_view(|_, _| AppearanceFixture {
            app: app.clone(),
            width: 1050.,
            height: 700.,
        });
        for _ in 0..3 {
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
        let (control, source, preview_id, original) = app.read_with(cx, |app, cx| {
            let source = app.preferences_ui.appearance_fields["accent"].clone();
            (
                app.preferences_ui.appearance_colors["accent"].clone(),
                source.clone(),
                app.preferences_ui
                    .appearance_preview
                    .as_ref()
                    .unwrap()
                    .entity_id(),
                source.read(cx).value(),
            )
        });
        cx.update(|window, cx| window.click("color-open-accent", cx));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| {
            assert!(control.read(cx).popup_open());
            assert!(app.read(cx).appearance_preview_visible(window, cx));
            let point = window.find("color-swatch-accent-1").bounds().center();
            window.dispatch_event(
                gpui_kit::MouseMoveEvent {
                    position: point,
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
        });
        cx.run_until_parked();
        // Label-above-control rows no longer place the picker over the
        // parameter scrollbar at the old fixed 1050px layout. Deliberately
        // construct that overlap from measured geometry; keep the real list,
        // real popup and down/up event assertions below unchanged.
        cx.simulate_resize(gpui_kit::size(gpui_kit::px(1050.), gpui_kit::px(3000.)));
        surface.update(cx, |surface, cx| {
            surface.height = 3000.;
            cx.notify();
        });
        for _ in 0..3 {
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
        // Resolve the overlap from the current responsive layout. A width
        // change can move the trigger into a different form column, so measure
        // again rather than assuming the old one-shot layout remains valid.
        for _ in 0..8 {
            let adjustment = cx.update(|window, cx| {
                let target = window.find("color-close-accent").bounds().center();
                let viewport = app
                    .read(cx)
                    .preferences_ui
                    .appearance_rows
                    .scroll
                    .viewport_bounds();
                target.x - viewport.right() + gpui_kit::component::scroll::Scrollbar::width() / 2.
            });
            if adjustment.abs() < gpui_kit::px(1.) {
                break;
            }
            surface.update(cx, |surface, cx| {
                surface.width += f32::from(adjustment);
                cx.notify();
            });
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
        cx.update(|window, cx| {
            assert_eq!(
                source.read(cx).value(),
                original,
                "hover committed a palette color"
            );
            assert!(app.read(cx).appearance_preview_visible(window, cx));
            let close = window.find("color-close-accent").bounds();
            let scroll_before = app
                .read(cx)
                .preferences_ui
                .appearance_rows
                .scroll
                .logical_scroll_top();
            // The close button overlaps the parameter scrollbar. A popup press
            // must not reach that underlying track between down and up.
            window.render_frame(cx);
            let target = window.find("color-close-accent").bounds().center();
            let viewport = app
                .read(cx)
                .preferences_ui
                .appearance_rows
                .scroll
                .viewport_bounds();
            assert!(
                viewport.contains(&target)
                    && target.x
                        >= viewport.right() - gpui_kit::component::scroll::Scrollbar::width(),
                "regression requires the close button over the scrollbar track"
            );
            window.dispatch_event(
                gpui_kit::MouseMoveEvent {
                    position: target,
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
            window.dispatch_event(
                gpui_kit::MouseDownEvent {
                    button: gpui_kit::MouseButton::Left,
                    position: target,
                    click_count: 1,
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
            let scroll_after = app
                .read(cx)
                .preferences_ui
                .appearance_rows
                .scroll
                .logical_scroll_top();
            assert_eq!(
                (scroll_after.item_ix, scroll_after.offset_in_item),
                (scroll_before.item_ix, scroll_before.offset_in_item),
                "popup press reached the underlying scrollbar"
            );
            window.render_frame(cx);
            assert_eq!(
                window.find("color-close-accent").bounds(),
                close,
                "popup moved between pointer down and up"
            );
            window.dispatch_event(
                gpui_kit::MouseUpEvent {
                    button: gpui_kit::MouseButton::Left,
                    position: target,
                    click_count: 1,
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
            assert!(
                !control.read(cx).popup_open(),
                "close click failed: close={close:?}; viewport={:?}; popup={:?}",
                window.viewport_size(),
                control.read(cx).popup_bounds()
            );
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert_eq!(source.read(cx).value(), original);
            assert!(app.read(cx).appearance_color_popup_bounds(cx).is_empty());
            window.click("color-open-accent", cx);
            window.render_frame(cx);
            window.click("color-swatch-accent-3", cx);
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert_eq!(source.read(cx).value(), original);
            assert!(control.read(cx).popup_open());
            window.click("color-close-accent", cx);
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert_eq!(source.read(cx).value().as_str(), "#EF4444");
            assert!(app.read(cx).preferences_ui.appearance_touched);
            assert!(app.read(cx).appearance_preview_visible(window, cx));
            assert_eq!(
                app.read(cx)
                    .preferences_ui
                    .appearance_preview
                    .as_ref()
                    .unwrap()
                    .entity_id(),
                preview_id
            );
            assert!(app.read(cx).appearance_color_popup_bounds(cx).is_empty());
        });
    }

    #[gpui_kit::test]
    fn appearance_cache_separates_page_entry_layout_from_parent_notifications(
        cx: &mut TestAppContext,
    ) {
        let (app, visual) = fixture(cx);
        app.update(visual, |app, _| {
            app.preferences_ui.section = SettingsSection::Appearance;
            app.preferences_ui.appearance_page = AppearancePage::Overview;
        });
        let surface = visual.replace_root_view(|_, _| AppearanceFixture {
            app: app.clone(),
            width: 1050.,
            height: 650.,
        });
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let entry_bounds = visual.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.select_appearance_page(AppearancePage::ThemeEditor, cx);
            });
            window.render_frame(cx);
            (
                window.find("appearance-parameters").bounds(),
                window.find("appearance-preview-pane").bounds(),
            )
        });
        // During page entry the cached view key and newly mounted color rows
        // notify while drawing. GPUI retains those dirty entity IDs until the
        // next draw. Prove this is initialization even WITHOUT a parent notify
        // or any MarkdownEditor, rather than counting it as parent-only work.
        visual.update(|window, cx| {
            let before = app.read(cx).appearance_render_work().0;
            window.draw(cx).clear(cx);
            // The new stacked controls make each virtual row taller. Only
            // visible rows are initialized, so a viewport can mount one or both
            // parts; the strict zero-rebuild parent-notify check below remains.
            let initialized = app.read(cx).appearance_render_work().0 - before;
            assert!(
                (1..=AppearancePage::ThemeEditor.parts().len() as u64).contains(&initialized),
                "unbounded page-entry row builds: {initialized}"
            );
            assert_eq!(
                (
                    window.find("appearance-parameters").bounds(),
                    window.find("appearance-preview-pane").bounds()
                ),
                entry_bounds
            );
            let app = app.read(cx);
            assert!(
                app.preferences_ui
                    .appearance_view
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .key
                    .as_ref()
                    == Some(&app.appearance_view_key(cx))
            );
        });
        let mut builds = Vec::new();
        for _ in 0..20 {
            // The fixture is the rendered parent in this safe surface-only
            // window. Include any test-mode automatic draws in the count.
            let before = app.read_with(visual, |app, _| app.appearance_render_work().0);
            surface.update(visual, |_, cx| cx.notify());
            visual.update(|window, cx| {
                window.draw(cx).clear(cx);
                builds.push(app.read(cx).appearance_render_work().0 - before);
            });
        }
        assert!(
            builds.iter().all(|&count| count == 0),
            "page entry left a redundant appearance invalidation: {builds:?}"
        );
        let before_edit = app.read_with(visual, |app, _| app.appearance_render_work().0);
        visual.update(|window, cx| {
            let input = app.read(cx).preferences_ui.appearance_fields["name"].clone();
            input.update(cx, |input, cx| input.set_value("cache probe", window, cx));
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        assert!(app.read_with(visual, |app, _| app.appearance_render_work().0) > before_edit);
        let before_resize = app.read_with(visual, |app, _| app.appearance_render_work().0);
        surface.update(visual, |surface, cx| {
            surface.width = 590.;
            surface.height = 380.;
            cx.notify();
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        assert!(app.read_with(visual, |app, _| app.appearance_render_work().0) > before_resize);
        app.read_with(visual, |app, cx| {
            assert!(
                app.preferences_ui
                    .appearance_preview
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .editor()
                    .is_none(),
                "cache fixture mounted a native editor"
            );
        });
    }

    #[gpui_kit::test]
    fn inline_reading_preview_collapses_without_replacing_the_parameters(cx: &mut TestAppContext) {
        let (app, visual) = fixture(cx);
        app.update(visual, |app, cx| {
            app.select_settings_target("appearance.theme", cx);
        });
        let surface = visual.replace_root_view(|_, _| AppearanceFixture {
            app: app.clone(),
            width: 1050.,
            height: 650.,
        });
        for (collapsed, width, height) in [
            (false, 1050., 650.),
            (true, 590., 380.),
            (false, 590., 380.),
            (false, 900., 500.),
        ] {
            app.update(visual, |app, cx| {
                if collapsed {
                    app.preferences_ui
                        .collapsed_previews
                        .insert("reading-preview".into());
                } else {
                    app.preferences_ui
                        .collapsed_previews
                        .remove("reading-preview");
                }
                app.preferences_ui.section_revision += 1;
                cx.notify();
            });
            surface.update(visual, |surface, cx| {
                surface.width = width;
                surface.height = height;
                cx.notify();
            });
            for _ in 0..3 {
                visual.run_until_parked();
                visual.update(|window, cx| window.draw(cx).clear(cx));
            }
            visual.update(|window, _| {
                let parameters = window.find("appearance-parameters").bounds();
                assert!(parameters.size.height > gpui_kit::px(0.));
                if collapsed {
                    assert!(window.try_find("appearance-preview-pane").is_none());
                } else {
                    let pane = window.find("appearance-preview-pane").bounds();
                    assert!(pane.top() >= parameters.top());
                    // The preview scrolls with its card and may be below the viewport.
                    assert!(pane.left() >= parameters.left());
                    assert!(pane.right() <= gpui_kit::px(width + 1.));
                    assert!(pane.size.height <= gpui_kit::px(220.));
                }
            });
        }
    }

    #[gpui_kit::test]
    fn appearance_surface_adapts_to_actual_content_width_and_height(cx: &mut TestAppContext) {
        let (app, visual) = fixture(cx);
        let preview = app.read_with(visual, |app, _| {
            app.preferences_ui.appearance_preview.clone().unwrap()
        });
        app.update(visual, |app, _cx| {
            app.preferences_ui.section = SettingsSection::Appearance;
            app.preferences_ui.appearance_page = AppearancePage::ThemeEditor;
        });
        let surface = visual.replace_root_view(|_, _| AppearanceFixture {
            app: app.clone(),
            width: 1050.,
            height: 650.,
        });
        for (width, height) in [
            (1050., 650.),
            // Native evidence at the supported 1000px window minimum assigns
            // 806px to this workspace; it must still use the compact layout.
            (806., 460.),
            (830., 460.),
            (590., 650.),
            (590., 380.),
            (900., 500.),
        ] {
            surface.update(visual, |surface, cx| {
                surface.width = width;
                surface.height = height;
                cx.notify();
            });
            for _ in 0..4 {
                visual.run_until_parked();
                visual.update(|window, cx| window.draw(cx).clear(cx));
            }
            visual.update(|window, cx| {
                let parameters = window.find("appearance-parameters").bounds();
                let sample = window.find("appearance-preview-pane").bounds();
                assert!(
                    parameters.size.height > gpui_kit::px(0.)
                        && sample.size.height > gpui_kit::px(0.),
                    "size={width}x{height} parameters={parameters:?} sample={sample:?}"
                );
                assert!(sample.size.height <= gpui_kit::px(220.));
                assert!(sample.right() <= gpui_kit::px(width + 1.));
                assert!(sample.top() >= parameters.top());
                assert!(sample.left() >= parameters.left());
                assert_eq!(
                    app.read(cx)
                        .preferences_ui
                        .appearance_preview
                        .as_ref()
                        .unwrap()
                        .entity_id(),
                    preview.entity_id()
                );
                assert!(
                    preview.read(cx).editor().is_none(),
                    "safe layout fixture mounted a native editor"
                );
            });
        }
    }
    impl Render for SettingsFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.app.update(cx, |app, cx| {
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(app.settings_header(false, cx))
                    .child(crate::settings_components::input(&app.interval, false))
                    .child(app.settings_footer(cx))
                    .children(app.settings_modal(cx))
            })
        }
    }

    fn fixture(cx: &mut TestAppContext) -> (Entity<StudyApp>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::apply(&Default::default(), false, cx);
        });
        let root = tempfile::tempdir().unwrap().keep();
        let worker = crate::worker::Worker::start(root.join("studyapp.sqlite3"), None);
        let mut app = None;
        let (_, visual) = cx.add_window_view(|window, cx| {
            let entity = cx.new(|cx| StudyApp::new(worker, window, cx));
            entity.update(cx, |app, cx| {
                app.page = Page::Settings;
                app.preferences_ui.saved = Some(PreferenceSnapshot::defaults());
                app.begin_settings(window, cx);
            });
            app = Some(entity.clone());
            SettingsFixture { app: entity }
        });
        (app.unwrap(), visual)
    }

    struct CompleteWorkspaceFixture {
        app: Entity<StudyApp>,
        snapshot: studyapp_core::Snapshot,
        width: f32,
        height: f32,
    }
    impl Render for CompleteWorkspaceFixture {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.app.update(cx, |app, cx| {
                div()
                    .w(gpui_kit::px(self.width))
                    .h(gpui_kit::px(self.height))
                    .child(app.settings(&self.snapshot, window, cx))
            })
        }
    }

    #[gpui_kit::test]
    fn portable_smoke_settings_navigation_reaches_folder_sync_in_tree_and_drawer(
        cx: &mut TestAppContext,
    ) {
        use gpui_kit::px;
        let (app, visual) = fixture(cx);
        visual.update(|_, cx| cx.set_reduce_motion(true));
        visual.simulate_resize(gpui_kit::size(px(1400.), px(1000.)));
        let root = tempfile::tempdir().unwrap();
        let engine = studyapp_core::Engine::open(&root.path().join("sync.sqlite"), 1000).unwrap();
        let saved = app.read_with(visual, |app, _| {
            app.preferences_ui.saved.as_ref().unwrap().settings.clone()
        });
        let surface = visual.replace_root_view(|_, cx| {
            cx.observe(&app, |_, _, cx| cx.notify()).detach();
            CompleteWorkspaceFixture {
                app: app.clone(),
                snapshot: engine.snapshot(),
                width: 1000.,
                height: 700.,
            }
        });
        for width in [1000., 620., 1000.] {
            surface.update(visual, |surface, cx| {
                surface.width = width;
                cx.notify();
            });
            visual.run_until_parked();
            visual.update(|window, cx| {
                crate::smoke::select_settings_page(window, &app, "learning", cx);
                // The installed-update guard uses the same rendered category
                // identity before editing the independent settings draft.
                let learning = crate::settings_navigation::group_element_id("learning");
                if width >= 710. {
                    assert!(window.find(learning).visible());
                } else {
                    assert!(window.try_find(learning).is_none());
                    assert!(!app.read(cx).preferences_ui.settings_drawer);
                }
                assert!(window.try_find("settings-section-learning").is_none());
                assert!(window.find("learning-interval").visible());
                app.update(cx, |app, cx| {
                    app.interval
                        .update(cx, |input, cx| input.set_value("35", window, cx));
                });
                assert!(app.read(cx).settings_dirty(cx));
                app.update(cx, |app, cx| app.discard_settings(window, cx));
                assert!(!app.read(cx).settings_dirty(cx));
                crate::smoke::select_settings_page(window, &app, "sync", cx);
            });
            visual.run_until_parked();
            visual.update(|window, cx| {
                assert!(app.read(cx).preferences_ui.section == SettingsSection::Sync);
                assert!(!app.read(cx).preferences_ui.settings_drawer);
                assert!(!app.read(cx).settings_dirty(cx));
                crate::smoke_library::reveal_scrolled(
                    window,
                    "sync-target".into(),
                    "settings-section-scroll",
                    cx,
                );
                studyapp_ui::settings_components::select_choice(window, "sync-target", 1, cx);
                assert_eq!(app.read(cx).sync_panel.read(cx).target, 1, "width={width}");
                assert!(app.read(cx).settings_dirty(cx));
                assert_eq!(
                    app.read(cx).preferences_ui.saved.as_ref().unwrap().settings,
                    saved
                );
                app.update(cx, |app, cx| app.discard_settings(window, cx));
                assert_eq!(app.read(cx).sync_panel.read(cx).target, 0);
                assert!(!app.read(cx).settings_dirty(cx));
            });
        }
    }

    #[gpui_kit::test]
    fn portable_smoke_reopens_the_theme_preview_in_tree_and_drawer(cx: &mut TestAppContext) {
        use gpui_kit::px;
        let (app, visual) = fixture(cx);
        visual.update(|_, cx| cx.set_reduce_motion(true));
        visual.simulate_resize(gpui_kit::size(px(1400.), px(1000.)));
        let root = tempfile::tempdir().unwrap();
        let engine =
            studyapp_core::Engine::open(&root.path().join("preview.sqlite"), 1000).unwrap();
        let saved = app.read_with(visual, |app, _| {
            app.preferences_ui.saved.as_ref().unwrap().settings.clone()
        });
        let preview = app.read_with(visual, |app, _| {
            app.preferences_ui.appearance_preview.clone().unwrap()
        });
        let surface = visual.replace_root_view(|_, cx| {
            cx.observe(&app, |_, _, cx| cx.notify()).detach();
            CompleteWorkspaceFixture {
                app: app.clone(),
                snapshot: engine.snapshot(),
                width: 1000.,
                height: 700.,
            }
        });
        for width in [1000., 620., 1000.] {
            surface.update(visual, |surface, cx| {
                surface.width = width;
                cx.notify();
            });
            visual.run_until_parked();
            visual.update(|window, cx| {
                // The group entry is an overview, not the theme editor.
                app.update(cx, |app, cx| {
                    app.select_settings_section(SettingsSection::Appearance, cx);
                });
                assert!(!app.read(cx).appearance_preview_visible(window, cx));
                for page in [
                    "appearance-theme-editor",
                    "learning",
                    "appearance-theme-editor",
                    "about",
                ] {
                    crate::smoke::select_settings_page(window, &app, page, cx);
                    let state = app.read(cx);
                    let is_theme = page == "appearance-theme-editor";
                    assert_eq!(state.appearance_preview_visible(window, cx), is_theme);
                    assert!(!state.preferences_ui.settings_drawer);
                    if is_theme {
                        assert!(
                            state.preferences_ui.appearance_page == AppearancePage::ThemeEditor
                        );
                        assert_eq!(state.preferences_ui.browsing.selected, "appearance.theme");
                        assert!(window.try_find("appearance-markdown-preview").is_some());
                    }
                    if page == "about" {
                        assert!(state.preferences_ui.section == SettingsSection::AboutAccount);
                        assert!(window.try_find("about-identity").is_some());
                        assert!(window.try_find("about-version").is_some());
                        assert!(window.try_find("update-release-notes").is_none());
                    }
                    assert_eq!(
                        state
                            .preferences_ui
                            .appearance_preview
                            .as_ref()
                            .unwrap()
                            .entity_id(),
                        preview.entity_id()
                    );
                    assert!(state.preferences_ui.editing_preset.is_none());
                    assert!(!state.preferences_ui.appearance_touched);
                    assert!(!state.settings_dirty(cx));
                    assert_eq!(state.preferences_ui.saved.as_ref().unwrap().settings, saved);
                }
                // This fixture only checks navigation and never configures or mounts a WebView.
                assert!(preview.read(cx).editor().is_none());
            });
        }
    }

    #[gpui_kit::test]
    fn portable_smoke_fully_reveals_the_theme_preview_in_short_windows(cx: &mut TestAppContext) {
        use gpui_kit::px;
        let (app, visual) = fixture(cx);
        visual.update(|_, cx| cx.set_reduce_motion(true));
        visual.simulate_resize(gpui_kit::size(px(1400.), px(1000.)));
        let root = tempfile::tempdir().unwrap();
        let engine =
            studyapp_core::Engine::open(&root.path().join("preview-scroll.sqlite"), 1000).unwrap();
        let surface = visual.replace_root_view(|_, cx| {
            cx.observe(&app, |_, _, cx| cx.notify()).detach();
            CompleteWorkspaceFixture {
                app: app.clone(),
                snapshot: engine.snapshot(),
                width: 1000.,
                height: 700.,
            }
        });
        for (width, height) in [(1000., 700.), (806., 600.), (620., 500.), (806., 460.)] {
            surface.update(visual, |surface, cx| {
                surface.width = width;
                surface.height = height;
                cx.notify();
            });
            visual.run_until_parked();
            visual.update(|window, cx| {
                crate::smoke::select_settings_page(window, &app, "learning", cx);
                crate::smoke::select_settings_page(window, &app, "appearance-theme-editor", cx);
                crate::smoke_library::reveal_fully_scrolled(
                    window,
                    "appearance-preview-pane".into(),
                    "settings-section-scroll",
                    cx,
                );
                let pane = window.find("appearance-preview-pane").bounds();
                let viewport = window.find("settings-section-scroll").bounds();
                assert_eq!(pane.size.height, px(220.));
                assert_eq!(
                    viewport.intersect(&pane),
                    pane,
                    "preview is still clipped in {width}x{height}: {viewport:?} / {pane:?}"
                );
                assert!(app.read(cx).appearance_preview_visible(window, cx));
                assert!(!app.read(cx).settings_dirty(cx));
                assert!(
                    app.read(cx)
                        .preferences_ui
                        .appearance_preview
                        .as_ref()
                        .unwrap()
                        .read(cx)
                        .editor()
                        .is_none()
                );
            });
        }
    }

    #[gpui_kit::test]
    fn native_settings_rgba_capture_reveals_the_complete_field_without_committing(
        cx: &mut TestAppContext,
    ) {
        use gpui_kit::px;
        let (app, visual) = fixture(cx);
        visual.update(|_, cx| cx.set_reduce_motion(true));
        visual.simulate_resize(gpui_kit::size(px(1400.), px(1000.)));
        let root = tempfile::tempdir().unwrap();
        let engine =
            studyapp_core::Engine::open(&root.path().join("color-capture.sqlite"), 1000).unwrap();
        let mut surface = None;
        visual.replace_root_view(|window, cx| {
            let content = cx.new(|cx| {
                cx.observe(&app, |_, _, cx| cx.notify()).detach();
                CompleteWorkspaceFixture {
                    app: app.clone(),
                    snapshot: engine.snapshot(),
                    width: 1000.,
                    height: 700.,
                }
            });
            surface = Some(content.clone());
            gpui_kit::base::Root::new(content, window, cx)
        });
        let surface = surface.unwrap();
        for width in [1000., 620.] {
            surface.update(visual, |surface, cx| {
                surface.width = width;
                cx.notify();
            });
            visual.run_until_parked();
            visual.update(|window, cx| {
                crate::smoke::select_settings_page(window, &app, "appearance-theme-editor", cx);
                crate::smoke_library::click_scrolled(
                    window,
                    "color-open-accent".into(),
                    "settings-section-scroll",
                    cx,
                );
            });
            visual.run_until_parked();
            let (control, source) = app.read_with(visual, |app, _| {
                (
                    app.preferences_ui.appearance_colors["accent"].clone(),
                    app.preferences_ui.appearance_fields["accent"].clone(),
                )
            });
            let original = source.read_with(visual, |input, _| input.value());
            visual.update(|window, cx| {
                crate::smoke_library::click_scrolled(
                    window,
                    "color-value-accent".into(),
                    "color-palette-scroll",
                    cx,
                );
                window.press(
                    if cfg!(target_os = "macos") {
                        "cmd-a"
                    } else {
                        "ctrl-a"
                    },
                    cx,
                );
            });
            visual.run_until_parked();
            visual.update(|window, cx| window.input("#33669980", cx));
            visual.run_until_parked();
            visual.update(|window, cx| {
                assert_eq!(control.read(cx).preview_value(cx).as_str(), "#33669980");
                assert_eq!(source.read(cx).value(), original);
                crate::smoke::prepare_color_input_capture(window, "accent", cx);
                window.click("color-cancel-accent", cx);
            });
            visual.run_until_parked();
            control.read_with(visual, |control, cx| {
                assert!(!control.popup_open());
                assert_eq!(source.read(cx).value(), original);
            });
        }
    }

    #[gpui_kit::test]
    fn native_settings_notification_tasks_cache_actual_tree_pages(cx: &mut TestAppContext) {
        use gpui_kit::px;
        let (app, visual) = fixture(cx);
        visual.update(|_, cx| cx.set_reduce_motion(true));
        visual.simulate_resize(gpui_kit::size(px(1400.), px(1000.)));
        let root = tempfile::tempdir().unwrap();
        let engine =
            studyapp_core::Engine::open(&root.path().join("notice-tasks.sqlite"), 1000).unwrap();
        let surface = visual.replace_root_view(|_, cx| {
            cx.observe(&app, |_, _, cx| cx.notify()).detach();
            CompleteWorkspaceFixture {
                app: app.clone(),
                snapshot: engine.snapshot(),
                width: 1000.,
                height: 700.,
            }
        });
        let saved = app.read_with(visual, |app, _| {
            app.preferences_ui.saved.as_ref().unwrap().settings.clone()
        });
        let targets = std::iter::once("notices.general".to_owned())
            .chain(
                (0..studyapp_core::NoticeStyle::ALL.len())
                    .map(|index| format!("notices.component.{index}")),
            )
            .collect::<Vec<_>>();
        for width in [1000., 620.] {
            surface.update(visual, |surface, cx| {
                surface.width = width;
                cx.notify();
            });
            visual.run_until_parked();
            for target in &targets {
                visual.update(|window, cx| {
                    crate::smoke::select_settings_page(window, &app, target, cx)
                });
                if target == "notices.general" {
                    let before_scroll = visual.update(|window, _| {
                        window.find("notification-width-increase").bounds().top()
                    });
                    visual.update(|window, cx| {
                        let body = window.find("settings-section-scroll").bounds();
                        window.dispatch_event(
                            gpui_kit::ScrollWheelEvent {
                                position: body.center(),
                                delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
                                    px(0.),
                                    px(-80.),
                                )),
                                ..Default::default()
                            }
                            .to_platform_input(),
                            cx,
                        );
                    });
                    for _ in 0..3 {
                        visual.update(|window, cx| window.draw(cx).clear(cx));
                    }
                    visual.update(|window, _| {
                        assert!(
                            window.find("notification-width-increase").bounds().top()
                                < before_scroll,
                            "cached task page swallowed real scrolling; width={width}"
                        );
                    });
                }
                for _ in 0..6 {
                    visual.run_until_parked();
                    visual.update(|window, cx| window.draw(cx).clear(cx));
                }
                let before = app.read_with(visual, |app, _| app.notification_render_work());
                for _ in 0..20 {
                    app.update(visual, |_, cx| cx.notify());
                    visual.update(|window, cx| window.draw(cx).clear(cx));
                }
                assert_eq!(
                    app.read_with(visual, |app, _| app.notification_render_work()),
                    before,
                    "actual tree page rebuilt on shell frames: {target}; width={width}"
                );
                app.update(visual, |app, cx| {
                    let appearance = &mut app
                        .preferences_ui
                        .draft
                        .as_mut()
                        .unwrap()
                        .appearance
                        .notifications;
                    appearance.reduce_motion = !appearance.reduce_motion;
                    cx.notify();
                });
                for _ in 0..3 {
                    visual.update(|window, cx| window.draw(cx).clear(cx));
                }
                let changed =
                    app.read_with(visual, |app, _| app.notification_render_work()) - before;
                assert!(
                    changed > 0 && changed < 22,
                    "relevant edit did not rebuild only selected controls: {target}: {changed}"
                );
                app.read_with(visual, |app, cx| {
                    assert_eq!(app.preferences_ui.browsing.selected, *target);
                    assert!(app.notification_task_cache_matches(cx));
                    assert_eq!(
                        app.settings_target_title(),
                        crate::settings_navigation::target_title(target)
                    );
                    assert!(!app.preferences_ui.settings_drawer);
                    assert_eq!(app.preferences_ui.saved.as_ref().unwrap().settings, saved);
                    assert!(
                        app.preferences_ui
                            .appearance_preview
                            .as_ref()
                            .unwrap()
                            .read(cx)
                            .editor()
                            .is_none()
                    );
                });
            }
        }
    }

    #[gpui_kit::test]
    fn native_settings_capture_transactions_preserve_page_state_in_tree_and_drawer(
        cx: &mut TestAppContext,
    ) {
        use gpui_kit::px;
        let (app, visual) = fixture(cx);
        visual.update(|_, cx| cx.set_reduce_motion(true));
        visual.simulate_resize(gpui_kit::size(px(1400.), px(1000.)));
        let root = tempfile::tempdir().unwrap();
        let engine =
            studyapp_core::Engine::open(&root.path().join("capture.sqlite"), 1000).unwrap();
        let surface = visual.replace_root_view(|_, cx| {
            cx.observe(&app, |_, _, cx| cx.notify()).detach();
            CompleteWorkspaceFixture {
                app: app.clone(),
                snapshot: engine.snapshot(),
                width: 1000.,
                height: 700.,
            }
        });
        let handle = visual.update(|window, _| window.window_handle());
        let mut captures = std::collections::BTreeMap::new();
        for width in [1000., 620.] {
            surface.update(visual, |surface, cx| {
                surface.width = width;
                cx.notify();
            });
            visual.run_until_parked();
            for (page, target, anchor) in [
                ("shortcuts", "shortcuts", "record-shortcut-app.new"),
                (
                    "appearance-theme-editor",
                    "appearance.theme",
                    "appearance-preview-pane",
                ),
                (
                    "appearance-workspace",
                    "appearance.workspace",
                    "show-file-paths",
                ),
                ("appearance-overview", "appearance.overview", "theme-mode"),
                (
                    "appearance-notifications",
                    "notices.general",
                    "notification-width-increase",
                ),
                (
                    "notifications",
                    "notices.rules",
                    "notification-update-badge",
                ),
                ("storage-overview", "storage.overview", "refresh-storage"),
                ("storage-images", "storage.images", "images-storage"),
                ("storage-library", "storage.library", "migrate-library"),
            ] {
                let called = std::rc::Rc::new(std::cell::Cell::new(false));
                let observed = called.clone();
                let app = app.clone();
                let task = visual.spawn(|mut cx| async move {
                    let callback_app = app.clone();
                    crate::smoke::with_settings_capture(
                        handle,
                        &app,
                        page,
                        move |window, cx| {
                            observed.set(true);
                            let app = &callback_app;
                            let state = app.read(cx);
                            assert_eq!(state.preferences_ui.browsing.selected, target);
                            assert_eq!(
                                state.settings_target_title(),
                                crate::settings_navigation::target_title(target)
                            );
                            if target == "notices.general" {
                                assert!(state.notification_task_cache_matches(cx));
                            }
                            assert!(
                                window.try_find(anchor).is_some(),
                                "page={page}; width={width}"
                            );
                            assert!(!state.preferences_ui.settings_drawer);
                            if target.starts_with("appearance.") {
                                assert!(state.notification_appearance_cache_matches(cx));
                                let expected = match target {
                                    "appearance.theme" => (1 << 1) | (1 << 3),
                                    "appearance.workspace" => 1 << 4,
                                    "appearance.overview" => 1,
                                    _ => unreachable!(),
                                };
                                // Rows are virtualized: only the visible subset needs
                                // building, but none may belong to a previous page.
                                let parts = state.preferences_ui.appearance_frame_parts.get();
                                assert_ne!(parts, 0, "no appearance rows built for {target}");
                                assert_eq!(
                                    parts & !expected,
                                    0,
                                    "cached rows did not match capture target {target}: {parts:b}"
                                );
                            }
                            assert!(!state.settings_dirty(cx));
                            assert!(
                                state
                                    .preferences_ui
                                    .appearance_preview
                                    .as_ref()
                                    .unwrap()
                                    .read(cx)
                                    .editor()
                                    .is_none()
                            );
                            assert_eq!(f32::from(window.viewport_size().width), 1400.);
                            let filename = crate::smoke::settings_capture_filename(window, page);
                            let layout = if width == 1000. { "tree" } else { "drawer" };
                            let pane_width =
                                f32::from(window.find("settings-section-pane").bounds().size.width)
                                    as u32;
                            assert_eq!(
                                filename,
                                format!("settings-{page}-1400-{layout}-{pane_width}.png")
                            );
                            filename
                        },
                        &mut cx,
                    )
                    .await
                });
                visual.run_until_parked();
                assert!(!called.get(), "capture ran before the platform frame");
                visual.update(|window, cx| {
                    window.simulate_next_frame(cx);
                });
                visual.run_until_parked();
                assert!(!called.get(), "capture ran before the child-view frame");
                visual.update(|window, cx| {
                    window.simulate_next_frame(cx);
                });
                visual.run_until_parked();
                assert!(called.get(), "capture did not follow the platform frames");
                let filename = visual.foreground_executor().block_test(task);
                if let Some(previous) = captures.insert(page, filename.clone()) {
                    assert_ne!(
                        previous, filename,
                        "drawer capture overwrote the tree: {page}"
                    );
                }
            }
        }
    }

    struct NotifiedWorkspaceFixture {
        workspace: Entity<CompleteWorkspaceFixture>,
        notifications: Entity<crate::notifications::NotificationCenter>,
    }
    impl Render for NotifiedWorkspaceFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .relative()
                .size_full()
                .child(self.workspace.clone())
                .child(self.notifications.clone())
        }
    }

    #[gpui_kit::test]
    fn portable_smoke_interval_save_from_restored_about_page(cx: &mut TestAppContext) {
        use gpui_kit::px;
        for width in [1400., 1000., 620.] {
            let (app, visual) = fixture(cx);
            visual.update(|_, cx| cx.set_reduce_motion(true));
            visual.simulate_resize(gpui_kit::size(px(width), px(700.)));
            let root = tempfile::tempdir().unwrap();
            let engine =
                studyapp_core::Engine::open(&root.path().join("save.sqlite"), 1000).unwrap();
            visual.replace_root_view(|_, cx| {
                let workspace = cx.new(|cx| {
                    cx.observe(&app, |_, _, cx| cx.notify()).detach();
                    CompleteWorkspaceFixture {
                        app: app.clone(),
                        snapshot: engine.snapshot(),
                        width,
                        height: 700.,
                    }
                });
                NotifiedWorkspaceFixture {
                    workspace,
                    notifications: app.read(cx).notifications.clone(),
                }
            });
            let due_id = visual.update(|window, cx| {
                crate::smoke::select_settings_page(window, &app, "about", cx);
                let center = app.read(cx).notifications.clone();
                center.update(cx, |center, cx| {
                    center.freeze_time();
                    center.configure(
                        &studyapp_core::NotificationSettings::default(),
                        &studyapp_core::NotificationAppearance {
                            reduce_motion: true,
                            ..Default::default()
                        },
                        studyapp_core::NoticeEnvironment {
                            foreground: true,
                            desktop_available: false,
                            anchor_available: false,
                            minute: 720,
                        },
                        false,
                        cx,
                    );
                });
                app.update(cx, |app, cx| {
                    app.push_notice(Ok("计时已暂停".into()), cx);
                    let mut due = studyapp_core::NoticeEvent::new(
                        studyapp_core::NoticeCategory::ReviewDue,
                        studyapp_core::NoticeSeverity::Information,
                        "有 1 项复习已到期，可以按自己的节奏开始。",
                    );
                    due.action = Some(studyapp_core::NoticeAction::OpenReviews);
                    app.emit_notification(due, cx).unwrap()
                })
            });
            visual.run_until_parked();
            visual.update(|window, cx| {
                window.render_frame(cx);
                let center = app.read(cx).notifications.read(cx);
                assert!(center.visible_count() > 0);
                assert!(!center.occlusions().is_empty());
                let footer = window.find("settings-save-footer").bounds();
                assert!(
                    center
                        .occlusions()
                        .iter()
                        .all(|rect| !rect.intersects(&footer)),
                    "notification covers settings actions at width {width}: {:?}",
                    center.occlusions()
                );
            });
            let handle = visual.update(|window, _| window.window_handle());
            crate::smoke::queue_interval_save(handle, &app, 25, None, &mut visual.to_async());
            app.read_with(visual, |app, cx| {
                assert_eq!(app.preferences_ui.browsing.selected, "learning");
                assert!(!app.preferences_ui.settings_drawer);
                assert_eq!(app.interval.read(cx).value(), "25");
                assert!(app.preferences_ui.busy);
                assert!(app.preferences_ui.error.is_none());
                assert!(
                    app.notifications.read(cx).test_event_route(due_id)
                        == Some(studyapp_core::NoticeRoute::Foreground(
                            studyapp_core::NoticeStyle::CornerCard
                        )),
                    "saving must not dismiss or activate the visible or queued review notification"
                );
                assert_eq!(
                    app.preferences_ui
                        .baseline
                        .as_ref()
                        .unwrap()
                        .settings
                        .interval_minutes,
                    60
                );
                assert!(
                    app.preferences_ui
                        .appearance_preview
                        .as_ref()
                        .unwrap()
                        .read(cx)
                        .editor()
                        .is_none()
                );
            });
        }
    }

    #[gpui_kit::test]
    fn settings_transition_completion_and_reduced_motion_keep_an_open_choice_menu(
        cx: &mut TestAppContext,
    ) {
        use gpui_kit::px;
        let (app, visual) = fixture(cx);
        visual.simulate_resize(gpui_kit::size(px(1400.), px(1000.)));
        let root = tempfile::tempdir().unwrap();
        let engine =
            studyapp_core::Engine::open(&root.path().join("transition.sqlite"), 1000).unwrap();
        visual.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.select_settings_target("sync", cx);
                // Keep the transition active until the test explicitly expires it.
                app.preferences_ui.section_changed_at =
                    Some(std::time::Instant::now() + std::time::Duration::from_secs(60));
            });
        });
        visual.replace_root_view(|_, cx| {
            cx.observe(&app, |_, _, cx| cx.notify()).detach();
            CompleteWorkspaceFixture {
                app: app.clone(),
                snapshot: engine.snapshot(),
                width: 1000.,
                height: 700.,
            }
        });
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("sync-target", cx);
            window.render_frame(cx);
            assert!(window.try_find("popup-menu").is_some());
            let menu_focus = window.focused(cx);
            assert!(menu_focus.is_some());
            let revision = app.read(cx).preferences_ui.section_revision;
            app.update(cx, |app, cx| {
                app.preferences_ui.section_changed_at =
                    Some(std::time::Instant::now() - std::time::Duration::from_millis(200));
                cx.notify();
            });
            window.render_frame(cx);
            assert!(
                window.try_find("popup-menu").is_some(),
                "finishing a category transition dismissed the open choice menu"
            );
            cx.set_reduce_motion(true);
            window.render_frame(cx);
            assert!(window.try_find("popup-menu").is_some());
            assert_eq!(app.read(cx).preferences_ui.section_revision, revision);
            assert_eq!(window.focused(cx), menu_focus);
            assert!(!app.read(cx).settings_dirty(cx));
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            for key in ["down", "down", "enter"] {
                window.press(key, cx);
                window.render_frame(cx);
            }
            assert_eq!(app.read(cx).sync_panel.read(cx).target, 1);
            assert!(app.read(cx).settings_dirty(cx));
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("popup-menu").is_none());
        });
    }

    #[gpui_kit::test]
    fn complete_workspace_uses_allocated_width_and_keeps_header_footer_outside_scroll(
        cx: &mut TestAppContext,
    ) {
        use gpui_kit::px;
        let (app, visual) = fixture(cx);
        visual.simulate_resize(gpui_kit::size(px(1400.), px(1000.)));
        let root = tempfile::tempdir().unwrap();
        let engine = studyapp_core::Engine::open(&root.path().join("layout.sqlite"), 1000).unwrap();
        let original = app.read_with(visual, |app, _| app.preferences_ui.draft.clone());
        let surface = visual.replace_root_view(|_, cx| {
            cx.observe(&app, |_, _, cx| cx.notify()).detach();
            CompleteWorkspaceFixture {
                app: app.clone(),
                snapshot: engine.snapshot(),
                width: 1000.,
                height: 700.,
            }
        });
        for font in [12, 22] {
            for width in [1000., 820., 620., 1000.] {
                surface.update(visual, |surface, cx| {
                    surface.width = width;
                    cx.notify();
                });
                visual.update(|_, cx| {
                    let appearance = studyapp_core::AppearanceSettings {
                        font_size: font,
                        ..Default::default()
                    };
                    crate::theme::apply(&appearance, false, cx);
                });
                for _ in 0..4 {
                    visual.run_until_parked();
                    visual.update(|window, cx| window.draw(cx).clear(cx));
                }
                visual.update(|window, cx| {
                    let workspace = window.find("settings-workspace").bounds();
                    let pane = window.find("settings-section-pane").bounds();
                    assert_eq!(workspace.size.width, px(width));
                    assert_eq!(pane.right(), workspace.right());
                    let header = window.find("settings-titlebar").bounds();
                    let body = window.find("settings-section-scroll").bounds();
                    let footer = window.find("settings-save-footer").bounds();
                    assert!(header.bottom() <= body.top());
                    assert!(body.bottom() <= footer.top());
                    assert!(body.size.height > px(200.));
                    assert!(footer.bottom() <= px(701.));
                    assert!(window.find("save-settings").bounds().right() <= pane.right());
                    if width >= 710. {
                        let tree = window.find("settings-category-navigation").bounds();
                        assert!(tree.right() <= pane.left());
                        assert!(window.try_find("settings-drawer-toggle").is_none());
                    } else {
                        assert!(window.try_find("settings-category-navigation").is_none());
                        window.click("settings-drawer-toggle", cx);
                        window.render_frame(cx);
                        assert!(app.read(cx).preferences_ui.settings_drawer);
                        window.click("settings-group-learning", cx);
                        window.render_frame(cx);
                        assert!(!app.read(cx).preferences_ui.settings_drawer);
                    }
                    assert_eq!(app.read(cx).preferences_ui.draft, original);
                    assert!(!app.read(cx).settings_dirty(cx));
                });
            }
        }
    }

    #[gpui_kit::test]
    fn roomy_storage_fixture_has_real_outer_overflow_without_clipping_its_inner_list(
        cx: &mut TestAppContext,
    ) {
        use crate::storage_backend::{CopyState, ManagedCopy, StorageReport};
        use gpui_kit::px;
        let (app, visual) = fixture(cx);
        visual.simulate_resize(gpui_kit::size(px(1024.), px(720.)));
        let root = tempfile::tempdir().unwrap();
        let engine = studyapp_core::Engine::open(&root.path().join("layout.sqlite"), 1000).unwrap();
        let mut report = StorageReport {
            copies: (0..18)
                .map(|index| ManagedCopy {
                    note_id: format!("fixture-{index}"),
                    title: format!("空间滚动测试 {index:02}"),
                    path: format!("C:/fixture/notes/{index}.md"),
                    bytes: 41,
                    state: CopyState::Present,
                    deletable: true,
                    reason: None,
                })
                .collect(),
            ..Default::default()
        };
        report.prepare_presentation();
        app.update(visual, |app, cx| {
            app.storage_report = Some(report);
            let appearance = &mut app.preferences_ui.draft.as_mut().unwrap().appearance;
            appearance.font_size = 22;
            appearance.spacing = 16;
            crate::theme::apply(appearance, false, cx);
            app.select_settings_target("storage.copies", cx);
        });
        visual.replace_root_view(|_, cx| {
            cx.observe(&app, |_, _, cx| cx.notify()).detach();
            CompleteWorkspaceFixture {
                app: app.clone(),
                snapshot: engine.snapshot(),
                width: 840.,
                height: 656.,
            }
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.render_frame(cx);
            crate::smoke_library::scroll_settings(window, 10000., cx);
            let top = window.find("storage-copy-list").bounds();
            crate::smoke_library::scroll_settings(window, -10000., cx);
            let bottom = window.find("storage-copy-list").bounds();
            let viewport = window.find("settings-section-scroll").bounds();
            assert!(top.top() > bottom.top(), "fixture has no outer range");
            assert_eq!(bottom.size.height, px(340.));
            assert!(bottom.top() >= viewport.top());
            assert!(bottom.bottom() <= viewport.bottom());
            assert!(app.read(cx).storage_ui.scroll.max_offset_for_scrollbar().y > px(0.));
        });
    }

    struct NavigationTreeFixture(Entity<StudyApp>);
    impl Render for NavigationTreeFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            use gpui_kit::{InteractiveElement, TestSupportExt, px};
            self.0.update(cx, |app, cx| {
                crate::components::row()
                    .relative()
                    .w(px(900.))
                    .h(px(400.))
                    .child(app.settings_tree(cx))
                    .child(app.settings_tree_separator(cx))
                    .child(
                        div()
                            .id("settings-section-scroll")
                            .test_support()
                            .w(px(500.))
                            .h_full(),
                    )
                    .child(app.settings_resize_events(cx))
            })
        }
    }

    #[gpui_kit::test]
    fn navigation_resize_ends_before_applying_unpressed_hover(cx: &mut TestAppContext) {
        use gpui_kit::{MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, point, px};

        let (app, visual) = fixture(cx);
        let original = app.read_with(visual, |app, _| app.preferences_ui.draft.clone());
        visual.replace_root_view(|_, cx| {
            cx.observe(&app, |_, _, cx| cx.notify()).detach();
            NavigationTreeFixture(app.clone())
        });
        for release_delivered in [false, true] {
            app.update(visual, |app, cx| {
                app.preferences_ui.browsing.width = 224;
                cx.notify();
            });
            visual.update(|window, cx| {
                window.render_frame(cx);
                assert!(window.try_find("settings-tree-width-reset").is_none());
                assert_eq!(
                    window.find("settings-tree-scroll").bounds().bottom(),
                    px(400.)
                );
                let start = window.find("settings-tree-resizer").bounds().center();
                window.dispatch_event(
                    MouseDownEvent {
                        button: MouseButton::Left,
                        position: start,
                        click_count: 1,
                        ..Default::default()
                    }
                    .to_platform_input(),
                    cx,
                );
                window.render_frame(cx);
                assert!(app.read(cx).preferences_ui.settings_resize.is_some());
                window.dispatch_event(
                    MouseMoveEvent {
                        position: start + point(px(32.), px(0.)),
                        pressed_button: Some(MouseButton::Left),
                        ..Default::default()
                    }
                    .to_platform_input(),
                    cx,
                );
                window.render_frame(cx);
                assert_eq!(app.read(cx).preferences_ui.browsing.width, 256);
                if release_delivered {
                    window.dispatch_event(
                        MouseUpEvent {
                            button: MouseButton::Left,
                            position: start + point(px(32.), px(0.)),
                            click_count: 1,
                            ..Default::default()
                        }
                        .to_platform_input(),
                        cx,
                    );
                    window.render_frame(cx);
                }
                for delta in [96., -96.] {
                    window.dispatch_event(
                        MouseMoveEvent {
                            position: start + point(px(delta), px(0.)),
                            ..Default::default()
                        }
                        .to_platform_input(),
                        cx,
                    );
                    window.render_frame(cx);
                    assert_eq!(
                        app.read(cx).preferences_ui.browsing.width,
                        256,
                        "unpressed hover changed the last actively dragged width"
                    );
                    assert!(app.read(cx).preferences_ui.settings_resize.is_none());
                }
                assert_eq!(app.read(cx).preferences_ui.draft, original);
                assert!(!app.read(cx).settings_dirty(cx));
            });
        }
    }

    #[gpui_kit::test]
    fn long_navigation_tree_scrolls_independently_without_editing_settings(
        cx: &mut TestAppContext,
    ) {
        let (app, visual) = fixture(cx);
        let original = app.read_with(visual, |app, _| app.preferences_ui.draft.clone());
        visual.replace_root_view(|_, cx| {
            cx.observe(&app, |_, _, cx| cx.notify()).detach();
            NavigationTreeFixture(app.clone())
        });
        visual.update(|window, cx| {
            window.render_frame(cx);
            let tree_viewport = window.find("settings-tree-scroll").bounds();
            assert!(
                window.find("settings-group-learning").bounds().right()
                    <= tree_viewport.right() - gpui_kit::component::scroll::Scrollbar::width()
            );
            assert!(
                app.read(cx)
                    .preferences_ui
                    .settings_tree_scroll
                    .max_offset()
                    .y
                    > gpui_kit::px(500.)
            );
            crate::smoke_settings::select_page(window, &app, "storage-library", cx);
            assert_eq!(
                app.read(cx).preferences_ui.browsing.selected,
                "storage.library"
            );
            assert!(app.read(cx).preferences_ui.settings_tree_scroll.offset().y < gpui_kit::px(0.));
            let target = window.find("settings-target-storage.library");
            let viewport = window.find("settings-tree-scroll").bounds();
            assert!(target.visible() && viewport.contains(&target.bounds().center()));
            assert_eq!(app.read(cx).preferences_ui.draft, original);
        });
    }

    #[gpui_kit::test]
    fn search_result_controls_leave_the_scrollbar_lane_and_track_drag_keeps_the_page(
        cx: &mut TestAppContext,
    ) {
        use gpui_kit::{point, px};
        let (app, visual) = fixture(cx);
        visual.simulate_resize(gpui_kit::size(px(1400.), px(1000.)));
        let root = tempfile::tempdir().unwrap();
        let engine = studyapp_core::Engine::open(&root.path().join("search.sqlite"), 1000).unwrap();
        visual.replace_root_view(|_, cx| {
            cx.observe(&app, |_, _, cx| cx.notify()).detach();
            CompleteWorkspaceFixture {
                app: app.clone(),
                snapshot: engine.snapshot(),
                width: 1000.,
                height: 700.,
            }
        });
        visual.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.open_settings_search(window, cx);
                app.preferences_ui
                    .search
                    .as_ref()
                    .unwrap()
                    .update(cx, |input, cx| input.set_value("通知", window, cx));
            })
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.render_frame(cx);
            let viewport = window.find("settings-search-results").bounds();
            let scrollbar_width = gpui_kit::component::scroll::Scrollbar::width();
            assert!(
                window
                    .find(("settings-search-result", 0_usize))
                    .bounds()
                    .right()
                    <= viewport.right() - scrollbar_width
            );
            let scroll = app.read(cx).preferences_ui.settings_search_scroll.clone();
            assert!(scroll.max_offset().y > px(0.));
            let selected = app.read(cx).preferences_ui.browsing.selected.clone();
            let draft = app.read(cx).preferences_ui.draft.clone();
            let x = viewport.right() - scrollbar_width / 2.;
            window.drag(
                point(x, viewport.top() + px(8.)),
                point(x, viewport.bottom() - px(2.)),
                cx,
            );
            assert_eq!(scroll.offset().y, -scroll.max_offset().y);
            assert!(app.read(cx).preferences_ui.settings_search_open);
            assert_eq!(app.read(cx).preferences_ui.browsing.selected, selected);
            assert_eq!(app.read(cx).preferences_ui.draft, draft);
        });
    }

    struct ThemePresetFixture {
        app: Entity<StudyApp>,
    }
    impl Render for ThemePresetFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.app.update(cx, |app, cx| {
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(app.theme_preset_controls(cx))
                    .children(app.settings_modal(cx))
            })
        }
    }

    fn theme_fixture(cx: &mut TestAppContext) -> (Entity<StudyApp>, &mut VisualTestContext) {
        let (app, visual) = fixture(cx);
        visual.update(|window, cx| {
            app.update(cx, |app, cx| {
                let appearance = &mut app
                    .preferences_ui
                    .saved
                    .as_mut()
                    .unwrap()
                    .settings
                    .appearance;
                appearance.mode = studyapp_core::ThemeMode::Custom;
                appearance.active_preset = Some("theme-0".into());
                appearance.custom_presets = (0..9)
                    .map(|index| studyapp_core::ThemePreset {
                        id: format!("theme-{index}"),
                        name: format!("主题 {index}"),
                        palette: Default::default(),
                        markdown: Default::default(),
                    })
                    .collect();
                appearance.notifications.width = 420;
                app.begin_settings(window, cx);
            })
        });
        visual.replace_root_view(|_, cx| {
            cx.observe(&app, |_, _, cx| cx.notify()).detach();
            ThemePresetFixture { app: app.clone() }
        });
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));
        (app, visual)
    }

    #[gpui_kit::test]
    fn custom_theme_choice_preserves_pending_edits_and_blocks_invalid_replacements(
        cx: &mut TestAppContext,
    ) {
        let (app, visual) = theme_fixture(cx);
        visual.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.preferences_ui.appearance_fields["name"]
                    .update(cx, |input, cx| input.set_value("edited zero", window, cx));
                app.preferences_ui.appearance_touched = true;
            });
            studyapp_ui::settings_components::select_choice(window, "custom-theme-preset", 1, cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            app.update(cx, |app, cx| {
                let appearance = &app.preferences_ui.draft.as_ref().unwrap().appearance;
                assert_eq!(appearance.active_preset.as_deref(), Some("theme-1"));
                assert_eq!(appearance.custom_presets[0].name, "edited zero");
                assert_eq!(
                    app.preferences_ui.appearance_fields["name"]
                        .read(cx)
                        .value(),
                    "主题 1"
                );
                app.preferences_ui.appearance_fields["text"]
                    .update(cx, |input, cx| input.set_value("invalid-color", window, cx));
                app.preferences_ui.appearance_touched = true;
            });
            studyapp_ui::settings_components::select_choice(window, "custom-theme-preset", 2, cx);
        });
        visual.run_until_parked();
        app.read_with(visual, |app, cx| {
            let appearance = &app.preferences_ui.draft.as_ref().unwrap().appearance;
            assert_eq!(appearance.active_preset.as_deref(), Some("theme-1"));
            assert_eq!(appearance.custom_presets[0].name, "edited zero");
            assert_eq!(
                app.preferences_ui.appearance_fields["text"]
                    .read(cx)
                    .value(),
                "invalid-color"
            );
            assert!(app.preferences_ui.appearance_touched);
            assert!(app.preferences_ui.error.is_some());
            assert_eq!(
                app.preferences_ui
                    .saved
                    .as_ref()
                    .unwrap()
                    .settings
                    .appearance
                    .custom_presets[0]
                    .name,
                "主题 0"
            );
        });
    }

    #[gpui_kit::test]
    fn theme_deletion_requires_confirmation_preserves_other_settings_and_is_discardable(
        cx: &mut TestAppContext,
    ) {
        let (app, visual) = theme_fixture(cx);
        let saved = app.read_with(visual, |app, _| {
            app.preferences_ui.saved.as_ref().unwrap().settings.clone()
        });
        visual.update(|window, cx| {
            app.read(cx).interval.clone().update(cx, |input, cx| {
                input.set_value("invalid learning", window, cx)
            });
            window.click("delete-custom-theme", cx);
        });
        visual.run_until_parked();
        app.read_with(visual, |app, cx| {
            assert!(app.has_required_modal(cx));
            assert_eq!(app.preferences_ui.draft.as_ref().unwrap(), &saved);
        });
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("cancel-delete-theme", cx);
        });
        visual.run_until_parked();
        app.read_with(visual, |app, _| {
            assert!(app.preferences_ui.theme_delete.is_none());
            assert_eq!(app.preferences_ui.draft.as_ref().unwrap(), &saved);
        });
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("delete-custom-theme", cx);
            app.update(cx, |app, cx| {
                app.preferences_ui.busy = true;
                cx.notify();
            });
            window.render_frame(cx);
            window.click("confirm-delete-theme", cx);
        });
        visual.run_until_parked();
        app.read_with(visual, |app, _| {
            assert_eq!(app.preferences_ui.draft.as_ref().unwrap(), &saved)
        });
        visual.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.preferences_ui.busy = false;
                cx.notify();
            });
            window.render_frame(cx);
            window.click("confirm-delete-theme", cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            app.update(cx, |app, cx| {
                let draft = app.preferences_ui.draft.as_ref().unwrap();
                assert_eq!(
                    draft.appearance.custom_presets,
                    saved.appearance.custom_presets[1..]
                );
                assert_eq!(draft.appearance.mode, studyapp_core::ThemeMode::System);
                assert!(draft.appearance.active_preset.is_none());
                assert_eq!(draft.appearance.notifications.width, 420);
                assert_eq!(app.interval.read(cx).value(), "invalid learning");
                assert_eq!(&app.preferences_ui.saved.as_ref().unwrap().settings, &saved);
                assert!(app.settings_dirty(cx));
                assert!(!app.has_required_modal(cx));
                app.discard_settings(window, cx);
                assert_eq!(app.preferences_ui.draft.as_ref().unwrap(), &saved);
                assert_eq!(
                    app.preferences_ui.editing_preset.as_deref(),
                    Some("theme-0")
                );
                assert!(!app.settings_dirty(cx));
            })
        });
    }

    #[gpui_kit::test]
    fn deleting_another_theme_does_not_overwrite_invalid_active_theme_fields(
        cx: &mut TestAppContext,
    ) {
        let (app, visual) = theme_fixture(cx);
        visual.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.preferences_ui.appearance_fields["text"]
                    .update(cx, |input, cx| input.set_value("unfinished", window, cx));
                app.preferences_ui.appearance_touched = true;
                app.preferences_ui.theme_delete = Some("theme-1".into());
                cx.notify();
            });
            window.render_frame(cx);
            window.click("confirm-delete-theme", cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            app.update(cx, |app, cx| {
                assert_eq!(
                    app.preferences_ui.appearance_fields["text"]
                        .read(cx)
                        .value(),
                    "unfinished"
                );
                assert!(app.preferences_ui.appearance_touched);
                let appearance = &app.preferences_ui.draft.as_ref().unwrap().appearance;
                assert_eq!(appearance.active_preset.as_deref(), Some("theme-0"));
                assert_eq!(appearance.custom_presets.len(), 8);
                assert!(
                    !appearance
                        .custom_presets
                        .iter()
                        .any(|preset| preset.id == "theme-1")
                );
                app.preferences_ui.theme_delete = Some("theme-0".into());
                app.begin_settings(window, cx);
                assert!(
                    app.preferences_ui.theme_delete.is_none(),
                    "reloaded settings retained a stale delete target"
                );
            })
        });
    }

    struct HtmlPreferenceFixture {
        app: Entity<StudyApp>,
    }
    impl Render for HtmlPreferenceFixture {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.app.update(cx, |app, cx| {
                app.appearance_part(0, window.viewport_size().width, cx)
            })
        }
    }
    #[gpui_kit::test]
    fn ordinary_appearance_cards_wrap_at_the_content_width_without_losing_raw_drafts(
        cx: &mut TestAppContext,
    ) {
        let (app, visual) = fixture(cx);
        visual.simulate_resize(gpui_kit::size(gpui_kit::px(1100.), gpui_kit::px(1800.)));
        visual.replace_root_view(|_, _| HtmlPreferenceFixture { app: app.clone() });
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
            let general = window.find("appearance-general-card").bounds();
            let table = window.find("appearance-table-card").bounds();
            let list = window.find("appearance-list-card").bounds();
            assert_eq!(general.origin.y, table.origin.y);
            assert!(table.origin.x > general.origin.x);
            assert!(list.size.width > general.size.width);
            let input = app.read(cx).preferences_ui.number_fields.borrow()["appearance-font-size"]
                .input
                .clone();
            input.update(cx, |input, cx| input.set_value("-", window, cx));
        });
        visual.simulate_resize(gpui_kit::size(gpui_kit::px(700.), gpui_kit::px(1800.)));
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
            let general = window.find("appearance-general-card").bounds();
            let table = window.find("appearance-table-card").bounds();
            assert!(table.origin.y > general.origin.y);
            assert_eq!(general.origin.x, table.origin.x);
            assert_eq!(
                app.read(cx).preferences_ui.number_fields.borrow()["appearance-font-size"]
                    .input
                    .read(cx)
                    .value(),
                "-"
            );
        });
    }

    struct ImageSearchFixture {
        app: Entity<StudyApp>,
    }
    impl Render for ImageSearchFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.app.update(cx, |app, cx| {
                if app.preferences_ui.settings_search_open {
                    app.settings_search_results(cx)
                } else {
                    app.image_settings_controls(cx)
                }
            })
        }
    }
    #[gpui_kit::test]
    fn searching_hidden_cloud_fields_reveals_disabled_controls_without_changing_the_draft(
        cx: &mut TestAppContext,
    ) {
        let (app, visual) = fixture(cx);
        visual.simulate_resize(gpui_kit::size(gpui_kit::px(1100.), gpui_kit::px(1800.)));
        let (images, kind, dirty) = visual.update(|window, cx| {
            app.update(cx, |app, cx| {
                let before = (
                    app.preferences_ui.draft.as_ref().unwrap().images.clone(),
                    app.preferences_ui.image_cloud.as_ref().unwrap().kind,
                    app.settings_dirty(cx),
                );
                app.preferences_ui
                    .search
                    .as_ref()
                    .unwrap()
                    .update(cx, |input, cx| input.set_value("Endpoint", window, cx));
                app.preferences_ui.settings_search_open = true;
                cx.notify();
                before
            })
        });
        visual.replace_root_view(|_, _| ImageSearchFixture { app: app.clone() });
        let index = crate::settings_search::search("Endpoint")
            .iter()
            .position(|entry| entry.label == "Endpoint")
            .unwrap();
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.click(("settings-search-result", index), cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
            assert!(window.try_find("image-cloud-field-endpoint").is_some());
            window.click("image-provider", cx);
            assert!(window.try_find("popup-menu").is_none());
            app.update(cx, |app, cx| {
                assert_eq!(app.preferences_ui.draft.as_ref().unwrap().images, images);
                assert_eq!(app.preferences_ui.image_cloud.as_ref().unwrap().kind, kind);
                assert_eq!(app.settings_dirty(cx), dirty);
                assert!(app.preferences_ui.image_cloud_search_reveal);
                app.select_settings_target("storage.images", cx);
                assert!(!app.preferences_ui.image_cloud_search_reveal);
            });
        });
    }

    #[gpui_kit::test]
    fn html_preference_is_a_live_discardable_draft_and_reset_preserves_other_pages(
        cx: &mut TestAppContext,
    ) {
        let (app, visual) = fixture(cx);
        visual.simulate_resize(gpui_kit::size(gpui_kit::px(1000.), gpui_kit::px(1000.)));
        visual.replace_root_view(|_, _| HtmlPreferenceFixture { app: app.clone() });
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.click("markdown-render-html", cx);
            app.update(cx, |app, cx| {
                assert!(
                    !app.preferences_ui
                        .draft
                        .as_ref()
                        .unwrap()
                        .appearance
                        .render_html
                );
                assert!(
                    app.preferences_ui
                        .saved
                        .as_ref()
                        .unwrap()
                        .settings
                        .appearance
                        .render_html
                );
                assert!(!app.effective_appearance().render_html);
                assert!(app.settings_dirty(cx));
                assert!(
                    !crate::theme::palette().editor_tokens()["renderHtml"]
                        .as_bool()
                        .unwrap()
                );
                assert!(
                    app.preferences_ui
                        .appearance_preview
                        .as_ref()
                        .unwrap()
                        .read(cx)
                        .editor()
                        .is_none()
                );
                app.discard_settings(window, cx);
                assert!(app.effective_appearance().render_html);
                let draft = app.preferences_ui.draft.as_mut().unwrap();
                draft.appearance.render_html = false;
                draft.notifications.do_not_disturb = true;
                draft.logging.enabled = false;
                app.reset_settings_page(SettingsResetScope::Appearance, window, cx);
                let draft = app.preferences_ui.draft.as_ref().unwrap();
                assert!(draft.appearance.render_html);
                assert!(draft.notifications.do_not_disturb);
                assert!(!draft.logging.enabled);
            });
        });
    }

    struct LoggingFixture {
        app: Entity<StudyApp>,
    }
    impl Render for LoggingFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.app.update(cx, |app, cx| {
                div().size_full().child(app.logging_controls(cx))
            })
        }
    }
    #[gpui_kit::test]
    fn logging_page_edits_only_draft_and_discard_or_reset_invalidates_late_picker(
        cx: &mut TestAppContext,
    ) {
        let (app, visual) = fixture(cx);
        visual.simulate_resize(gpui_kit::size(gpui_kit::px(1000.), gpui_kit::px(1000.)));
        visual.replace_root_view(|_, _| LoggingFixture { app: app.clone() });
        visual.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.select_settings_section(SettingsSection::Logging, cx)
            });
            window.draw(cx).clear(cx);
            studyapp_ui::settings_components::select_choice(window, "logging-level", 4, cx);
            window.click("logging-enabled", cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            app.update(cx, |app, cx| {
                let draft = app.preferences_ui.draft.as_ref().unwrap();
                assert!(!draft.logging.enabled);
                assert_eq!(draft.logging.minimum_level, studyapp_core::LogLevel::Trace);
                assert_eq!(
                    app.preferences_ui.saved.as_ref().unwrap().settings.logging,
                    Default::default()
                );
                let directory = app
                    .preferences_ui
                    .logging_directory
                    .as_ref()
                    .unwrap()
                    .clone();
                directory.update(cx, |input, cx| {
                    input.set_value("C:/draft-only-logs", window, cx)
                });
                assert!(app.settings_dirty(cx));
                let request = app.preferences_ui.logging_picker.begin(&app.last_library);
                app.discard_settings(window, cx);
                app.preferences_ui
                    .logging_picker
                    .reply_test(request, "C:/stale-reply");
                app.poll_logging_settings(window, cx);
                assert_eq!(directory.read(cx).value(), "");
                assert!(!app.settings_dirty(cx));
                let draft = app.preferences_ui.draft.as_mut().unwrap();
                draft.notifications.do_not_disturb = true;
                draft.appearance.notifications.width = 444;
                draft.logging.enabled = false;
                directory.update(cx, |input, cx| {
                    input.set_value("invalid-relative", window, cx)
                });
                app.reset_settings_page(SettingsResetScope::Logging, window, cx);
                let draft = app.preferences_ui.draft.as_ref().unwrap();
                assert_eq!(draft.logging, Default::default());
                assert!(draft.notifications.do_not_disturb);
                assert_eq!(draft.appearance.notifications.width, 444);
                assert_eq!(directory.read(cx).value(), "");
            })
        });
    }
    #[gpui_kit::test]
    fn invalid_logging_directory_preserves_draft_and_feedback_never_claims_save(
        cx: &mut TestAppContext,
    ) {
        let (app, visual) = fixture(cx);
        visual.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.preferences_ui
                    .logging_directory
                    .as_ref()
                    .unwrap()
                    .update(cx, |input, cx| {
                        input.set_value("relative-private", window, cx)
                    });
                app.save_preferences(cx);
                assert!(!app.preferences_ui.busy);
                assert!(app.preferences_ui.error.is_some());
                assert!(app.settings_dirty(cx));
                assert_eq!(
                    app.preferences_ui.saved.as_ref().unwrap().settings.logging,
                    Default::default()
                );
                assert_eq!(
                    app.preferences_ui
                        .logging_directory
                        .as_ref()
                        .unwrap()
                        .read(cx)
                        .value(),
                    "relative-private"
                );
            })
        });
    }

    struct NotificationFixture {
        app: Entity<StudyApp>,
        appearance: bool,
        color_only: bool,
    }
    impl Render for NotificationFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.app.update(cx, |app, cx| {
                let content = if self.color_only {
                    div().child(
                        app.preferences_ui.appearance_colors["notification-background"].clone(),
                    )
                } else if self.appearance {
                    app.preferences_ui.appearance_page = AppearancePage::Notifications;
                    app.appearance_surface(cx)
                } else {
                    app.notification_surface(cx)
                };
                div().flex().flex_col().size_full().child(content)
            })
        }
    }

    fn reveal_notification_rule(
        app: &Entity<StudyApp>,
        visual: &mut VisualTestContext,
        category: studyapp_core::NoticeCategory,
    ) {
        let index = studyapp_core::NoticeCategory::ALL
            .iter()
            .position(|value| *value == category)
            .unwrap();
        let target = index / 5;
        loop {
            visual.run_until_parked();
            let page = app.read_with(visual, |app, _| app.preferences_ui.notification_page);
            if page == target {
                break;
            }
            visual.update(|window, cx| {
                window.render_frame(cx);
                window.click(
                    if page < target {
                        "notice-page-next"
                    } else {
                        "notice-page-prev"
                    },
                    cx,
                );
            });
        }
        visual.run_until_parked();
        app.update(visual, |app, cx| {
            app.preferences_ui
                .notification_rows
                .scroll
                .scroll_to(gpui_kit::ListOffset {
                    item_ix: index % 5 + 2,
                    offset_in_item: gpui_kit::px(0.),
                });
            app.preferences_ui
                .notification_view
                .as_ref()
                .unwrap()
                .update(cx, |_, cx| cx.notify());
        });
        visual.run_until_parked();
        visual.update(|window, cx| window.render_frame(cx));
    }

    #[gpui_kit::test]
    fn notification_surfaces_reuse_unchanged_frames_and_rebuild_only_visible_rows(
        cx: &mut TestAppContext,
    ) {
        let (app, visual) = fixture(cx);
        visual.simulate_resize(gpui_kit::size(gpui_kit::px(1000.), gpui_kit::px(650.)));
        for appearance in [false, true] {
            let surface = visual.replace_root_view(|_, _| NotificationFixture {
                app: app.clone(),
                appearance,
                color_only: false,
            });
            for _ in 0..4 {
                visual.run_until_parked();
                visual.update(|window, cx| window.draw(cx).clear(cx));
            }
            let start = app.read_with(visual, |app, _| app.notification_render_work());
            for _ in 0..20 {
                surface.update(visual, |_, cx| cx.notify());
                visual.update(|window, cx| window.draw(cx).clear(cx));
            }
            assert_eq!(
                app.read_with(visual, |app, _| app.notification_render_work()),
                start,
                "parent-only frames rebuilt notification form; appearance={appearance}"
            );
            // A relevant edit must invalidate the cached leaf, while policy
            // edits must not instantiate all twenty category rows.
            app.update(visual, |app, cx| {
                let draft = app.preferences_ui.draft.as_mut().unwrap();
                if appearance {
                    draft.appearance.notifications.width = 420;
                } else {
                    draft.notifications.do_not_disturb = true;
                }
                cx.notify();
            });
            surface.update(visual, |_, cx| cx.notify());
            let mut edit_frames = Vec::new();
            for _ in 0..3 {
                visual.update(|window, cx| window.draw(cx).clear(cx));
                edit_frames
                    .push(app.read_with(visual, |app, _| app.notification_render_work()) - start);
            }
            let changed = app.read_with(visual, |app, _| app.notification_render_work()) - start;
            assert!(
                changed > 0,
                "relevant notification edit was cached away: {edit_frames:?}"
            );
            if !appearance {
                assert!(
                    changed < 20,
                    "all category controls were rebuilt: {changed}"
                );
            }
            if !appearance {
                reveal_notification_rule(
                    &app,
                    visual,
                    studyapp_core::NoticeCategory::StartupRecovery,
                );
                surface.update(visual, |_, cx| cx.notify());
                visual.update(|window, cx| {
                    window.render_frame(cx);
                    window.click("notification-rule-StartupRecovery-background", cx);
                });
                assert!(!app.read_with(visual, |app, _| {
                    app.preferences_ui
                        .draft
                        .as_ref()
                        .unwrap()
                        .notifications
                        .rule(studyapp_core::NoticeCategory::StartupRecovery)
                        .background
                }));
            }
        }
    }

    #[gpui_kit::test]
    fn notification_cache_edit_flows_through_the_actual_app_root_without_native_editors(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::apply(&Default::default(), false, cx);
        });
        let root = tempfile::tempdir().unwrap().keep();
        let worker = crate::worker::Worker::start(root.join("studyapp.sqlite3"), None);
        let mut app = None;
        let (_, visual) = cx.add_window_view(|window, cx| {
            let owner = cx.new(|cx| StudyApp::new(worker, window, cx));
            owner.update(cx, |app, cx| {
                let settings = studyapp_core::Settings::default();
                app.snapshot = Some(
                    studyapp_core::Snapshot {
                        saved_bodies: Default::default(),
                        snapshot_changes: Default::default(),
                        pending_snapshot_count: 0,
                        settings,
                        table_views: Default::default(),
                        notes: Vec::new(),
                        folders: Vec::new(),
                        versions: Default::default(),
                        sessions: Vec::new(),
                        reviews: Vec::new(),
                        review_events: Vec::new(),
                        timer: studyapp_core::TimerState {
                            kind: Default::default(),
                            label: String::new(),
                            finished_at: None,
                            paused: true,
                            active_seconds: 0,
                            target_seconds: 1500,
                            started_at: 0,
                            spans: Vec::new(),
                        },
                        data_path: String::new(),
                    }
                    .into(),
                );
                app.page = Page::Settings;
                app.preferences_ui.saved = Some(PreferenceSnapshot::defaults());
                app.begin_settings(window, cx);
                app.select_settings_section(SettingsSection::Notifications, cx);
                app.preferences_ui.section_changed_at = None;
            });
            app = Some(owner.clone());
            gpui_kit::base::Root::new(owner, window, cx)
        });
        let app = app.unwrap();
        visual.simulate_resize(gpui_kit::size(gpui_kit::px(1000.), gpui_kit::px(750.)));
        for appearance in [false, true] {
            app.update(visual, |app, cx| {
                app.select_settings_section(
                    if appearance {
                        SettingsSection::Appearance
                    } else {
                        SettingsSection::Notifications
                    },
                    cx,
                );
                if appearance {
                    app.select_appearance_page(AppearancePage::Notifications, cx);
                }
                app.preferences_ui.section_changed_at = None;
            });
            for _ in 0..4 {
                visual.update(|window, cx| window.draw(cx).clear(cx));
            }
            visual.update(|window, cx| {
                assert!(
                    window
                        .try_find(if appearance {
                            "notification-width-increase"
                        } else {
                            "notification-update-badge"
                        })
                        .is_some()
                );
                assert!(if appearance {
                    app.read(cx).notification_appearance_cache_matches(cx)
                } else {
                    app.read(cx).notification_cache_matches(cx)
                });
            });
            let before = app.read_with(visual, |app, _| app.notification_render_work());
            assert!(before > 0);
            for _ in 0..20 {
                app.update(visual, |_, cx| cx.notify());
                visual.update(|window, cx| window.draw(cx).clear(cx));
            }
            assert_eq!(
                app.read_with(visual, |app, _| app.notification_render_work()),
                before
            );
            app.update(visual, |app, cx| {
                let draft = app.preferences_ui.draft.as_mut().unwrap();
                if appearance {
                    draft.appearance.notifications.width = 420;
                } else {
                    draft.notifications.do_not_disturb = true;
                }
                cx.notify();
            });
            let mut frames = Vec::new();
            for _ in 0..3 {
                visual.update(|window, cx| window.draw(cx).clear(cx));
                frames
                    .push(app.read_with(visual, |app, _| app.notification_render_work()) - before);
            }
            assert!(
                frames[2] > 0 && frames[2] < 22,
                "actual root appearance={appearance}: {frames:?}"
            );
            visual.update(|_, cx| {
                assert!(if appearance {
                    app.read(cx).notification_appearance_cache_matches(cx)
                } else {
                    app.read(cx).notification_cache_matches(cx)
                });
                assert!(!app.read(cx).note_content.read(cx).ready());
                assert!(
                    app.read(cx)
                        .preferences_ui
                        .appearance_preview
                        .as_ref()
                        .unwrap()
                        .read(cx)
                        .editor()
                        .is_none()
                );
            });
        }
    }

    #[gpui_kit::test]
    fn notification_switches_edit_only_their_draft_and_discard_restores_them(
        cx: &mut TestAppContext,
    ) {
        let (app, visual) = fixture(cx);
        visual.simulate_resize(gpui_kit::size(gpui_kit::px(1000.), gpui_kit::px(1200.)));
        visual.replace_root_view(|_, _| NotificationFixture {
            app: app.clone(),
            appearance: false,
            color_only: false,
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.click("notification-update-badge", cx);
            window.click("notification-dnd", cx);
        });
        reveal_notification_rule(&app, visual, studyapp_core::NoticeCategory::UpdateAvailable);
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.click("notification-rule-UpdateAvailable-foreground", cx);
            app.update(cx, |app, cx| {
                let draft = app.preferences_ui.draft.as_ref().unwrap();
                assert!(!draft.notifications.update_badge);
                assert!(draft.notifications.do_not_disturb);
                assert!(
                    !draft
                        .notifications
                        .rule(studyapp_core::NoticeCategory::UpdateAvailable)
                        .foreground
                );
                assert!(
                    draft
                        .notifications
                        .rule(studyapp_core::NoticeCategory::TimerComplete)
                        .foreground
                );
                assert_eq!(
                    app.preferences_ui
                        .saved
                        .as_ref()
                        .unwrap()
                        .settings
                        .notifications,
                    studyapp_core::NotificationSettings::default()
                );
                assert!(app.settings_dirty(cx));
                assert_eq!(app.notifications.read(cx).visible_count(), 0);
                app.discard_settings(window, cx);
                assert_eq!(
                    app.preferences_ui.draft.as_ref().unwrap().notifications,
                    studyapp_core::NotificationSettings::default()
                );
                assert!(!app.settings_dirty(cx));
            });
        });
    }

    #[gpui_kit::test]
    fn notification_resets_are_independent_of_styles_theme_learning_and_widget(
        cx: &mut TestAppContext,
    ) {
        let (app, visual) = fixture(cx);
        visual.update(|window, cx| {
            app.update(cx, |app, cx| {
                let draft = app.preferences_ui.draft.as_mut().unwrap();
                draft.notifications.do_not_disturb = true;
                draft.appearance.notifications.width = 420;
                draft.timer_widget.opacity = 55;
                app.interval.update(cx, |input, cx| {
                    input.set_value("invalid learning", window, cx)
                });
                app.reset_settings_page(SettingsResetScope::Notifications, window, cx);
                let draft = app.preferences_ui.draft.as_mut().unwrap();
                assert_eq!(
                    draft.notifications,
                    studyapp_core::NotificationSettings::default()
                );
                assert_eq!(draft.appearance.notifications.width, 420);
                draft.notifications.do_not_disturb = true;
                app.reset_settings_page(SettingsResetScope::NotificationAppearance, window, cx);
                let draft = app.preferences_ui.draft.as_ref().unwrap();
                assert!(draft.notifications.do_not_disturb);
                assert_eq!(
                    draft.appearance.notifications,
                    studyapp_core::NotificationAppearance::default()
                );
                assert_eq!(draft.timer_widget.opacity, 55);
                assert_eq!(app.interval.read(cx).value(), "invalid learning");
            })
        });
    }

    #[gpui_kit::test]
    fn nonpalette_popup_appearance_previews_and_discards_without_replacing_editor_state(
        cx: &mut TestAppContext,
    ) {
        use studyapp_core::{NoticeDensity, NoticeLayout, NoticeStyle};
        let (app, visual) = fixture(cx);
        visual.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.refresh_appearance(window, cx);
                let original = app
                    .preferences_ui
                    .draft
                    .as_ref()
                    .unwrap()
                    .appearance
                    .clone();
                let palette = crate::theme::palette();
                let generation = app.note_content.read(cx).generation();
                let value = app.note_content.read(cx).value().to_owned();
                let notices = &mut app
                    .preferences_ui
                    .draft
                    .as_mut()
                    .unwrap()
                    .appearance
                    .notifications;
                notices.surface_patch_mut(NoticeStyle::Anchored).max_height = Some(480);
                notices.surface_patch_mut(NoticeStyle::PopupMenu).density =
                    Some(NoticeDensity::Compact);
                notices
                    .layouts
                    .insert(NoticeStyle::PopupMenu, NoticeLayout::Alternative);
                let expected = notices.clone();
                assert_eq!(
                    crate::theme::resolve(&app.effective_appearance(), palette.dark),
                    palette,
                    "fixture must change component appearance without changing palette tokens",
                );
                app.refresh_appearance(window, cx);
                assert_eq!(crate::theme::notification_appearance(), expected);
                assert_eq!(crate::theme::palette(), palette);
                assert_eq!(app.note_content.read(cx).generation(), generation);
                assert_eq!(app.note_content.read(cx).value(), value);
                assert!(!app.note_content.read(cx).creating());
                assert_eq!(
                    app.preferences_ui
                        .saved
                        .as_ref()
                        .unwrap()
                        .settings
                        .appearance,
                    original
                );
                app.discard_settings(window, cx);
                assert_eq!(
                    crate::theme::notification_appearance(),
                    original.notifications
                );
                assert_eq!(crate::theme::palette(), palette);
                assert_eq!(app.note_content.read(cx).generation(), generation);
                assert_eq!(app.note_content.read(cx).value(), value);
            });
        });
    }

    #[gpui_kit::test]
    fn notification_appearance_preview_changes_never_create_an_event_or_mount_a_native_editor(
        cx: &mut TestAppContext,
    ) {
        let (app, visual) = fixture(cx);
        visual.simulate_resize(gpui_kit::size(gpui_kit::px(1000.), gpui_kit::px(1200.)));
        visual.replace_root_view(|_, _| NotificationFixture {
            app: app.clone(),
            appearance: true,
            color_only: false,
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
            studyapp_ui::settings_components::select_choice(window, "notification-position", 0, cx);
        });
        // Close the menu and deliver the cached form's layout notification
        // before clicking a control whose compact row may have moved.
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.click("notification-width-increase", cx);
            assert_eq!(
                app.read(cx).preferences_ui.number_fields.borrow()["notification-width"]
                    .input
                    .read(cx)
                    .value(),
                "361"
            );
        });
        // NumericSetting emits InputEvent::Change. Flush semantic subscribers
        // before checking the application draft, not a layout timer or delay.
        visual.run_until_parked();
        visual.update(|_, cx| {
            app.update(cx, |app, cx| {
                let settings = app.preferences_ui.draft.as_ref().unwrap();
                assert_eq!(
                    settings.appearance.notifications.style,
                    studyapp_core::NotificationAppearance::default().style
                );
                assert_eq!(
                    settings.appearance.notifications.position,
                    studyapp_core::NoticePosition::TopLeft
                );
                assert_eq!(settings.appearance.notifications.width, 361);
                settings.validate().unwrap();
                assert!(!app.preferences_ui.appearance_touched);
                assert_eq!(app.notifications.read(cx).visible_count(), 0);
                assert!(
                    app.preferences_ui
                        .appearance_preview
                        .as_ref()
                        .unwrap()
                        .read(cx)
                        .editor()
                        .is_none()
                );
                assert!(app.settings_dirty(cx));
            });
        });
    }

    #[gpui_kit::test]
    fn notification_color_is_staged_applied_and_discardable_without_dirtying_theme(
        cx: &mut TestAppContext,
    ) {
        let (app, visual) = fixture(cx);
        visual.replace_root_view(|_, _| NotificationFixture {
            app: app.clone(),
            appearance: false,
            color_only: true,
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.click("color-open-notification-background", cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.click("color-swatch-notification-background-3", cx);
        });
        visual.run_until_parked();
        app.read_with(visual, |app, cx| {
            assert_eq!(
                app.preferences_ui.appearance_colors["notification-background"]
                    .read(cx)
                    .preview_value(cx),
                "#EF4444"
            );
            assert!(
                app.preferences_ui
                    .draft
                    .as_ref()
                    .unwrap()
                    .appearance
                    .notifications
                    .background
                    .is_none()
            );
            assert!(!app.settings_dirty(cx));
        });
        visual.update(|window, cx| window.click("color-close-notification-background", cx));
        visual.run_until_parked();
        visual.update(|window, cx| {
            app.update(cx, |app, cx| {
                assert_eq!(
                    app.preferences_ui
                        .draft
                        .as_ref()
                        .unwrap()
                        .appearance
                        .notifications
                        .background
                        .as_deref(),
                    Some("#EF4444")
                );
                assert!(!app.preferences_ui.appearance_touched);
                assert!(app.settings_dirty(cx));
                app.discard_settings(window, cx);
                assert!(
                    app.preferences_ui
                        .draft
                        .as_ref()
                        .unwrap()
                        .appearance
                        .notifications
                        .background
                        .is_none()
                );
                assert_eq!(
                    app.preferences_ui.notification_fields["notification-background"]
                        .read(cx)
                        .value(),
                    ""
                );
                assert!(!app.settings_dirty(cx));
            })
        });
    }

    #[gpui_kit::test]
    fn desktop_delivery_reuses_the_queue_and_foreground_return_closes_only_the_passive_window(
        cx: &mut TestAppContext,
    ) {
        let (app, visual) = fixture(cx);
        let active_before = visual.update(|window, _| window.is_window_active());
        let event = studyapp_core::NoticeEvent::new(
            studyapp_core::NoticeCategory::IntegrityResult,
            studyapp_core::NoticeSeverity::Error,
            "仅在应用内展示的检查详情",
        );
        let event_id = event.id;
        let appearance = studyapp_core::NotificationAppearance {
            reduce_motion: true,
            ..Default::default()
        };
        app.update(visual, |app, cx| {
            app.notifications.update(cx, |center, cx| {
                center.configure(
                    &Default::default(),
                    &appearance,
                    studyapp_core::NoticeEnvironment {
                        foreground: false,
                        desktop_available: true,
                        anchor_available: false,
                        minute: 720,
                    },
                    false,
                    cx,
                )
            });
            assert_eq!(app.emit_notification(event, cx), Some(event_id));
        });
        visual.run_until_parked();
        let queue_id = app.read_with(visual, |app, cx| {
            assert!(
                app.notification_window.is_some(),
                "passive desktop window did not open in the virtual platform"
            );
            let center = app.notifications.read(cx);
            assert_eq!(center.visible_count(), 0);
            let items = center.desktop_items();
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].0.event.as_ref().unwrap().id, event_id);
            items[0].0.id
        });
        assert_eq!(
            visual.update(|window, _| window.is_window_active()),
            active_before
        );
        app.update(visual, |app, cx| {
            app.notifications.update(cx, |center, cx| {
                center.configure(
                    &Default::default(),
                    &appearance,
                    studyapp_core::NoticeEnvironment {
                        foreground: true,
                        desktop_available: true,
                        anchor_available: false,
                        minute: 720,
                    },
                    false,
                    cx,
                )
            })
        });
        visual.run_until_parked();
        app.read_with(visual, |app, cx| {
            assert!(app.notification_window.is_none());
            assert!(!app.notifications.read(cx).desktop_visible());
            assert_eq!(app.notifications.read(cx).test_state()[0].0, queue_id);
        });
    }

    #[gpui_kit::test]
    fn passive_desktop_window_does_not_retain_the_main_app_or_its_library_worker(
        cx: &mut TestAppContext,
    ) {
        let (app, visual) = fixture(cx);
        app.update(visual, |app, cx| {
            app.notifications.update(cx, |center, cx| {
                center.configure(
                    &Default::default(),
                    &studyapp_core::NotificationAppearance {
                        reduce_motion: true,
                        ..Default::default()
                    },
                    studyapp_core::NoticeEnvironment {
                        foreground: false,
                        desktop_available: true,
                        anchor_available: false,
                        minute: 720,
                    },
                    false,
                    cx,
                )
            });
            app.emit_notification(
                studyapp_core::NoticeEvent::new(
                    studyapp_core::NoticeCategory::IntegrityResult,
                    studyapp_core::NoticeSeverity::Error,
                    "检查未完成",
                ),
                cx,
            );
        });
        visual.run_until_parked();
        assert!(app.read_with(visual, |app, _| app.notification_window.is_some()));
        let weak = app.downgrade();
        drop(app);
        struct Empty;
        impl Render for Empty {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div()
            }
        }
        visual.replace_root_view(|_, _| Empty);
        visual.run_until_parked();
        assert!(
            weak.upgrade().is_none(),
            "desktop notification retained the app and its library lock"
        );
    }

    struct TimerModalFixture {
        app: Entity<StudyApp>,
    }
    impl Render for TimerModalFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.app.update(cx, |app, cx| {
                div().size_full().children(app.timer_modal(cx))
            })
        }
    }

    #[gpui_kit::test]
    fn changing_a_progressed_timer_waits_for_explicit_confirmation_and_cancel_keeps_progress(
        cx: &mut TestAppContext,
    ) {
        let (app, visual) = fixture(cx);
        let root = tempfile::tempdir().unwrap().keep();
        let engine = studyapp_core::Engine::open(&root.join("timer.sqlite"), 1000).unwrap();
        app.update(visual, |app, cx| {
            let mut snapshot = engine.snapshot();
            snapshot.timer.active_seconds = 90;
            app.snapshot = Some(snapshot.into());
            app.request_timer_change(
                studyapp_core::Command::StartTimer {
                    kind: studyapp_core::TimerKind::CountDown,
                    target_seconds: Some(60),
                    label: String::new(),
                },
                cx,
            );
            assert!(app.timer_pending.is_some());
            assert_eq!(
                app.snapshot.as_ref().unwrap().timer.kind,
                studyapp_core::TimerKind::Study
            );
        });
        visual.replace_root_view(|_, _| TimerModalFixture { app: app.clone() });
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("timer-cancel-change", cx);
            assert!(app.read(cx).timer_pending.is_none());
            assert_eq!(
                app.read(cx).snapshot.as_ref().unwrap().timer.active_seconds,
                90
            );
            assert_eq!(
                app.read(cx).snapshot.as_ref().unwrap().timer.kind,
                studyapp_core::TimerKind::Study
            );
        });
    }

    #[gpui_kit::test]
    fn widget_opens_and_renders_without_reentering_the_owner_update(cx: &mut TestAppContext) {
        let (app, visual) = fixture(cx);
        visual.update(|_, cx| app.update(cx, |app, cx| app.open_widget(cx)));
        let widget = app.read_with(visual, |app, _| {
            app.timer_widget.expect("virtual widget opens")
        });
        visual.run_until_parked();
        visual.update(|_, cx| {
            widget
                .update(cx, |_, window, cx| {
                    window.render_frame(cx);
                    assert!(window.try_find("timer-widget-card").is_some());
                    assert!(window.try_find("widget-pause").is_some());
                    let root = window.root::<gpui_kit::base::Root>().unwrap().unwrap();
                    root.update(cx, |root, _| {
                        assert_eq!(root.style().background, Some(gpui_kit::rgba(0).into()));
                    });
                })
                .unwrap();
            app.update(cx, |app, cx| {
                let settings = &mut app.preferences_ui.draft.as_mut().unwrap().timer_widget;
                settings.opacity = 55;
                settings.show_controls = false;
                cx.notify();
            });
        });
        visual.run_until_parked();
        visual.update(|_, cx| {
            widget
                .update(cx, |_, window, cx| {
                    window.render_frame(cx);
                    assert!(window.try_find("timer-widget-card").is_some());
                    assert!(window.try_find("widget-pause").is_none());
                })
                .unwrap();
            app.update(cx, |app, cx| app.close_widget(false, cx));
            assert!(app.read(cx).timer_widget.is_none());
        });
    }

    #[gpui_kit::test]
    fn widget_pin_icon_updates_only_the_preview_draft_and_discard_restores_it(
        cx: &mut TestAppContext,
    ) {
        let (app, visual) = fixture(cx);
        visual.update(|_, cx| app.update(cx, |app, cx| app.open_widget(cx)));
        let widget = app.read_with(visual, |app, _| app.timer_widget.unwrap());
        visual.run_until_parked();
        visual.update(|_, cx| {
            widget
                .update(cx, |_, window, cx| {
                    window.render_frame(cx);
                    assert_eq!(window.find("widget-pin").label(), Some("取消置顶"));
                    window.click("widget-pin", cx);
                })
                .unwrap()
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            app.update(cx, |app, cx| {
                assert!(!app.effective_widget_settings().pinned);
                assert!(
                    app.preferences_ui
                        .saved
                        .as_ref()
                        .unwrap()
                        .settings
                        .timer_widget
                        .pinned
                );
                assert!(app.settings_dirty(cx));
                app.discard_settings(window, cx);
                assert!(app.effective_widget_settings().pinned);
                assert!(!app.settings_dirty(cx));
            });
        });
        visual.run_until_parked();
        visual.update(|_, cx| {
            widget
                .update(cx, |_, window, cx| {
                    window.render_frame(cx);
                    assert_eq!(window.find("widget-pin").label(), Some("取消置顶"));
                })
                .unwrap();
            app.update(cx, |app, cx| app.close_widget(false, cx));
        });
    }

    #[gpui_kit::test]
    fn widget_settings_scroll_keeps_every_group_and_preview_reachable(cx: &mut TestAppContext) {
        let (app, visual) = fixture(cx);
        visual.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.select_appearance_page(AppearancePage::Widget, cx);
            })
        });
        let surface = visual.replace_root_view(|_, _| AppearanceFixture {
            app: app.clone(),
            width: 360.,
            height: 240.,
        });
        for (width, height, font) in [(360., 240., 14), (720., 320., 22)] {
            visual.simulate_resize(gpui_kit::size(gpui_kit::px(width), gpui_kit::px(height)));
            surface.update(visual, |surface, cx| {
                surface.width = width;
                surface.height = height;
                cx.notify();
            });
            visual.run_until_parked();
            visual.update(|window, cx| {
                crate::theme::apply(
                    &studyapp_core::AppearanceSettings {
                        font_size: font,
                        ..Default::default()
                    },
                    false,
                    cx,
                );
                window.render_frame(cx);
                let viewport = window.find("timer-widget-settings-scroll").bounds();
                assert!(
                    viewport.size.height > gpui_kit::px(0.)
                        && viewport.bottom() <= gpui_kit::px(height),
                    "viewport={viewport:?}, height={height}"
                );
                let scroll = app.read(cx).settings_scroll.clone();
                assert!(
                    scroll.max_offset().y > gpui_kit::px(100.),
                    "all settings must have a real bounded scroll range: {:?}",
                    scroll.max_offset()
                );
                scroll.set_offset(gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(0.)));
                window.render_frame(cx);
                window.scroll(
                    "timer-widget-settings-scroll",
                    gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
                        gpui_kit::px(0.),
                        gpui_kit::px(-80.),
                    )),
                    cx,
                );
                assert!(
                    scroll.offset().y < gpui_kit::px(0.),
                    "a real wheel event must move the widget settings"
                );
                scroll.set_offset(gpui_kit::point(gpui_kit::px(0.), -scroll.max_offset().y));
                window.render_frame(cx);
                let last = window.find("widget-settings-end").bounds();
                assert!(last.bottom() <= viewport.bottom() && last.bottom() > viewport.top());
            });
        }
    }

    #[gpui_kit::test]
    fn widget_draft_preview_and_invalid_inputs_are_discardable(cx: &mut TestAppContext) {
        let (app, visual) = fixture(cx);
        visual.update(|window, cx| {
            app.update(cx, |app, cx| {
                let baseline = app.effective_widget_settings();
                app.preferences_ui
                    .draft
                    .as_mut()
                    .unwrap()
                    .timer_widget
                    .opacity = 55;
                assert_eq!(app.effective_widget_settings().opacity, 55);
                assert!(app.settings_dirty(cx));
                app.preferences_ui.widget_fields["widget-presets"]
                    .update(cx, |input, cx| input.set_value("0, 181", window, cx));
                assert!(
                    app.widget_settings_value(app.effective_widget_settings(), cx)
                        .is_err()
                );
                assert!(app.widget_fields_dirty(&baseline, cx));
                app.discard_settings(window, cx);
                assert_eq!(app.effective_widget_settings(), baseline);
                assert!(!app.settings_dirty(cx));
                assert!(
                    app.preferences_ui
                        .appearance_preview
                        .as_ref()
                        .unwrap()
                        .read(cx)
                        .editor()
                        .is_none()
                );
            })
        });
    }

    #[gpui_kit::test]
    fn widget_custom_zoom_input_previews_rejects_invalid_values_and_discards(
        cx: &mut TestAppContext,
    ) {
        let (app, visual) = fixture(cx);
        for scale in [50, 73, 173, 200, 100] {
            visual.update(|window, cx| {
                let input = app.read(cx).preferences_ui.widget_fields["widget-scale"].clone();
                input.update(cx, |input, cx| {
                    input.set_value(scale.to_string(), window, cx);
                    cx.emit(gpui_kit::component::input::InputEvent::Change);
                });
            });
            visual.run_until_parked();
            app.read_with(visual, |app, cx| {
                assert_eq!(app.preview_widget_settings(cx).scale, scale);
                assert_eq!(
                    app.widget_settings_value(app.effective_widget_settings(), cx)
                        .unwrap()
                        .scale,
                    scale
                );
            });
        }
        for text in ["49", "201", "73.5", "", "abc"] {
            visual.update(|window, cx| {
                let input = app.read(cx).preferences_ui.widget_fields["widget-scale"].clone();
                input.update(cx, |input, cx| {
                    input.set_value(text, window, cx);
                    cx.emit(gpui_kit::component::input::InputEvent::Change);
                });
            });
            visual.run_until_parked();
            app.read_with(visual, |app, cx| {
                assert_eq!(app.preview_widget_settings(cx).scale, 100);
                assert!(
                    app.widget_settings_value(app.effective_widget_settings(), cx)
                        .is_err()
                );
                assert!(app.settings_dirty(cx));
            });
        }
        visual.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.discard_settings(window, cx);
                assert_eq!(
                    app.preferences_ui.widget_fields["widget-scale"]
                        .read(cx)
                        .value(),
                    "100"
                );
                assert_eq!(app.effective_widget_settings().scale, 100);
                assert!(!app.settings_dirty(cx));
            })
        });
    }

    #[gpui_kit::test]
    fn widget_reset_only_resets_its_own_discardable_category(cx: &mut TestAppContext) {
        let (app, visual) = fixture(cx);
        visual.update(|window, cx| {
            app.update(cx, |app, cx| {
                let draft = app.preferences_ui.draft.as_mut().unwrap();
                draft.timer_widget.opacity = 55;
                draft.show_file_paths = false;
                app.interval.update(cx, |input, cx| {
                    input.set_value("invalid learning", window, cx)
                });
                app.reset_settings_page(SettingsResetScope::Widget, window, cx);
                assert_eq!(
                    app.effective_widget_settings(),
                    studyapp_core::TimerWidgetSettings::default()
                );
                assert!(!app.preferences_ui.draft.as_ref().unwrap().show_file_paths);
                assert_eq!(app.interval.read(cx).value(), "invalid learning");
            })
        });
    }

    #[gpui_kit::test]
    fn resetting_shortcuts_preserves_invalid_learning_theme_and_sync_secret(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = fixture(cx);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.preferences_ui.section = SettingsSection::Shortcuts;
                app.interval.update(cx, |input, cx| {
                    input.set_value("invalid learning", window, cx)
                });
                app.preferences_ui.appearance_fields["name"].update(cx, |input, cx| {
                    input.set_value("unfinished theme", window, cx)
                });
                app.preferences_ui.appearance_touched = true;
                let password = app.sync_panel.read(cx).password.clone();
                password.update(cx, |input, cx| {
                    input.set_value("unsaved-secret", window, cx)
                });
                app.preferences_ui.shortcuts["editor.save"]
                    .update(cx, |input, cx| input.set_value("F9", window, cx));
            });
            window.render_frame(cx);
            window.click("reset-settings", cx);
            window.render_frame(cx);
            window.click("confirm-reset-settings", cx);
            let app = app.read(cx);
            assert_eq!(app.interval.read(cx).value(), "invalid learning");
            assert_eq!(
                app.preferences_ui.appearance_fields["name"]
                    .read(cx)
                    .value(),
                "unfinished theme"
            );
            assert!(app.preferences_ui.appearance_touched);
            assert_eq!(
                app.sync_panel.read(cx).password.read(cx).value(),
                "unsaved-secret"
            );
            assert_eq!(
                app.preferences_ui.shortcuts["editor.save"].read(cx).value(),
                "Ctrl+S"
            );
            assert_eq!(
                app.preferences_ui.baseline.as_ref().unwrap().settings,
                studyapp_core::Settings::default()
            );
        });
    }

    #[gpui_kit::test]
    fn configured_save_binding_saves_settings_from_a_focused_input(cx: &mut TestAppContext) {
        let (app, cx) = fixture(cx);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.interval
                    .update(cx, |input, cx| input.set_value("31", window, cx));
                app.interval.read(cx).focus_handle(cx).focus(window, cx);
            });
            window.render_frame(cx);
            window.press("ctrl-s", cx);
            assert!(
                app.read(cx).preferences_ui.busy,
                "Ctrl+S must queue settings save from a focused input"
            );
        });
    }

    #[gpui_kit::test]
    fn learning_reset_is_a_discardable_draft_and_preserves_other_pages(cx: &mut TestAppContext) {
        let (app, cx) = fixture(cx);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.interval
                    .update(cx, |input, cx| input.set_value("invalid", window, cx));
                app.preferences_ui
                    .draft
                    .as_mut()
                    .unwrap()
                    .appearance
                    .font_size = 20;
                app.preferences_ui.appearance_fields["name"]
                    .update(cx, |input, cx| input.set_value("unfinished", window, cx));
                app.preferences_ui.appearance_touched = true;
                app.preferences_ui.shortcuts["editor.save"]
                    .update(cx, |input, cx| input.set_value("F9", window, cx));
                app.preferences_ui
                    .image_root
                    .as_ref()
                    .unwrap()
                    .update(cx, |input, cx| {
                        input.set_value("invalid image path", window, cx)
                    });
                let password = app.sync_panel.read(cx).password.clone();
                password.update(cx, |input, cx| input.set_value("secret", window, cx));
                app.reset_settings_page(SettingsResetScope::Learning, window, cx);
                assert_eq!(app.interval.read(cx).value(), "60");
                assert_eq!(
                    app.preferences_ui
                        .draft
                        .as_ref()
                        .unwrap()
                        .appearance
                        .font_size,
                    20
                );
                assert_eq!(
                    app.preferences_ui.appearance_fields["name"]
                        .read(cx)
                        .value(),
                    "unfinished"
                );
                assert_eq!(
                    app.preferences_ui
                        .image_root
                        .as_ref()
                        .unwrap()
                        .read(cx)
                        .value(),
                    "invalid image path"
                );
                assert_eq!(
                    app.preferences_ui.shortcuts["editor.save"].read(cx).value(),
                    "F9"
                );
                assert_eq!(password.read(cx).value(), "secret");
                assert!(app.settings_dirty(cx));
                app.discard_settings(window, cx);
                assert!(!app.settings_dirty(cx));
                assert_eq!(
                    app.preferences_ui.draft.as_ref().unwrap(),
                    &studyapp_core::Settings::default()
                );
            })
        });
    }

    #[gpui_kit::test]
    fn appearance_subpage_resets_preserve_presets_and_other_subpage_drafts(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = fixture(cx);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                let preset = studyapp_core::ThemePreset {
                    id: "retained".into(),
                    name: "Saved preset".into(),
                    palette: Default::default(),
                    markdown: Default::default(),
                };
                let draft = app.preferences_ui.draft.as_mut().unwrap();
                draft.appearance.custom_presets.push(preset.clone());
                draft.appearance.active_preset = Some(preset.id.clone());
                draft.appearance.mode = studyapp_core::ThemeMode::Dark;
                draft.appearance.font_size = 20;
                draft.appearance.spacing = 12;
                draft.appearance.list_style.levels[2] = Some(studyapp_core::ListMarker::Star);
                draft.show_file_paths = true;
                draft.layout.navigation_visible = false;
                app.preferences_ui.editing_preset = Some(preset.id.clone());
                app.preferences_ui.appearance_fields["accent"]
                    .update(cx, |input, cx| input.set_value("invalid-color", window, cx));
                app.preferences_ui.appearance_touched = true;
                app.interval.update(cx, |input, cx| {
                    input.set_value("invalid learning", window, cx)
                });
                app.reset_settings_page(SettingsResetScope::Appearance, window, cx);
                assert_eq!(
                    app.preferences_ui.appearance_fields["accent"]
                        .read(cx)
                        .value(),
                    "invalid-color"
                );
                assert!(app.preferences_ui.appearance_touched);
                let draft = app.preferences_ui.draft.as_ref().unwrap();
                assert_eq!(draft.appearance.font_size, 14);
                assert_eq!(
                    draft.appearance.list_style,
                    studyapp_core::ListStyle::default()
                );
                assert_eq!(draft.appearance.mode, studyapp_core::ThemeMode::System);
                assert!(draft.show_file_paths);
                assert_eq!(draft.appearance.custom_presets, vec![preset.clone()]);
                app.reset_settings_page(SettingsResetScope::Workspace, window, cx);
                assert_eq!(
                    app.preferences_ui.draft.as_ref().unwrap().layout,
                    studyapp_core::LayoutSettings::default()
                );
                assert!(!app.preferences_ui.draft.as_ref().unwrap().show_file_paths);
                assert_eq!(
                    app.preferences_ui.appearance_fields["accent"]
                        .read(cx)
                        .value(),
                    "invalid-color"
                );
                app.reset_settings_page(SettingsResetScope::ThemeEditor, window, cx);
                assert_eq!(
                    app.preferences_ui.editing_preset.as_deref(),
                    Some("retained")
                );
                assert_eq!(
                    app.preferences_ui.appearance_fields["accent"]
                        .read(cx)
                        .value(),
                    studyapp_core::ThemePalette::default().accent
                );
                assert_eq!(
                    app.preferences_ui
                        .draft
                        .as_ref()
                        .unwrap()
                        .appearance
                        .custom_presets,
                    vec![preset]
                );
                assert_eq!(app.interval.read(cx).value(), "invalid learning");
            })
        });
    }

    #[gpui_kit::test]
    fn image_and_sync_reset_touch_only_their_own_drafts(cx: &mut TestAppContext) {
        let (app, cx) = fixture(cx);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.interval.update(cx, |input, cx| {
                    input.set_value("invalid learning", window, cx)
                });
                app.preferences_ui
                    .image_root
                    .as_ref()
                    .unwrap()
                    .update(cx, |input, cx| input.set_value("custom-images", window, cx));
                let password = app.sync_panel.read(cx).password.clone();
                password.update(cx, |input, cx| {
                    input.set_value("unsaved-secret", window, cx)
                });
                app.reset_settings_page(SettingsResetScope::Images, window, cx);
                assert_eq!(
                    app.preferences_ui
                        .image_root
                        .as_ref()
                        .unwrap()
                        .read(cx)
                        .value(),
                    ""
                );
                assert_eq!(password.read(cx).value(), "unsaved-secret");
                let image = app.preferences_ui.image_root.as_ref().unwrap();
                image.update(cx, |input, cx| {
                    input.set_value("retained-image-draft", window, cx)
                });
                app.reset_settings_page(SettingsResetScope::Sync, window, cx);
                assert_eq!(password.read(cx).value(), "");
                assert_eq!(
                    app.sync_panel.read(cx).draft_settings(cx).unwrap().0,
                    crate::sync_runtime::SyncSettings::default()
                );
                assert_eq!(
                    app.preferences_ui
                        .image_root
                        .as_ref()
                        .unwrap()
                        .read(cx)
                        .value(),
                    "retained-image-draft"
                );
                assert_eq!(app.interval.read(cx).value(), "invalid learning");
            })
        });
    }

    #[gpui_kit::test]
    fn read_only_pages_hide_reset_and_editable_pages_name_the_scope(cx: &mut TestAppContext) {
        let (app, cx) = fixture(cx);
        cx.update(|window, cx| {
            for section in [SettingsSection::AboutAccount, SettingsSection::Storage] {
                for storage in [
                    StoragePage::Overview,
                    StoragePage::Copies,
                    StoragePage::Library,
                ] {
                    app.update(cx, |app, _| {
                        app.preferences_ui.section = section;
                        app.preferences_ui.storage_page = storage;
                    });
                    window.render_frame(cx);
                    assert!(window.try_find("reset-settings").is_none());
                }
            }
            app.update(cx, |app, _| {
                app.preferences_ui.section = SettingsSection::Appearance;
                app.preferences_ui.appearance_page = AppearancePage::Workspace;
                assert_eq!(app.settings_reset_scope().unwrap().name(), "外观 · 工作区");
            });
            window.render_frame(cx);
            assert!(window.try_find("reset-settings").is_some());
        });
    }

    #[gpui_kit::test]
    fn save_remap_recording_and_unchanged_shortcuts_respect_scope(cx: &mut TestAppContext) {
        let (app, cx) = fixture(cx);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                let save = Keystroke::parse("ctrl-s").unwrap();
                assert!(app.handle_shortcut(&save, window, cx));
                assert!(!app.preferences_ui.busy);
                assert!(app.notice.is_none(), "unchanged save must stay quiet");
                app.start_shortcut_recording("app.new", false, window, cx);
                assert!(app.handle_shortcut(&save, window, cx));
                assert!(!app.preferences_ui.busy);
                assert_eq!(
                    app.preferences_ui.shortcuts["app.new"].read(cx).value(),
                    "Ctrl+S"
                );
                app.begin_settings(window, cx);
                app.preferences_ui
                    .saved
                    .as_mut()
                    .unwrap()
                    .shortcuts
                    .insert("editor.save".into(), "F9".into());
                app.interval
                    .update(cx, |input, cx| input.set_value("31", window, cx));
                assert!(!app.handle_shortcut(&save, window, cx));
                assert!(!app.preferences_ui.busy);
                assert!(app.handle_shortcut(&Keystroke::parse("f9").unwrap(), window, cx));
                assert!(app.preferences_ui.busy);
            })
        });
    }

    #[gpui_kit::test]
    fn modal_busy_replacing_storage_and_sync_block_settings_shortcut(cx: &mut TestAppContext) {
        let (app, cx) = fixture(cx);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.interval
                    .update(cx, |input, cx| input.set_value("31", window, cx));
                let save = Keystroke::parse("ctrl-s").unwrap();
                for state in 0..5 {
                    app.preferences_ui.reset_confirmation = state == 0;
                    app.preferences_ui.busy = state == 1;
                    app.preferences_ui.replacing = state == 2;
                    app.storage_busy = state == 3;
                    app.sync_panel
                        .update(cx, |panel, _| panel.status.busy = state == 4);
                    app.handle_shortcut(&save, window, cx);
                    assert_eq!(
                        app.preferences_ui.busy,
                        state == 1,
                        "blocked state {state} queued a save"
                    );
                    assert!(app.preferences_ui.error.is_none());
                }
            })
        });
    }
    #[gpui_kit::test]
    fn dismissing_settings_validation_never_discards_the_invalid_draft_or_clears_failure(
        cx: &mut TestAppContext,
    ) {
        let (app, visual) = fixture(cx);
        visual.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.notifications
                    .update(cx, |center, _| center.freeze_time());
                app.interval.update(cx, |input, cx| {
                    input.set_value("not-an-integer", window, cx)
                });
                app.save_preferences(cx);
                assert!(!app.preferences_ui.busy);
                assert!(app.preferences_ui.error.is_some());
                assert!(app.settings_dirty(cx));
                let id = app.notifications.read(cx).test_state()[0].0;
                app.notifications.update(cx, |center, cx| {
                    center.dismiss(id, cx);
                    center.advance_time(std::time::Duration::from_millis(250), cx);
                    assert_eq!(center.visible_count(), 0);
                });
                assert!(app.preferences_ui.error.is_some());
                assert!(app.settings_dirty(cx));
                assert_eq!(app.interval.read(cx).value(), "not-an-integer");
                assert!(
                    app.preferences_ui
                        .appearance_preview
                        .as_ref()
                        .unwrap()
                        .read(cx)
                        .editor()
                        .is_none()
                );
            })
        });
    }

    struct ListPreferenceFixture {
        app: Entity<StudyApp>,
    }
    impl Render for ListPreferenceFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.app.update(cx, |app, cx| app.list_style_settings(cx))
        }
    }
    #[gpui_kit::test]
    fn list_styles_are_live_discardable_drafts_and_reset_is_scoped(cx: &mut TestAppContext) {
        let (app, visual) = fixture(cx);
        visual.simulate_resize(gpui_kit::size(gpui_kit::px(1000.), gpui_kit::px(1000.)));
        visual.replace_root_view(|_, _| ListPreferenceFixture { app: app.clone() });
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
            studyapp_ui::settings_components::select_choice(window, "list-preset", 1, cx);
            app.update(cx, |app, cx| {
                assert_eq!(
                    app.preferences_ui
                        .draft
                        .as_ref()
                        .unwrap()
                        .appearance
                        .list_style
                        .preset,
                    studyapp_core::ListPreset::Classic
                );
                assert_eq!(
                    crate::theme::palette().list_markers[1],
                    studyapp_core::ListMarker::Circle
                );
                assert_eq!(
                    app.preferences_ui
                        .saved
                        .as_ref()
                        .unwrap()
                        .settings
                        .appearance
                        .list_style,
                    studyapp_core::ListStyle::default()
                );
                assert!(app.settings_dirty(cx));
                app.discard_settings(window, cx);
                assert_eq!(
                    crate::theme::palette().list_markers[1],
                    studyapp_core::ListMarker::Square
                );
                let draft = app.preferences_ui.draft.as_mut().unwrap();
                draft.appearance.list_style.levels[2] = Some(studyapp_core::ListMarker::Plus);
                draft.logging.enabled = false;
                draft.notifications.do_not_disturb = true;
                app.reset_settings_page(SettingsResetScope::Appearance, window, cx);
                let draft = app.preferences_ui.draft.as_ref().unwrap();
                assert_eq!(
                    draft.appearance.list_style,
                    studyapp_core::ListStyle::default()
                );
                assert!(!draft.logging.enabled);
                assert!(draft.notifications.do_not_disturb);
                assert!(
                    app.preferences_ui
                        .appearance_preview
                        .as_ref()
                        .unwrap()
                        .read(cx)
                        .editor()
                        .is_none()
                );
            });
        });
    }
    #[gpui_kit::test]
    fn list_marker_inheritance_survives_pagination_and_resets_without_touching_editor_rules(
        cx: &mut TestAppContext,
    ) {
        let (app, visual) = fixture(cx);
        visual.simulate_resize(gpui_kit::size(gpui_kit::px(1000.), gpui_kit::px(1600.)));
        app.update(visual, |app, cx| {
            let draft = app.preferences_ui.draft.as_mut().unwrap();
            draft.appearance.list_style.indent_subtree = true;
            draft.notifications.do_not_disturb = true;
            cx.notify();
        });
        visual.replace_root_view(|_, _| ListPreferenceFixture { app: app.clone() });
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
            assert!(
                app.read(cx)
                    .preferences_ui
                    .draft
                    .as_ref()
                    .unwrap()
                    .appearance
                    .list_style
                    .levels
                    .iter()
                    .all(Option::is_none)
            );
            window.click(
                (
                    gpui_kit::ElementId::from(("list-level", 0usize)),
                    "customize",
                ),
                cx,
            );
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
            let style = app
                .read(cx)
                .preferences_ui
                .draft
                .as_ref()
                .unwrap()
                .appearance
                .list_style;
            assert_eq!(style.levels[0], Some(style.preset.markers()[0]));
            window.click("list-levels-next", cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.click(
                (
                    gpui_kit::ElementId::from(("list-level", 4usize)),
                    "customize",
                ),
                cx,
            );
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
            let style = app
                .read(cx)
                .preferences_ui
                .draft
                .as_ref()
                .unwrap()
                .appearance
                .list_style;
            assert_eq!(style.levels[4], Some(style.preset.markers()[4]));
            window.click("list-levels-previous", cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.click(
                (gpui_kit::ElementId::from(("list-level", 0usize)), "inherit"),
                cx,
            );
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
            let style = app
                .read(cx)
                .preferences_ui
                .draft
                .as_ref()
                .unwrap()
                .appearance
                .list_style;
            assert_eq!(style.levels[0], None);
            assert_eq!(style.levels[4], Some(style.preset.markers()[4]));
            window.click("list-preset-reset", cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
            let app = app.read(cx);
            let draft = app.preferences_ui.draft.as_ref().unwrap();
            assert!(
                draft
                    .appearance
                    .list_style
                    .levels
                    .iter()
                    .all(Option::is_none)
            );
            assert!(draft.appearance.list_style.indent_subtree);
            assert!(draft.notifications.do_not_disturb);
            assert_eq!(
                app.preferences_ui
                    .saved
                    .as_ref()
                    .unwrap()
                    .settings
                    .appearance
                    .list_style,
                studyapp_core::ListStyle::default()
            );
        });
    }

    struct TypedControlFixture {
        app: Entity<StudyApp>,
        selection: bool,
    }
    impl Render for TypedControlFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.app.update(cx, |app, cx| {
                let body = if self.selection {
                    app.selection_settings(cx)
                } else {
                    app.notice_surface_controls(cx)
                };
                div().w(gpui_kit::px(650.)).child(body)
            })
        }
    }
    #[gpui_kit::test]
    fn fixed_layout_dimensions_disable_edits_but_preserve_custom_and_invalid_drafts(
        cx: &mut TestAppContext,
    ) {
        use studyapp_core::{NoticeLayout, NoticeStyle};
        let (app, visual) = fixture(cx);
        visual.simulate_resize(gpui_kit::size(gpui_kit::px(900.), gpui_kit::px(1600.)));
        visual.update(|_, cx| {
            app.update(cx, |app, _| {
                app.preferences_ui.notice_component = Some(NoticeStyle::Snackbar);
            })
        });
        visual.replace_root_view(|_, cx| {
            cx.observe(&app, |_, _, cx| cx.notify()).detach();
            TypedControlFixture {
                app: app.clone(),
                selection: false,
            }
        });
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("surface-width-Snackbar-inherit", cx);
            window.render_frame(cx);
            window.click("surface-width-Snackbar-increase", cx);
            let input =
                app.read(cx).preferences_ui.number_fields.borrow()["surface-width-Snackbar"]
                    .input
                    .clone();
            input.update(cx, |input, cx| input.set_value("-", window, cx));
            let before = app
                .read(cx)
                .preferences_ui
                .draft
                .as_ref()
                .unwrap()
                .appearance
                .notifications
                .surface(NoticeStyle::Snackbar)
                .width;
            app.update(cx, |app, cx| {
                app.preferences_ui
                    .draft
                    .as_mut()
                    .unwrap()
                    .appearance
                    .notifications
                    .layouts
                    .insert(NoticeStyle::Snackbar, NoticeLayout::Alternative);
                cx.notify();
            });
            window.render_frame(cx);
            window.click("surface-width-Snackbar-increase", cx);
            window.click("surface-width-Snackbar-inherit", cx);
            assert_eq!(input.read(cx).value(), "-");
            assert_eq!(
                app.read(cx)
                    .preferences_ui
                    .draft
                    .as_ref()
                    .unwrap()
                    .appearance
                    .notifications
                    .surface(NoticeStyle::Snackbar)
                    .width,
                before
            );
            app.update(cx, |app, cx| {
                app.preferences_ui
                    .draft
                    .as_mut()
                    .unwrap()
                    .appearance
                    .notifications
                    .layouts
                    .insert(NoticeStyle::Snackbar, NoticeLayout::Primary);
                cx.notify();
            });
            window.render_frame(cx);
            assert_eq!(input.read(cx).value(), "-");
            app.update(cx, |app, cx| app.discard_settings(window, cx));
            assert!(!app.read(cx).settings_dirty(cx));
        });
    }
    #[gpui_kit::test]
    fn component_density_customization_inherits_and_discards_without_persisting(
        cx: &mut TestAppContext,
    ) {
        use studyapp_core::{NoticeDensity, NoticeStyle};
        let (app, visual) = fixture(cx);
        visual.simulate_resize(gpui_kit::size(gpui_kit::px(900.), gpui_kit::px(1600.)));
        visual.replace_root_view(|_, cx| {
            cx.observe(&app, |_, _, cx| cx.notify()).detach();
            TypedControlFixture {
                app: app.clone(),
                selection: false,
            }
        });
        for style in NoticeStyle::ALL {
            visual.update(|window, cx| {
                app.update(cx, |app, cx| {
                    app.preferences_ui.notice_component = Some(style);
                    cx.notify();
                });
                window.render_frame(cx);
                assert_eq!(
                    window
                        .try_find((
                            gpui_kit::ElementId::from("notice-component-density"),
                            "customize"
                        ))
                        .is_some(),
                    style.supports_density(),
                    "{style:?}"
                );
                if !style.supports_density() {
                    return;
                }
                window.click(
                    (
                        gpui_kit::ElementId::from("notice-component-density"),
                        "customize",
                    ),
                    cx,
                );
                window.render_frame(cx);
                crate::settings_components::select_choice(
                    window,
                    "notice-component-density",
                    1,
                    cx,
                );
                assert_eq!(
                    app.read(cx)
                        .preferences_ui
                        .draft
                        .as_ref()
                        .unwrap()
                        .appearance
                        .notifications
                        .surface(style)
                        .density,
                    NoticeDensity::Compact
                );
                assert_eq!(
                    app.read(cx)
                        .preferences_ui
                        .saved
                        .as_ref()
                        .unwrap()
                        .settings
                        .appearance
                        .notifications
                        .surface(style)
                        .density,
                    NoticeDensity::Comfortable
                );
                window.click(
                    (
                        gpui_kit::ElementId::from("notice-component-density"),
                        "inherit",
                    ),
                    cx,
                );
                app.update(cx, |app, cx| {
                    let appearance = &mut app
                        .preferences_ui
                        .draft
                        .as_mut()
                        .unwrap()
                        .appearance
                        .notifications;
                    assert_eq!(appearance.surface_patches[&style].density, None);
                    appearance.density = NoticeDensity::Compact;
                    assert_eq!(appearance.surface(style).density, NoticeDensity::Compact);
                    app.discard_settings(window, cx);
                });
                assert!(!app.read(cx).settings_dirty(cx));
            });
        }
    }
    #[gpui_kit::test]
    fn numeric_rows_have_natural_height_and_scoped_reset_preserves_other_invalid_drafts(
        cx: &mut TestAppContext,
    ) {
        let (app, visual) = fixture(cx);
        visual.simulate_resize(gpui_kit::size(gpui_kit::px(900.), gpui_kit::px(1600.)));
        visual.replace_root_view(|_, _| TypedControlFixture {
            app: app.clone(),
            selection: false,
        });
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.click("surface-width-Bubble-inherit", cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
            assert!(window.find("surface-width-Bubble").bounds().size.height > gpui_kit::px(20.));
            let input = app.read(cx).preferences_ui.number_fields.borrow()["surface-width-Bubble"]
                .input
                .clone();
            input.update(cx, |input, cx| input.set_value("-", window, cx));
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.select_settings_target("notes.editing", cx);
                app.select_settings_target("notices.component.0", cx);
                assert_eq!(
                    app.preferences_ui.number_fields.borrow()["surface-width-Bubble"]
                        .input
                        .read(cx)
                        .value(),
                    "-"
                );
                let other = app.preferences_ui.notification_fields["surface-corner-bg"].clone();
                other.update(cx, |input, cx| input.set_value("invalid-color", window, cx));
                app.reset_settings_page(
                    SettingsResetScope::NoticeComponent(studyapp_core::NoticeStyle::Bubble),
                    window,
                    cx,
                );
                assert!(
                    !app.preferences_ui
                        .number_fields
                        .borrow()
                        .contains_key("surface-width-Bubble")
                );
                assert_eq!(other.read(cx).value(), "invalid-color");
                app.reset_settings_page(SettingsResetScope::NotificationAppearance, window, cx);
                assert_eq!(other.read(cx).value(), "invalid-color");
            });
        });
    }
    #[gpui_kit::test]
    fn selection_category_raw_color_drafts_survive_switching_and_unrelated_resets(
        cx: &mut TestAppContext,
    ) {
        let (app, visual) = fixture(cx);
        let key = "selection-navigation-background";
        visual.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.preferences_ui.selection_target =
                    Some(studyapp_core::SelectionCategory::Navigation);
                app.preferences_ui.appearance_fields[key]
                    .update(cx, |input, cx| input.set_value("#12", window, cx));
                app.preferences_ui.selection_target =
                    Some(studyapp_core::SelectionCategory::Directory);
                app.reset_settings_page(SettingsResetScope::ThemeEditor, window, cx);
                assert_eq!(
                    app.preferences_ui.appearance_fields[key].read(cx).value(),
                    "#12"
                );
                app.preferences_ui.selection_target =
                    Some(studyapp_core::SelectionCategory::Navigation);
            });
        });
        visual.replace_root_view(|_, _| TypedControlFixture {
            app: app.clone(),
            selection: true,
        });
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
            assert_eq!(
                app.read(cx).preferences_ui.appearance_fields[key]
                    .read(cx)
                    .value(),
                "#12"
            );
            app.update(cx, |app, cx| {
                assert!(app.settings_dirty(cx));
                app.reset_settings_page(SettingsResetScope::Selection, window, cx);
                assert_eq!(
                    app.preferences_ui.appearance_fields[key].read(cx).value(),
                    ""
                );
            });
        });
    }
    #[gpui_kit::test]
    fn quiet_hours_disabled_times_keep_raw_drafts_and_resume_without_saving(
        cx: &mut TestAppContext,
    ) {
        let (app, visual) = fixture(cx);
        visual.simulate_resize(gpui_kit::size(gpui_kit::px(1000.), gpui_kit::px(1400.)));
        visual.replace_root_view(|_, cx| {
            cx.observe(&app, |_, _, cx| cx.notify()).detach();
            NotificationFixture {
                app: app.clone(),
                appearance: false,
                color_only: false,
            }
        });
        visual.run_until_parked();
        let (start, end) = app.read_with(visual, |app, _| {
            (
                app.preferences_ui.notification_fields["notification-start"].clone(),
                app.preferences_ui.notification_fields["notification-end"].clone(),
            )
        });
        let hours = studyapp_core::QuietHours {
            start_minute: 1420,
            end_minute: 75,
        };
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert!(
                window.find("quiet-hours-enabled-hint").bounds().size.height > gpui_kit::px(0.)
            );
            let initial = start.read(cx).value();
            window.click("notification-start-hour-up", cx);
            assert_eq!(start.read(cx).value(), initial);
            window.click("notification-quiet", cx);
            window.render_frame(cx);
            for (input, raw) in [(&start, "23:40"), (&end, "01:15")] {
                input.update(cx, |input, cx| {
                    input.set_value(raw, window, cx);
                    cx.emit(gpui_kit::component::input::InputEvent::Change);
                });
            }
        });
        // Input events are delivered after the outer App update, as in the
        // native event loop; rendering alone does not flush subscriptions.
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(
                app.read(cx)
                    .preferences_ui
                    .draft
                    .as_ref()
                    .unwrap()
                    .notifications
                    .quiet_hours,
                Some(hours.clone())
            );
            window.click("notification-quiet", cx);
            window.render_frame(cx);
            window.click("notification-start-minute-up", cx);
            assert_eq!(start.read(cx).value(), "23:40");
            assert_eq!(end.read(cx).value(), "01:15");
            assert!(
                app.read(cx)
                    .preferences_ui
                    .draft
                    .as_ref()
                    .unwrap()
                    .notifications
                    .quiet_hours
                    .is_none()
            );
            assert!(window.find("quiet-hours-range-hint").bounds().size.height > gpui_kit::px(0.));
            window.click("notification-quiet", cx);
            window.render_frame(cx);
            assert_eq!(
                app.read(cx)
                    .preferences_ui
                    .draft
                    .as_ref()
                    .unwrap()
                    .notifications
                    .quiet_hours,
                Some(hours)
            );
            start.update(cx, |input, cx| {
                input.set_value("12:", window, cx);
                cx.emit(gpui_kit::component::input::InputEvent::Change);
            });
            window.render_frame(cx);
            window.click("notification-quiet", cx);
            window.render_frame(cx);
            window.click("notification-quiet", cx);
            window.render_frame(cx);
            assert_eq!(start.read(cx).value(), "12:");
            app.update(cx, |app, cx| {
                let draft = app.preferences_ui.draft.as_ref().unwrap();
                assert!(
                    app.notification_settings_value(draft.notifications.clone(), cx)
                        .is_err()
                );
                assert_eq!(
                    app.preferences_ui
                        .saved
                        .as_ref()
                        .unwrap()
                        .settings
                        .notifications,
                    studyapp_core::NotificationSettings::default()
                );
                assert_eq!(app.notifications.read(cx).visible_count(), 0);
                app.discard_settings(window, cx);
                assert!(!app.settings_dirty(cx));
            });
        });
    }

    struct StageSettingsFixture {
        app: Entity<StudyApp>,
        widget: bool,
    }
    impl Render for StageSettingsFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.app.update(cx, |app, cx| {
                if self.widget {
                    app.widget_preset_stages(cx)
                } else {
                    app.review_stages(cx)
                }
            })
        }
    }
    #[gpui_kit::test]
    fn stage_group_reset_stays_in_its_header_on_every_page_and_preserves_other_drafts(
        cx: &mut TestAppContext,
    ) {
        let (app, visual) = fixture(cx);
        for widget in [false, true] {
            visual.update(|window, cx| {
                app.update(cx, |app, cx| {
                    app.fill_review_stages(&[1, 2, 3, 4, 5, 6, 7], window, cx);
                    app.fill_widget_preset_stages(&[1, 2, 3, 4, 5, 6, 7, 8], window, cx);
                    app.preferences_ui.review_page = 1;
                    app.preferences_ui.widget_preset_page = 1;
                    app.interval.update(cx, |input, cx| {
                        input.set_value("unrelated draft", window, cx)
                    });
                })
            });
            visual.replace_root_view(|_, cx| {
                cx.observe(&app, |_, _, cx| cx.notify()).detach();
                StageSettingsFixture {
                    app: app.clone(),
                    widget,
                }
            });
            visual.update(|window, cx| {
                window.render_frame(cx);
                let (header, reset) = if widget {
                    (
                        "settings-section-header-倒计时预设",
                        "widget_preset-reset-stages",
                    )
                } else {
                    ("settings-section-header-复习间隔", "review-reset-stages")
                };
                let bounds = window.find(header).bounds();
                let action = window.find(reset).bounds();
                assert!(action.top() >= bounds.top() && action.bottom() <= bounds.bottom());
                assert!(action.left() >= bounds.left() && action.right() <= bounds.right());
                window.click(reset, cx);
                app.update(cx, |app, cx| {
                    assert_eq!(app.interval.read(cx).value(), "unrelated draft");
                    if widget {
                        assert_eq!(app.preferences_ui.widget_preset_page, 0);
                        assert_eq!(
                            app.preferences_ui.widget_preset_stages.len(),
                            studyapp_core::TimerWidgetSettings::default()
                                .countdown_presets
                                .len()
                        );
                        assert_eq!(app.preferences_ui.review_stages.len(), 7);
                    } else {
                        assert_eq!(app.preferences_ui.review_page, 0);
                        assert_eq!(
                            app.preferences_ui.review_stages.len(),
                            studyapp_core::Settings::default().review_days.len()
                        );
                        assert_eq!(app.preferences_ui.widget_preset_stages.len(), 8);
                    }
                    assert_eq!(
                        app.preferences_ui.saved.as_ref().unwrap().settings,
                        studyapp_core::Settings::default()
                    );
                });
            });
        }
    }

    struct TooltipSearchFixture(Entity<StudyApp>);
    impl Render for TooltipSearchFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.0.update(cx, |app, cx| {
                crate::components::column()
                    .w(gpui_kit::px(650.))
                    .child(app.settings_search_results(cx))
                    .child(app.notice_surface_controls(cx))
            })
        }
    }
    #[gpui_kit::test]
    fn tooltip_position_search_navigates_without_editing_then_choice_is_discardable(
        cx: &mut TestAppContext,
    ) {
        use studyapp_core::{NoticeStyle, TooltipPosition};
        let (app, visual) = fixture(cx);
        visual.simulate_resize(gpui_kit::size(gpui_kit::px(900.), gpui_kit::px(1800.)));
        visual.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.preferences_ui.notice_component = Some(NoticeStyle::Bubble);
                app.preferences_ui
                    .search
                    .as_ref()
                    .unwrap()
                    .update(cx, |input, cx| input.set_value("说明定位", window, cx));
            })
        });
        visual.replace_root_view(|_, cx| {
            cx.observe(&app, |_, _, cx| cx.notify()).detach();
            TooltipSearchFixture(app.clone())
        });
        let index = crate::settings_search::search("说明定位")
            .iter()
            .position(|entry| entry.label == "说明定位")
            .unwrap();
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("tooltip-position").is_none());
            window.click(("settings-search-result", index), cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(
                app.read(cx).preferences_ui.notice_component,
                Some(NoticeStyle::Tooltip)
            );
            assert!(!app.read(cx).settings_dirty(cx));
            crate::settings_components::select_choice(window, "tooltip-position", 1, cx);
            assert_eq!(
                app.read(cx)
                    .preferences_ui
                    .draft
                    .as_ref()
                    .unwrap()
                    .appearance
                    .notifications
                    .tooltip_position,
                TooltipPosition::BesideControl
            );
            assert_eq!(
                app.read(cx)
                    .preferences_ui
                    .saved
                    .as_ref()
                    .unwrap()
                    .settings
                    .appearance
                    .notifications
                    .tooltip_position,
                TooltipPosition::FollowPointer
            );
            app.update(cx, |app, cx| app.discard_settings(window, cx));
            assert!(!app.read(cx).settings_dirty(cx));
        });
    }

    struct NoticeFieldsFixture(Entity<StudyApp>);
    impl Render for NoticeFieldsFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.0.update(cx, |app, cx| app.notice_content_controls(cx))
        }
    }
    #[gpui_kit::test]
    fn components_without_optional_fields_do_not_offer_inoperative_content_presets(
        cx: &mut TestAppContext,
    ) {
        let (app, visual) = fixture(cx);
        visual.replace_root_view(|_, cx| {
            cx.observe(&app, |_, _, cx| cx.notify()).detach();
            NoticeFieldsFixture(app.clone())
        });
        for style in studyapp_core::NoticeStyle::ALL {
            visual.update(|window, cx| {
                app.update(cx, |app, cx| {
                    app.preferences_ui.notice_component = Some(style);
                    cx.notify();
                });
                window.render_frame(cx);
                let configurable = !style.configurable_fields().is_empty();
                assert_eq!(
                    window.try_find("notice-fields-brief").is_some(),
                    configurable,
                    "{style:?}"
                );
                assert_eq!(
                    window.try_find("notice-fields-detailed").is_some(),
                    configurable,
                    "{style:?}"
                );
                assert_eq!(
                    window.try_find("notice-fields-inherit").is_some(),
                    configurable,
                    "{style:?}"
                );
                assert!(!app.read(cx).settings_dirty(cx));
            });
        }
    }

    struct WorkspaceFixture {
        app: Entity<StudyApp>,
        scroll: gpui_kit::ScrollHandle,
        width: f32,
    }
    impl Render for WorkspaceFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.app.update(cx, |app, cx| {
                crate::components::scroll_body("workspace-fixture-scroll", &self.scroll)
                    .w(gpui_kit::px(self.width))
                    .h_full()
                    .child(app.workspace_settings(gpui_kit::px(self.width), cx))
            })
        }
    }
    #[gpui_kit::test]
    fn workspace_widths_are_discardable_drafts_and_disabled_children_keep_values(
        cx: &mut TestAppContext,
    ) {
        let (app, visual) = fixture(cx);
        let scroll = gpui_kit::ScrollHandle::new();
        visual.replace_root_view(|_, cx| {
            cx.observe(&app, |_, _, cx| cx.notify()).detach();
            WorkspaceFixture {
                app: app.clone(),
                scroll: scroll.clone(),
                width: 700.,
            }
        });
        visual.update(|window, cx| window.render_frame(cx));
        let saved = app.read_with(visual, |app, _| app.preferences_ui.saved.clone().unwrap());
        visual.update(|window, cx| window.click("workspace-navigation-width-increase", cx));
        visual.run_until_parked();
        visual.update(|window, cx| {
            assert_eq!(app.read(cx).layout.navigation_width, 161);
            window.render_frame(cx);
            window.click("settings-navigation-visible", cx);
            window.render_frame(cx);
            window.click("workspace-navigation-width-increase", cx);
            assert_eq!(app.read(cx).layout.navigation_width, 161);
            assert!(!app.read(cx).layout.navigation_visible);
            window.click("settings-navigation-visible", cx);
            window.render_frame(cx);
            window.click("workspace-navigation-width-reset", cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            assert_eq!(app.read(cx).layout.navigation_width, 160);
            let input = app.read(cx).preferences_ui.number_fields.borrow()["workspace-notes-width"]
                .input
                .clone();
            input.update(cx, |input, cx| {
                input.set_value("333", window, cx);
                cx.emit(gpui_kit::component::input::InputEvent::Change);
            });
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            let input = app.read(cx).preferences_ui.number_fields.borrow()["workspace-notes-width"]
                .input
                .clone();
            assert_eq!(app.read(cx).layout.notes_width, 333);
            assert_eq!(
                app.read(cx)
                    .preferences_ui
                    .draft
                    .as_ref()
                    .unwrap()
                    .layout
                    .notes_width,
                333
            );
            assert_eq!(
                app.read(cx).preferences_ui.saved.as_ref().unwrap().settings,
                saved.settings
            );
            input.update(cx, |input, cx| {
                input.set_value("-", window, cx);
                cx.emit(gpui_kit::component::input::InputEvent::Change);
            });
            app.update(cx, |app, cx| {
                app.select_settings_target("learning", cx);
                app.select_settings_target("appearance.workspace", cx);
                assert!(app.setting_numbers_invalid(cx));
                assert_eq!(
                    app.preferences_ui.number_fields.borrow()["workspace-notes-width"]
                        .input
                        .read(cx)
                        .value(),
                    "-"
                );
                app.interval
                    .update(cx, |input, cx| input.set_value("other invalid", window, cx));
                app.reset_settings_page(SettingsResetScope::Workspace, window, cx);
                assert_eq!(app.layout, saved.settings.layout);
                assert!(
                    !app.preferences_ui
                        .number_fields
                        .borrow()
                        .contains_key("workspace-notes-width")
                );
                assert_eq!(app.interval.read(cx).value(), "other invalid");
                app.discard_settings(window, cx);
                assert!(!app.settings_dirty(cx));
            });
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.render_frame(cx);
            scroll.set_offset(gpui_kit::point(gpui_kit::px(0.), -scroll.max_offset().y));
            window.render_frame(cx);
            window.click("workspace-outline-shown", cx);
            window.render_frame(cx);
            crate::settings_components::select_choice(window, "workspace-outline-position", 2, cx);
            assert_eq!(
                app.read(cx).layout.outline,
                Some(studyapp_core::OutlinePreference {
                    position: studyapp_core::OutlinePosition::Right,
                    shown: true,
                })
            );
            window.render_frame(cx);
            window.click("workspace-outline-shown", cx);
            window.render_frame(cx);
            assert!(!app.read(cx).layout.outline.unwrap().shown);
            assert_eq!(
                app.read(cx).layout.outline.unwrap().position,
                studyapp_core::OutlinePosition::Right
            );
            app.update(cx, |app, cx| app.discard_settings(window, cx));
            assert_eq!(app.read(cx).layout, saved.settings.layout);
        });
    }
    #[gpui_kit::test]
    fn workspace_cards_adapt_to_content_width_without_editing_preferences(cx: &mut TestAppContext) {
        let (app, visual) = fixture(cx);
        visual.simulate_resize(gpui_kit::size(gpui_kit::px(1400.), gpui_kit::px(1600.)));
        let surface = visual.replace_root_view(|_, cx| {
            cx.observe(&app, |_, _, cx| cx.notify()).detach();
            WorkspaceFixture {
                app: app.clone(),
                scroll: gpui_kit::ScrollHandle::new(),
                width: 1100.,
            }
        });
        for width in [1100., 520., 1000.] {
            surface.update(visual, |view, cx| {
                view.width = width;
                cx.notify();
            });
            visual.run_until_parked();
            visual.update(|window, cx| {
                window.render_frame(cx);
                let order = window.find("workspace-order-card").bounds();
                let outline = window.find("workspace-outline-card").bounds();
                if width >= 800. {
                    assert!((f32::from(order.top() - outline.top())).abs() < 1.);
                    assert!(order.right() <= outline.left());
                } else {
                    assert!(outline.top() >= order.bottom());
                }
                assert!(order.right() <= gpui_kit::px(width + 1.));
                assert!(outline.right() <= gpui_kit::px(width + 1.));
                assert!(!app.read(cx).settings_dirty(cx));
            });
        }
    }
}
