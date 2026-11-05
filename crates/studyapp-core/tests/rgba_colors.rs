use studyapp_core::{MarkdownStyle, RgbaColor, ThemePalette, parse_rgba_color};

#[test]
fn actual_surface_composition_and_transparent_text_drive_warnings() {
    let palette = ThemePalette {
        background: "#00000080".into(),
        surface: "#FFFFFF80".into(),
        text: "#00000000".into(),
        accent: "#FF000080".into(),
    };
    palette.validate().unwrap();
    let [page, card, text, accent] = palette.composited(0xFFFFFF);
    assert_eq!(page, 0x7F7F7F);
    assert_eq!(card, 0xBFBFBF);
    assert_eq!(text, card);
    assert_eq!(accent, RgbaColor::new(0xFF0000, 128).over(card));
    let warnings = palette.contrast_warnings();
    assert!(
        warnings
            .iter()
            .filter(|warning| warning.foreground == "文字")
            .all(|warning| warning.ratio == 1.)
    );
    let style = MarkdownStyle::default();
    let colors = style.resolved_rgba_colors(&palette);
    assert_eq!(colors[0], RgbaColor::new(0, 0));
    assert!(
        style
            .contrast_warnings(&palette)
            .iter()
            .any(|warning| warning.foreground == "body" && warning.ratio == 1.)
    );
}

#[test]
fn markdown_and_selection_rgba_are_exact_but_native_rgb_is_explicitly_composited() {
    let mut style = MarkdownStyle::default();
    for (key, value) in [
        ("body", "#00000000"),
        ("quote_background", "#FF000080"),
        ("quote", "#0000FF80"),
        ("selection_background", "#11223380"),
        ("selection_text", "#FFFFFF00"),
    ] {
        style.colors.insert(key.into(), value.into());
    }
    let palette = ThemePalette::default();
    style.validate(&palette).unwrap();
    let rgba = style.resolved_rgba_colors(&palette);
    assert_eq!(rgba[5], parse_rgba_color("#FF000080").unwrap());
    assert_eq!(rgba[25], parse_rgba_color("#FFFFFF00").unwrap());
    let rgb = style.resolved_colors(&palette);
    assert_eq!(rgb[0], 0xFFFFFF);
    assert_eq!(rgb[5], 0xFF7F7F);
    assert_eq!(rgb[4], rgba[4].over(rgb[5]));
    assert_eq!(rgb[25], rgb[24]);
    assert!(
        style
            .contrast_warnings(&palette)
            .iter()
            .any(|warning| warning.foreground == "selection_text" && warning.ratio == 1.)
    );
}

#[test]
fn opaque_legacy_parser_does_not_accept_or_truncate_rgba() {
    assert_eq!(studyapp_core::parse_theme_color("#123456"), Some(0x123456));
    assert_eq!(studyapp_core::parse_theme_color("#12345678"), None);
    assert_eq!(parse_rgba_color("#12345678").unwrap().rgb(), 0x123456);
}
