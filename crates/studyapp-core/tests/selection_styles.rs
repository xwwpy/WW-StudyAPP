use studyapp_core::{MarkdownStyle, ThemePalette, theme_contrast};

#[test]
fn selection_defaults_are_distinct_and_readable_in_light_and_dark() {
    for palette in [
        ThemePalette::default(),
        ThemePalette {
            background: "#1D1F23".into(),
            surface: "#272A30".into(),
            text: "#EFF0F3".into(),
            accent: "#A7BCD2".into(),
        },
    ] {
        let colors = MarkdownStyle::default().resolved_colors(&palette);
        let surface = studyapp_core::parse_theme_color(&palette.surface).unwrap();
        assert!(theme_contrast(colors[24], surface) >= 1.5);
        assert!(theme_contrast(colors[24], colors[25]) >= 4.5);
    }
}

#[test]
fn explicit_selection_styles_roundtrip_and_low_contrast_is_advisory() {
    let mut style = MarkdownStyle::default();
    style
        .colors
        .insert("selection_background".into(), "#FFFFFF".into());
    style
        .colors
        .insert("selection_text".into(), "#FEFEFE".into());
    style.validate(&ThemePalette::default()).unwrap();
    let restored: MarkdownStyle =
        serde_json::from_str(&serde_json::to_string(&style).unwrap()).unwrap();
    assert_eq!(restored, style);
    let colors = restored.resolved_colors(&ThemePalette::default());
    assert_eq!(colors[24..], [0xFFFFFF, 0xFEFEFE]);
    let warnings = restored.contrast_warnings(&ThemePalette::default());
    assert!(
        warnings
            .iter()
            .any(|warning| warning.foreground == "selection_background")
    );
    assert!(
        warnings
            .iter()
            .any(|warning| warning.foreground == "selection_text")
    );
    style
        .colors
        .insert("selection_background".into(), "red;url(evil)".into());
    assert!(style.validate(&ThemePalette::default()).is_err());
}
