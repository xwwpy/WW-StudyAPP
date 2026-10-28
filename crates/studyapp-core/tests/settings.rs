use serde_json::{Value, json};
use studyapp_core::{Command, Engine, LayoutSettings, Settings};

#[test]
fn legacy_settings_supply_system_appearance_without_disk_migration() {
    let settings: Settings = serde_json::from_value(json!({
        "interval_minutes": 45, "answer_minutes": 3, "review_days": [1, 5]
    }))
    .unwrap();
    let value = serde_json::to_value(settings).unwrap();
    assert_eq!(value["appearance"]["mode"], "system");
    assert_eq!(value["appearance"]["font_size"], 14);
}

#[test]
fn markdown_styles_load_legacy_presets_and_validate_actual_surfaces() {
    let legacy = json!({"mode":"custom","active_preset":"paper","custom_presets":[{"id":"paper","name":"Paper","palette":{"accent":"#2478D4","background":"#ffffff","surface":"#ffffff","text":"#24272D"}}]});
    let settings: studyapp_core::AppearanceSettings =
        serde_json::from_value(legacy.clone()).unwrap();
    assert!(settings.validate().is_ok());
    assert_eq!(
        serde_json::to_value(&settings).unwrap()["custom_presets"][0]["markdown"]["reading_width"],
        720
    );
    for markdown in [
        json!({"colors":{"unknown":"#202020"}}),
        json!({"code_size":50}),
        json!({"reading_width":100}),
    ] {
        let mut bad = legacy.clone();
        bad["custom_presets"][0]["markdown"] = markdown;
        let parsed: studyapp_core::AppearanceSettings = serde_json::from_value(bad).unwrap();
        assert!(parsed.validate().is_err());
    }
    let mut styled = legacy;
    styled["custom_presets"][0]["markdown"] = json!({"body_font":"serif","code_size":18,"reading_width":740,"table_stripes":true,"colors":{"heading":"#234567","code":"#102030","code_background":"#ffffff"}});
    let parsed: studyapp_core::AppearanceSettings = serde_json::from_value(styled).unwrap();
    assert!(parsed.validate().is_ok());
    assert_eq!(
        serde_json::from_value::<studyapp_core::AppearanceSettings>(
            serde_json::to_value(&parsed).unwrap()
        )
        .unwrap(),
        parsed
    );
}

#[test]
fn legacy_midgray_text_on_black_and_white_surfaces_remains_valid() {
    let palette = studyapp_core::ThemePalette {
        background: "#FFFFFF".into(),
        surface: "#000000".into(),
        text: "#757575".into(),
        accent: "#808080".into(),
    };
    assert!(palette.validate().is_ok());
    let style = studyapp_core::MarkdownStyle::default();
    assert!(style.validate(&palette).is_ok());
    for index in [0, 2, 3, 12] {
        let value = style.resolved_colors(&palette)[index];
        assert!(studyapp_core::theme_contrast(value, 0) >= 4.5);
        assert!(studyapp_core::theme_contrast(value, 0xFFFFFF) >= 4.5);
    }
}

#[test]
fn unsafe_custom_appearance_cannot_be_saved() {
    let directory = tempfile::tempdir().unwrap().keep();
    let mut engine = Engine::open(&directory.join("appearance.sqlite"), 1000).unwrap();
    let original = engine.snapshot().settings;
    for palette in [
        json!({"accent":"red;url(javascript:x)","background":"#ffffff","surface":"#ffffff","text":"#202020"}),
        json!({"accent":"#586D83","background":"#ffffff","surface":"#ffffff","text":"#12345"}),
    ] {
        let mut proposed = serde_json::to_value(&original).unwrap();
        proposed["appearance"] = json!({"mode":"custom","active_preset":"unsafe","custom_presets":[{"id":"unsafe","name":"Unsafe","palette":palette}],"font_size":14,"spacing":8});
        let proposed = serde_json::from_value(proposed).unwrap();
        assert!(engine.apply(Command::SaveSettings(proposed), 1001).is_err());
        assert_eq!(engine.snapshot().settings, original);
    }
}

