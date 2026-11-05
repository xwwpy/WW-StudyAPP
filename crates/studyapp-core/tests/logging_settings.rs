use serde_json::json;
use studyapp_core::Settings;

fn defaults() -> serde_json::Value {
    json!({"enabled":true,"minimum_level":"info","directory":null})
}
#[test]
fn logging_is_explicitly_enabled_at_info_without_changing_notifications() {
    let encoded = serde_json::to_value(Settings::default()).unwrap();
    assert_eq!(encoded.get("logging"), Some(&defaults()));
    assert_eq!(
        encoded["notifications"],
        serde_json::to_value(studyapp_core::NotificationSettings::default()).unwrap()
    );
}
#[test]
fn missing_legacy_logging_defaults_only_in_memory() {
    let mut encoded = serde_json::to_value(Settings::default()).unwrap();
    encoded.as_object_mut().unwrap().remove("logging");
    let settings: Settings = serde_json::from_value(encoded.clone()).unwrap();
    assert_eq!(
        serde_json::to_value(settings).unwrap()["logging"],
        defaults()
    );
    assert!(encoded.get("logging").is_none());
}
#[test]
fn malformed_logging_fields_fail_instead_of_silently_disabling_diagnostics() {
    for config in [
        json!({"enabled":true,"minimum_level":"verbose"}),
        json!({"enabled":true,"unknown":true}),
    ] {
        let mut encoded = serde_json::to_value(Settings::default()).unwrap();
        encoded["logging"] = config;
        assert!(serde_json::from_value::<Settings>(encoded).is_err());
    }
    for directory in [
        "",
        "relative/path",
        "/logs/../private",
        r"C:\logs\..\private",
        "/logs\nprivate",
    ] {
        let mut encoded = serde_json::to_value(Settings::default()).unwrap();
        encoded["logging"] = defaults();
        encoded["logging"]["directory"] = directory.into();
        let settings: Settings = serde_json::from_value(encoded).unwrap();
        assert!(settings.validate().is_err(), "invalid directory accepted");
    }
}

#[test]
fn level_threshold_is_inclusive_and_disable_is_independent() {
    use studyapp_core::{LogLevel, LoggingSettings};
    for minimum in LogLevel::ALL {
        let mut settings = LoggingSettings {
            minimum_level: minimum,
            ..Default::default()
        };
        for level in LogLevel::ALL {
            assert_eq!(settings.records(level), level <= minimum);
        }
        settings.enabled = false;
        assert!(
            LogLevel::ALL
                .into_iter()
                .all(|level| !settings.records(level))
        );
    }
}
#[test]
fn portable_absolute_directories_validate_without_accessing_them() {
    use studyapp_core::LoggingSettings;
    for path in [
        "/var/log/studyapp",
        r"C:/Users/Example/logs",
        r"D:/日志",
        "//server/share/logs",
    ] {
        let settings = LoggingSettings {
            directory: Some(path.into()),
            ..Default::default()
        };
        settings.validate().unwrap();
    }
    for path in [
        " /logs",
        "/logs ",
        "/logs/../x",
        "/logs/".repeat(1000).as_str(),
    ] {
        assert!(
            LoggingSettings {
                directory: Some(path.into()),
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }
}
