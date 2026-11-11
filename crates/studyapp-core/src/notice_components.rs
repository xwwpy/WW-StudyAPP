//! Component-specific choices. Interactive surfaces are never ordinary queued
//! notifications; their controllers own the real selection, form or task.
use crate::{
    NoticeCategory, NoticeContent, NoticeSeverity, NoticeStack, NoticeStyle, NotificationAppearance,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NoticeLayout {
    #[default]
    Primary,
    Alternative,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TooltipPosition {
    #[default]
    FollowPointer,
    BesideControl,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum NoticeRegion {
    Toast,
    Progress,
    Corner,
    Banner,
    Snackbar,
}
impl NoticeRegion {
    pub const ALL: [Self; 5] = [
        Self::Toast,
        Self::Progress,
        Self::Corner,
        Self::Banner,
        Self::Snackbar,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Toast => "轻提示区",
            Self::Progress => "任务进度区",
            Self::Corner => "角落通知区",
            Self::Banner => "页面横幅区",
            Self::Snackbar => "底部反馈区",
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct NoticeRegionSettings {
    pub stack: NoticeStack,
    pub max_visible: u8,
}
impl Default for NoticeRegionSettings {
    fn default() -> Self {
        Self {
            stack: NoticeStack::Vertical,
            max_visible: 2,
        }
    }
}
/// Unset fields keep following component defaults, not a copied effective value.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct NoticeContentPatch {
    pub title: Option<bool>,
    pub summary: Option<bool>,
    pub icon: Option<bool>,
    pub repetitions: Option<bool>,
    pub event_id: Option<bool>,
    pub action: Option<bool>,
    pub detailed: Option<bool>,
}
impl NoticeContentPatch {
    pub fn apply(&self, mut defaults: NoticeContent) -> NoticeContent {
        macro_rules! fields { ($($field:ident),*) => { $(if let Some(value) = self.$field { defaults.$field = value; })* }; }
        fields!(
            title,
            summary,
            icon,
            repetitions,
            event_id,
            action,
            detailed
        );
        defaults
    }
}
impl NoticeStyle {
    /// Only these surfaces have independently adjustable content/command spacing.
    /// Other components keep the spacing defined by their structural layout.
    pub fn supports_density(self) -> bool {
        matches!(self, Self::CornerCard | Self::PopupMenu)
    }
    pub fn region(self) -> Option<NoticeRegion> {
        match self {
            Self::Bubble => Some(NoticeRegion::Toast),
            Self::Hud => Some(NoticeRegion::Progress),
            Self::CornerCard => Some(NoticeRegion::Corner),
            Self::Banner => Some(NoticeRegion::Banner),
            Self::Snackbar => Some(NoticeRegion::Snackbar),
            _ => None,
        }
    }
    pub fn layout_label(self, layout: NoticeLayout) -> &'static str {
        let alternative = layout == NoticeLayout::Alternative;
        match (self, alternative) {
            (Self::Bubble, false) => "图标与单行胶囊",
            (Self::Bubble, true) => "单行纯文字胶囊",
            (Self::Hud, false) => "横向任务进度",
            (Self::Hud, true) => "居中图标进度",
            (Self::ModalCard, false) => "居中强调与并排确认",
            (Self::ModalCard, true) => "左图标右侧风险说明",
            (Self::ActionSheet, false) => "底部纵向操作列表",
            (Self::ActionSheet, true) => "图标网格操作",
            (Self::Share, false) => "居中双栏预览与目标",
            (Self::Share, true) => "底部分层分享",
            (Self::ActivityView, false) => "右侧任务抽屉",
            (Self::ActivityView, true) => "居中任务表",
            (Self::Dialog, false) => "居中宽面板",
            (Self::Dialog, true) => "近全屏工作面板",
            (Self::Actionbar, false) => "选区附近浮动工具栏",
            (Self::Actionbar, true) => "内容顶部占位工具栏",
            (Self::Snackbar, false) => "底部居中悬浮条",
            (Self::Snackbar, true) => "贴底全宽反馈条",
            (Self::CornerCard, false) => "完整正文卡片",
            (Self::CornerCard, true) => "两行摘要与展开",
            (Self::Banner, false) => "内容顶部占位横幅",
            (Self::Banner, true) => "顶部悬浮横幅",
            (Self::Anchored, false) => "带箭头的控件详情",
            (Self::Anchored, true) => "紧凑控件详情",
            (Self::PopupMenu, false) => "纵向命令与子菜单",
            (Self::PopupMenu, true) => "紧凑命令与子菜单",
            (Self::Tooltip, false) => "纯文本说明",
            (Self::Tooltip, true) => "紧凑纯文本说明",
        }
    }
}
impl NotificationAppearance {
    pub fn layout(&self, style: NoticeStyle) -> NoticeLayout {
        self.layouts.get(&style).copied().unwrap_or_default()
    }
    pub fn region_settings(&self, region: NoticeRegion) -> NoticeRegionSettings {
        self.regions
            .get(&region)
            .copied()
            .unwrap_or(NoticeRegionSettings {
                stack: self.stack,
                max_visible: self.max_visible,
            })
    }
    pub fn component_content(&self, style: NoticeStyle) -> NoticeContent {
        let mut fields = self
            .component_fields
            .get(&style)
            .cloned()
            .unwrap_or_else(|| self.content.clone());
        match style {
            NoticeStyle::Bubble => {
                fields.title = false;
                fields.action = false;
                fields.event_id = false;
                fields.detailed = false;
            }
            NoticeStyle::Snackbar => {
                fields.title = false;
                fields.event_id = false;
            }
            NoticeStyle::Tooltip => {
                fields.title = false;
                fields.icon = false;
                fields.action = false;
                fields.repetitions = false;
                fields.event_id = false;
            }
            _ => {}
        }
        fields
    }
    pub fn effective_content(
        &self,
        style: NoticeStyle,
        category: Option<NoticeCategory>,
        severity: NoticeSeverity,
    ) -> NoticeContent {
        let defaults = self.component_content(style);
        let mut fields = category
            .and_then(|category| self.scenario_fields.get(&category))
            .map_or(defaults.clone(), |patch| patch.apply(defaults));
        // Structural constraints and mandatory risk/actions outrank optional fields.
        if matches!(
            style,
            NoticeStyle::Bubble | NoticeStyle::Snackbar | NoticeStyle::Tooltip
        ) {
            fields.title = false;
            fields.event_id = false;
        }
        if matches!(style, NoticeStyle::Bubble | NoticeStyle::Tooltip) {
            fields.action = false;
        }
        if severity >= NoticeSeverity::Error && style != NoticeStyle::Tooltip {
            fields.summary = true;
            fields.action = style != NoticeStyle::Bubble;
        }
        fields
    }
}

impl NoticeCategory {
    /// Ordinary events cannot instantiate a form, menu, sharing flow or task.
    pub fn notification_styles(self) -> &'static [NoticeStyle] {
        use NoticeStyle::*;
        match self {
            Self::SaveFailure
            | Self::SyncFailure
            | Self::SyncConflict
            | Self::ImageFailure
            | Self::OperationFailure
            | Self::UpdateCheckFailure
            | Self::UpdateDownloadFailure
            | Self::UpdateInstallFailure
            | Self::StartupRecovery => &[CornerCard, Banner, ModalCard],
            _ => &[Bubble, Snackbar, CornerCard, Banner, ModalCard],
        }
    }
    pub fn default_style(self, severity: NoticeSeverity) -> NoticeStyle {
        use NoticeStyle::*;
        match severity {
            NoticeSeverity::Critical => ModalCard,
            NoticeSeverity::Error => CornerCard,
            _ if !self.notification_styles().contains(&Bubble) => CornerCard,
            _ if matches!(
                self,
                Self::TimerComplete
                    | Self::ReviewDue
                    | Self::SyncConflict
                    | Self::UpdateAvailable
                    | Self::UpdateReady
                    | Self::StartupRecovery
            ) =>
            {
                CornerCard
            }
            NoticeSeverity::Warning => Banner,
            _ => Bubble,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeField {
    Title,
    Summary,
    Icon,
    Repetitions,
    EventId,
    Action,
    Detailed,
}
impl NoticeField {
    pub fn label(self) -> &'static str {
        match self {
            Self::Title => "标题",
            Self::Summary => "提示正文",
            Self::Icon => "状态图标",
            Self::Repetitions => "重复次数",
            Self::EventId => "事件标识",
            Self::Action => "操作入口",
            Self::Detailed => "完整正文",
        }
    }
    pub fn get(self, fields: &NoticeContent) -> bool {
        match self {
            Self::Title => fields.title,
            Self::Summary => fields.summary,
            Self::Icon => fields.icon,
            Self::Repetitions => fields.repetitions,
            Self::EventId => fields.event_id,
            Self::Action => fields.action,
            Self::Detailed => fields.detailed,
        }
    }
    pub fn set(self, fields: &mut NoticeContent, value: bool) {
        match self {
            Self::Title => fields.title = value,
            Self::Summary => fields.summary = value,
            Self::Icon => fields.icon = value,
            Self::Repetitions => fields.repetitions = value,
            Self::EventId => fields.event_id = value,
            Self::Action => fields.action = value,
            Self::Detailed => fields.detailed = value,
        }
    }
    pub fn patch(self, fields: &mut NoticeContentPatch) -> &mut Option<bool> {
        match self {
            Self::Title => &mut fields.title,
            Self::Summary => &mut fields.summary,
            Self::Icon => &mut fields.icon,
            Self::Repetitions => &mut fields.repetitions,
            Self::EventId => &mut fields.event_id,
            Self::Action => &mut fields.action,
            Self::Detailed => &mut fields.detailed,
        }
    }
}
impl NoticeStyle {
    pub fn configurable_fields(self) -> &'static [NoticeField] {
        use NoticeField::*;
        match self {
            Self::CornerCard => &[Title, Summary, Icon, Repetitions, EventId, Action, Detailed],
            Self::Bubble => &[Icon, Repetitions],
            Self::Snackbar => &[Action],
            Self::Banner => &[Icon, Action, Repetitions],
            _ => &[],
        }
    }
    pub fn queued_feedback(self) -> bool {
        matches!(
            self,
            Self::Bubble | Self::CornerCard | Self::Banner | Self::Snackbar | Self::ModalCard
        )
    }
}