#[test]
fn low_contrast_colors_save_and_resolve_exactly_as_requested() {
    let directory = tempfile::tempdir().unwrap().keep();
    let mut engine = Engine::open(&directory.join("low-contrast.sqlite"), 1000).unwrap();
    let mut proposed = serde_json::to_value(engine.snapshot().settings).unwrap();
    proposed["appearance"] = json!({"mode":"custom","active_preset":"quiet","custom_presets":[{"id":"quiet","name":"Quiet","palette":{"accent":"#EEEEEE","background":"#FFFFFF","surface":"#FFFFFF","text":"#FFFFFF"},"markdown":{"reading_width":920,"colors":{"quote":"#FFFFFF","quote_background":"#FFFFFF","syntax_keyword":"#EEEEEE","code_background":"#FFFFFF"}}}]});
    let proposed: Settings = serde_json::from_value(proposed).unwrap();
    engine
        .apply(Command::SaveSettings(proposed.clone()), 1001)
        .unwrap();
    assert_eq!(engine.snapshot().settings, proposed);
    let preset = &proposed.appearance.custom_presets[0];
    let colors = preset.markdown.resolved_colors(&preset.palette);
    assert_eq!(colors[4], 0xFFFFFF);
    assert_eq!(colors[15], 0xEEEEEE);
    assert_eq!(preset.markdown.layout.reading_width, 920);
}

#[test]
fn custom_theme_presets_round_trip_and_reject_missing_selection_and_duplicates() {
    use studyapp_core::{AppearanceSettings, ThemeMode, ThemePalette, ThemePreset};
    let mut appearance = AppearanceSettings {
        mode: ThemeMode::Custom,
        active_preset: Some("paper".into()),
        custom_presets: vec![ThemePreset {
            id: "paper".into(),
            name: "Paper".into(),
            palette: ThemePalette::default(),
            markdown: Default::default(),
        }],
        font_size: 16,
        spacing: 12,
    };
    assert!(appearance.validate().is_ok());
    assert_eq!(
        serde_json::from_value::<AppearanceSettings>(serde_json::to_value(&appearance).unwrap())
            .unwrap(),
        appearance
    );
    appearance
        .custom_presets
        .push(appearance.custom_presets[0].clone());
    assert!(appearance.validate().is_err());
    appearance.custom_presets.pop();
    appearance.active_preset = Some("missing".into());
    assert!(appearance.validate().is_err());
    appearance.active_preset = Some("paper".into());
    for size in [0, 11, 23, u16::MAX] {
        appearance.font_size = size;
        assert!(appearance.validate().is_err());
    }
}

#[test]
fn legacy_settings_restore_default_workspace_layout() {
    let settings: Settings = serde_json::from_value(json!({
        "interval_minutes": 45, "answer_minutes": 3, "review_days": [1, 5]
    }))
    .unwrap();
    assert_eq!(
        serde_json::to_value(settings).unwrap()["layout"],
        json!({
            "navigation_width": 160, "folders_width": 144, "notes_width": 280,
            "navigation_visible": true, "folders_visible": true, "library_sort": "name_ascending"
        })
    );
}

#[test]
fn workspace_layout_persists_without_changing_learning_or_path_settings() {
    let directory = tempfile::tempdir().unwrap().keep();
    let database = directory.join("layout.sqlite");
    let layout = LayoutSettings {
        navigation_width: 256,
        folders_width: 240,
        notes_width: 320,
        navigation_visible: false,
        folders_visible: true,
        library_sort: studyapp_core::LibrarySort::Newest,
    };
    for layout_first in [false, true] {
        let mut engine = Engine::open(&database, 1000).unwrap();
        let commands = if layout_first {
            vec![
                Command::SaveLayout(layout.clone()),
                learning_update(30, 8, vec![2, 6]),
                Command::SetShowFilePaths(true),
            ]
        } else {
            vec![
                learning_update(30, 8, vec![2, 6]),
                Command::SetShowFilePaths(true),
                Command::SaveLayout(layout.clone()),
            ]
        };
        for command in commands {
            engine.apply(command, 1000).unwrap();
        }
        engine.tick(1001, 1).unwrap();
        let timer = engine.snapshot().timer;
        engine
            .apply(Command::SaveLayout(layout.clone()), 1001)
            .unwrap();
        assert_eq!(engine.snapshot().timer.active_seconds, timer.active_seconds);
        assert_eq!(engine.snapshot().timer.target_seconds, timer.target_seconds);
        drop(engine);
        let restored = Engine::open(&database, 1001).unwrap().snapshot();
        assert_eq!(restored.settings.layout, layout);
        assert_eq!(restored.settings.interval_minutes, 30);
        assert_eq!(restored.settings.answer_minutes, 8);
        assert_eq!(restored.settings.review_days, vec![2, 6]);
        assert!(restored.settings.show_file_paths);
    }
}

