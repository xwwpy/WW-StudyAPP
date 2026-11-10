//! Shared native/component/editor appearance resolved only from safe persisted tokens.
use gpui_kit::{
    component::{Theme, ThemeMode as ComponentMode},
    *,
};
use std::cell::Cell;
use studyapp_core::{
    AppearanceSettings, MARKDOWN_COLORS, MarkdownLayout, RgbaColor, ThemeMode, ThemePalette,
    readable_theme_color, theme_contrast,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedTheme {
    pub table_defaults: studyapp_core::TableDefaults,
    pub indent_subtree: bool,
    pub list_markers: [studyapp_core::ListMarker; studyapp_core::LIST_LEVELS],
    pub render_html: bool,
    pub hover_content: studyapp_core::HoverContent,
    pub hover_background: RgbaColor,
    pub hover_border: RgbaColor,
    pub hover_width: u16,
    pub hover_height: u16,
    pub hover_radius: u8,
    pub hover_border_width: u8,
    pub selected_background: u32,
    pub selected_text: u32,
    pub selected_border: u32,
    pub idle_background: u32,
    pub idle_text: u32,
    pub selection_indicator: bool,
    pub selection_radius: u8,
    pub dark: bool,
    pub background: u32,
    pub surface: u32,
    pub surface_raised: u32,
    pub inset: u32,
    pub text: u32,
    pub muted: u32,
    pub accent: u32,
    pub accent_text: u32,
    pub accent_soft: u32,
    pub border: u32,
    pub border_strong: u32,
    pub focus: u32,
    pub danger: u32,
    pub warning: u32,
    pub success: u32,
    pub success_soft: u32,
    pub warning_soft: u32,
    pub danger_soft: u32,
    pub chart: [u32; 4],
    pub font_size: u16,
    pub spacing: u16,
    pub markdown_colors: [u32; 26],
    pub markdown_rgba: [RgbaColor; 26],
    pub markdown_layout: MarkdownLayout,
}

fn blend(first: u32, second: u32, weight: f32) -> u32 {
    let channel = |shift: u32| {
        (((first >> shift) & 255u32) as f32 * (1. - weight)
            + ((second >> shift) & 255u32) as f32 * weight)
            .round() as u32
    };
    channel(16) << 16 | channel(8) << 8 | channel(0)
}

pub fn resolve(settings: &AppearanceSettings, system_dark: bool) -> ResolvedTheme {
    let mut dark = match settings.mode {
        ThemeMode::System => system_dark,
        ThemeMode::Dark => true,
        _ => false,
    };
    let (mut background, mut surface, mut text, mut accent) = if dark {
        (0x1D1F23, 0x272A30, 0xEFF0F3, 0xA7BCD2)
    } else {
        (0xF4F5F7, 0xFFFFFF, 0x24272D, 0x586D83)
    };
    if settings.mode == ThemeMode::Custom
        && let Some(custom) = settings
            .active_palette()
            .filter(|custom| custom.validate().is_ok())
    {
        [background, surface, text, accent] = custom.composited(background);
        dark = theme_contrast(background, 0xFFFFFF) > theme_contrast(background, 0x000000);
    }
    let markdown = if settings.mode == ThemeMode::Custom {
        settings
            .custom_presets
            .iter()
            .find(|preset| Some(&preset.id) == settings.active_preset.as_ref())
            .filter(|preset| {
                preset.palette.validate().is_ok()
                    && preset.markdown.validate(&preset.palette).is_ok()
            })
            .map(|preset| preset.markdown.clone())
            .unwrap_or_default()
    } else {
        Default::default()
    };
    let resolved_palette = ThemePalette {
        background: format!("#{background:06X}"),
        surface: format!("#{surface:06X}"),
        text: format!("#{text:06X}"),
        accent: format!("#{accent:06X}"),
    };
    let selected_surface = blend(surface, accent, if dark { 0.22 } else { 0.10 });
    let accent_soft = if theme_contrast(text, selected_surface) >= 4.5 {
        selected_surface
    } else {
        surface
    };
    let safe_surface = |weight| {
        let candidate = blend(surface, text, weight);
        if theme_contrast(text, candidate) >= 4.5 {
            candidate
        } else {
            surface
        }
    };
    let surface_raised = safe_surface(0.02);
    let inset = safe_surface(0.045);
    let semantic = |seed| readable_theme_color(seed, &[background, surface, surface_raised, inset]);
    let danger = semantic(if dark { 0xFF9393 } else { 0xBE3434 });
    let warning = semantic(if dark { 0xF3BE79 } else { 0x9B5719 });
    let success = semantic(if dark { 0x84D6AD } else { 0x247448 });
    let soft = |semantic| blend(surface, semantic, 0.08);
    let focus = readable_theme_color(accent, &[surface]);
    let chosen = &settings.selection;
    let custom = |value: &Option<String>, base, fallback| {
        value
            .as_deref()
            .and_then(studyapp_core::parse_rgba_color)
            .map_or(fallback, |v| v.over(base))
    };
    let selected_default = blend(surface, accent, if dark { 0.30 } else { 0.20 });
    let selected_background = custom(
        &chosen.selected_background,
        surface,
        if theme_contrast(text, selected_default) >= 4.5 {
            selected_default
        } else {
            surface
        },
    );
    let idle_background = custom(&chosen.idle_background, surface, surface);
    let hover = settings
        .notifications
        .surface(studyapp_core::NoticeStyle::Anchored);
    let hover_background = hover
        .background
        .as_deref()
        .and_then(studyapp_core::parse_rgba_color)
        .unwrap_or(RgbaColor::opaque(surface_raised));
    let hover_background = RgbaColor::new(
        hover_background.rgb(),
        (u16::from(hover_background.alpha()) * u16::from(hover.opacity) / 100) as u8,
    );
    ResolvedTheme {
        hover_content: settings.notifications.hover_content,
        hover_background,
        hover_border: hover
            .border_color
            .as_deref()
            .and_then(studyapp_core::parse_rgba_color)
            .unwrap_or(RgbaColor::opaque(blend(surface, text, 0.10))),
        hover_width: hover.width,
        hover_height: hover.max_height,
        hover_radius: hover.radius,
        hover_border_width: hover.border_width,
        selected_background,
        selected_text: custom(
            &chosen.selected_text,
            selected_background,
            readable_theme_color(text, &[selected_background]),
        ),
        selected_border: custom(&chosen.selected_border, selected_background, focus),
        idle_background,
        idle_text: custom(
            &chosen.idle_text,
            idle_background,
            readable_theme_color(text, &[idle_background]),
        ),
        selection_indicator: chosen.indicator,
        selection_radius: chosen.radius,
        dark,
        background,
        surface,
        surface_raised,
        inset,
        text,
        accent,
        muted: readable_theme_color(
            blend(text, surface, 0.38),
            &[
                background,
                surface,
                surface_raised,
                inset,
                accent_soft,
                selected_background,
                neutral_selection(surface, text),
            ],
        ),
        accent_text: if theme_contrast(accent, 0xFFFFFF) >= theme_contrast(accent, 0x000000) {
            0xFFFFFF
        } else {
            0x000000
        },
        accent_soft,
        border: blend(surface, text, 0.10),
        border_strong: blend(surface, readable_theme_color(text, &[surface]), 0.22),
        focus,
        danger,
        warning,
        success,
        danger_soft: soft(danger),
        warning_soft: soft(warning),
        success_soft: soft(success),
        chart: [0.15, 0.35, 0.60, 0.85].map(|weight| blend(surface, focus, weight)),
        font_size: settings.font_size.clamp(12, 22),
        spacing: settings.spacing.clamp(4, 16),
        markdown_colors: markdown.resolved_colors(&resolved_palette),
        markdown_rgba: {
            let mut values = markdown.resolved_rgba_colors(&resolved_palette);
            if let Some(custom) = settings
                .active_palette()
                .filter(|custom| settings.mode == ThemeMode::Custom && custom.validate().is_ok())
            {
                for index in [0, 1] {
                    if !markdown.colors.contains_key(MARKDOWN_COLORS[index]) {
                        values[index] =
                            studyapp_core::parse_rgba_color(&custom.text).unwrap_or(values[index]);
                    }
                }
            }
            values
        },
        markdown_layout: markdown.layout,
        render_html: settings.render_html,
        table_defaults: settings.table_defaults,
        list_markers: settings.list_style.markers(),
        indent_subtree: settings.list_style.indent_subtree,
    }
}

thread_local! { static PALETTE: Cell<ResolvedTheme> = Cell::new(resolve(&AppearanceSettings::default(), false)); }
pub fn palette() -> ResolvedTheme {
    PALETTE.with(Cell::get)
}

/// Explicit semantic conversion for legacy native palette tokens. Raw accent
/// foreground is available as palette().accent_text, preserving custom contrast.
pub fn color(token: u32) -> Rgba {
    rgb(resolve_color(token))
}

pub fn resolve_color(token: u32) -> u32 {
    let p = palette();
    match token {
        0x233631 | 0x24272D => p.text,
        0x74827C | 0x6B7280 => p.muted,
        0x187B5B | 0x2478D4 => p.accent,
        0xEAF4ED | 0xEDF6EF | 0xF0F7F2 => p.accent_soft,
        0xE3E9E3 | 0xDDE5DF => p.border,
        0xF5F7F3 | 0xF4F5F7 => p.background,
        0xFFFFFF | 0xFAFBF9 => p.surface,
        0xBE3434 | 0xB83A3A | 0xB42318 => p.danger,
        0xA34829 | 0xA56618 | 0x9B5719 => p.warning,
        0x247448 => p.success,
        other => other,
    }
}

impl ResolvedTheme {
    pub fn editor_tokens(self) -> serde_json::Value {
        let hex = |color| format!("#{color:06X}");
        let mut markdown = serde_json::to_value(self.markdown_layout).expect("safe style layout");
        markdown["colors"] = MARKDOWN_COLORS
            .into_iter()
            .zip(self.markdown_rgba)
            .map(|(key, value)| (key.to_owned(), serde_json::Value::String(value.to_hex())))
            .collect::<serde_json::Map<_, _>>()
            .into();
        serde_json::json!({
            "renderHtml": self.render_html,
            "tableDefaults": self.table_defaults,
            "listMarkers": self.list_markers,
            "indentSubtree": self.indent_subtree,
            "mode": if self.dark { "dark" } else { "light" },
            "accent": hex(self.accent), "background": hex(self.surface),
            "surface": hex(self.surface), "text": hex(self.text),
            "toolbarBackground": hex(self.surface),
            "codeBackground": hex(self.markdown_colors[7]), "codeText": hex(self.markdown_colors[6]),
            "codeBorder": hex(self.markdown_colors[14]), "inlineCodeBackground": hex(self.inset),
            "quoteBackground": hex(self.markdown_colors[5]), "tableHeader": hex(self.surface_raised), "grid": hex(self.markdown_colors[13]),
            "muted": hex(self.muted), "border": hex(self.border),
            "selection": hex(self.markdown_colors[24]), "selectionText": hex(self.markdown_colors[25]), "fontSize": self.font_size,
            "spacing": 1.75 + (self.spacing as f64 - 8.) * 0.05,
            "markdown": markdown,
        })
    }
}

pub fn system_is_dark(appearance: WindowAppearance) -> bool {
    matches!(
        appearance,
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    )
}

/// A quiet surface selection, shared by tree rows and collection navigation.
pub fn selection_color() -> Hsla {
    rgb(selection_surface(palette())).into()
}

pub fn hover_color() -> Hsla {
    rgb(interaction_surface(palette(), 0.045)).into()
}

pub fn pressed_color() -> Hsla {
    rgb(interaction_surface(palette(), 0.12)).into()
}

fn interaction_surface(p: ResolvedTheme, amount: f32) -> u32 {
    let fill = blend(p.surface, p.text, amount);
    if [p.text, p.muted]
        .into_iter()
        .all(|text| theme_contrast(text, fill) >= 4.5)
    {
        fill
    } else {
        p.surface
    }
}

fn selection_surface(current: ResolvedTheme) -> u32 {
    current.selected_background
}

fn neutral_selection(surface: u32, text: u32) -> u32 {
    let fill = blend(surface, text, 0.08);
    if theme_contrast(text, fill) >= 4.5 {
        fill
    } else {
        surface
    }
}

pub fn apply(settings: &AppearanceSettings, system_dark: bool, cx: &mut App) -> ResolvedTheme {
    let p = resolve(settings, system_dark);
    PALETTE.with(|palette| palette.set(p));
    Theme::change(
        if p.dark {
            ComponentMode::Dark
        } else {
            ComponentMode::Light
        },
        None,
        cx,
    );
    Theme::update(cx, |theme| {
        theme.scrollbar_mode = gpui_kit::component::scroll::ScrollbarMode::Always;
        theme.font_family = crate::design::UI_FONT.into();
        theme.font_size = px(p.font_size as f32);
        theme.radius = px(crate::design::CONTROL_RADIUS);
        theme.radius_lg = px(crate::design::CARD_RADIUS);
        let c = &mut theme.colors;
        c.background = rgb(p.background).into();
        c.foreground = rgb(p.text).into();
        c.border = rgb(p.border).into();
        c.input = rgb(p.border_strong).into();
        c.primary = rgb(p.accent).into();
        c.primary_hover = rgb(blend(p.accent, p.text, 0.08)).into();
        c.primary_active = rgb(blend(p.accent, p.text, 0.16)).into();
        c.primary_foreground = rgb(p.accent_text).into();
        c.button_primary = c.primary;
        c.button_primary_hover = c.primary_hover;
        c.button_primary_active = c.primary_active;
        c.button_primary_foreground = c.primary_foreground;
        c.button = rgb(p.surface).into();
        c.button_foreground = rgb(p.text).into();
        c.button_hover = hover_color();
        c.button_active = pressed_color();
        c.secondary = rgb(p.surface).into();
        c.secondary_foreground = rgb(p.text).into();
        c.secondary_hover = hover_color();
        c.secondary_active = selection_color();
        c.button_secondary = c.secondary;
        c.button_secondary_foreground = c.secondary_foreground;
        c.button_secondary_hover = c.secondary_hover;
        c.button_secondary_active = pressed_color();
        c.accent = hover_color();
        c.accent_foreground = rgb(p.text).into();
        c.muted = rgb(p.background).into();
        c.muted_foreground = rgb(p.muted).into();
        c.ring = rgb(p.focus).into();
        c.caret = rgb(p.accent).into();
        c.selection = rgb(p.markdown_colors[24]).into();
        c.popover = rgb(p.surface_raised).into();
        c.popover_foreground = rgb(p.text).into();
        c.list = rgb(p.surface).into();
        c.list_hover = hover_color();
        c.list_active = selection_color();
        c.list_active_border = rgb(p.accent).into();
        c.sidebar = rgb(p.background).into();
        c.sidebar_foreground = rgb(p.text).into();
        c.sidebar_border = rgb(p.border).into();
        c.sidebar_accent = selection_color();
        c.sidebar_accent_foreground = rgb(p.text).into();
        c.tab = rgb(p.background).into();
        c.tab_active = rgb(p.surface).into();
        c.danger = rgb(p.danger).into();
        c.danger_hover = rgb(blend(p.danger, p.text, 0.08)).into();
        c.danger_active = rgb(blend(p.danger, p.text, 0.16)).into();
        c.danger_foreground = rgb(if theme_contrast(p.danger, 0xFFFFFF) >= 4.5 {
            0xFFFFFF
        } else {
            0x000000
        })
        .into();
        c.warning = rgb(p.warning).into();
        c.warning_hover = rgb(blend(p.warning, p.text, 0.08)).into();
        c.warning_active = rgb(blend(p.warning, p.text, 0.16)).into();
        c.warning_foreground = rgb(if theme_contrast(p.warning, 0xFFFFFF) >= 4.5 {
            0xFFFFFF
        } else {
            0x000000
        })
        .into();
        c.success = rgb(p.success).into();
        c.success_hover = rgb(blend(p.success, p.text, 0.08)).into();
        c.success_active = rgb(blend(p.success, p.text, 0.16)).into();
        c.success_foreground = rgb(if theme_contrast(p.success, 0xFFFFFF) >= 4.5 {
            0xFFFFFF
        } else {
            0x000000
        })
        .into();
        c.button_danger = c.danger;
        c.button_danger_hover = c.danger_hover;
        c.button_danger_active = c.danger_active;
        c.button_danger_foreground = c.danger_foreground;
        c.button_warning = c.warning;
        c.button_warning_hover = c.warning_hover;
        c.button_warning_active = c.warning_active;
        c.button_warning_foreground = c.warning_foreground;
        c.button_success = c.success;
        c.button_success_hover = c.success_hover;
        c.button_success_active = c.success_active;
        c.button_success_foreground = c.success_foreground;
        c.link = rgb(p.accent).into();
        c.link_hover = c.link;
        c.group_box = rgb(p.surface).into();
        c.group_box_foreground = rgb(p.text).into();
    });
    p
}

#[cfg(test)]
mod tests {
    use super::resolve;
    use studyapp_core::{AppearanceSettings, ThemeMode};
    #[cfg(feature = "ui-test")]
    #[gpui_kit::test]
    fn ordinary_hover_press_and_selection_have_distinct_neutral_surfaces(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        for dark in [false, true] {
            cx.update(|cx| {
                super::apply(&AppearanceSettings::default(), dark, cx);
                let theme = gpui_kit::component::Theme::global(cx);
                assert_ne!(
                    theme.colors.button_hover, theme.colors.button_active,
                    "pressed state must differ from hover"
                );
                assert_ne!(
                    theme.colors.button_hover, theme.colors.secondary_active,
                    "selection must differ from hover"
                );
                assert_ne!(
                    theme.colors.button_active, theme.colors.secondary_active,
                    "selection must differ from press"
                );
                assert_ne!(
                    theme.colors.button_hover, theme.colors.selection,
                    "ordinary hover must not consume the accent selection color"
                );
            });
        }
    }
    #[test]
    fn built_in_selection_is_visible_and_all_labels_remain_readable() {
        for dark in [false, true] {
            let current = resolve(&AppearanceSettings::default(), dark);
            let selection = super::selection_surface(current);
            assert_ne!(selection, current.surface, "selection must remain visible");
            for label in [current.text, current.muted] {
                assert!(studyapp_core::theme_contrast(label, selection) >= 4.5);
            }
        }
    }

    #[test]
    fn system_resolution_and_editor_tokens_follow_same_safe_palette() {
        let settings = AppearanceSettings::default();
        assert!(!resolve(&settings, false).dark);
        let dark = resolve(&settings, true);
        assert!(dark.dark);
        let tokens = dark.editor_tokens();
        assert_eq!(tokens["mode"], "dark");
        assert_eq!(tokens["surface"], "#272A30");
        assert_ne!(tokens["codeBackground"], tokens["surface"]);
        assert_eq!(tokens["codeText"], "#EFF0F3");
        for key in [
            "toolbarBackground",
            "codeBorder",
            "inlineCodeBackground",
            "quoteBackground",
            "tableHeader",
            "grid",
        ] {
            assert!(studyapp_core::parse_theme_color(tokens[key].as_str().unwrap()).is_some());
        }
        assert!(tokens["spacing"].as_f64().unwrap() >= 1.3);
    }
    #[test]
    fn invalid_custom_palette_resolves_without_panic_or_executable_tokens() {
        let mut settings = AppearanceSettings {
            mode: ThemeMode::Custom,
            active_preset: Some("bad".into()),
            ..Default::default()
        };
        settings.custom_presets.push(studyapp_core::ThemePreset {
            id: "bad".into(),
            name: "bad".into(),
            markdown: Default::default(),
            palette: studyapp_core::ThemePalette {
                text: "url(x)".into(),
                ..Default::default()
            },
        });
        assert_eq!(resolve(&settings, false).text, 0x24272D);
        assert!(
            !resolve(&settings, false)
                .editor_tokens()
                .to_string()
                .contains("url(")
        );
    }
    #[test]
    fn semantic_styles_change_resolution_and_protect_derived_text() {
        let mut settings = AppearanceSettings {
            mode: ThemeMode::Custom,
            active_preset: Some("gray".into()),
            ..Default::default()
        };
        settings.custom_presets.push(studyapp_core::ThemePreset {
            id: "gray".into(),
            name: "Gray".into(),
            palette: studyapp_core::ThemePalette {
                background: "#FFFFFF".into(),
                surface: "#FFFFFF".into(),
                text: "#767676".into(),
                accent: "#808080".into(),
            },
            markdown: Default::default(),
        });
        let before = resolve(&settings, false);
        for text in [before.text, before.muted] {
            assert!(studyapp_core::theme_contrast(text, super::selection_surface(before)) >= 4.5);
            for amount in [0.045, 0.12] {
                assert!(
                    studyapp_core::theme_contrast(text, super::interaction_surface(before, amount))
                        >= 4.5
                );
            }
        }
        assert!(studyapp_core::theme_contrast(before.muted, before.background) >= 4.5);
        assert!(studyapp_core::theme_contrast(before.text, before.accent_soft) >= 4.5);
        assert!(studyapp_core::theme_contrast(before.muted, before.accent_soft) >= 4.5);
        assert!(studyapp_core::theme_contrast(before.markdown_colors[2], before.background) >= 4.5);
        settings.custom_presets[0].markdown.layout.code_size = 19;
        let after = resolve(&settings, false);
        assert_ne!(
            before, after,
            "appearance refresh must see a Markdown-only edit"
        );
        assert_eq!(after.editor_tokens()["markdown"]["code_size"], 19);
        assert_eq!(
            after.editor_tokens()["markdown"]["colors"]["body"],
            "#767676"
        );
    }

    #[test]
    fn low_contrast_custom_base_and_markdown_colors_are_preserved() {
        let settings = AppearanceSettings {
            mode: ThemeMode::Custom,
            active_preset: Some("low".into()),
            custom_presets: vec![studyapp_core::ThemePreset {
                id: "low".into(),
                name: "Low".into(),
                palette: studyapp_core::ThemePalette {
                    text: "#EEEEEE".into(),
                    accent: "#FAFAFA".into(),
                    ..Default::default()
                },
                markdown: studyapp_core::MarkdownStyle {
                    colors: [("body".into(), "#FFFFFF".into())].into(),
                    ..Default::default()
                },
            }],
            ..Default::default()
        };
        let resolved = resolve(&settings, false);
        assert_eq!(resolved.text, 0xEEEEEE);
        assert_eq!(resolved.accent, 0xFAFAFA);
        assert_eq!(resolved.markdown_colors[0], 0xFFFFFF);
        assert!(
            !settings.custom_presets[0]
                .palette
                .contrast_warnings()
                .is_empty()
        );
        assert!(
            !settings.custom_presets[0]
                .markdown
                .contrast_warnings(&settings.custom_presets[0].palette)
                .is_empty()
        );
    }

    #[test]
    fn rgba_uses_composited_native_surfaces_and_exact_editor_overrides() {
        let settings: AppearanceSettings = serde_json::from_value(serde_json::json!({
            "mode":"custom","active_preset":"alpha","custom_presets":[{
                "id":"alpha","name":"Alpha",
                "palette":{"background":"#00000000","surface":"#00000080","text":"#FFFFFF80","accent":"#FF000080"},
                "markdown":{"colors":{"quote_background":"#FF000080","selection_background":"#33669966","selection_text":"#FFFFFF00"}}
            }]
        })).unwrap();
        settings.validate().unwrap();
        let resolved = resolve(&settings, false);
        let expected = settings.custom_presets[0].palette.composited(0xF4F5F7);
        assert_eq!(
            [
                resolved.background,
                resolved.surface,
                resolved.text,
                resolved.accent
            ],
            expected
        );
        assert!(resolved.surface <= 0xFFFFFF);
        let tokens = resolved.editor_tokens();
        assert_eq!(
            tokens["markdown"]["colors"]["quote_background"],
            "#FF000080"
        );
        assert_eq!(tokens["markdown"]["colors"]["body"], "#FFFFFF80");
        assert_eq!(
            tokens["markdown"]["colors"]["selection_background"],
            "#33669966"
        );
        assert_eq!(tokens["markdown"]["colors"]["selection_text"], "#FFFFFF00");
    }

    #[test]
    fn semantic_tones_and_chart_steps_remain_distinct_and_readable() {
        use crate::components::Tone;
        let mut palettes = vec![
            resolve(&AppearanceSettings::default(), false),
            resolve(&AppearanceSettings::default(), true),
        ];
        for value in ["#767676", "#FFFFFF"] {
            palettes.push(resolve(
                &AppearanceSettings {
                    mode: ThemeMode::Custom,
                    active_preset: Some("gray".into()),
                    custom_presets: vec![studyapp_core::ThemePreset {
                        id: "gray".into(),
                        name: "Gray".into(),
                        palette: studyapp_core::ThemePalette {
                            text: value.into(),
                            accent: "#FFFFFF".into(),
                            background: "#FFFFFF".into(),
                            surface: "#FFFFFF".into(),
                        },
                        markdown: Default::default(),
                    }],
                    ..Default::default()
                },
                false,
            ));
        }
        for palette in &palettes {
            for tone in [
                Tone::Neutral,
                Tone::Success,
                Tone::Warning,
                Tone::Danger,
                Tone::Accent,
            ] {
                let (foreground, background) = tone.colors(*palette);
                assert!(
                    studyapp_core::theme_contrast(foreground, background) >= 4.5,
                    "{tone:?}"
                );
            }
            for color in [
                palette.muted,
                palette.success,
                palette.warning,
                palette.danger,
            ] {
                assert!(studyapp_core::theme_contrast(color, palette.surface) >= 4.5);
            }
            assert_eq!(
                palette
                    .chart
                    .into_iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len(),
                4
            );
        }
        assert_ne!(palettes[0].chart, palettes[1].chart);
        assert_ne!(palettes[0].surface, palettes[0].inset);
        assert_ne!(palettes[1].surface, palettes[1].surface_raised);
    }
}

#[cfg(test)]
mod table_defaults_tests {
    #[test]
    fn global_table_defaults_are_independent_of_builtin_and_custom_themes() {
        use studyapp_core::{
            AppearanceSettings, TableAlignment, TableDefaults, TableSizing, ThemeMode, ThemePreset,
        };
        let defaults = TableDefaults {
            sizing: TableSizing::Adaptive,
            alignment: TableAlignment::Center,
        };
        let mut settings = AppearanceSettings {
            table_defaults: defaults,
            ..Default::default()
        };
        for mode in [
            ThemeMode::System,
            ThemeMode::Light,
            ThemeMode::Dark,
            ThemeMode::Custom,
        ] {
            settings.mode = mode;
            if mode == ThemeMode::Custom {
                settings.active_preset = Some("table-theme".into());
                settings.custom_presets = vec![ThemePreset {
                    id: "table-theme".into(),
                    name: "Table theme".into(),
                    palette: Default::default(),
                    markdown: Default::default(),
                }];
            }
            let theme = super::resolve(&settings, false);
            assert_eq!(theme.table_defaults, defaults);
            let tokens = theme.editor_tokens();
            assert_eq!(tokens["tableDefaults"]["sizing"], "adaptive");
            assert_eq!(tokens["tableDefaults"]["alignment"], "center");
        }
    }
}

#[cfg(test)]
mod list_style_tests {
    #[test]
    fn global_list_markers_follow_all_theme_modes_and_default_reset() {
        use studyapp_core::{AppearanceSettings, ListMarker, ThemeMode};
        let mut settings = AppearanceSettings::default();
        settings.list_style.levels[2] = Some(ListMarker::Star);
        for mode in [
            ThemeMode::System,
            ThemeMode::Light,
            ThemeMode::Dark,
            ThemeMode::Custom,
        ] {
            settings.mode = mode;
            let theme = super::resolve(&settings, false);
            assert_eq!(theme.list_markers[2], ListMarker::Star);
            assert_eq!(theme.editor_tokens()["listMarkers"][2], "star");
        }
        settings.list_style = Default::default();
        assert_eq!(
            super::resolve(&settings, true).list_markers[2],
            ListMarker::Triangle
        );
    }
}
