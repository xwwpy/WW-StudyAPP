//! Per-component presentation, shared by foreground and passive desktop hosts.
use crate::{NoticeDensity, NoticeStyle};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NoticeStack {
    #[default]
    Vertical,
    NewestFirst,
    Staggered,
    Queue,
}
impl NoticeStack {
    pub const ALL: [Self; 4] = [
        Self::Vertical,
        Self::NewestFirst,
        Self::Staggered,
        Self::Queue,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Vertical => "纵向排列（新的在下）",
            Self::NewestFirst => "纵向排列（新的在上）",
            Self::Staggered => "阶梯排列",
            Self::Queue => "单张展示，其余排队",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct NoticeContent {
    pub title: bool,
    pub summary: bool,
    pub icon: bool,
    pub repetitions: bool,
    pub event_id: bool,
    pub action: bool,
    pub detailed: bool,
}
impl Default for NoticeContent {
    fn default() -> Self {
        Self {
            title: true,
            summary: true,
            icon: true,
            repetitions: true,
            event_id: false,
            action: true,
            detailed: true,
        }
    }
}
impl NoticeContent {
    pub fn brief() -> Self {
        Self {
            icon: false,
            repetitions: false,
            detailed: false,
            ..Self::default()
        }
    }
    pub fn detailed() -> Self {
        Self {
            event_id: true,
            ..Self::default()
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct NoticeSurface {
    pub opacity: u8,
    pub border_width: u8,
    pub border_color: Option<String>,
    pub background: Option<String>,
    pub width: u16,
    pub radius: u8,
    pub max_height: u16,
    pub density: NoticeDensity,
}
impl Default for NoticeSurface {
    fn default() -> Self {
        Self {
            opacity: 100,
            border_width: 1,
            border_color: None,
            background: None,
            width: 360,
            radius: 12,
            max_height: 164,
            density: NoticeDensity::Comfortable,
        }
    }
}
impl NoticeSurface {
    pub fn validate(&self) -> Result<()> {
        ensure!((20..=100).contains(&self.opacity), "气泡透明度须为20–100%");
        ensure!(
            self.border_width <= 4
                && self.radius <= 32
                && (200..=640).contains(&self.width)
                && (48..=480).contains(&self.max_height),
            "气泡大小或边框超出范围"
        );
        for value in [&self.border_color, &self.background].into_iter().flatten() {
            ensure!(
                crate::parse_rgba_color(value).is_some(),
                "气泡颜色须为#RRGGBB或#RRGGBBAA"
            );
        }
        Ok(())
    }
}
impl crate::NotificationAppearance {
    pub fn surface(&self, style: NoticeStyle) -> NoticeSurface {
        self.surfaces.get(&style).cloned().unwrap_or_else(|| {
            let mut surface = NoticeSurface {
                width: self.width,
                radius: self.radius,
                density: self.density,
                background: self.background.clone(),
                ..Default::default()
            };
            match style {
                NoticeStyle::Bubble | NoticeStyle::Snackbar => {
                    surface.density = NoticeDensity::Compact;
                    surface.radius = self.radius.min(10);
                }
                NoticeStyle::ModalCard | NoticeStyle::ActionSheet => surface.max_height = 320,
                NoticeStyle::Anchored => {
                    surface.width = self.width.min(320);
                    surface.radius = self.radius.min(6);
                }
                NoticeStyle::ActivityView => surface.max_height = 240,
                _ => {}
            }
            surface
        })
    }
    pub fn surface_mut(&mut self, style: NoticeStyle) -> &mut NoticeSurface {
        let fallback = self.surface(style);
        self.surfaces.entry(style).or_insert(fallback)
    }
    pub fn visible_limit(&self) -> u8 {
        if self.stack == NoticeStack::Queue {
            1
        } else {
            self.max_visible
        }
    }
}