#[test]
fn invalid_workspace_widths_are_rejected_without_persisting_changes() {
    let directory = tempfile::tempdir().unwrap().keep();
    let database = directory.join("invalid-layout.sqlite");
    let mut engine = Engine::open(&database, 1000).unwrap();
    let original = engine.snapshot().settings;
    for (navigation_width, folders_width, notes_width) in [
        (127, 176, 240),
        (321, 176, 240),
        (184, 111, 240),
        (184, 361, 240),
        (184, 176, 159),
        (184, 176, 441),
        (u32::MAX, 176, 240),
    ] {
        let layout = LayoutSettings {
            navigation_width,
            folders_width,
            notes_width,
            navigation_visible: false,
            folders_visible: true,
            library_sort: studyapp_core::LibrarySort::Newest,
        };
        assert!(
            engine
                .apply(Command::SaveLayout(layout.clone()), 1001)
                .is_err()
        );
        let mut full = original.clone();
        full.layout = layout;
        assert!(engine.apply(Command::SaveSettings(full), 1001).is_err());
        assert_eq!(engine.snapshot().settings, original);
    }
    drop(engine);
    assert_eq!(
        Engine::open(&database, 1001).unwrap().snapshot().settings,
        original
    );
}

#[test]
fn new_settings_hide_file_paths_by_default() {
    let encoded = serde_json::to_value(Settings::default()).unwrap();
    assert_eq!(encoded["show_file_paths"], false);
}

#[test]
fn legacy_settings_without_path_visibility_default_to_hidden() {
    let legacy = json!({
        "interval_minutes": 45,
        "answer_minutes": 3,
        "review_days": [1, 5, 12]
    });
    let settings: Settings = serde_json::from_value(legacy).unwrap();
    let encoded = serde_json::to_value(settings).unwrap();
    assert_eq!(encoded["show_file_paths"], false);
    assert_eq!(encoded["interval_minutes"], 45);
    assert_eq!(encoded["answer_minutes"], 3);
    assert_eq!(encoded["review_days"], json!([1, 5, 12]));
}

#[test]
fn file_path_visibility_persists_when_enabled_and_disabled() {
    let directory = tempfile::tempdir().unwrap().keep();
    let database = directory.join("learning.sqlite");
    let mut engine = Engine::open(&database, 1000).unwrap();

    for (index, visible) in [true, false].into_iter().enumerate() {
        let now = 1001 + index as i64;
        let mut encoded = serde_json::to_value(engine.snapshot().settings).unwrap();
        encoded["show_file_paths"] = json!(visible);
        let settings = serde_json::from_value(encoded).unwrap();
        engine.apply(Command::SaveSettings(settings), now).unwrap();
        drop(engine);
        engine = Engine::open(&database, now).unwrap();
        let restored = serde_json::to_value(engine.snapshot().settings).unwrap();
        assert_eq!(restored["show_file_paths"], visible);
    }
}

#[test]
fn opening_legacy_database_preserves_settings_and_hides_paths() {
    let directory = tempfile::tempdir().unwrap().keep();
    let database = directory.join("learning.sqlite");
    drop(Engine::open(&database, 1000).unwrap());
    let connection = rusqlite::Connection::open(&database).unwrap();
    let raw: String = connection
        .query_row("SELECT data FROM app_state WHERE id=1", [], |row| {
            row.get(0)
        })
        .unwrap();
    let mut state: Value = serde_json::from_str(&raw).unwrap();
    state["settings"]
        .as_object_mut()
        .unwrap()
        .remove("show_file_paths");
    state["settings"]["answer_minutes"] = json!(8);
    connection
        .execute(
            "UPDATE app_state SET data=?1 WHERE id=1",
            [serde_json::to_string(&state).unwrap()],
        )
        .unwrap();
    drop(connection);

    let engine = Engine::open(&database, 1001).unwrap();
    let restored = serde_json::to_value(engine.snapshot().settings).unwrap();
    assert_eq!(restored["show_file_paths"], false);
    assert_eq!(restored["answer_minutes"], 8);
}

