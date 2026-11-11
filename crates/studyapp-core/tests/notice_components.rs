use studyapp_core::*;
fn environment() -> NoticeEnvironment {
    NoticeEnvironment {
        foreground: true,
        desktop_available: false,
        anchor_available: true,
        minute: 720,
    }
}
#[test]
fn interactive_components_cannot_become_events_or_hide_critical_risk() {
    let mut policy = NotificationSettings::default();
    let appearance = NotificationAppearance::default();
    for style in NoticeStyle::ALL {
        policy.rules.insert(
            NoticeCategory::OperationResult,
            NoticeRule {
                style: Some(style),
                ..Default::default()
            },
        );
        let route = policy.route(
            NoticeCategory::OperationResult,
            NoticeSeverity::Information,
            &appearance,
            environment(),
        );
        let NoticeRoute::Foreground(actual) = route else {
            panic!("foreground event was lost");
        };
        assert!(
            NoticeCategory::OperationResult
                .notification_styles()
                .contains(&actual)
        );
        assert_eq!(
            policy.route(
                NoticeCategory::OperationResult,
                NoticeSeverity::Critical,
                &appearance,
                environment()
            ),
            NoticeRoute::Foreground(NoticeStyle::ModalCard)
        );
        assert_ne!(
            policy.route(
                NoticeCategory::OperationResult,
                NoticeSeverity::Error,
                &appearance,
                environment()
            ),
            NoticeRoute::Foreground(NoticeStyle::Bubble)
        );
    }
}
#[test]
fn appearance_editor_does_not_route_events_and_real_actions_use_snackbar() {
    let policy = NotificationSettings::default();
    let mut appearance = NotificationAppearance::default();
    let mut event = NoticeEvent::new(
        NoticeCategory::OperationResult,
        NoticeSeverity::Success,
        "操作完成",
    );
    for style in NoticeStyle::ALL {
        appearance.style = style;
        assert_eq!(
            policy.route_event(&event, &appearance, environment()),
            NoticeRoute::Foreground(NoticeStyle::Bubble)
        );
    }
    event.action = Some(NoticeAction::OpenLibrary);
    assert_eq!(
        policy.route_event(&event, &appearance, environment()),
        NoticeRoute::Foreground(NoticeStyle::Snackbar)
    );
}
#[test]
fn overrides_remain_inherited_per_property_and_survive_serialization() {
    let mut value = NotificationAppearance::default();
    value.surface_patch_mut(NoticeStyle::CornerCard).opacity = Some(60);
    value.width = 440;
    value.background = Some("#123456".into());
    let surface = value.surface(NoticeStyle::CornerCard);
    assert_eq!(surface.opacity, 60);
    assert_eq!(surface.width, 440);
    assert_eq!(surface.background.as_deref(), Some("#123456"));
    value.surface_patch_mut(NoticeStyle::CornerCard).background = Some(String::new());
    assert!(value.surface(NoticeStyle::CornerCard).background.is_none());
    value.surface_patch_mut(NoticeStyle::CornerCard).background = None;
    assert_eq!(
        value.surface(NoticeStyle::CornerCard).background,
        value.background
    );
    let restored: NotificationAppearance =
        serde_json::from_str(&serde_json::to_string(&value).unwrap()).unwrap();
    assert_eq!(restored, value);
    restored.validate().unwrap();
}
#[test]
fn scenario_fields_inherit_defaults_and_preserve_risk_and_component_structure() {
    let mut value = NotificationAppearance::default();
    value.scenario_fields.insert(
        NoticeCategory::OperationResult,
        NoticeContentPatch {
            icon: Some(false),
            ..Default::default()
        },
    );
    value
        .component_fields
        .insert(NoticeStyle::CornerCard, NoticeContent::brief());
    let effective = value.effective_content(
        NoticeStyle::CornerCard,
        Some(NoticeCategory::OperationResult),
        NoticeSeverity::Success,
    );
    assert!(!effective.icon && !effective.detailed);
    value
        .component_fields
        .get_mut(&NoticeStyle::CornerCard)
        .unwrap()
        .detailed = true;
    assert!(
        value
            .effective_content(
                NoticeStyle::CornerCard,
                Some(NoticeCategory::OperationResult),
                NoticeSeverity::Success
            )
            .detailed
    );
    value.scenario_fields.insert(
        NoticeCategory::SaveFailure,
        NoticeContentPatch {
            summary: Some(false),
            action: Some(false),
            ..Default::default()
        },
    );
    let required = value.effective_content(
        NoticeStyle::CornerCard,
        Some(NoticeCategory::SaveFailure),
        NoticeSeverity::Error,
    );
    assert!(required.summary && required.action);
    let toast = value.effective_content(NoticeStyle::Bubble, None, NoticeSeverity::Success);
    assert!(!toast.title && !toast.action && !toast.event_id);
}
