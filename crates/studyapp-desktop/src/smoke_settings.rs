//! Settings categories and recorded-key native-window regressions.
use crate::{
    app::{Page, StudyApp},
    settings_ui::{AppearancePage, SettingsSection, StoragePage},
    smoke_library::{click_scrolled, reveal_scrolled, settled, until},
};
use gpui_kit::{test::TestWindowExt, *};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

// Stay inside the production minimum and the known hosted-runner client area.
// Wider content/shorter panes are covered by the safe GPUI surface regressions.
const PREVIEW_VIEWPORTS: [(f32, f32); 4] =
    [(1000., 700.), (1024., 700.), (1000., 720.), (1024., 720.)];
const LIVE_PREVIEW_VIEWPORT: (f32, f32) = (1000., 700.);

fn settings_files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn read(root: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(root).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                read(&path, result);
            } else {
                result.insert(path.clone(), std::fs::read(path).unwrap());
            }
        }
    }
    let mut result = BTreeMap::new();
    read(root, &mut result);
    result
}

async fn exercise_save_shortcut(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    database: &Path,
    cx: &mut AsyncApp,
) {
    let original = window
        .update(cx, |_, window, cx| {
            window.click(("nav", 4_usize), cx);
            window.click("settings-section-learning", cx);
            let original = view.read(cx).interval.read(cx).value();
            let next = if original.as_str() == "35" {
                "36"
            } else {
                "35"
            };
            let input = view.read(cx).interval.clone();
            input.update(cx, |input, cx| input.set_value(next, window, cx));
            input.read(cx).focus_handle(cx).focus(window, cx);
            window.press("ctrl-s", cx);
            assert!(
                view.read(cx).preferences_ui.busy,
                "focused input Ctrl+S did not save preferences"
            );
            original
        })
        .unwrap();
    until(
        window,
        view,
        "settings keyboard save",
        |app, cx| !app.preferences_ui.busy && !app.settings_dirty(cx),
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            let root = database.parent().unwrap().join("settings");
            let before = settings_files(&root);
            let revision = view
                .read(cx)
                .preferences_ui
                .saved
                .as_ref()
                .unwrap()
                .revision
                .clone();
            view.update(cx, |app, cx| app.clear_notices(cx));
            for _ in 0..4 {
                window.press("ctrl-s", cx);
            }
            assert!(!view.read(cx).preferences_ui.busy);
            assert!(
                view.read(cx).notice.is_none(),
                "unchanged Ctrl+S produced a success notice"
            );
            assert_eq!(
                view.read(cx)
                    .preferences_ui
                    .saved
                    .as_ref()
                    .unwrap()
                    .revision,
                revision
            );
            assert_eq!(
                settings_files(&root),
                before,
                "unchanged Ctrl+S wrote preferences"
            );
            let input = view.read(cx).interval.clone();
            input.update(cx, |input, cx| input.set_value(original, window, cx));
            input.read(cx).focus_handle(cx).focus(window, cx);
            window.press("ctrl-s", cx);
        })
        .unwrap();
    until(
        window,
        view,
        "restore keyboard-saved settings",
        |app, cx| !app.preferences_ui.busy && !app.settings_dirty(cx),
        cx,
    )
    .await;
}

fn exercise_color_controls(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    database: &Path,
    cx: &mut AsyncApp,
) {
    let files_root = database.parent().unwrap().join("settings");
    let files = settings_files(&files_root);
    let (accent, original_accent, body, original_body, touched, control) = window
        .update(cx, |_, window, cx| {
            window.click("appearance-theme-editor", cx);
            window.render_frame(cx);
            assert!(
                window.try_find("color-value-accent").is_none(),
                "HEX input must stay inside its popup"
            );
            let app = view.read(cx);
            let accent = app.preferences_ui.appearance_fields["accent"].clone();
            let original_accent = accent.read(cx).value();
            let body = app.preferences_ui.appearance_fields["md-body"].clone();
            let original_body = body.read(cx).value();
            let touched = app.preferences_ui.appearance_touched;
            let control = app.preferences_ui.appearance_colors["accent"].clone();
            assert!(
                control.read(cx).picker_id().is_none(),
                "closed color fields allocated their popup state"
            );
            click_scrolled(
                window,
                "color-open-accent".into(),
                "settings-section-scroll",
                cx,
            );
            (
                accent,
                original_accent,
                body,
                original_body,
                touched,
                control,
            )
        })
        .unwrap();
    let picker = window
        .update(cx, |_, window, cx| {
            assert!(control.read(cx).popup_open());
            assert!(window.try_find("color-value-accent").is_some());
            let picker = control
                .read(cx)
                .picker_id()
                .expect("open picker was not allocated");
            window.render_frame(cx);
            window
                .render_to_image()
                .unwrap()
                .save(database.with_file_name("settings-color-picker.png"))
                .unwrap();
            window.click("color-swatch-accent-3", cx);
            picker
        })
        .unwrap();
    // Selection emits a component event. End each update before checking its
    // subscriber, exactly as real native event delivery does.
    window
        .update(cx, |_, window, cx| {
            assert_eq!(accent.read(cx).value().as_str(), "#EF4444");
            assert!(!control.read(cx).popup_open());
            assert!(view.read(cx).preferences_ui.appearance_touched);
            window.click("color-open-accent", cx);
            assert_eq!(
                control.read(cx).picker_id(),
                Some(picker),
                "reopening recreated the picker"
            );
            window.click("color-close-accent", cx);
            window.click("settings-section-learning", cx);
        })
        .unwrap();
    window
        .update(cx, |_, window, cx| {
            assert!(!control.read(cx).popup_open());
            assert!(view.read(cx).preferences_ui.section == SettingsSection::Learning);
            assert_eq!(
                accent.read(cx).value().as_str(),
                "#EF4444",
                "category switch discarded color draft"
            );
            window.click("settings-section-appearance", cx);
            assert!(view.read(cx).preferences_ui.section == SettingsSection::Appearance);
            assert_eq!(control.read(cx).picker_id(), Some(picker));
            click_scrolled(
                window,
                "color-open-md-body".into(),
                "settings-section-scroll",
                cx,
            );
        })
        .unwrap();
    window
        .update(cx, |_, window, cx| {
            window.click("color-swatch-md-body-0", cx)
        })
        .unwrap();
    window
        .update(cx, |_, window, cx| {
            assert_eq!(body.read(cx).value().as_str(), "#233631");
            window.click("color-auto-md-body", cx);
        })
        .unwrap();
    window
        .update(cx, |_, window, cx| {
            assert!(
                body.read(cx).value().is_empty(),
                "automatic color did not restore inherited configuration"
            );
            assert_eq!(
                settings_files(&files_root),
                files,
                "picker persisted settings before Save"
            );
            accent.update(cx, |input, cx| input.set_value(original_accent, window, cx));
            body.update(cx, |input, cx| input.set_value(original_body, window, cx));
            view.update(cx, |app, cx| {
                app.preferences_ui.appearance_touched = touched;
                cx.notify();
            });
            window.render_frame(cx);
        })
        .unwrap();
}

fn timing_summary(mut samples: Vec<u64>) -> serde_json::Value {
    samples.sort_unstable();
    let count = samples.len();
    serde_json::json!({
        "count": count,
        "p50_us": samples[count.div_ceil(2) - 1],
        "p95_us": samples[(count * 95).div_ceil(100) - 1],
        "samples_us_sorted": samples,
    })
}

fn capture_task_pages(window: &mut Window, view: &Entity<StudyApp>, database: &Path, cx: &mut App) {
    let (section, appearance, storage) = {
        let app = view.read(cx);
        (
            app.preferences_ui.section,
            app.preferences_ui.appearance_page,
            app.preferences_ui.storage_page,
        )
    };
    for (category, id, label) in [
        ("appearance", "appearance-overview", "appearance-overview"),
        ("appearance", "appearance-theme-editor", "theme-editor"),
        ("appearance", "appearance-workspace", "workspace"),
        ("storage", "storage-overview", "storage-overview"),
        ("storage", "storage-copies", "storage-copies"),
        ("storage", "storage-images", "image-storage"),
        ("storage", "storage-library", "library-storage"),
    ] {
        window.click(
            SharedString::from(format!("settings-section-{category}")),
            cx,
        );
        window.click(id, cx);
        window.render_frame(cx);
        match id {
            "appearance-overview" => {
                assert!(window.try_find("theme-mode-system").is_some());
                assert!(window.try_find("color-open-accent").is_none());
            }
            "appearance-theme-editor" => {
                assert!(
                    view.read(cx).preferences_ui.appearance_page == AppearancePage::ThemeEditor
                );
                assert!(window.try_find("color-value-accent").is_none());
            }
            "appearance-workspace" => {
                assert!(window.try_find("show-file-paths").is_some());
                assert!(window.try_find("theme-mode-system").is_none());
            }
            "storage-overview" => {
                assert!(view.read(cx).preferences_ui.storage_page == StoragePage::Overview);
                assert!(window.try_find("refresh-storage").is_some());
                assert!(window.try_find("storage-copy-list").is_none());
            }
            "storage-copies" => {
                assert!(view.read(cx).preferences_ui.storage_page == StoragePage::Copies);
                assert!(window.try_find(("storage-category", 0_usize)).is_none());
            }
            "storage-images" => {
                assert!(window.try_find("images-storage-local").is_some());
                assert!(window.try_find("refresh-storage").is_none());
            }
            "storage-library" => {
                assert!(window.try_find("migrate-library").is_some());
                assert!(window.try_find("images-storage-local").is_none());
            }
            _ => unreachable!(),
        }
        window
            .render_to_image()
            .unwrap()
            .save(database.with_file_name(format!(
                "settings-{label}-{}.png",
                f32::from(window.viewport_size().width) as u32
            )))
            .unwrap();
    }
    view.update(cx, |app, cx| {
        app.select_settings_section(section, cx);
        app.select_appearance_page(appearance, cx);
        app.select_storage_page(storage, cx);
    });
    window.render_frame(cx);
}

