//! Runner-only native settings transaction and actual log-output acceptance.
use crate::{
    app::{Page, StudyApp},
    settings_ui::SettingsSection,
};
use gpui_kit::{test::TestWindowExt, *};
use std::{
    path::Path,
    time::{Duration, Instant},
};
use studyapp_core::{LogLevel, LoggingSettings};

async fn saved(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    expected: &LoggingSettings,
    cx: &mut AsyncApp,
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let done = window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                let app = view.read(cx);
                !app.preferences_ui.busy
                    && !app.settings_dirty(cx)
                    && app
                        .preferences_ui
                        .saved
                        .as_ref()
                        .is_some_and(|saved| saved.settings.logging == *expected)
            })
            .unwrap();
        if done {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "native logging preferences did not commit within five seconds"
        );
        cx.background_executor()
            .timer(Duration::from_millis(50))
            .await;
    }
}
fn read_records(directory: &Path) -> Vec<serde_json::Value> {
    let mut records = Vec::new();
    for file in crate::audit_log::inventory(directory).unwrap() {
        if file
            .extension()
            .is_some_and(|extension| extension == "jsonl")
        {
            for line in std::fs::read_to_string(file).unwrap().lines() {
                let value: serde_json::Value = serde_json::from_str(line).unwrap();
                if value.get("phase").is_some() {
                    records.push(value);
                }
            }
        }
    }
    records
}
pub async fn exercise(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    database: &Path,
    cx: &mut AsyncApp,
) {
    assert_eq!(
        std::env::var("GITHUB_ACTIONS").as_deref(),
        Ok("true"),
        "native logging acceptance is runner-only"
    );
    let directory = database.with_file_name(format!("native-logs-{}", uuid::Uuid::new_v4()));
    let custom = LoggingSettings {
        enabled: true,
        minimum_level: LogLevel::Trace,
        directory: Some(directory.to_string_lossy().into_owned()),
    };
    let original = window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.navigate(Page::Settings, window, cx);
                app.select_settings_section(SettingsSection::Logging, cx);
                app.clear_notices(cx);
                app.preferences_ui
                    .saved
                    .as_ref()
                    .unwrap()
                    .settings
                    .logging
                    .clone()
            })
        })
        .unwrap();
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            crate::smoke_settings::select_choice(window, "logging-level", 4, cx);
        })
        .unwrap();
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.preferences_ui.draft.as_mut().unwrap().logging.enabled = true;
                app.preferences_ui
                    .logging_directory
                    .as_ref()
                    .unwrap()
                    .update(cx, |input, cx| {
                        input.set_value(custom.directory.clone().unwrap(), window, cx)
                    });
                assert!(app.settings_dirty(cx));
                assert_eq!(
                    app.preferences_ui.saved.as_ref().unwrap().settings.logging,
                    original
                );
                assert!(
                    !directory.exists(),
                    "unsaved directory choice performed I/O"
                );
                cx.notify();
            });
            window.render_frame(cx);
            crate::smoke::capture(window, database.with_file_name("logging-settings.png")).unwrap();
            window.click("save-settings", cx);
        })
        .unwrap();
    saved(window, view, &custom, cx).await;
    let logger = crate::audit_log::global().expect("production logger in runner process");
    let mut event = studyapp_core::NoticeEvent::new(
        studyapp_core::NoticeCategory::SaveFailure,
        studyapp_core::NoticeSeverity::Error,
        "通知与日志关联样例",
    );
    event.dedupe_key = Some("native-logging-sample".into());
    let event_id = event.id;
    window
        .update(cx, |_, _, cx| {
            view.update(cx, |app, cx| {
                // Suppress the source category in memory only. The event must still log.
                let mut policy = app
                    .preferences_ui
                    .saved
                    .as_ref()
                    .unwrap()
                    .settings
                    .notifications
                    .clone();
                policy
                    .rules
                    .entry(studyapp_core::NoticeCategory::SaveFailure)
                    .or_insert_with(|| studyapp_core::NoticeCategory::SaveFailure.default_rule())
                    .enabled = false;
                app.notifications.update(cx, |center, cx| {
                    center.configure(
                        &policy,
                        &Default::default(),
                        studyapp_core::NoticeEnvironment {
                            foreground: true,
                            desktop_available: false,
                            anchor_available: false,
                            minute: 720,
                        },
                        false,
                        cx,
                    )
                });
                assert!(app.emit_notification(event, cx).is_none());
            })
        })
        .unwrap();
    assert!(logger.flush(Duration::from_secs(5)));
    let records = read_records(&directory);
    assert!(
        records
            .iter()
            .any(|record| record["event"] == event_id.to_string()
                && record["phase"] == "business_outcome")
    );
    let disabled = LoggingSettings {
        enabled: false,
        ..custom.clone()
    };
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("logging-enabled", cx);
            window.click("save-settings", cx);
        })
        .unwrap();
    saved(window, view, &disabled, cx).await;
    assert!(logger.flush(Duration::from_secs(5)));
    let before = read_records(&directory);
    assert!(!logger.record(crate::audit_log::Record::new(
        LogLevel::Error,
        crate::audit_log::Phase::BusinessOutcome,
        crate::audit_log::Outcome::Failure
    )));
    assert_eq!(read_records(&directory), before);
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.preferences_ui.draft.as_mut().unwrap().logging = original.clone();
                app.preferences_ui
                    .logging_directory
                    .as_ref()
                    .unwrap()
                    .update(cx, |input, cx| {
                        input.set_value(original.directory.clone().unwrap_or_default(), window, cx)
                    });
                app.save_preferences(cx);
            })
        })
        .unwrap();
    saved(window, view, &original, cx).await;
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| app.sync_notification_preferences(window, cx))
        })
        .unwrap();
    std::fs::write(database.with_file_name("logging-native-result.json"),serde_json::to_vec_pretty(&serde_json::json!({
        "draft_has_no_io":true,"custom_directory_committed":true,"event_recorded_with_notifications_disabled":true,
        "disable_stops_new_records":true,"original_logging_restored":true,"event":event_id.to_string(),"records":records.len()
    })).unwrap()).unwrap();
}
