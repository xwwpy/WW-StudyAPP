//! One interaction palette for navigation, settings choices and library rows.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SelectionAppearance {
    pub selected_background: Option<String>,
    pub selected_text: Option<String>,
    pub selected_border: Option<String>,
    pub idle_background: Option<String>,
    pub idle_text: Option<String>,
    pub indicator: bool,
    pub radius: u8,
}
impl Default for SelectionAppearance {
    fn default() -> Self {
        Self {
            selected_background: None,
            selected_text: None,
            selected_border: None,
            idle_background: None,
            idle_text: None,
            indicator: true,
            radius: 8,
        }
    }
}
impl SelectionAppearance {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.radius <= 24, "选中项圆角须为0–24");
        for color in [
            &self.selected_background,
            &self.selected_text,
            &self.selected_border,
            &self.idle_background,
            &self.idle_text,
        ]
        .into_iter()
        .flatten()
        {
            ensure!(
                crate::parse_rgba_color(color).is_some(),
                "选中项颜色须为#RRGGBB或#RRGGBBAA"
            );
        }
        Ok(())
    }
}