/// Same-process A/B on the actual row builder. Event measurements end after
/// GPUI's refresh/draw/clear, not compositor presentation or OS input delivery.
fn measure_settings(window: &mut Window, view: &Entity<StudyApp>, database: &Path, cx: &mut App) {
    let mut results = Vec::new();
    for (section, category, eager) in [
        (SettingsSection::Shortcuts, "shortcuts_eager_control", true),
        (SettingsSection::Shortcuts, "shortcuts_virtual", false),
        (SettingsSection::Learning, "learning", false),
        (
            SettingsSection::Appearance,
            "appearance_eager_control",
            true,
        ),
        (
            SettingsSection::Appearance,
            "appearance_virtual_cached",
            false,
        ),
    ] {
        eprintln!("settings timing: {category}");
        view.update(cx, |app, cx| {
            app.select_settings_section(section, cx);
            if section == SettingsSection::Appearance {
                app.select_appearance_page(AppearancePage::ThemeEditor, cx);
                app.preferences_ui.appearance_preview_mode = false;
            }
            app.preferences_ui.section_changed_at = None;
            app.preferences_ui.shortcut_eager_baseline = eager;
            app.preferences_ui.appearance_eager_baseline = eager;
            app.reset_shortcut_scroll();
            cx.notify();
        });
        for _ in 0..if eager { 1 } else { 3 } {
            window.render_frame(cx);
        }
        let input = match section {
            SettingsSection::Shortcuts => view
                .read(cx)
                .preferences_ui
                .shortcut_search
                .as_ref()
                .unwrap()
                .clone(),
            SettingsSection::Learning => view.read(cx).interval.clone(),
            SettingsSection::Appearance => {
                view.read(cx).preferences_ui.appearance_fields["name"].clone()
            }
            _ => unreachable!(),
        };
        let original = input.read(cx).value();
        let appearance_touched = view.read(cx).preferences_ui.appearance_touched;
        let mut frames = Vec::new();
        let mut builds = Vec::new();
        let mut distinct = Vec::new();
        let work = |app: &StudyApp| {
            if section == SettingsSection::Appearance {
                app.appearance_render_work()
            } else {
                app.shortcut_render_work()
            }
        };
        // The deliberately eager debug control can take seconds per frame on
        // hosted software rendering. Keep a small comparison sample; production
        // virtual/cache paths retain all 20 samples and functional assertions.
        for _ in 0..if eager { 3 } else { 20 } {
            view.update(cx, |_, cx| cx.notify());
            let before = work(view.read(cx)).0;
            let start = std::time::Instant::now();
            window.render_frame(cx);
            frames.push(start.elapsed().as_micros() as u64);
            let (after, rows) = work(view.read(cx));
            builds.push(after - before);
            distinct.push(rows);
        }
        if section == SettingsSection::Shortcuts {
            if eager {
                assert!(
                    distinct
                        .iter()
                        .all(|&rows| rows as usize == crate::shortcuts::SHORTCUT_ACTIONS.len())
                );
            } else {
                assert!(
                    distinct.iter().all(|&rows| rows > 0
                        && (rows as usize) < crate::shortcuts::SHORTCUT_ACTIONS.len()),
                    "shortcut viewport still constructs every row"
                );
            }
        }
        if section == SettingsSection::Appearance {
            assert!(
                distinct.iter().all(|&parts| if eager {
                    parts == 2
                } else {
                    parts > 0 && parts <= 2
                }),
                "theme editor must only build its own two parameter groups; preview is a separate fixed view"
            );
        }
        input.read(cx).focus_handle(cx).focus(window, cx);
        window.press("end", cx);
        window.render_frame(cx);
        let mut inputs = Vec::new();
        let mut input_builds = Vec::new();
        for ix in 0..if eager { 2 } else { 20 } {
            let before = work(view.read(cx)).0;
            let start = std::time::Instant::now();
            let keystroke = if ix % 2 == 0 {
                let mut key = Keystroke::parse("h").unwrap();
                key.key_char = Some("h".into());
                key
            } else {
                Keystroke::parse("backspace").unwrap().with_simulated_ime()
            };
            window.dispatch_keystroke(keystroke.clone(), cx);
            window.dispatch_event(KeyUpEvent { keystroke }.to_platform_input(), cx);
            window.render_frame(cx);
            inputs.push(start.elapsed().as_micros() as u64);
            input_builds.push(work(view.read(cx)).0 - before);
        }
        assert_eq!(
            input.read(cx).value(),
            original,
            "performance input sequence changed the draft"
        );
        // Restore the touched flag: the benchmark types and erases a temporary
        // character, and must not change subsequent save/leave assertions.
        view.update(cx, |app, cx| {
            app.preferences_ui.appearance_touched = appearance_touched;
            app.reset_shortcut_scroll();
            cx.notify();
        });
        window.render_frame(cx);
        let mut wheels = Vec::new();
        let mut wheel_builds = Vec::new();
        let wheel_samples = if eager { 2 } else { 20 };
        for ix in 0..wheel_samples {
            let viewport = window.find("settings-section-scroll").bounds();
            let before = work(view.read(cx)).0;
            let start = std::time::Instant::now();
            window.dispatch_event(
                ScrollWheelEvent {
                    position: point(viewport.left() + px(5.), viewport.center().y),
                    delta: ScrollDelta::Pixels(point(
                        px(0.),
                        px(if ix < wheel_samples / 2 { -100. } else { 100. }),
                    )),
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
            wheels.push(start.elapsed().as_micros() as u64);
            wheel_builds.push(work(view.read(cx)).0 - before);
        }
        results.push(serde_json::json!({
            "category": category,
            "unchanged_render_frame": timing_summary(frames),
            "input_dispatch_and_render": timing_summary(inputs),
            "wheel_dispatch_and_render": timing_summary(wheels),
            "row_or_section_builder_calls_per_unchanged_frame": (section != SettingsSection::Learning).then_some(builds),
            "distinct_rows_or_sections_per_unchanged_frame": (section != SettingsSection::Learning).then_some(distinct),
            "row_or_section_builder_calls_per_input_frame": (section != SettingsSection::Learning).then_some(input_builds),
            "row_or_section_builder_calls_per_wheel_frame": (section != SettingsSection::Learning).then_some(wheel_builds),
        }));
    }
    view.update(cx, |app, cx| {
        app.preferences_ui.shortcut_eager_baseline = false;
        app.preferences_ui.appearance_eager_baseline = false;
        app.select_settings_section(SettingsSection::Shortcuts, cx);
        app.preferences_ui.section_changed_at = None;
        app.reset_shortcut_scroll();
        cx.notify();
    });
    window.render_frame(cx);
    std::fs::write(database.with_file_name("settings-render-timing.json"), serde_json::to_string_pretty(&serde_json::json!({
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "measurement": "GPUI TestWindowExt refresh/draw/clear. Input and wheel include GPUI event dispatch; excludes OS input transit and compositor presentation. Eager and virtual use the same row builder in this binary; this is not a historical executable comparison.",
        "viewport": { "width": f32::from(window.viewport_size().width), "height": f32::from(window.viewport_size().height) },
        "shortcut_actions": crate::shortcuts::SHORTCUT_ACTIONS.len(),
        "results": results,
    })).unwrap()).unwrap();
}

async fn measure_appearance_cache(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    database: &Path,
    cx: &mut AsyncApp,
) {
    let entry = window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.navigate(Page::Settings, window, cx);
                app.select_settings_section(SettingsSection::Appearance, cx);
                app.select_appearance_page(AppearancePage::ThemeEditor, cx);
                app.preferences_ui.appearance_preview_mode = false;
                app.preferences_ui.section_changed_at = None;
                app.preferences_ui.appearance_eager_baseline = false;
                cx.notify();
            });
            window.render_frame(cx);
            appearance_cache_state(view.read(cx), window, cx)
        })
        .unwrap();
    // Page entry updates the cached view key and configures newly mounted rows.
    // GPUI retains their during-draw dirty IDs for the next draw; native host
    // startup also produces asynchronous ready/layout notifications.
    // Establish actual readiness and matching measured/native geometry before
    // measuring parent-only notifications, exactly as the preview smoke does.
    // Keep this initialization evidence separate from all 20 measured samples.
    let started = std::time::Instant::now();
    let mut initialization = vec![serde_json::json!({"phase":"entry", "state":entry})];
    loop {
        let (ready, trace) = window
            .update(cx, |_, window, cx| {
                let before = view.read(cx).appearance_render_work().0;
                window.draw(cx).clear(cx);
                let state = appearance_cache_state(view.read(cx), window, cx);
                (
                    state["readyForMeasurement"] == true,
                    serde_json::json!({
                        "phase":"initialization",
                        "sectionBuilderCalls":view.read(cx).appearance_render_work().0 - before,
                        "state":state,
                    }),
                )
            })
            .unwrap();
        initialization.push(trace);
        std::fs::write(
            database.with_file_name("settings-appearance-cache-initialization.json"),
            serde_json::to_string_pretty(&initialization).unwrap(),
        )
        .unwrap();
        if ready {
            break;
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(12),
            "appearance cache measurement never reached a ready native preview: {initialization:?}"
        );
        cx.background_executor()
            .timer(std::time::Duration::from_millis(40))
            .await;
    }
    let mut samples = Vec::new();
    let mut builds = Vec::new();
    for _ in 0..20 {
        // Flush the parent notification in its own update. A render_frame call
        // would force Window::refresh and deliberately bypass GPUI view caches.
        window
            .update(cx, |_, _, cx| view.update(cx, |_, cx| cx.notify()))
            .unwrap();
        let (micros, count) = window
            .update(cx, |_, window, cx| {
                let before = view.read(cx).appearance_render_work().0;
                let started = std::time::Instant::now();
                window.draw(cx).clear(cx);
                (
                    started.elapsed().as_micros() as u64,
                    view.read(cx).appearance_render_work().0 - before,
                )
            })
            .unwrap();
        samples.push(micros);
        builds.push(count);
    }
    let (input, original, before_edit) = window
        .update(cx, |_, _, cx| {
            let input = view.read(cx).preferences_ui.appearance_fields["name"].clone();
            let original = input.read(cx).value();
            (input, original, view.read(cx).appearance_render_work().0)
        })
        .unwrap();
    window
        .update(cx, |_, window, cx| {
            input.update(cx, |input, cx| {
                input.set_value(format!("{original} cache-probe"), window, cx)
            });
            view.update(cx, |_, cx| cx.notify());
        })
        .unwrap();
    let after_edit = window
        .update(cx, |_, window, cx| {
            window.draw(cx).clear(cx);
            view.read(cx).appearance_render_work().0
        })
        .unwrap();
    window
        .update(cx, |_, window, cx| {
            input.update(cx, |input, cx| input.set_value(original, window, cx));
            view.update(cx, |_, cx| cx.notify());
        })
        .unwrap();
    window
        .update(cx, |_, window, cx| {
            window.draw(cx).clear(cx);
        })
        .unwrap();
    std::fs::write(database.with_file_name("settings-appearance-cache-timing.json"), serde_json::to_string_pretty(&serde_json::json!({
        "measurement": "Parent cx.notify flushed in a separate update, then Window::draw/clear WITHOUT Window::refresh; excludes OS presentation. Zero section builds means the cached viewport was reused, not zero GPU work.",
        "initialization": initialization,
        "parent_notify_draw": timing_summary(samples),
        "section_builder_calls_per_draw": builds,
        "section_builder_calls_for_changed_field": after_edit - before_edit,
    })).unwrap()).unwrap();
    assert!(
        builds.iter().all(|&count| count == 0),
        "unchanged parent notifications rebuilt the appearance viewport"
    );
    assert!(
        after_edit > before_edit,
        "changed appearance input did not invalidate the cached viewport"
    );
}

