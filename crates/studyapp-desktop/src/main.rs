#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod app_startup;
mod appearance_preview;
mod appearance_ui;
mod assets;
mod components;
mod controls;
mod credentials;
mod design;
mod draft_moves;
mod drafts;
mod editor_lifecycle;
mod external_drop;
mod external_drop_ui;
mod feedback;
mod folder_picker;
mod folders_ui;
mod history_preview;
mod image_cloud;
mod imports;
mod maintenance_execute;
mod maintenance_handoff;
mod maintenance_runtime;
mod maintenance_ui;
mod markdown_editor;
mod media;
mod media_ui;
mod native_shortcuts;
mod note_actions;
mod note_commands;
mod note_editor;
mod notes_view;
mod notifications;
mod overlay_regions;
mod preferences;
mod resize_overlay;
mod settings_cache;
mod settings_color;
mod settings_components;
mod settings_search;
mod settings_ui;
mod shortcuts;
mod smoke;
#[cfg(all(windows, feature = "ui-test"))]
mod smoke_clipboard;
#[cfg(feature = "ui-test")]
mod smoke_editor;
#[cfg(all(windows, feature = "ui-test"))]
mod smoke_inputs;
#[cfg(all(windows, feature = "ui-test"))]
mod smoke_interactions;
#[cfg(feature = "ui-test")]
mod smoke_library;
#[cfg(all(windows, feature = "ui-test"))]
mod smoke_note_rename;
#[cfg(all(windows, feature = "ui-test"))]
mod smoke_settings;
#[cfg(all(windows, feature = "ui-test"))]
mod smoke_statistics;
#[cfg(all(windows, feature = "ui-test"))]
mod smoke_storage_performance;
#[cfg(all(windows, feature = "ui-test"))]
mod smoke_updates;
#[cfg(all(windows, feature = "ui-test"))]
mod smoke_visibility;
#[cfg(all(windows, feature = "ui-test"))]
mod smoke_window_drag;
#[cfg(all(windows, feature = "ui-test"))]
mod smoke_workspace;
mod storage_backend;
mod storage_policy;
mod storage_ui;
mod sync_runtime;
mod sync_ui;
mod theme;
mod update_backend;
mod update_install;
mod update_manifest;
mod update_nsis;
mod update_signature;
mod update_ui;
mod views;
mod web_editor_host;
mod window_chrome;
mod worker;
mod workspace_layout;

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;
use std::path::PathBuf;

fn paths() -> anyhow::Result<(PathBuf, Option<PathBuf>, bool)> {
    let args: Vec<_> = std::env::args().collect();
    let smoke = args.iter().any(|a| a == "--smoke-test");
    let project = directories::ProjectDirs::from("io", "StudyApp", "StudyApp")
        .ok_or_else(|| anyhow::anyhow!("无法定位用户数据目录"))?;
    let config = project.config_dir().join("location.json");
    let explicit = args
        .iter()
        .position(|a| a == "--data-dir")
        .map(|index| {
            args.get(index + 1)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("--data-dir 后需要目录路径"))
        })
        .transpose()?;
    if smoke && explicit.is_none() {
        anyhow::bail!("--smoke-test 必须配合 --data-dir 指定隔离的测试目录");
    }
    let directory = if let Some(path) = &explicit {
        PathBuf::from(path)
    } else if config.exists() {
        serde_json::from_slice::<PathBuf>(&std::fs::read(&config)?)?
    } else {
        project.data_dir().to_path_buf()
    };
    Ok((
        directory.join("studyapp.sqlite3"),
        explicit.is_none().then_some(config),
        smoke,
    ))
}

