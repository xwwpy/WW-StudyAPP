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
/// New edits override individual properties. Empty color means theme automatic;
/// an absent property continues following its parent after future changes.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct NoticeSurfacePatch {
    pub opacity: Option<u8>,
    pub border_width: Option<u8>,
    pub border_color: Option<String>,
    pub background: Option<String>,
    pub width: Option<u16>,
    pub radius: Option<u8>,
    pub max_height: Option<u16>,
    pub density: Option<NoticeDensity>,
}
impl NoticeSurfacePatch {
    fn apply(&self, surface: &mut NoticeSurface) {
        macro_rules! fields { ($($field:ident),*) => { $(if let Some(value) = self.$field { surface.$field = value; })* }; }
        fields!(opacity, border_width, width, radius, max_height, density);
        for (value, field) in [
            (&self.background, &mut surface.background),
            (&self.border_color, &mut surface.border_color),
        ] {
            if let Some(value) = value {
                *field = (!value.is_empty()).then(|| value.clone());
            }
        }
    }
}
impl crate::NotificationAppearance {
    pub fn surface(&self, style: NoticeStyle) -> NoticeSurface {
        let mut resolved = self.surfaces.get(&style).cloned().unwrap_or_else(|| {
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
                NoticeStyle::ModalCard | NoticeStyle::ActionSheet => {
                    surface.max_height = 320;
                    surface.width = 480;
                }
                NoticeStyle::Share | NoticeStyle::Dialog => {
                    surface.max_height = 400;
                    surface.width = 640;
                }
                NoticeStyle::Tooltip => {
                    surface.width = 300;
                    surface.radius = 6;
                    surface.max_height = 200;
                }
                NoticeStyle::Anchored => {
                    surface.width = self.width.min(320);
                    surface.radius = self.radius.min(6);
                }
                NoticeStyle::ActivityView => surface.max_height = 240,
                _ => {}
            }
            surface
        });
        if let Some(patch) = self.surface_patches.get(&style) {
            patch.apply(&mut resolved);
        }
        resolved
    }
    pub fn surface_patch_mut(&mut self, style: NoticeStyle) -> &mut NoticeSurfacePatch {
        // Convert a legacy full override once, preserving every untouched field,
        // including an explicit theme-auto color rather than inheriting global.
        if let Some(legacy) = self.surfaces.remove(&style) {
            let patch = self.surface_patches.entry(style).or_default();
            patch.opacity.get_or_insert(legacy.opacity);
            patch.border_width.get_or_insert(legacy.border_width);
            patch.width.get_or_insert(legacy.width);
            patch.max_height.get_or_insert(legacy.max_height);
            patch.radius.get_or_insert(legacy.radius);
            patch.density.get_or_insert(legacy.density);
            patch
                .background
                .get_or_insert_with(|| legacy.background.unwrap_or_default());
            patch
                .border_color
                .get_or_insert_with(|| legacy.border_color.unwrap_or_default());
        }
        self.surface_patches.entry(style).or_default()
    }
    /// Compatibility API for existing fully explicit component configurations.
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
