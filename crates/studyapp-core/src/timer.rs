use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TimerKind {
    #[default]
    Study,
    CountUp,
    CountDown,
}
impl TimerKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Study => "专注",
            Self::CountUp => "正计时",
            Self::CountDown => "倒计时",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WidgetLayout {
    Compact,
    #[default]
    Standard,
    Expanded,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WidgetBackground {
    #[default]
    Follow,
    Solid,
    Inset,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct TimerWidgetSettings {
    pub pinned: bool,
    pub layout: WidgetLayout,
    pub opacity: u8,
    pub corner_radius: u8,
    pub scale: u8,
    pub background: WidgetBackground,
    pub accent: Option<String>,
    pub show_seconds: bool,
    pub show_progress: bool,
    pub show_segment: bool,
    pub show_controls: bool,
    pub countdown_presets: Vec<u32>,
}
impl Default for TimerWidgetSettings {
    fn default() -> Self {
        Self {
            pinned: true,
            layout: WidgetLayout::Standard,
            opacity: 92,
            corner_radius: 12,
            scale: 100,
            background: WidgetBackground::Follow,
            accent: None,
            show_seconds: true,
            show_progress: true,
            show_segment: true,
            show_controls: true,
            countdown_presets: vec![5, 15, 25, 45],
        }
    }
}
impl TimerWidgetSettings {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (30..=100).contains(&self.opacity),
            "小组件背景不透明度应为 30–100%"
        );
        ensure!(self.corner_radius <= 28, "小组件圆角应为 0–28");
        ensure!((80..=140).contains(&self.scale), "小组件缩放应为 80–140%");
        if let Some(color) = &self.accent {
            ensure!(
                crate::parse_rgba_color(color).is_some(),
                "小组件强调色须为 #RRGGBB 或 #RRGGBBAA"
            );
        }
        ensure!(
            !self.countdown_presets.is_empty()
                && self.countdown_presets.len() <= 8
                && self.countdown_presets.iter().all(|n| (1..=180).contains(n)),
            "倒计时预设须为 1–180 分钟，共 1–8 项"
        );
        let unique: std::collections::BTreeSet<_> = self.countdown_presets.iter().collect();
        ensure!(
            unique.len() == self.countdown_presets.len(),
            "倒计时预设不能重复"
        );
        Ok(())
    }
}
