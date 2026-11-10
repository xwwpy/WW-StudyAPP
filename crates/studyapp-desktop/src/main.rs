#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod app_migration;
mod app_startup;
mod app_usage;
mod appearance_preview;
mod appearance_ui;
mod assets;
mod audit_log;
mod card_window;
mod components;
mod controls;
mod credentials;
mod design;
mod document_renderer;
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
mod installation_integrity;
mod integrity_ui;
mod library_owner;
mod library_owner_ui;
mod logging_settings;
mod maintenance_execute;
mod maintenance_handoff;
mod maintenance_runtime;
mod maintenance_ui;
mod markdown_editor;
mod media;
mod media_ui;
mod native_shortcuts;
#[cfg(any(
    all(windows, feature = "ui-test"),
    all(target_os = "macos", feature = "update-acceptance")
))]
#[path = "update_acceptance_pointer.rs"]
mod native_test_pointer;
mod note_actions;
mod note_commands;
mod note_editor;
mod notes_view;
mod notice_surface_settings;
mod notification_desktop;
mod notification_runtime;
mod notification_settings;
mod notification_sound;
mod notifications;
mod overlay_regions;
mod owned_poll;
mod preferences;
mod resize_overlay;
mod selection_settings;
mod settings_cache;
mod settings_color;
mod settings_components;
mod settings_lists;
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
mod smoke_logging;
#[cfg(all(windows, feature = "ui-test"))]
mod smoke_note_rename;
#[cfg(all(windows, feature = "ui-test"))]
mod smoke_notifications;
#[cfg(all(windows, feature = "ui-test"))]
mod smoke_settings;
#[cfg(all(windows, feature = "ui-test"))]
mod smoke_statistics;
#[cfg(all(windows, feature = "ui-test"))]
mod smoke_storage_performance;
#[cfg(all(feature = "ui-test", not(test)))]
mod smoke_timer_pin;
#[cfg(all(windows, feature = "ui-test"))]
mod smoke_timer_widget;
#[cfg(all(windows, feature = "ui-test"))]
mod smoke_updates;
#[cfg(all(windows, feature = "ui-test"))]
mod smoke_visibility;
#[cfg(all(windows, feature = "ui-test"))]
mod smoke_window_drag;
#[cfg(all(windows, feature = "ui-test"))]
mod smoke_workspace;
mod snapshot_settings;
mod storage_backend;
mod storage_policy;
mod storage_tree;
mod storage_ui;
mod sync_runtime;
mod sync_ui;
mod theme;
mod timer_widget;
mod timer_widget_pin;
mod timer_widget_ui;
mod timer_widget_window;
#[cfg(all(test, feature = "ui-test"))]
mod today_timer_tests;
#[cfg(feature = "update-acceptance")]
mod update_acceptance;
mod update_backend;
mod update_install;
mod update_manifest;
mod update_nsis;
mod update_read_state;
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

    let explicit = args
        .iter()
        .position(|a| a == "--data-dir")
        .map(|index| {
            args.get(index + 1)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("--data-dir 后需要目录路径"))
        })
        .transpose()?;
    #[cfg(feature = "ui-test")]
    if smoke
        && explicit.is_none()
        && let Some(i) = args.iter().position(|a| a == "--smoke-app-dir")
    {
        anyhow::ensure!(
            std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true"),
            "应用位置交互测试仅在 CI 运行"
        );
        let directory = PathBuf::from(
            args.get(i + 1)
                .ok_or_else(|| anyhow::anyhow!("缺少隔离测试目录"))?,
        );
        anyhow::ensure!(
            directory.is_absolute() && !directory.exists(),
            "测试目录必须是新的绝对路径"
        );
        let paths = app_usage::runtime_paths()?;
        app_migration::remember_library(&paths.legacy_location(), &directory)?;
        return Ok((
            directory.join("studyapp.sqlite3"),
            Some(paths.legacy_location()),
            true,
        ));
    }
    if smoke && explicit.is_none() {
        anyhow::bail!("--smoke-test 必须配合 --data-dir 指定隔离的测试目录");
    }
    // Explicit profiles bypass all per-user locator reads.
    let (directory, config) = if let Some(path) = &explicit {
        (PathBuf::from(path), None)
    } else {
        let paths = studyapp_desktop::app_paths::AppPaths::current()?;
        let library = paths.resolve_library(None)?;
        if paths.configured_library().is_some() {
            anyhow::ensure!(
                library.join("studyapp.sqlite3").is_file(),
                "配置的资料库不可用，请连接原磁盘后重试"
            );
        }
        (library, Some(paths.legacy_location()))
    };
    Ok((directory.join("studyapp.sqlite3"), config, smoke))
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
    #[cfg(feature = "update-acceptance")]
    update_acceptance::install_failure_hook();
    if let Err(error) = run() {
        #[cfg(feature = "update-acceptance")]
        update_acceptance::startup_failure(&format!("{error:#}"));
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
            app_migration::ensure_no_pending()?;
            if let Some(recovery) = maintenance_execute::discover_recovery(&executable)? {
                return maintenance_ui::run_recovery(recovery);
            }
            return maintenance_ui::run_interactive(
                studyapp_desktop::maintenance::maintenance_data_directory(&arguments)?,
            );
        }
        studyapp_desktop::maintenance::EntryMode::MaintenanceApply(transaction) => {
            app_migration::ensure_no_pending()?;
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
    #[cfg(feature = "update-acceptance")]
    if update_acceptance::active() && !database.exists() {
        smoke::prepare(&database)?;
    }
    let resume = if config.is_some() {
        app_migration::pending(&app_usage::runtime_paths()?)?
    } else {
        None
    };
    app_migration::set_startup_pending(resume.is_some());
    // A thread failure must not prevent the library or application from opening.
    let mut logging = audit_log::initialize(&database, config.is_some()).ok();

    let worker = worker::Worker::start_resuming(database.clone(), config, resume);
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
        #[cfg(feature = "update-acceptance")]
        if update_acceptance::active() {
            update_acceptance::exercise(window, view.clone(), database.clone(), cx);
        }
        if smoke_test {
            smoke::exercise(window, view, database, cx);
        }
        cx.activate(true);
    });
    audit_log::stage(
        studyapp_core::LogLevel::Info,
        audit_log::Phase::ApplicationStop,
        audit_log::Outcome::Success,
    );
    if let Some(runtime) = &mut logging {
        let _ = runtime.shutdown(std::time::Duration::from_secs(2));
    }
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