fn appearance_cache_state(app: &StudyApp, window: &Window, cx: &App) -> serde_json::Value {
    let editor = app
        .preferences_ui
        .appearance_preview
        .as_ref()
        .and_then(|preview| preview.read(cx).editor());
    let Some(editor) = editor else {
        return serde_json::json!({"editorAllocated":false, "readyForMeasurement":false});
    };
    let editor = editor.read(cx);
    let native = editor.native_state().ok();
    let measured = window
        .try_find("continuous-markdown-editor")
        .zip(window.try_find("appearance-preview-pane"))
        .map(|(editor, pane)| editor.bounds().intersect(&pane.bounds()));
    let geometry_matches = native
        .as_ref()
        .zip(measured)
        .is_some_and(|(native, bounds)| {
            native["visible"] == true
                && native["controllerVisible"] == true
                && native["insideParent"] == true
                && bounds.size.width > px(0.)
                && bounds.size.height > px(0.)
                && native["logicalBounds"].as_array().is_some_and(|logical| {
                    [
                        bounds.left(),
                        bounds.top(),
                        bounds.size.width,
                        bounds.size.height,
                    ]
                    .into_iter()
                    .enumerate()
                    .all(|(index, expected)| {
                        logical[index].as_f64().is_some_and(|actual| {
                            (actual - f64::from(f32::from(expected))).abs() <= 2.
                        })
                    })
                })
        });
    serde_json::json!({
        "editorAllocated":true,
        "creating":editor.creating(),
        "editorReady":editor.ready(),
        "editorError":editor.error(),
        "native":native,
        "measuredBounds":measured.map(|bounds| [f32::from(bounds.left()), f32::from(bounds.top()), f32::from(bounds.size.width), f32::from(bounds.size.height)]),
        "readyForMeasurement":editor.ready() && geometry_matches,
    })
}

