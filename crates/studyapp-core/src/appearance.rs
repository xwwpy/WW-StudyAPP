//! Declarative appearance preferences. No CSS or executable content is accepted.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
    Custom,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThemePalette {
    pub accent: String,
    pub background: String,
    pub surface: String,
    pub text: String,
}

impl Default for ThemePalette {
    fn default() -> Self {
        Self {
            accent: "#586D83".into(),
            background: "#F4F5F7".into(),
            surface: "#FFFFFF".into(),
            text: "#24272D".into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThemePreset {
    pub id: String,
    pub name: String,
    pub palette: ThemePalette,
    #[serde(default)]
    pub markdown: MarkdownStyle,
}

/// Shared stable keys. Values are only six-digit colors, never CSS.
pub const MARKDOWN_COLORS: [&str; 24] = [
    "body",
    "heading",
    "link",
    "muted",
    "quote",
    "quote_background",
    "code",
    "code_background",
    "table",
    "table_header",
    "table_background",
    "table_stripe",
    "list",
    "grid",
    "code_border",
    "syntax_keyword",
    "syntax_string",
    "syntax_number",
    "syntax_comment",
    "syntax_type",
    "syntax_property",
    "syntax_tag",
    "syntax_function",
    "syntax_punctuation",
];

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReadingFont {
    #[default]
    Sans,
    Serif,
    Mono,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct MarkdownLayout {
    pub body_font: ReadingFont,
    pub mono_font: ReadingFont,
    /// Zero inherits the application reading scale.
    pub body_size: u16,
    pub code_size: u16,
    /// Percentage; zero inherits the application spacing preference.
    pub line_height: u16,
    pub reading_width: u16,
    pub table_grid: bool,
    pub table_stripes: bool,
    pub code_border: bool,
    pub code_radius: u16,
}

impl Default for MarkdownLayout {
    fn default() -> Self {
        Self {
            body_font: ReadingFont::Sans,
            mono_font: ReadingFont::Mono,
            body_size: 0,
            code_size: 0,
            line_height: 0,
            reading_width: 720,
            table_grid: true,
            table_stripes: true,
            code_border: true,
            code_radius: 8,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct MarkdownStyle {
    pub colors: std::collections::BTreeMap<String, String>,
    #[serde(flatten)]
    pub layout: MarkdownLayout,
}

/// A readable warning only: contrast never prevents saving an explicit color.
#[derive(Clone, Debug, PartialEq)]
pub struct ContrastWarning {
    pub foreground: &'static str,
    pub background: &'static str,
    pub ratio: f64,
    pub minimum: f64,
}

impl std::fmt::Display for ContrastWarning {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} / {} 对比度 {:.2}:1，建议至少 {:.1}:1；仍可预览和保存",
            self.foreground, self.background, self.ratio, self.minimum
        )
    }
}

fn contrast_warning(
    foreground: &'static str,
    background: &'static str,
    color: u32,
    surface: u32,
    minimum: f64,
) -> Option<ContrastWarning> {
    let ratio = theme_contrast(color, surface);
    (ratio < minimum).then_some(ContrastWarning {
        foreground,
        background,
        ratio,
        minimum,
    })
}

/// Preserve the requested hue as far as possible while raising its contrast.
pub fn readable_theme_color(color: u32, backgrounds: &[u32]) -> u32 {
    let passes = |value| {
        backgrounds
            .iter()
            .all(|bg| theme_contrast(value, *bg) >= 4.5)
    };
    if passes(color) {
        return color;
    }
    let score = |value| {
        backgrounds
            .iter()
            .map(|bg| theme_contrast(value, *bg))
            .fold(f64::INFINITY, f64::min)
    };
    let target = if score(0xFFFFFF) > score(0) {
        0xFFFFFF
    } else {
        0
    };
    for step in 1u32..=100 {
        let channel = |shift: u32| {
            (((color >> shift) & 255u32) * (100 - step) + ((target >> shift) & 255u32) * step) / 100
        };
        let candidate = channel(16) << 16 | channel(8) << 8 | channel(0);
        if passes(candidate) {
            return candidate;
        }
    }
    // Some valid legacy palettes use a middle gray on both black and white.
    // Neither endpoint works there, but a narrow gray interval still does.
    if let Some(gray) = (0u32..=255)
        .map(|value| value * 0x010101)
        .find(|value| passes(*value))
    {
        return gray;
    }
    target
}

impl MarkdownStyle {
    pub fn resolved_colors(&self, palette: &ThemePalette) -> [u32; 24] {
        let background = parse_theme_color(&palette.background).unwrap_or(0xFFFFFF);
        let surface = parse_theme_color(&palette.surface).unwrap_or(0xFFFFFF);
        let text = parse_theme_color(&palette.text).unwrap_or(0x24272D);
        let accent = parse_theme_color(&palette.accent).unwrap_or(0x2478D4);
        let dark = theme_contrast(surface, 0xFFFFFF) > theme_contrast(surface, 0);
        let syntax = if dark {
            [
                0xD6A4E8, 0x9BCEA1, 0xEFBD83, 0xB1BDC9, 0xE7CE8B, 0x8DCBD9, 0xF0AAAD, 0x99CAEC,
                0xC3CBD5,
            ]
        } else {
            [
                0x854191, 0x28683E, 0x91531A, 0x586B79, 0x7C5A19, 0x17677B, 0x9B4446, 0x24648C,
                0x536473,
            ]
        };
        let blend = |a: u32, b: u32| {
            let channel = |shift: u32| (((a >> shift) & 255) * 4 + ((b >> shift) & 255)) / 5;
            channel(16) << 16 | channel(8) << 8 | channel(0)
        };
        let border = blend(surface, text);
        let inset = {
            let channel = |shift: u32| {
                (((surface >> shift) & 255) * 955 + ((text >> shift) & 255) * 45) / 1000
            };
            let proposed = channel(16) << 16 | channel(8) << 8 | channel(0);
            if theme_contrast(text, proposed) >= 4.5 {
                proposed
            } else {
                surface
            }
        };
        let mut result = [
            text,
            text,
            accent,
            blend(text, surface),
            text,
            inset,
            text,
            inset,
            text,
            text,
            surface,
            inset,
            accent,
            border,
            border,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ];
        result[15..].copy_from_slice(&syntax);
        for (index, key) in MARKDOWN_COLORS.iter().enumerate() {
            if let Some(value) = self
                .colors
                .get(*key)
                .and_then(|value| parse_theme_color(value))
            {
                result[index] = value;
            }
        }
        for index in 0..result.len() {
            // Explicit Markdown colors and the inherited base body/heading
            // color are user choices, including deliberately low contrast.
            if self.colors.contains_key(MARKDOWN_COLORS[index]) || matches!(index, 0 | 1) {
                continue;
            }
            let stripe = if self.layout.table_stripes {
                result[11]
            } else {
                result[10]
            };
            let backgrounds = match index {
                5 | 7 | 10 | 11 | 13 | 14 => continue,
                4 => vec![result[5]],
                6 | 15..=23 => vec![result[7]],
                8 => vec![result[10], stripe, surface],
                9 => vec![surface],
                // Inline semantics may occur in paragraphs, quotes and tables.
                0 | 2 | 3 | 12 => vec![background, result[5], result[10], stripe, surface],
                _ => vec![background],
            };
            result[index] = readable_theme_color(result[index], &backgrounds);
        }
        result
    }

    pub fn validate(&self, _palette: &ThemePalette) -> Result<()> {
        let layout = self.layout;
        ensure!(
            layout.body_size == 0 || (12..=28).contains(&layout.body_size),
            "正文字号须为 12–28，0 表示继承"
        );
        ensure!(
            layout.code_size == 0 || (12..=26).contains(&layout.code_size),
            "代码字号须为 12–26，0 表示继承"
        );
        ensure!(
            layout.line_height == 0 || (130..=240).contains(&layout.line_height),
            "行高须为 130–240%，0 表示继承"
        );
        ensure!(
            (480..=1600).contains(&layout.reading_width),
            "阅读宽度须为 480–1600 像素"
        );
        ensure!(layout.code_radius <= 24, "代码圆角须为 0–24 像素");
        for (key, value) in &self.colors {
            if !MARKDOWN_COLORS.contains(&key.as_str()) {
                anyhow::bail!("未知 Markdown 颜色：{key}");
            }
            parse_theme_color(value).ok_or_else(|| anyhow::anyhow!("{key}：颜色须使用 #RRGGBB"))?;
        }
        Ok(())
    }

    pub fn contrast_warnings(&self, palette: &ThemePalette) -> Vec<ContrastWarning> {
        let resolved = self.resolved_colors(palette);
        let surface = parse_theme_color(&palette.surface).unwrap_or(0xFFFFFF);
        let mut warnings = Vec::new();
        for (index, key) in MARKDOWN_COLORS.iter().enumerate() {
            let backgrounds: Vec<(&'static str, u32)> = match index {
                5 | 7 | 10 | 11 | 13 | 14 => continue,
                4 => vec![("quote_background", resolved[5])],
                6 | 15..=23 => vec![("code_background", resolved[7])],
                8 => vec![
                    ("table_background", resolved[10]),
                    (
                        "table_stripe",
                        if self.layout.table_stripes {
                            resolved[11]
                        } else {
                            resolved[10]
                        },
                    ),
                ],
                0 | 2 | 3 | 12 => vec![
                    ("surface", surface),
                    ("quote_background", resolved[5]),
                    ("table_background", resolved[10]),
                    (
                        "table_stripe",
                        if self.layout.table_stripes {
                            resolved[11]
                        } else {
                            resolved[10]
                        },
                    ),
                ],
                _ => vec![("surface", surface)],
            };
            for (background, color) in backgrounds {
                if let Some(warning) =
                    contrast_warning(key, background, resolved[index], color, 4.5)
                {
                    warnings.push(warning);
                }
            }
        }
        warnings
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct AppearanceSettings {
    pub mode: ThemeMode,
    pub active_preset: Option<String>,
    pub custom_presets: Vec<ThemePreset>,
    pub font_size: u16,
    pub spacing: u16,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            mode: ThemeMode::System,
            active_preset: None,
            custom_presets: vec![],
            font_size: 14,
            spacing: 8,
        }
    }
}

pub fn parse_theme_color(value: &str) -> Option<u32> {
    let hex = value.strip_prefix('#')?;
    (hex.len() == 6 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| u32::from_str_radix(hex, 16).ok())
        .flatten()
}

fn luminance(color: u32) -> f64 {
    let channel = |shift: u32| {
        let value = ((color >> shift) & 255u32) as f64 / 255.;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
}

pub fn theme_contrast(first: u32, second: u32) -> f64 {
    let a = luminance(first);
    let b = luminance(second);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

impl ThemePalette {
    pub fn validate(&self) -> Result<()> {
        let text = parse_theme_color(&self.text);
        let background = parse_theme_color(&self.background);
        let surface = parse_theme_color(&self.surface);
        let accent = parse_theme_color(&self.accent);
        ensure!(
            [text, background, surface, accent]
                .iter()
                .all(Option::is_some),
            "颜色须使用 #RRGGBB 六位十六进制格式"
        );
        Ok(())
    }

    pub fn contrast_warnings(&self) -> Vec<ContrastWarning> {
        let (Some(text), Some(background), Some(surface), Some(accent)) = (
            parse_theme_color(&self.text),
            parse_theme_color(&self.background),
            parse_theme_color(&self.surface),
            parse_theme_color(&self.accent),
        ) else {
            return Vec::new();
        };
        [
            contrast_warning("文字", "背景", text, background, 4.5),
            contrast_warning("文字", "卡片", text, surface, 4.5),
            contrast_warning("强调色", "卡片", accent, surface, 3.),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

impl AppearanceSettings {
    pub fn active_palette(&self) -> Option<&ThemePalette> {
        let id = self.active_preset.as_ref()?;
        self.custom_presets
            .iter()
            .find(|preset| &preset.id == id)
            .map(|preset| &preset.palette)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!((12..=22).contains(&self.font_size), "字号须为 12–22 像素");
        ensure!((4..=16).contains(&self.spacing), "间距须为 4–16 像素");
        ensure!(self.custom_presets.len() <= 20, "自定义主题最多 20 个");
        let mut ids = std::collections::BTreeSet::new();
        let mut names = std::collections::BTreeSet::new();
        for preset in &self.custom_presets {
            ensure!(
                !preset.id.is_empty()
                    && preset.id.len() <= 80
                    && preset
                        .id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
                "主题标识无效"
            );
            ensure!(ids.insert(&preset.id), "主题标识不能重复");
            let name = preset.name.trim();
            ensure!(
                !name.is_empty()
                    && name.chars().count() <= 40
                    && !name.chars().any(char::is_control),
                "主题名称须为 1–40 个可显示字符"
            );
            ensure!(names.insert(name.to_lowercase()), "主题名称不能重复");
            preset.palette.validate()?;
            preset.markdown.validate(&preset.palette)?;
        }
        if let Some(id) = &self.active_preset {
            ensure!(ids.contains(id), "所选自定义主题不存在");
        }
        ensure!(
            self.mode != ThemeMode::Custom || self.active_palette().is_some(),
            "请选择自定义主题"
        );
        Ok(())
    }
}