fn startup_error_argument_is_headless(argument: &str, ui_test: bool) -> bool {
    matches!(
        argument,
        "--maintenance-apply" | "--maintenance-execution-smoke"
    ) || (ui_test
        && matches!(
            argument,
            "--smoke-test" | "--maintenance-smoke" | "--maintenance-staged-uninstall-smoke"
        ))
}
fn main() {
    if let Some(code) = update_install::dispatch() {
        std::process::exit(code);
    }
    if let Err(error) = run() {
        eprintln!("WW-StudyApp: {error:#}");
        if std::env::args()
            .any(|arg| startup_error_argument_is_headless(&arg, cfg!(feature = "ui-test")))
        {
            std::process::exit(1);
        }
        rfd::MessageDialog::new()
            .set_title("WW-StudyApp 启动失败")
            .set_description(format!("{error:#}"))
            .set_level(rfd::MessageLevel::Error)
            .show();
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    // Maintenance must dispatch before paths(), smoke preparation, Worker and
    // any database/preferences/editor/network initialization.
    let executable = std::env::current_exe()?;
    let basename = executable
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("应用入口文件名无效"))?;
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    #[cfg(all(windows, feature = "ui-test"))]
    if maintenance_execute::run_acceptance_if_requested(&arguments)? {
        return Ok(());
    }
    #[cfg(all(windows, feature = "ui-test"))]
    if maintenance_ui::run_smoke_if_requested(&arguments)? {
        return Ok(());
    }
    match studyapp_desktop::maintenance::parse_entry_mode(basename, &arguments)? {
        studyapp_desktop::maintenance::EntryMode::Normal => {}
        studyapp_desktop::maintenance::EntryMode::UninstallInteractive => {
            if let Some(recovery) = maintenance_execute::discover_recovery(&executable)? {
                return maintenance_ui::run_recovery(recovery);
            }
            return maintenance_ui::run_interactive(
                studyapp_desktop::maintenance::maintenance_data_directory(&arguments)?,
            );
        }
        studyapp_desktop::maintenance::EntryMode::MaintenanceApply(transaction) => {
            return maintenance_handoff::run_apply(&transaction);
        }
    }
    if !maintenance_handoff::initialize_health_channel()?
        && let Some(recovery) = maintenance_execute::discover_recovery(&executable)?
    {
        return maintenance_ui::run_recovery(recovery);
    }
    let (database, config, smoke_test) = paths()?;
    if smoke_test {
        smoke::prepare(&database)?;
    }
    let worker = worker::Worker::start(database.clone(), config);
    #[cfg(target_os = "linux")]
    let desktop = gpui_kit::platform::linux(WindowingModes::X11);
    #[cfg(not(target_os = "linux"))]
    let desktop = application();
    desktop.with_assets(assets::StudyAssets).run(move |cx| {
        init(cx);
        Theme::change(ThemeMode::Light, None, cx);
        let theme = Theme::global_mut(cx);
        theme.font_family = design::UI_FONT.into();
        theme.font_size = px(14.);
        theme.colors.primary = rgb(components::GREEN).into();
        theme.colors.primary_foreground = rgb(0xFFFFFF).into();
        theme.colors.ring = rgb(components::GREEN).into();
        let options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some("WW-StudyApp".into()),
                appears_transparent: true,
                ..Default::default()
            }),
            app_owns_titlebar_drag: true,
            // DWM owns this material; no CPU blur or per-frame backdrop
            // capture. Opaque content surfaces cover it outside navigation.
            window_background: WindowBackgroundAppearance::MicaBackdrop,
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(1280.), px(860.)),
                cx,
            ))),
            window_min_size: Some(size(px(1000.), px(700.))),
            ..Default::default()
        };
        let (window, view) = open_window(options, cx, |window, cx| {
            cx.new(|cx| app::StudyApp::new(worker, window, cx))
        })
        .expect("创建 WW-StudyApp 窗口失败");
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        if smoke_test {
            smoke::exercise(window, view, database, cx);
        }
        cx.activate(true);
    });
    Ok(())
}

#[cfg(test)]
mod startup_error_tests {
    #[test]
    fn maintenance_smoke_startup_errors_are_headless_only_in_test_builds() {
        for argument in [
            "--smoke-test",
            "--maintenance-smoke",
            "--maintenance-staged-uninstall-smoke",
        ] {
            assert!(
                super::startup_error_argument_is_headless(argument, true),
                "{argument}"
            );
            assert!(
                !super::startup_error_argument_is_headless(argument, false),
                "{argument}"
            );
        }
        for argument in ["--maintenance-apply", "--maintenance-execution-smoke"] {
            for test_build in [true, false] {
                assert!(super::startup_error_argument_is_headless(
                    argument, test_build
                ));
            }
        }
        for argument in [
            "--data-dir",
            "--uninstall",
            "--smoke-test-extra",
            "ordinary",
        ] {
            for test_build in [true, false] {
                assert!(!super::startup_error_argument_is_headless(
                    argument, test_build
                ));
            }
        }
    }
}