fn learning_update(interval_minutes: u32, answer_minutes: u32, review_days: Vec<u32>) -> Command {
    Command::SaveLearningSettings {
        interval_minutes,
        answer_minutes,
        review_days,
    }
}

fn rapid_updates(learning_first: bool) {
    for initially_visible in [false, true] {
        let directory = tempfile::tempdir().unwrap().keep();
        let database = directory.join("learning.sqlite");
        let mut engine = Engine::open(&database, 1000).unwrap();
        let mut initial = engine.snapshot().settings;
        initial.show_file_paths = initially_visible;
        engine.apply(Command::SaveSettings(initial), 1000).unwrap();
        // Queue both intentions before accepting a refreshed worker snapshot.
        let learning = learning_update(30, 8, vec![2, 6, 15]);
        let visibility = Command::SetShowFilePaths(!initially_visible);
        let commands = if learning_first {
            [learning, visibility]
        } else {
            [visibility, learning]
        };
        for command in commands {
            engine.apply(command, 1001).unwrap();
        }
        let expected = Settings {
            interval_minutes: 30,
            answer_minutes: 8,
            review_days: vec![2, 6, 15],
            show_file_paths: !initially_visible,
            layout: Default::default(),
            appearance: Default::default(),
            images: Default::default(),
        };
        assert_eq!(
            engine.snapshot().settings,
            expected,
            "queued setting intent was lost"
        );
        assert_eq!(engine.snapshot().timer.target_seconds, 30 * 60);
        drop(engine);
        let restored = Engine::open(&database, 1002).unwrap();
        assert_eq!(
            restored.snapshot().settings,
            expected,
            "merged settings did not persist"
        );
    }
}

#[test]
fn rapid_learning_then_visibility_updates_preserve_both_intentions() {
    rapid_updates(true);
}

#[test]
fn rapid_visibility_then_learning_updates_preserve_both_intentions() {
    rapid_updates(false);
}

#[test]
fn invalid_learning_update_preserves_visibility_and_all_saved_fields() {
    let directory = tempfile::tempdir().unwrap().keep();
    let database = directory.join("learning.sqlite");
    let mut engine = Engine::open(&database, 1000).unwrap();
    engine.apply(Command::SetShowFilePaths(true), 1001).unwrap();
    let saved = engine.snapshot().settings;
    for (interval, answer, days) in [(4, 5, vec![1]), (60, 31, vec![1]), (60, 5, vec![0])] {
        assert!(
            engine
                .apply(learning_update(interval, answer, days), 1002)
                .is_err()
        );
        assert_eq!(engine.snapshot().settings, saved);
        assert_eq!(engine.snapshot().timer.target_seconds, 3600);
    }
    drop(engine);
    assert_eq!(
        Engine::open(&database, 1003).unwrap().snapshot().settings,
        saved
    );
}

#[test]
fn learning_update_preserves_current_timer_and_changes_the_next_cycle() {
    let directory = tempfile::tempdir().unwrap().keep();
    let mut engine = Engine::open(&directory.join("learning.sqlite"), 1000).unwrap();
    engine.apply(learning_update(5, 5, vec![1]), 1000).unwrap();
    engine.tick(1001, 1).unwrap();
    engine.apply(learning_update(10, 2, vec![2]), 1001).unwrap();
    engine.apply(Command::SetShowFilePaths(true), 1001).unwrap();
    assert_eq!(engine.snapshot().timer.active_seconds, 1);
    assert_eq!(engine.snapshot().timer.target_seconds, 300);
    for now in (1011..=1301).step_by(10) {
        engine.tick(now, 10).unwrap();
    }
    let snapshot = engine.snapshot();
    assert_eq!(snapshot.timer.active_seconds, 1);
    assert_eq!(snapshot.timer.target_seconds, 600);
    assert_eq!(snapshot.sessions.len(), 1);
    assert_eq!(snapshot.sessions[0].ended_at, 1300);
    assert_eq!(snapshot.sessions[0].deadline, 1420);
    assert!(snapshot.settings.show_file_paths);
}