pub async fn exercise(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    database: &Path,
    cx: &mut AsyncApp,
) {
    settled(window, view, cx).await;
    measure_appearance_cache(window, view, database, cx).await;
    exercise_color_controls(window, view, database, cx);
    exercise_markdown_preview(window, view, database, cx).await;
    window
        .update(cx, |_, window, cx| {
            crate::smoke_library::resize(window, size(px(1280.), px(900.)));
            window.click(("nav", 4_usize), cx);
            window.render_frame(cx);
            view.update(cx, |app, cx| app.clear_notices(cx));
            let original = view.read(cx).preferences_ui.saved.as_ref().unwrap().clone();
            let files_root = database.parent().unwrap().join("settings");
            let files = settings_files(&files_root);
            let before_note_editor = view.read(cx).note_editor.is_some();
            let before_selected = view.read(cx).selected_note.clone();
            let before_versions = view.read(cx).snapshot.as_ref().unwrap().versions.len();

            window.click("settings-section-learning", cx);
            let interval = view.read(cx).interval.clone();
            interval.update(cx, |input, cx| input.set_value("35", window, cx));
            window.click("settings-section-appearance", cx);
            window.click("appearance-workspace", cx);
            window.render_frame(cx);
            let preview_rebuilds = view.read(cx).appearance_preview_rebuilds();
            click_scrolled(
                window,
                "show-file-paths".into(),
                "settings-section-scroll",
                cx,
            );
            assert_eq!(
                view.read(cx).appearance_preview_rebuilds(),
                preview_rebuilds,
                "unrelated appearance checkbox rebuilt Markdown preview colors"
            );
            let body_size = view.read(cx).preferences_ui.appearance_fields["md-body-size"].clone();
            let original_body_size = body_size.read(cx).value();
            body_size.update(cx, |input, cx| input.set_value("18", window, cx));
            window.render_frame(cx);
            assert_eq!(
                view.read(cx).appearance_preview_rebuilds(),
                preview_rebuilds + 1,
                "programmatic field change did not refresh preview"
            );
            body_size.update(cx, |input, cx| {
                input.set_value(original_body_size, window, cx)
            });
            window.render_frame(cx);
            assert_eq!(
                view.read(cx).appearance_preview_rebuilds(),
                preview_rebuilds + 2
            );
            for (section, id, _) in SettingsSection::ALL {
                window.click(SharedString::from(format!("settings-section-{id}")), cx);
                window.render_frame(cx);
                let app = view.read(cx);
                assert!(app.preferences_ui.section == section);
                assert_eq!(
                    app.interval.read(cx).value().as_str(),
                    "35",
                    "category switch discarded Learning input"
                );
                assert_eq!(
                    app.preferences_ui.draft.as_ref().unwrap().show_file_paths,
                    !original.settings.show_file_paths
                );
                assert_eq!(
                    app.preferences_ui.saved.as_ref().unwrap().revision,
                    original.revision
                );
                assert!(app.settings_dirty(cx));
                assert_eq!(
                    settings_files(&files_root),
                    files,
                    "category switching wrote configuration files"
                );
            }
            capture_task_pages(window, view, database, cx);

            window.click("settings-section-storage", cx);
            window.click("storage-images", cx);
            click_scrolled(
                window,
                "images-storage-cloud".into(),
                "settings-section-scroll",
                cx,
            );
            assert_eq!(
                view.read(cx)
                    .preferences_ui
                    .image_cloud
                    .as_ref()
                    .unwrap()
                    .kind,
                0
            );
            for key in [
                "endpoint",
                "bucket",
                "region",
                "access_key_id",
                "secret",
                "prefix",
            ] {
                let id = SharedString::from(format!("image-cloud-field-{key}"));
                reveal_scrolled(window, id.clone(), "settings-section-scroll", cx);
                assert!(window.find(id).bounds().size.height > px(0.));
            }
            assert_eq!(
                settings_files(&files_root),
                files,
                "cloud form wrote settings before save"
            );
            click_scrolled(
                window,
                "images-storage-local".into(),
                "settings-section-scroll",
                cx,
            );
            window.click("settings-section-shortcuts", cx);
            window.render_frame(cx);
            let nav = window.find("settings-category-navigation").bounds();
            let content = window.find("settings-section-scroll").bounds();
            if window.viewport_size().width < px(1120.) {
                assert!(
                    nav.bottom() <= content.top(),
                    "compact categories must stay above the scroll viewport"
                );
            } else {
                assert!(
                    nav.right() <= content.left(),
                    "wide settings categories must occupy a separate rail"
                );
            }
            let rebuilds = view.read(cx).shortcut_analysis_rebuilds();
            measure_settings(window, view, database, cx);
            crate::smoke_storage_performance::measure(window, view, database, cx);
            assert_eq!(
                view.read(cx).shortcut_analysis_rebuilds(),
                rebuilds,
                "unchanged settings frames reparsed all bindings"
            );
            click_scrolled(
                window,
                "record-shortcut-app.new".into(),
                "settings-section-scroll",
                cx,
            );
            assert!(view.read(cx).preferences_ui.recording.is_some());
            window.render_frame(cx);
            let capture = window.find("shortcut-recording-target").bounds();
            let cell = window.find("record-shortcut-app.new").bounds();
            assert!(capture.left() >= cell.left() && capture.right() <= cell.right());
            assert!(
                capture.top() >= cell.top() && capture.bottom() <= cell.bottom(),
                "key capture escaped its binding cell"
            );
            window
                .render_to_image()
                .unwrap()
                .save(database.with_file_name("settings-inline-recording.png"))
                .unwrap();
            window.press("ctrl-s", cx);
            window.render_frame(cx);
            assert!(view.read(cx).preferences_ui.recording.is_none());
            assert_eq!(
                view.read(cx).shortcut_analysis_rebuilds(),
                rebuilds + 1,
                "one captured binding must rebuild analysis exactly once"
            );
            assert_eq!(
                view.read(cx).preferences_ui.shortcuts["app.new"]
                    .read(cx)
                    .value()
                    .as_str(),
                "Ctrl+S"
            );
            assert!(
                !view.read(cx).note_saving && view.read(cx).note_intent.is_none(),
                "recorded Ctrl+S executed save"
            );
            assert!(window.try_find("shortcut-conflict-app.new").is_some());
            // Virtual rows are absent outside the viewport; reveal the other
            // binding before checking its actual red conflict message.
            reveal_scrolled(
                window,
                "shortcut-conflict-editor.save".into(),
                "settings-section-scroll",
                cx,
            );
            assert!(
                window.try_find("shortcut-conflict-editor.save").is_some(),
                "conflict omitted counterpart row"
            );
            window.click("save-settings", cx);
            assert!(
                !view.read(cx).preferences_ui.busy,
                "invalid shortcut draft started save"
            );
            assert!(view.read(cx).preferences_ui.error.is_some());
            assert_eq!(
                settings_files(&files_root),
                files,
                "invalid draft wrote configuration"
            );

            // Restore New with an actual recorded key, then capture that application
            // command on Import. A leaked Ctrl+N would open the dirty-leave dialog.
            click_scrolled(
                window,
                "record-shortcut-app.new".into(),
                "settings-section-scroll",
                cx,
            );
            window.press("ctrl-n", cx);
            click_scrolled(
                window,
                "record-shortcut-app.import".into(),
                "settings-section-scroll",
                cx,
            );
            window.press("ctrl-n", cx);
            window.render_frame(cx);
            assert!(
                view.read(cx).preferences_ui.leave.is_none(),
                "recorded application command executed"
            );
            assert_eq!(view.read(cx).page, Page::Settings);
            assert_eq!(view.read(cx).note_editor.is_some(), before_note_editor);
            assert_eq!(view.read(cx).selected_note, before_selected);
            assert_eq!(
                view.read(cx).snapshot.as_ref().unwrap().versions.len(),
                before_versions
            );
            assert!(window.find("shortcut-conflict-app.new").visible());
            assert!(window.find("shortcut-conflict-app.import").visible());
            window
                .render_to_image()
                .unwrap()
                .save(database.with_file_name("settings-shortcut-conflicts.png"))
                .unwrap();

            click_scrolled(
                window,
                "append-shortcut-app.import".into(),
                "settings-section-scroll",
                cx,
            );
            window.press("f8", cx);
            assert_eq!(
                view.read(cx).preferences_ui.shortcuts["app.import"]
                    .read(cx)
                    .value()
                    .as_str(),
                "Ctrl+N;F8"
            );
            click_scrolled(
                window,
                "record-shortcut-app.import".into(),
                "settings-section-scroll",
                cx,
            );
            window.press("escape", cx);
            assert!(view.read(cx).preferences_ui.recording.is_none());
            assert_eq!(
                view.read(cx).preferences_ui.shortcuts["app.import"]
                    .read(cx)
                    .value()
                    .as_str(),
                "Ctrl+N;F8",
                "Escape changed binding"
            );
            click_scrolled(
                window,
                "clear-shortcut-app.import".into(),
                "settings-section-scroll",
                cx,
            );
            assert!(
                view.read(cx).preferences_ui.shortcuts["app.import"]
                    .read(cx)
                    .value()
                    .is_empty()
            );

            click_scrolled(
                window,
                "record-shortcut-app.import".into(),
                "settings-section-scroll",
                cx,
            );
            window.blur(cx);
            window.press("f9", cx);
            assert!(
                view.read(cx).preferences_ui.recording.is_none(),
                "focus loss left recorder armed"
            );
            assert!(
                view.read(cx).preferences_ui.shortcuts["app.import"]
                    .read(cx)
                    .value()
                    .is_empty(),
                "escaped focus still captured a key"
            );
            // Bindings now use recorded keycaps. After capture ends, the real
            // search input must accept text and filter the grouped actions.
            let input = view
                .read(cx)
                .preferences_ui
                .shortcut_search
                .as_ref()
                .unwrap()
                .clone();
            window.render_frame(cx);
            input.read(cx).focus_handle(cx).focus(window, cx);
            window.input("heading-6", cx);
            assert_eq!(input.read(cx).value().as_str(), "heading-6");
            window.render_frame(cx);
            assert!(window.try_find("shortcut-row-editor.heading-6").is_some());
            assert!(window.try_find("shortcut-row-app.import").is_none());
            input.update(cx, |input, cx| input.set_value("", window, cx));
            click_scrolled(
                window,
                "record-shortcut-app.import".into(),
                "settings-section-scroll",
                cx,
            );
            assert_eq!(
                view.read(cx)
                    .preferences_ui
                    .recording
                    .as_ref()
                    .map(|recording| recording.action.as_str()),
                Some("app.import")
            );
            window.press("f10", cx);
            assert_eq!(
                view.read(cx).preferences_ui.shortcuts["app.import"]
                    .read(cx)
                    .value()
                    .as_str(),
                "F10"
            );
            assert!(view.read(cx).preferences_ui.recording.is_none());
            assert!(
                input.read(cx).focus_handle(cx).is_focused(window),
                "recording completion did not return focus to the visible search input"
            );
            assert_eq!(settings_files(&files_root), files);

            let header = window.find("save-settings").bounds();
            reveal_scrolled(
                window,
                "clear-shortcut-editor.delete-group-forward".into(),
                "settings-section-scroll",
                cx,
            );
            assert_eq!(
                window.find("save-settings").bounds(),
                header,
                "save bar scrolled with section"
            );
            let retained_interval = view.read(cx).interval.read(cx).value();
            window.click("reset-settings", cx);
            window.render_frame(cx);
            window.click("confirm-reset-settings", cx);
            assert_eq!(
                view.read(cx).interval.read(cx).value(),
                retained_interval,
                "shortcut reset discarded another page's learning draft"
            );
            assert_eq!(
                view.read(cx)
                    .preferences_ui
                    .saved
                    .as_ref()
                    .unwrap()
                    .revision,
                original.revision
            );
            assert_eq!(
                settings_files(&files_root),
                files,
                "reset wrote before explicit save"
            );
            for action in crate::shortcuts::SHORTCUT_ACTIONS {
                assert_eq!(
                    view.read(cx).preferences_ui.shortcuts[action.id]
                        .read(cx)
                        .value()
                        .as_str(),
                    action.default
                );
            }
            view.update(cx, |app, cx| app.discard_settings(window, cx));
            assert!(!view.read(cx).settings_dirty(cx));
            assert_eq!(
                view.read(cx)
                    .preferences_ui
                    .saved
                    .as_ref()
                    .unwrap()
                    .revision,
                original.revision
            );
            window.click(("nav", 1_usize), cx);
            assert_eq!(view.read(cx).page, Page::Notes);
        })
        .unwrap();
    // Windows resize queues SetWindowPos asynchronously; yield before reading
    // the viewport or asserting responsive layout, unlike an in-process draw.
    let original_size = window
        .update(cx, |_, window, cx| {
            window.click(("nav", 4_usize), cx);
            window.click("settings-section-shortcuts", cx);
            let original = window.viewport_size();
            crate::smoke_library::resize(window, size(px(1024.), px(720.)));
            original
        })
        .unwrap();
    for attempt in 0..100 {
        cx.background_executor()
            .timer(std::time::Duration::from_millis(40))
            .await;
        let compact = window
            .update(cx, |_, window, _| window.viewport_size().width <= px(1024.))
            .unwrap();
        if compact {
            break;
        }
        assert!(attempt < 99, "native resize did not reach compact viewport");
    }
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let nav = window.find("settings-category-navigation").bounds();
            let content = window.find("settings-section-scroll").bounds();
            assert!(
                nav.bottom() <= content.top(),
                "compact nav overlaps form: viewport={:?}, nav={nav:?}, content={content:?}",
                window.viewport_size()
            );
            assert!(
                content.size.height > px(220.),
                "compact settings lost its usable content viewport"
            );
            for (_, id, _) in SettingsSection::ALL {
                let category = window.find(SharedString::from(format!("settings-section-{id}")));
                assert!(
                    category.visible(),
                    "category {id} became unreachable in compact settings"
                );
                assert!(category.bounds().right() <= nav.right());
            }
            assert!(window.find("save-settings").visible());
            window
                .render_to_image()
                .unwrap()
                .save(database.with_file_name("settings-compact.png"))
                .unwrap();
            capture_task_pages(window, view, database, cx);
            crate::smoke_library::resize(window, original_size);
        })
        .unwrap();
    for attempt in 0..100 {
        cx.background_executor()
            .timer(std::time::Duration::from_millis(40))
            .await;
        let restored = window
            .update(cx, |_, window, _| window.viewport_size() == original_size)
            .unwrap();
        if restored {
            break;
        }
        assert!(attempt < 99, "native viewport did not restore");
    }
    window
        .update(cx, |_, window, cx| window.click(("nav", 1_usize), cx))
        .unwrap();
    exercise_save_shortcut(window, view, database, cx).await;
    let original_appearance = window
        .update(cx, |_, _, cx| {
            view.read(cx)
                .preferences_ui
                .saved
                .as_ref()
                .unwrap()
                .settings
                .appearance
                .clone()
        })
        .unwrap();
    for mode in ["dark", "light", "restore"] {
        window
            .update(cx, |_, window, cx| {
                window.click(("nav", 4_usize), cx);
                window.click("settings-section-appearance", cx);
                window.click("appearance-overview", cx);
                if mode == "restore" {
                    view.update(cx, |app, cx| {
                        app.preferences_ui.draft.as_mut().unwrap().appearance =
                            original_appearance.clone();
                        app.refresh_appearance(window, cx);
                        cx.notify();
                    });
                } else {
                    window.click(SharedString::from(format!("theme-mode-{mode}")), cx);
                }
                window.click("save-settings", cx);
            })
            .unwrap();
        until(
            window,
            view,
            "native theme committed",
            |app, cx| !app.preferences_ui.busy && !app.settings_dirty(cx),
            cx,
        )
        .await;
        let editor = window
            .update(cx, |_, window, cx| {
                window.click(("nav", 1_usize), cx);
                view.update(cx, |app, cx| app.clear_notices(cx));
                view.read(cx).note_content.clone()
            })
            .unwrap();
        crate::smoke_visibility::visible(window, &editor, cx).await;
        let state = crate::smoke_editor::bridge(
            window,
            &editor,
            serde_json::json!({"type":"snapshot"}),
            cx,
        )
        .await;
        window
            .update(cx, |_, _, _| {
                let palette = crate::theme::palette();
                assert_eq!(
                    state["theme"]["mode"],
                    if palette.dark { "dark" } else { "light" }
                );
                assert_eq!(
                    state["theme"]["background"],
                    format!("#{:06X}", palette.surface)
                );
                assert_eq!(
                    state["theme"]["surface"],
                    format!("#{:06X}", palette.surface)
                );
                assert_eq!(state["theme"]["text"], format!("#{:06X}", palette.text));
            })
            .unwrap();
        crate::smoke_visibility::capture_window(
            window,
            &editor,
            &database.with_file_name(format!("native-theme-{mode}.bmp")),
            cx,
        )
        .await;
    }
    std::fs::write(database.with_file_name("settings-capture-smoke-result.txt"),
        "PASS: all six categories preserve one draft and byte-identical settings files; fixed save bar remains visible; wide category rail and settled 1024px compact navigation have bounded form content; inline recording stays inside its binding cell; 20 unchanged frames reuse one analysis and one captured edit rebuilds once; platform Ctrl+S and Ctrl+N record without dispatch; both conflicting rows displayed in screenshot; strict save rejects; F8 appends, Escape cancels, clear unbinds, focus loss disarms and normal input resumes; reset remains draft until save, discard restores baseline; light/dark theme persistence, native/editor token coherence and full native screenshots, original appearance restored. Key events use native-window GPUI test dispatch; separate strict Win32 input gates remain required.\n").unwrap();
}

