use studyapp_core::{AppearanceSettings, Command, Engine, ListMarker, ListPreset, ListStyle};

#[test]
fn list_presets_and_level_overrides_are_bounded_and_repeat_without_source_state() {
    for preset in ListPreset::ALL {
        let style = ListStyle {
            preset,
            ..Default::default()
        };
        assert_ne!(style.marker(1), style.marker(2));
        assert_ne!(style.marker(2), style.marker(3));
        for depth in 1..=128 {
            assert_eq!(style.marker(depth), style.marker(depth + 8));
        }
        assert_eq!(
            serde_json::from_str::<ListStyle>(&serde_json::to_string(&style).unwrap()).unwrap(),
            style
        );
    }
    let mut style = ListStyle::default();
    style.levels[2] = Some(ListMarker::Star);
    assert_eq!(style.marker(3), ListMarker::Star);
    assert_eq!(style.marker(11), ListMarker::Star);
    assert_eq!(style.marker(1), ListMarker::Disc);
    assert_eq!(style.marker(0), ListMarker::Disc);
    style.levels[2] = None;
    assert_eq!(style.marker(3), ListMarker::Triangle);
}

#[test]
fn list_styles_reject_unbounded_or_executable_values_and_default_legacy_settings() {
    let legacy: AppearanceSettings = serde_json::from_str("{}").unwrap();
    assert_eq!(legacy.list_style, ListStyle::default());
    for bad in [
        r#"{"preset":"custom-css"}"#,
        r#"{"levels":["<script>"]}"#,
        r#"{"levels":[null,null,null,null,null,null,null,null,null]}"#,
        r#"{"style":"url(https://invalid.example)"}"#,
        r#"{"levels":["__proto__",null,null,null,null,null,null,null]}"#,
    ] {
        assert!(serde_json::from_str::<ListStyle>(bad).is_err(), "{bad}");
    }
}

#[test]
fn list_styles_persist_separately_from_theme_and_other_settings() {
    let directory = tempfile::tempdir().unwrap().keep();
    let database = directory.join("lists.sqlite");
    let mut engine = Engine::open(&database, 1000).unwrap();
    let mut settings = engine.snapshot().settings;
    settings.appearance.list_style = ListStyle {
        preset: ListPreset::Diamonds,
        ..Default::default()
    };
    settings.appearance.list_style.levels[7] = Some(ListMarker::Plus);
    let original = settings.clone();
    engine
        .apply(Command::SaveSettings(Box::new(settings)), 1001)
        .unwrap();
    engine.apply(Command::SetShowFilePaths(true), 1002).unwrap();
    drop(engine);
    let engine = Engine::open(&database, 1003).unwrap();
    let actual = engine.snapshot().settings;
    assert_eq!(actual.appearance, original.appearance);
    assert_eq!(actual.interval_minutes, original.interval_minutes);
    assert!(actual.show_file_paths);
}
