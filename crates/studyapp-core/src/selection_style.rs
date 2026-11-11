//! One interaction palette for navigation, settings choices and library rows.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum SelectionCategory {
    Navigation,
    Options,
    Library,
    Directory,
}
impl SelectionCategory {
    pub const ALL: [Self; 4] = [
        Self::Navigation,
        Self::Options,
        Self::Library,
        Self::Directory,
    ];
    pub const fn index(self) -> usize {
        match self {
            Self::Navigation => 0,
            Self::Options => 1,
            Self::Library => 2,
            Self::Directory => 3,
        }
    }
    pub const fn label(self) -> &'static str {
        match self {
            Self::Navigation => "页面导航与设置分类",
            Self::Options => "页签与配置选项",
            Self::Library => "笔记与文件列表",
            Self::Directory => "目录树",
        }
    }
}
/// A patch, not a copied palette: changing one property leaves every other
/// property following the global default, including later global edits.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SelectionOverride {
    pub selected_background: Option<String>,
    pub selected_text: Option<String>,
    pub selected_border: Option<String>,
    pub idle_background: Option<String>,
    pub idle_text: Option<String>,
    pub indicator: Option<bool>,
    pub radius: Option<u8>,
}

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
    pub categories: BTreeMap<SelectionCategory, SelectionOverride>,
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
            categories: BTreeMap::new(),
        }
    }
}
impl SelectionAppearance {
    pub fn effective(&self, category: SelectionCategory) -> Self {
        let mut result = self.clone();
        result.categories.clear();
        if let Some(value) = self.categories.get(&category) {
            macro_rules! inherit { ($($field:ident),*) => { $(if value.$field.is_some() { result.$field = value.$field.clone(); })* }; }
            inherit!(
                selected_background,
                selected_text,
                selected_border,
                idle_background,
                idle_text
            );
            if let Some(indicator) = value.indicator {
                result.indicator = indicator;
            }
            if let Some(radius) = value.radius {
                result.radius = radius;
            }
        }
        result
    }
    pub fn validate(&self) -> Result<()> {
        for category in self.categories.keys() {
            self.effective(*category).validate()?;
        }
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