/// A real readonly native Markdown surface, independent from the user's note.
/// These native operations run only in the isolated Windows CI smoke process.
async fn exercise_markdown_preview(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    database: &Path,
    cx: &mut AsyncApp,
) {
    let (
        note,
        generation,
        value,
        before_files,
        original_width,
        original_text,
        original_size,
        touched,
    ) = window
        .update(cx, |_, window, cx| {
            let app = view.read(cx);
            let note = app.note_content.clone();
            let generation = note.read(cx).generation();
            let value = note.read(cx).value().to_owned();
            let original_width = app.preferences_ui.appearance_fields["md-reading-width"]
                .read(cx)
                .value();
            let touched = app.preferences_ui.appearance_touched;
            let original_text = app.preferences_ui.appearance_fields["text"]
                .read(cx)
                .value();
            let original_size = window.viewport_size();
            view.update(cx, |app, cx| {
                app.select_settings_section(SettingsSection::Appearance, cx);
                app.select_appearance_page(AppearancePage::ThemeEditor, cx);
                app.preferences_ui.appearance_preview_mode = true;
                cx.notify();
            });
            window.render_frame(cx);
            (
                note,
                generation,
                value,
                settings_files(&database.parent().unwrap().join("settings")),
                original_width,
                original_text,
                original_size,
                touched,
            )
        })
        .unwrap();
    until(
        window,
        view,
        "independent Markdown appearance preview ready",
        |app, cx| {
            app.preferences_ui
                .appearance_preview
                .as_ref()
                .and_then(|preview| preview.read(cx).editor())
                .is_some_and(|editor| editor.read(cx).ready())
        },
        cx,
    )
    .await;
    let preview = window
        .update(cx, |_, _, cx| {
            view.read(cx)
                .preferences_ui
                .appearance_preview
                .as_ref()
                .unwrap()
                .read(cx)
                .editor()
                .unwrap()
        })
        .unwrap();
    let before =
        crate::smoke_editor::bridge(window, &preview, serde_json::json!({"type":"snapshot"}), cx)
            .await;
    let before_evidence =
        record_preview_continuity(window, &preview, database, "initial", None, &before, cx);
    assert_eq!(before["text"], crate::appearance_preview::PREVIEW_SAMPLE);
    assert_eq!(before["readonly"], true);
    assert_eq!(before["canRequestEdit"], false);
    let native = crate::smoke_visibility::visible(window, &preview, cx).await;
    assert!(
        native["bounds"]["height"].as_i64().unwrap() >= 300,
        "appearance preview collapsed to the editor's 180px minimum: {native}"
    );
    crate::smoke_interactions::exercise_pointer_bubble(
        window, view, &preview, database, "preview", cx,
    )
    .await;
    assert!(
        before["bindings"]
            .as_object()
            .unwrap()
            .values()
            .all(|binding| binding == "")
    );
    window
        .update(cx, |_, window, cx| {
            let input = view.read(cx).preferences_ui.appearance_fields["md-reading-width"].clone();
            input.update(cx, |input, cx| input.set_value("640", window, cx));
            window.render_frame(cx);
        })
        .unwrap();
    let changed =
        crate::smoke_editor::bridge(window, &preview, serde_json::json!({"type":"snapshot"}), cx)
            .await;
    assert_eq!(changed["generation"], before["generation"]);
    assert_eq!(changed["text"], before["text"]);
    assert_eq!(changed["theme"]["readingWidth"], "640px");
    crate::smoke_visibility::capture_window(
        window,
        &preview,
        &database.with_file_name("settings-markdown-preview.bmp"),
        cx,
    )
    .await;
    // Position the readonly sample away from the top before changing style.
    crate::smoke_editor::bridge(window, &preview, serde_json::json!({
        "type":"select", "anchor":crate::appearance_preview::PREVIEW_SAMPLE.encode_utf16().count() / 2
    }), cx).await;
    let scrolled =
        crate::smoke_editor::bridge(window, &preview, serde_json::json!({"type":"snapshot"}), cx)
            .await;
    let scrolled_evidence = record_preview_continuity(
        window,
        &preview,
        database,
        "scrolled",
        Some(&before_evidence),
        &scrolled,
        cx,
    );
    assert!(
        scrolled["scrollTop"].as_f64().unwrap() > 50.,
        "sample was never scrolled: {scrolled}"
    );
    window
        .update(cx, |_, window, cx| {
            window.click("appearance-edit", cx);
            window.render_frame(cx);
            click_scrolled(
                window,
                "color-open-text".into(),
                "settings-section-scroll",
                cx,
            );
        })
        .unwrap();
    until(
        window,
        view,
        "measured color popup above live preview",
        |app, cx| {
            app.preferences_ui.appearance_colors["text"]
                .read(cx)
                .popup_bounds()
                .is_some()
        },
        cx,
    )
    .await;
    // Switching from focused preview to parameters changes the native sample's
    // width. Popup measurement acknowledges GPUI only, not Chromium/CM layout.
    settle_preview_viewport(window, view, original_size, cx).await;
    let (picker_id, open_native, popup, popup_shapes) = window
        .update(cx, |_, _window, cx| {
            let control = view.read(cx).preferences_ui.appearance_colors["text"].clone();
            let popup = control.read(cx).popup_bounds().unwrap();
            let native = preview.read(cx).native_state().unwrap();
            (
                control.read(cx).picker_id(),
                native,
                popup,
                control.read(cx).popup_shapes(),
            )
        })
        .unwrap();
    let opening =
        crate::smoke_editor::bridge(window, &preview, serde_json::json!({"type":"snapshot"}), cx)
            .await;
    let opening_evidence = record_preview_continuity(
        window,
        &preview,
        database,
        "opening",
        Some(&scrolled_evidence),
        &opening,
        cx,
    );
    assert_preview_popup_hit(&open_native, popup);
    assert_preview_popup_arrow(&open_native, &popup_shapes);
    let logical = open_native["logicalBounds"].as_array().unwrap();
    let layout_ready = crate::smoke_editor::bridge(
        window,
        &preview,
        serde_json::json!({"type":"layout", "width":logical[2], "height":logical[3]}),
        cx,
    )
    .await;
    record_preview_continuity(
        window,
        &preview,
        database,
        "setup-layout",
        Some(&opening_evidence),
        &layout_ready,
        cx,
    );
    // Keep independent immediate snapshots from here onward. Settling after an
    // interaction would conceal a real hover/dismiss/commit scroll regression.
    let opened =
        crate::smoke_editor::bridge(window, &preview, serde_json::json!({"type":"snapshot"}), cx)
            .await;
    let opened_evidence = record_preview_continuity(
        window,
        &preview,
        database,
        "opened",
        Some(&opening_evidence),
        &opened,
        cx,
    );
    window
        .update(cx, |_, window, cx| {
            let swatch = window.find("color-swatch-text-1").bounds().center();
            window.dispatch_event(
                MouseMoveEvent {
                    position: swatch,
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
        })
        .unwrap();
    let hovered =
        crate::smoke_editor::bridge(window, &preview, serde_json::json!({"type":"snapshot"}), cx)
            .await;
    record_preview_continuity(
        window,
        &preview,
        database,
        "hover",
        Some(&opened_evidence),
        &hovered,
        cx,
    );
    assert_eq!(
        hovered["theme"], scrolled["theme"],
        "hover committed a color"
    );
    assert_preview_continuity(&opened, &hovered, true);
    crate::smoke_visibility::capture_window(
        window,
        &preview,
        &database.with_file_name("settings-markdown-picker-hover.bmp"),
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            window.click("color-close-text", cx);
            window.render_frame(cx);
        })
        .unwrap();
    let dismissed =
        crate::smoke_editor::bridge(window, &preview, serde_json::json!({"type":"snapshot"}), cx)
            .await;
    record_preview_continuity(
        window,
        &preview,
        database,
        "dismiss",
        Some(&opened_evidence),
        &dismissed,
        cx,
    );
    assert_eq!(
        dismissed["theme"], scrolled["theme"],
        "dismissal committed hovered color"
    );
    assert_preview_continuity(&opened, &dismissed, true);
    window
        .update(cx, |_, window, cx| {
            window.click("color-open-text", cx);
            window.render_frame(cx);
            assert_eq!(
                view.read(cx).preferences_ui.appearance_colors["text"]
                    .read(cx)
                    .picker_id(),
                picker_id
            );
            window.click("color-swatch-text-3", cx);
        })
        .unwrap();
    // The click queues a picker Change event. Its settings subscriber commits
    // the field first; a later normal StudyApp render configures the preview.
    // A browser snapshot dispatched immediately here can overtake that render.
    let commit = window
        .update(cx, |_, _, cx| {
            let app = view.read(cx);
            serde_json::json!({
                "expectedText":"#EF4444",
                "committedText":app.preferences_ui.appearance_fields["text"].read(cx).value(),
                "pickerOpen":app.preferences_ui.appearance_colors["text"].read(cx).popup_open(),
                "appliedTheme":app.preferences_ui.appearance_preview.as_ref().unwrap().read(cx).applied_theme().map(|theme| theme.editor_tokens()),
                "native":preview.read(cx).native_state().unwrap(),
            })
        })
        .unwrap();
    let commit_path = database.with_file_name("settings-markdown-color-commit.json");
    std::fs::write(&commit_path, serde_json::to_string_pretty(&commit).unwrap()).unwrap();
    assert_eq!(
        commit["committedText"], "#EF4444",
        "swatch did not commit: {commit}"
    );
    assert_eq!(
        commit["pickerOpen"], false,
        "committed picker stayed open: {commit}"
    );
    until(
        window,
        view,
        "committed text color applied by the normal preview render",
        |app, cx| {
            app.preferences_ui
                .appearance_preview
                .as_ref()
                .unwrap()
                .read(cx)
                .applied_theme()
                .is_some_and(|theme| theme.text == 0xEF4444)
        },
        cx,
    )
    .await;
    let applied = window
        .update(cx, |_, _, cx| {
            view.read(cx)
                .preferences_ui
                .appearance_preview
                .as_ref()
                .unwrap()
                .read(cx)
                .applied_theme()
                .unwrap()
                .editor_tokens()
        })
        .unwrap();
    let selected =
        crate::smoke_editor::bridge(window, &preview, serde_json::json!({"type":"snapshot"}), cx)
            .await;
    let selected_evidence = record_preview_continuity(
        window,
        &preview,
        database,
        "commit",
        Some(&opened_evidence),
        &selected,
        cx,
    );
    std::fs::write(
        &commit_path,
        serde_json::to_string_pretty(&serde_json::json!({
            "commit":commit,
            "appliedTheme":applied,
            "browser":selected,
        }))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(selected["theme"]["text"], "#EF4444");
    assert_preview_continuity(&opened, &selected, true);
    let selected_native = window
        .update(cx, |_, _, cx| preview.read(cx).native_state().unwrap())
        .unwrap();
    assert!(
        selected_native["occlusionArrowPolygons"]
            .as_array()
            .unwrap()
            .is_empty(),
        "closed color popup left its arrow hole"
    );
    assert_eq!(selected_native["hwnd"], open_native["hwnd"]);
    assert_eq!(
        selected_native["visibilityUpdates"], open_native["visibilityUpdates"],
        "picker interaction hid/reopened the native sample"
    );
    crate::smoke_visibility::capture_window(
        window,
        &preview,
        &database.with_file_name("settings-markdown-picker-selected.bmp"),
        cx,
    )
    .await;

    let mut geometry = Vec::new();
    for (width, height) in PREVIEW_VIEWPORTS {
        window
            .update(cx, |_, window, _| {
                crate::smoke_library::resize(window, size(px(width), px(height)))
            })
            .unwrap();
        settle_preview_viewport(window, view, size(px(width), px(height)), cx).await;
        let measured = window.update(cx, |_, window, cx| {
            let viewport = window.viewport_size();
            assert!((viewport.width - px(width)).abs() <= px(1.) && (viewport.height - px(height)).abs() <= px(1.), "native preview viewport did not reach supported request: requested={width}x{height}, actual={viewport:?}");
            let parameters = window.find("appearance-parameters").bounds();
            let pane = window.find("appearance-preview-pane").bounds();
            let editor = window.find("continuous-markdown-editor").bounds().intersect(&pane);
            let native = preview.read(cx).native_state().unwrap();
            let logical = native["logicalBounds"].as_array().unwrap();
            for (index, expected) in [editor.left(), editor.top(), editor.size.width, editor.size.height].into_iter().enumerate() {
                assert!((logical[index].as_f64().unwrap() - f64::from(f32::from(expected))).abs() <= 2., "native bounds escaped measured preview pane: {native}; pane={pane:?}; editor={editor:?}");
            }
            assert!(parameters.size.height > px(0.) && pane.size.height > px(0.));
            let workspace = parameters.size.width + pane.size.width;
            let stacked = pane.top() >= parameters.bottom();
            assert!(stacked || pane.left() >= parameters.right());
            if !stacked { assert!(pane.size.width > workspace * 0.4); }
            assert_eq!(view.read(cx).preferences_ui.appearance_preview.as_ref().unwrap().read(cx).editor().unwrap().entity_id(), preview.entity_id());
            serde_json::json!({"requested":[width,height],"viewport":[f32::from(viewport.width),f32::from(viewport.height)],"parameters":[f32::from(parameters.left()),f32::from(parameters.top()),f32::from(parameters.size.width),f32::from(parameters.size.height)],"pane":[f32::from(pane.left()),f32::from(pane.top()),f32::from(pane.size.width),f32::from(pane.size.height)],"stacked":stacked,"native":native})
        }).unwrap();
        let resized = crate::smoke_editor::bridge(
            window,
            &preview,
            serde_json::json!({"type":"snapshot"}),
            cx,
        )
        .await;
        record_preview_continuity(
            window,
            &preview,
            database,
            &format!("resize-{width:.0}-{height:.0}"),
            Some(&selected_evidence),
            &resized,
            cx,
        );
        assert_preview_continuity(&selected, &resized, false);
        assert_eq!(
            resized["theme"]["readingWidth"], "640px",
            "resize rewrote the requested maximum reading width"
        );
        assert!(
            resized["scrollTop"].as_f64().unwrap() > 50.,
            "resize reset sample scroll"
        );
        // Extremely short panes are validated by exact native clipping above;
        // the existing screenshot gate requires a >120px native viewport.
        if measured["native"]["bounds"]["height"].as_i64().unwrap() > 120 {
            crate::smoke_visibility::capture_window(
                window,
                &preview,
                &database
                    .with_file_name(format!("settings-markdown-size-{width:.0}-{height:.0}.bmp")),
                cx,
            )
            .await;
        }
        geometry.push(measured);
    }
    assert!(
        geometry.iter().any(|entry| entry["stacked"] == true),
        "supported compact viewport did not exercise stacked panes"
    );
    assert!(
        geometry.iter().any(|entry| entry["stacked"] == false),
        "supported wide viewport did not exercise side-by-side panes"
    );
    window
        .update(cx, |_, window, cx| {
            click_scrolled(
                window,
                "color-open-text".into(),
                "settings-section-scroll",
                cx,
            );
            window.render_frame(cx);
        })
        .unwrap();
    until(
        window,
        view,
        "picker reopens after responsive movement",
        |app, cx| {
            app.preferences_ui.appearance_colors["text"]
                .read(cx)
                .popup_bounds()
                .is_some()
        },
        cx,
    )
    .await;
    let (picker_before_resize, size_before_resize) = window
        .update(cx, |_, window, cx| {
            (
                view.read(cx).preferences_ui.appearance_colors["text"]
                    .read(cx)
                    .picker_id(),
                window.viewport_size(),
            )
        })
        .unwrap();
    let before_live_resize =
        crate::smoke_editor::bridge(window, &preview, serde_json::json!({"type":"snapshot"}), cx)
            .await;
    let before_resize_evidence = record_preview_continuity(
        window,
        &preview,
        database,
        "before-live-resize",
        None,
        &before_live_resize,
        cx,
    );
    let live_size = size(px(LIVE_PREVIEW_VIEWPORT.0), px(LIVE_PREVIEW_VIEWPORT.1));
    window
        .update(cx, |_, window, _| {
            crate::smoke_library::resize(window, live_size)
        })
        .unwrap();
    let live_resized =
        settle_preview_picker_geometry(window, view, &preview, picker_before_resize, live_size, cx)
            .await;
    let after_live_resize =
        crate::smoke_editor::bridge(window, &preview, serde_json::json!({"type":"snapshot"}), cx)
            .await;
    record_preview_continuity(
        window,
        &preview,
        database,
        "live-resize",
        Some(&before_resize_evidence),
        &after_live_resize,
        cx,
    );
    assert_preview_continuity(&before_live_resize, &after_live_resize, false);
    assert_eq!(after_live_resize["theme"], before_live_resize["theme"]);
    assert_eq!(
        after_live_resize["selection"], before_live_resize["selection"],
        "resize changed the sample reading anchor"
    );
    assert!(
        after_live_resize["scrollTop"].as_f64().unwrap() > 50.,
        "open-picker resize reset sample scroll"
    );
    window
        .update(cx, |_, window, _| {
            crate::smoke_library::resize(window, size_before_resize)
        })
        .unwrap();
    let live_restored = settle_preview_picker_geometry(
        window,
        view,
        &preview,
        picker_before_resize,
        size_before_resize,
        cx,
    )
    .await;
    crate::smoke_visibility::capture_window(
        window,
        &preview,
        &database.with_file_name("settings-markdown-picker-resized.bmp"),
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.clear_notices(cx);
                app.push_notice(
                    Err("预览遮挡回归：通知应位于正文之上，关闭按钮仍能点击。\n".repeat(12)),
                    cx,
                );
                app.push_notice(
                    Err("第二条独立通知：不得被原生阅读预览盖住。\n".repeat(12)),
                    cx,
                );
            });
            window.render_frame(cx);
        })
        .unwrap();
    until(
        window,
        view,
        "preview receives native notification occlusions",
        |_, cx| {
            preview.read(cx).native_state().ok().is_some_and(|state| {
                state["occlusions"]
                    .as_array()
                    .is_some_and(|bounds| bounds.len() >= 2)
            })
        },
        cx,
    )
    .await;
    let occluded = window
        .update(cx, |_, _, cx| preview.read(cx).native_state().unwrap())
        .unwrap();
    window
        .update(cx, |_, _, cx| {
            let app = view.read(cx);
            assert!(
                app.preferences_ui.appearance_colors["text"]
                    .read(cx)
                    .popup_bounds()
                    .is_some()
            );
            assert!(
                occluded["occlusions"].as_array().unwrap().len() >= 2,
                "notification replaced picker occlusion: {occluded}"
            );
        })
        .unwrap();
    crate::smoke_visibility::capture_window(
        window,
        &preview,
        &database.with_file_name("settings-markdown-picker-notices.bmp"),
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.clear_notices(cx);
                app.preferences_ui.appearance_fields["md-reading-width"]
                    .update(cx, |input, cx| input.set_value(original_width, window, cx));
                app.preferences_ui.appearance_fields["text"]
                    .update(cx, |input, cx| input.set_value(original_text, window, cx));
                app.preferences_ui.appearance_touched = touched;
                app.preferences_ui.appearance_preview_mode = false;
                app.select_appearance_page(AppearancePage::Workspace, cx);
                cx.notify();
            });
            crate::smoke_library::resize(window, original_size);
            window.render_frame(cx);
            assert!(view.read(cx).appearance_color_popup_bounds(cx).is_empty());
            assert_eq!(note.read(cx).generation(), generation);
            assert_eq!(note.read(cx).value(), value);
            assert_eq!(
                settings_files(&database.parent().unwrap().join("settings")),
                before_files
            );
        })
        .unwrap();
    let hidden = window
        .update(cx, |_, _, cx| preview.read(cx).native_state().unwrap())
        .unwrap();
    assert_eq!(
        hidden["visible"], false,
        "leaving preview must hide its native child"
    );
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.select_appearance_page(AppearancePage::ThemeEditor, cx)
            });
            window.render_frame(cx);
        })
        .unwrap();
    until(
        window,
        view,
        "preview reopens without stale color or notification regions",
        |app, cx| {
            assert_eq!(
                app.preferences_ui
                    .appearance_preview
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .editor()
                    .unwrap()
                    .entity_id(),
                preview.entity_id()
            );
            preview.read(cx).native_state().ok().is_some_and(|state| {
                state["visible"] == true
                    && state["occlusions"].as_array().is_some_and(Vec::is_empty)
            })
        },
        cx,
    )
    .await;
    let reopened =
        crate::smoke_editor::bridge(window, &preview, serde_json::json!({"type":"snapshot"}), cx)
            .await;
    record_preview_continuity(
        window,
        &preview,
        database,
        "reopened",
        Some(&before_evidence),
        &reopened,
        cx,
    );
    assert_preview_continuity(&before, &reopened, false);
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.select_appearance_page(AppearancePage::Workspace, cx)
            });
            window.render_frame(cx);
        })
        .unwrap();
    std::fs::write(
        database.with_file_name("settings-markdown-preview.json"),
        serde_json::to_string_pretty(
            &serde_json::json!({"before":before,"changed":changed,"scrolled":scrolled,"opened":opened,"hovered":hovered,"dismissed":dismissed,"selected":selected,"openNative":open_native,"selectedNative":selected_native,"geometry":geometry,"liveResized":live_resized,"liveRestored":live_restored,"afterLiveResize":after_live_resize,"occluded":occluded,"hidden":hidden,"reopened":reopened}),
        )
        .unwrap(),
    )
    .unwrap();
}

