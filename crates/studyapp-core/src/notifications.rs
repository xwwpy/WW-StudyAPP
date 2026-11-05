//! Declarative notification choices. Required operation confirmations are not events.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum NoticeCategory {
    UpdateAvailable,
    UpdateCheckFailure,
    UpdateReady,
    UpdateDownloadFailure,
    UpdateInstallFailure,
    TimerComplete,
    ReviewDue,
    SaveFailure,
    SaveSuccess,
    SyncFailure,
    SyncConflict,
    SyncComplete,
    ImportResult,
    ExportResult,
    MigrationResult,
    IntegrityResult,
    ImageFailure,
    OperationResult,
    OperationFailure,
    StartupRecovery,
}
impl NoticeCategory {
    pub const ALL: [Self; 20] = [
        Self::UpdateAvailable,
        Self::UpdateCheckFailure,
        Self::UpdateReady,
        Self::UpdateDownloadFailure,
        Self::UpdateInstallFailure,
        Self::TimerComplete,
        Self::ReviewDue,
        Self::SaveFailure,
        Self::SaveSuccess,
        Self::SyncFailure,
        Self::SyncConflict,
        Self::SyncComplete,
        Self::ImportResult,
        Self::ExportResult,
        Self::MigrationResult,
        Self::IntegrityResult,
        Self::ImageFailure,
        Self::OperationResult,
        Self::OperationFailure,
        Self::StartupRecovery,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::UpdateAvailable => "发现新版本",
            Self::UpdateCheckFailure => "检查更新失败",
            Self::UpdateReady => "更新下载完成",
            Self::UpdateDownloadFailure => "更新下载失败",
            Self::UpdateInstallFailure => "更新校验与安装失败",
            Self::TimerComplete => "计时完成",
            Self::ReviewDue => "复习到期",
            Self::SaveFailure => "保存失败",
            Self::SaveSuccess => "主动保存成功",
            Self::SyncFailure => "同步失败",
            Self::SyncConflict => "同步冲突",
            Self::SyncComplete => "同步完成",
            Self::ImportResult => "导入结果",
            Self::ExportResult => "导出结果",
            Self::MigrationResult => "位置迁移结果",
            Self::IntegrityResult => "文件完整性结果",
            Self::ImageFailure => "图片操作失败",
            Self::OperationResult => "其他操作完成",
            Self::OperationFailure => "文件、目录与其它操作错误",
            Self::StartupRecovery => "启动与更新恢复",
        }
    }
    pub fn default_rule(self) -> NoticeRule {
        NoticeRule {
            enabled: self != Self::SaveSuccess,
            ..Default::default()
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NoticeStyle {
    Bubble,
    CornerCard,
    Banner,
    ModalCard,
    Anchored,
}
impl NoticeStyle {
    pub const ALL: [Self; 5] = [
        Self::Bubble,
        Self::CornerCard,
        Self::Banner,
        Self::ModalCard,
        Self::Anchored,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Bubble => "轻量气泡",
            Self::CornerCard => "角落操作卡片",
            Self::Banner => "顶部悬浮横幅",
            Self::ModalCard => "居中弹出卡片",
            Self::Anchored => "控件定位气泡",
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum NoticeSeverity {
    Information,
    Success,
    Warning,
    Error,
    Critical,
}
impl NoticeSeverity {
    pub fn persistent(self) -> bool {
        self >= Self::Error
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct NoticeRule {
    pub enabled: bool,
    pub foreground: bool,
    pub background: bool,
    pub style: Option<NoticeStyle>,
}
impl Default for NoticeRule {
    fn default() -> Self {
        Self {
            enabled: true,
            foreground: true,
            background: true,
            style: None,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct QuietHours {
    pub start_minute: u16,
    pub end_minute: u16,
}
impl QuietHours {
    pub fn contains(&self, minute: u16) -> bool {
        if self.start_minute < self.end_minute {
            (self.start_minute..self.end_minute).contains(&minute)
        } else {
            minute >= self.start_minute || minute < self.end_minute
        }
    }
    fn validate(&self) -> Result<()> {
        ensure!(
            self.start_minute < 1440
                && self.end_minute < 1440
                && self.start_minute != self.end_minute,
            "免打扰时段须为一天内不同的开始与结束时间"
        );
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct NotificationSettings {
    pub rules: BTreeMap<NoticeCategory, NoticeRule>,
    pub update_badge: bool,
    pub do_not_disturb: bool,
    pub quiet_hours: Option<QuietHours>,
    pub critical_during_quiet: bool,
    pub private_desktop: bool,
    pub sound: bool,
}
impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            rules: BTreeMap::new(),
            update_badge: true,
            do_not_disturb: false,
            quiet_hours: None,
            critical_during_quiet: true,
            private_desktop: true,
            sound: false,
        }
    }
}
impl NotificationSettings {
    pub fn rule(&self, category: NoticeCategory) -> NoticeRule {
        self.rules
            .get(&category)
            .cloned()
            .unwrap_or_else(|| category.default_rule())
    }
    pub fn validate(&self) -> Result<()> {
        if let Some(hours) = &self.quiet_hours {
            hours.validate()?;
        }
        Ok(())
    }
    pub fn quiet(&self, minute: u16) -> bool {
        self.do_not_disturb
            || self
                .quiet_hours
                .as_ref()
                .is_some_and(|hours| hours.contains(minute))
    }
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NoticePosition {
    TopLeft,
    TopRight,
    BottomLeft,
    #[default]
    BottomRight,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NoticeDensity {
    Compact,
    #[default]
    Comfortable,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct NotificationAppearance {
    pub style: NoticeStyle,
    pub position: NoticePosition,
    pub duration_seconds: u8,
    pub max_visible: u8,
    pub width: u16,
    pub radius: u8,
    pub density: NoticeDensity,
    pub background: Option<String>,
    pub accent: Option<String>,
    pub animation: bool,
    pub reduce_motion: bool,
    pub shadow: bool,
}
impl Default for NotificationAppearance {
    fn default() -> Self {
        Self {
            style: NoticeStyle::Bubble,
            position: Default::default(),
            duration_seconds: 5,
            max_visible: 2,
            width: 360,
            radius: 12,
            density: Default::default(),
            background: None,
            accent: None,
            animation: true,
            reduce_motion: false,
            shadow: true,
        }
    }
}
impl NotificationAppearance {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (2..=30).contains(&self.duration_seconds),
            "通知停留时间须为2–30秒"
        );
        ensure!((1..=4).contains(&self.max_visible), "通知叠放数量须为1–4");
        ensure!(
            (240..=480).contains(&self.width) && self.radius <= 24,
            "通知宽度须为240–480，圆角须为0–24"
        );
        for color in [&self.background, &self.accent].into_iter().flatten() {
            ensure!(
                crate::parse_rgba_color(color).is_some(),
                "通知颜色须为#RRGGBB或#RRGGBBAA"
            );
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeRoute {
    Foreground(NoticeStyle),
    DesktopCard,
    Deferred,
    Suppressed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoticeEnvironment {
    pub foreground: bool,
    pub desktop_available: bool,
    pub anchor_available: bool,
    pub minute: u16,
}
impl NotificationSettings {
    pub fn route(
        &self,
        category: NoticeCategory,
        severity: NoticeSeverity,
        appearance: &NotificationAppearance,
        environment: NoticeEnvironment,
    ) -> NoticeRoute {
        let rule = self.rule(category);
        if !rule.enabled
            || (self.quiet(environment.minute)
                && !(severity == NoticeSeverity::Critical && self.critical_during_quiet))
        {
            return NoticeRoute::Suppressed;
        }
        if !environment.foreground {
            return if !rule.background {
                NoticeRoute::Suppressed
            } else if environment.desktop_available {
                NoticeRoute::DesktopCard
            } else {
                NoticeRoute::Deferred
            };
        }
        if !rule.foreground {
            return NoticeRoute::Suppressed;
        }
        let style = rule.style.unwrap_or(match severity {
            NoticeSeverity::Critical => NoticeStyle::ModalCard,
            NoticeSeverity::Error => NoticeStyle::CornerCard,
            _ if matches!(
                category,
                NoticeCategory::TimerComplete
                    | NoticeCategory::ReviewDue
                    | NoticeCategory::SyncConflict
            ) =>
            {
                NoticeStyle::CornerCard
            }
            _ => appearance.style,
        });
        NoticeRoute::Foreground(
            if style == NoticeStyle::Anchored && !environment.anchor_available {
                NoticeStyle::CornerCard
            } else {
                style
            },
        )
    }
}

/// UUID correlation identity; independent of text, language or queue position.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NoticeEventId(uuid::Uuid);
impl Default for NoticeEventId {
    fn default() -> Self {
        Self::new()
    }
}
impl NoticeEventId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}
impl std::fmt::Display for NoticeEventId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "EV-{}", self.0)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeAction {
    OpenUpdate,
    RetryUpdate,
    OpenTimer,
    OpenReviews,
    OpenSave,
    OpenLibrary,
    OpenPreferences,
    OpenLogging,
    OpenSync,
    RetrySync,
    OpenStorage,
    OpenIntegrity,
    ShowDetails,
}
impl NoticeAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::OpenUpdate => "查看更新",
            Self::RetryUpdate => "重试更新",
            Self::OpenTimer => "查看计时",
            Self::OpenReviews => "查看复习",
            Self::OpenPreferences => "查看设置",
            Self::OpenLogging => "查看日志设置",
            Self::OpenLibrary => "查看笔记库",
            Self::OpenSave => "查看未保存内容",
            Self::OpenSync => "查看同步",
            Self::RetrySync => "重试同步",
            Self::OpenStorage => "查看存储",
            Self::OpenIntegrity => "查看检查结果",
            Self::ShowDetails => "查看详情",
        }
    }
}
#[derive(Clone, Debug)]
pub struct NoticeEvent {
    pub id: NoticeEventId,
    pub category: NoticeCategory,
    pub severity: NoticeSeverity,
    pub summary: String,
    pub dedupe_key: Option<String>,
    pub action: Option<NoticeAction>,
    pub anchor: Option<String>,
}
impl NoticeEvent {
    pub fn new(
        category: NoticeCategory,
        severity: NoticeSeverity,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            id: NoticeEventId::new(),
            category,
            severity,
            summary: summary.into(),
            dedupe_key: None,
            action: None,
            anchor: None,
        }
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.summary.trim().is_empty()
                && self.summary.len() <= 8192
                && !self
                    .summary
                    .chars()
                    .any(|c| c.is_control() && !matches!(c, '\n' | '\t')),
            "通知摘要无效"
        );
        ensure!(
            self.dedupe_key.as_ref().is_none_or(|key| !key.is_empty()
                && key.len() <= 160
                && !key.chars().any(char::is_control)),
            "通知去重标识无效"
        );
        ensure!(
            self.anchor.as_ref().is_none_or(|key| !key.is_empty()
                && key.len() <= 80
                && key
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))),
            "通知锚点无效"
        );
        Ok(())
    }
}
