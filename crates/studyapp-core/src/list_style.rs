//! Presentation-only unordered list markers; no CSS, HTML or source rewriting.
use serde::{Deserialize, Serialize};

pub const LIST_LEVELS: usize = 8;
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ListMarker {
    Disc,
    Circle,
    Square,
    HollowSquare,
    Triangle,
    HollowTriangle,
    Diamond,
    HollowDiamond,
    Arrow,
    Dash,
    Plus,
    Star,
}
impl ListMarker {
    pub const ALL: [Self; 12] = [
        Self::Disc,
        Self::Circle,
        Self::Square,
        Self::HollowSquare,
        Self::Triangle,
        Self::HollowTriangle,
        Self::Diamond,
        Self::HollowDiamond,
        Self::Arrow,
        Self::Dash,
        Self::Plus,
        Self::Star,
    ];
    pub const fn symbol(self) -> &'static str {
        match self {
            Self::Disc => "•",
            Self::Circle => "◦",
            Self::Square => "▪",
            Self::HollowSquare => "▫",
            Self::Triangle => "▸",
            Self::HollowTriangle => "▹",
            Self::Diamond => "◆",
            Self::HollowDiamond => "◇",
            Self::Arrow => "➤",
            Self::Dash => "–",
            Self::Plus => "+",
            Self::Star => "✦",
        }
    }
    pub const fn label(self) -> &'static str {
        match self {
            Self::Disc => "实心圆",
            Self::Circle => "空心圆",
            Self::Square => "实心方块",
            Self::HollowSquare => "空心方块",
            Self::Triangle => "实心三角",
            Self::HollowTriangle => "空心三角",
            Self::Diamond => "实心菱形",
            Self::HollowDiamond => "空心菱形",
            Self::Arrow => "箭头",
            Self::Dash => "短横线",
            Self::Plus => "加号",
            Self::Star => "星形",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ListPreset {
    #[default]
    Geometry,
    Classic,
    Diamonds,
    Arrows,
    Minimal,
}
impl ListPreset {
    pub const ALL: [Self; 5] = [
        Self::Geometry,
        Self::Classic,
        Self::Diamonds,
        Self::Arrows,
        Self::Minimal,
    ];
    pub const fn label(self) -> &'static str {
        match self {
            Self::Geometry => "几何层级",
            Self::Classic => "经典",
            Self::Diamonds => "菱形",
            Self::Arrows => "箭头",
            Self::Minimal => "简约",
        }
    }
    pub const fn markers(self) -> [ListMarker; LIST_LEVELS] {
        use ListMarker::*;
        match self {
            Self::Geometry => [
                Disc,
                Square,
                Triangle,
                Diamond,
                Circle,
                HollowSquare,
                HollowTriangle,
                HollowDiamond,
            ],
            Self::Classic => [
                Disc,
                Circle,
                Square,
                HollowSquare,
                Triangle,
                HollowTriangle,
                Diamond,
                HollowDiamond,
            ],
            Self::Diamonds => [
                Diamond,
                HollowDiamond,
                Triangle,
                HollowTriangle,
                Disc,
                Circle,
                Square,
                HollowSquare,
            ],
            Self::Arrows => [
                Triangle,
                HollowTriangle,
                Arrow,
                Diamond,
                HollowDiamond,
                Disc,
                Circle,
                Square,
            ],
            Self::Minimal => [
                Disc,
                Dash,
                Circle,
                Plus,
                Square,
                HollowSquare,
                Triangle,
                HollowTriangle,
            ],
        }
    }
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct ListStyle {
    pub preset: ListPreset,
    /// Cursor Tab affects only its line unless explicitly enabled.
    pub indent_subtree: bool,
    pub levels: [Option<ListMarker>; LIST_LEVELS],
}
impl ListStyle {
    pub fn markers(self) -> [ListMarker; LIST_LEVELS] {
        let base = self.preset.markers();
        std::array::from_fn(|i| self.levels[i].unwrap_or(base[i]))
    }
    /// Depth counts all enclosing list AST nodes (ordered lists included).
    /// Levels beyond eight repeat the configured eight-level sequence.
    pub fn marker(self, depth: usize) -> ListMarker {
        self.markers()[depth.saturating_sub(1) % LIST_LEVELS]
    }
}