async fn settle_preview_viewport(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    requested: Size<Pixels>,
    cx: &mut AsyncApp,
) {
    let started = std::time::Instant::now();
    loop {
        let (settled, state) = window
            .update(cx, |_, window, cx| {
                // Resize delivery and native placement must agree with a normal
                // draw; a fixed frame count cannot acknowledge either boundary.
                window.draw(cx).clear(cx);
                let viewport = window.viewport_size();
                let state = appearance_cache_state(view.read(cx), window, cx);
                let resized = (viewport.width - requested.width).abs() <= px(1.)
                    && (viewport.height - requested.height).abs() <= px(1.);
                (
                    resized && state["readyForMeasurement"] == true,
                    serde_json::json!({
                        "viewport":[f32::from(viewport.width),f32::from(viewport.height)],
                        "requested":[f32::from(requested.width),f32::from(requested.height)],
                        "preview":state,
                    }),
                )
            })
            .unwrap();
        if settled {
            return;
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "native preview viewport did not settle: {state}"
        );
        cx.background_executor()
            .timer(std::time::Duration::from_millis(40))
            .await;
    }
}

/// Settle against the measured surfaces, not an assumed number of resize frames.
async fn settle_preview_picker_geometry(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    preview: &Entity<crate::markdown_editor::MarkdownEditor>,
    picker_id: Option<EntityId>,
    requested: Size<Pixels>,
    cx: &mut AsyncApp,
) -> serde_json::Value {
    let mut last = serde_json::Value::Null;
    for _ in 0..125 {
        let (settled, measured) = window.update(cx, |_, window, cx| {
            window.render_frame(cx);
            let app = view.read(cx);
            let control = app.preferences_ui.appearance_colors["text"].read(cx);
            assert!(control.popup_open(), "resize dismissed the live picker");
            assert_eq!(control.picker_id(), picker_id, "resize recreated the picker");
            assert_eq!(app.preferences_ui.appearance_preview.as_ref().unwrap().read(cx).editor().unwrap().entity_id(), preview.entity_id());
            let native = preview.read(cx).native_state().unwrap();
            let pane = window.find("appearance-preview-pane").bounds();
            let editor = window.find("continuous-markdown-editor").bounds().intersect(&pane);
            let Some(popup) = control.popup_bounds() else { return (false, native) };
            let input = window.find("color-value-text").bounds();
            let logical = native["logicalBounds"].as_array().unwrap();
            let matches = [editor.left(), editor.top(), editor.size.width, editor.size.height].into_iter().enumerate()
                .all(|(index, value)| (logical[index].as_f64().unwrap() - f64::from(f32::from(value))).abs() <= 2.);
            let popup_contains_input = popup.left() <= input.left() && popup.right() >= input.right()
                && popup.top() <= input.top() && popup.bottom() >= input.bottom();
            let overlap = preview_native_bounds(&native).intersect(&popup);
            let overlaps = overlap.size.width > px(4.) && overlap.size.height > px(4.);
            let viewport = window.viewport_size();
            let resized = (viewport.width - requested.width).abs() <= px(1.) && (viewport.height - requested.height).abs() <= px(1.);
            let settled = resized && matches && popup_contains_input && native["visible"] == true;
            let arrow_hit = if settled {
                assert_preview_popup_arrow(&native, &control.popup_shapes())
            } else { false };
            if settled && overlaps { assert_preview_popup_hit(&native, popup); }
            (settled, serde_json::json!({"native":native,"popup":[f32::from(popup.left()),f32::from(popup.top()),f32::from(popup.size.width),f32::from(popup.size.height)],"overlapHitVerified":overlaps,"arrowHitVerified":arrow_hit,"paintedShapes":format!("{:?}",control.popup_shapes()),"matchesPane":matches,"containsInput":popup_contains_input,"viewport":[f32::from(viewport.width),f32::from(viewport.height)],"requested":[f32::from(requested.width),f32::from(requested.height)]}))
        }).unwrap();
        if settled {
            return measured;
        }
        last = measured;
        cx.background_executor()
            .timer(std::time::Duration::from_millis(40))
            .await;
    }
    panic!("live popup/native geometry did not settle: {last}");
}

