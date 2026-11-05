use studyapp_core::{
    NoticeCategory as Category, NoticeEnvironment as Environment, NoticeRoute as Route,
    NoticeSeverity as Severity, NoticeStyle as Style, *,
};
fn env(foreground: bool) -> Environment {
    Environment {
        foreground,
        desktop_available: true,
        anchor_available: false,
        minute: 720,
    }
}
#[test]
fn defaults_are_individual_and_do_not_include_required_confirmations_or_system_notifications() {
    let settings = NotificationSettings::default();
    let appearance = NotificationAppearance::default();
    settings.validate().unwrap();
    appearance.validate().unwrap();
    for category in Category::ALL {
        assert_eq!(
            settings.rule(category).enabled,
            category != Category::SaveSuccess
        );
        if category != Category::SaveSuccess {
            assert_eq!(
                settings.route(category, Severity::Error, &appearance, env(false)),
                Route::DesktopCard
            );
        }
    }
    assert_eq!(
        settings.route(
            Category::SaveSuccess,
            Severity::Success,
            &appearance,
            env(true)
        ),
        Route::Suppressed
    );
    assert_eq!(
        settings.route(
            Category::TimerComplete,
            Severity::Information,
            &appearance,
            env(true)
        ),
        Route::Foreground(Style::CornerCard)
    );
    assert_eq!(
        settings.route(
            Category::UpdateAvailable,
            Severity::Information,
            &appearance,
            env(true)
        ),
        Route::Foreground(Style::Bubble)
    );
    assert_eq!(
        settings.route(
            Category::SaveFailure,
            Severity::Critical,
            &appearance,
            env(true)
        ),
        Route::Foreground(Style::ModalCard)
    );
    assert!(Severity::Error.persistent());
    assert!(!Severity::Information.persistent());
    assert!(
        serde_json::from_value::<NoticeStyle>(serde_json::json!("system_notification")).is_err()
    );
}
#[test]
fn switches_styles_anchor_fallback_and_desktop_unavailability_are_explicit() {
    let mut settings = NotificationSettings::default();
    let appearance = NotificationAppearance::default();
    for style in Style::ALL {
        settings.rules.insert(
            Category::IntegrityResult,
            NoticeRule {
                style: Some(style),
                ..Default::default()
            },
        );
        assert_eq!(
            settings.route(
                Category::IntegrityResult,
                Severity::Success,
                &appearance,
                env(true)
            ),
            Route::Foreground(if style == Style::Anchored {
                Style::CornerCard
            } else {
                style
            })
        );
        assert_eq!(
            settings.route(
                Category::IntegrityResult,
                Severity::Success,
                &appearance,
                Environment {
                    anchor_available: true,
                    ..env(true)
                }
            ),
            Route::Foreground(style)
        );
    }
    assert_eq!(
        settings.route(
            Category::IntegrityResult,
            Severity::Error,
            &appearance,
            Environment {
                desktop_available: false,
                ..env(false)
            }
        ),
        Route::Deferred
    );
    let mut rule = settings.rule(Category::IntegrityResult);
    rule.background = false;
    settings.rules.insert(Category::IntegrityResult, rule);
    assert_eq!(
        settings.route(
            Category::IntegrityResult,
            Severity::Error,
            &appearance,
            env(false)
        ),
        Route::Suppressed
    );
    let mut rule = settings.rule(Category::IntegrityResult);
    rule.enabled = false;
    settings.rules.insert(Category::IntegrityResult, rule);
    assert_eq!(
        settings.route(
            Category::IntegrityResult,
            Severity::Critical,
            &appearance,
            env(true)
        ),
        Route::Suppressed
    );
    assert!(
        settings.update_badge,
        "event and red-dot switches must be independent"
    );
}
#[test]
fn quiet_hours_cross_midnight_without_disabling_required_business_checks() {
    let mut settings = NotificationSettings {
        quiet_hours: Some(QuietHours {
            start_minute: 1320,
            end_minute: 420,
        }),
        ..Default::default()
    };
    for minute in [0, 1, 419, 1320, 1439] {
        assert!(settings.quiet(minute));
    }
    for minute in [420, 720, 1319] {
        assert!(!settings.quiet(minute));
    }
    let appearance = NotificationAppearance::default();
    let night = Environment {
        minute: 60,
        ..env(true)
    };
    assert_eq!(
        settings.route(
            Category::TimerComplete,
            Severity::Information,
            &appearance,
            night
        ),
        Route::Suppressed
    );
    assert_eq!(
        settings.route(
            Category::SaveFailure,
            Severity::Critical,
            &appearance,
            night
        ),
        Route::Foreground(Style::ModalCard)
    );
    settings.critical_during_quiet = false;
    assert_eq!(
        settings.route(
            Category::SaveFailure,
            Severity::Critical,
            &appearance,
            night
        ),
        Route::Suppressed
    );
}
#[test]
fn legacy_settings_supply_safe_defaults_and_invalid_styles_remain_rejected() {
    let legacy: Settings = serde_json::from_value(
        serde_json::json!({"interval_minutes":60,"answer_minutes":5,"review_days":[1]}),
    )
    .unwrap();
    assert_eq!(legacy.notifications, NotificationSettings::default());
    assert_eq!(
        legacy.appearance.notifications,
        NotificationAppearance::default()
    );
    for value in [
        serde_json::json!({"duration_seconds":0}),
        serde_json::json!({"max_visible":5}),
        serde_json::json!({"background":"#123456;url(x)"}),
        serde_json::json!({"radius":40}),
    ] {
        let appearance: NotificationAppearance = serde_json::from_value(value).unwrap();
        assert!(appearance.validate().is_err());
    }
    assert!(
        serde_json::from_value::<NotificationSettings>(
            serde_json::json!({"rules":{"imagined_category":{}}})
        )
        .is_err()
    );
    let colors: NotificationAppearance =
        serde_json::from_value(serde_json::json!({"background":"#FFFFFF80","accent":"#11223300"}))
            .unwrap();
    colors.validate().unwrap();
    let bad = NotificationSettings {
        quiet_hours: Some(QuietHours {
            start_minute: 60,
            end_minute: 60,
        }),
        ..Default::default()
    };
    assert!(bad.validate().is_err());
}
#[test]
fn event_ids_are_independent_of_repeated_text_and_payload_is_bounded() {
    let a = NoticeEvent::new(Category::SaveFailure, Severity::Error, "无法保存，请重试");
    let b = NoticeEvent::new(Category::SaveFailure, Severity::Error, "无法保存，请重试");
    assert_ne!(a.id, b.id);
    assert!(a.id.to_string().starts_with("EV-"));
    a.validate().unwrap();
    let mut invalid = a.clone();
    invalid.summary = "x".repeat(8193);
    assert!(invalid.validate().is_err());
    invalid = a.clone();
    invalid.anchor = Some("../private".into());
    assert!(invalid.validate().is_err());
}