fn preview_native_bounds(native: &serde_json::Value) -> Bounds<Pixels> {
    let logical = native["logicalBounds"].as_array().unwrap();
    Bounds::new(
        point(
            px(logical[0].as_f64().unwrap() as f32),
            px(logical[1].as_f64().unwrap() as f32),
        ),
        size(
            px(logical[2].as_f64().unwrap() as f32),
            px(logical[3].as_f64().unwrap() as f32),
        ),
    )
}

fn record_preview_continuity(
    window: AnyWindowHandle,
    preview: &Entity<crate::markdown_editor::MarkdownEditor>,
    database: &Path,
    phase: &str,
    before: Option<&serde_json::Value>,
    browser: &serde_json::Value,
    cx: &mut AsyncApp,
) -> serde_json::Value {
    let after = window
        .update(cx, |_, window, cx| {
            let pane = window.find("appearance-preview-pane").bounds();
            let editor = window
                .find("continuous-markdown-editor")
                .bounds()
                .intersect(&pane);
            let bounds = |bounds: Bounds<Pixels>| {
                [
                    f32::from(bounds.left()),
                    f32::from(bounds.top()),
                    f32::from(bounds.size.width),
                    f32::from(bounds.size.height),
                ]
            };
            let viewport = window.viewport_size();
            serde_json::json!({
                "browser":browser,
                "paneBounds":bounds(pane),
                "editorBounds":bounds(editor),
                "viewport":[f32::from(viewport.width), f32::from(viewport.height)],
                "native":preview.read(cx).native_state().unwrap(),
            })
        })
        .unwrap();
    // Persist before checking anything: a panic must retain the exact immediate
    // snapshots and both measured/native geometries instead of only a bitmap.
    std::fs::write(
        database.with_file_name(format!("native-preview-continuity-{phase}.json")),
        serde_json::to_string_pretty(&serde_json::json!({
            "phase":phase, "before":before, "after":after,
        }))
        .unwrap(),
    )
    .unwrap();
    after
}

fn assert_preview_continuity(
    before: &serde_json::Value,
    after: &serde_json::Value,
    same_scroll: bool,
) {
    assert_eq!(after["generation"], before["generation"]);
    assert_eq!(after["text"], before["text"]);
    assert_eq!(after["readonly"], true);
    if same_scroll {
        assert!(
            (after["scrollTop"].as_f64().unwrap() - before["scrollTop"].as_f64().unwrap()).abs()
                < 5.,
            "style choice reset the sample viewport: before={before}; after={after}"
        );
    }
}

// Some responsive placements keep the color trigger/arrow outside the native
// pane. Sample its actual triangle when it overlaps; the separate controlled
// pointer fixture guarantees triangle-only overlap at both native hosts.
fn assert_preview_popup_arrow(
    native: &serde_json::Value,
    shapes: &[studyapp_ui::bubble::OverlayShape],
) -> bool {
    use studyapp_ui::bubble::OverlayShape;
    assert_eq!(
        shapes.len(),
        2,
        "color popup omitted actual painted geometry"
    );
    let OverlayShape::RoundedRect { bounds: body, .. } = shapes[0] else {
        panic!("missing color body")
    };
    let OverlayShape::Triangle(arrow) = shapes[1] else {
        panic!("missing color arrow")
    };
    let sample = preview_native_bounds(native);
    let inside = (arrow[0] + arrow[1] + arrow[2]) / 3.;
    let beside = point(
        arrow[0].x + px(1.),
        arrow[1].y + (arrow[0].y - arrow[1].y) * 0.15,
    );
    if sample.contains(&inside) {
        crate::smoke_interactions::assert_overlay_hit(native, inside, true);
        if sample.contains(&beside) && !body.contains(&beside) {
            assert!(
                !crate::smoke_interactions::raster_arrow_contains(native, beside),
                "adjacent color-arrow sample entered its physical raster outline"
            );
            crate::smoke_interactions::assert_overlay_hit(native, beside, false);
        }
        true
    } else {
        false
    }
}

// WindowFromPoint proves actual native z-order at an overlapping point. GPUI
// dispatch alone could click a popup that is visually covered by the WebView.
fn assert_preview_popup_hit(native: &serde_json::Value, popup: Bounds<Pixels>) {
    use windows::Win32::{
        Foundation::{HWND, POINT},
        UI::WindowsAndMessaging::WindowFromPoint,
    };
    assert_eq!(native["visible"], true);
    let logical = native["logicalBounds"].as_array().unwrap();
    let sample = preview_native_bounds(native);
    let overlap = sample.intersect(&popup);
    assert!(
        overlap.size.width > px(4.) && overlap.size.height > px(4.),
        "popup fixture did not overlap native preview: popup={popup:?}; native={native}"
    );
    let center = overlap.center();
    let scale = logical[4].as_f64().unwrap() / 96.;
    let x = native["parentBounds"]["x"].as_i64().unwrap() as i32
        + (f64::from(f32::from(center.x)) * scale).round() as i32;
    let y = native["parentBounds"]["y"].as_i64().unwrap() as i32
        + (f64::from(f32::from(center.y)) * scale).round() as i32;
    let parent = HWND(native["parentHwnd"].as_u64().unwrap() as usize as *mut _);
    assert_eq!(
        unsafe { WindowFromPoint(POINT { x, y }) },
        parent,
        "native WebView covers the actual color popup"
    );
}

#[cfg(test)]
mod viewport_tests {
    #[test]
    fn preview_requests_fit_the_native_runner_viewport_range() {
        // The WindowOptions in main.rs require at least 1000x700. Windows
        // clamps smaller requests, making an exact-size readiness gate invalid.
        for (width, height) in super::PREVIEW_VIEWPORTS
            .into_iter()
            .chain([super::LIVE_PREVIEW_VIEWPORT])
        {
            assert!(
                width >= 1000. && height >= 700.,
                "preview request {width}x{height} is below the 1000x700 native minimum"
            );
            assert!(
                width <= 1024. && height <= 720.,
                "preview request {width}x{height} exceeds the 1024x720 native runner fixture"
            );
        }
    }
}
