use studyapp_core::*;
#[test]
fn legacy_notification_appearance_retains_defaults_and_type_overrides_are_independent() {
    let mut value: NotificationAppearance =
        serde_json::from_str(r#"{"style":"bubble","width":300,"radius":8}"#).unwrap();
    value.validate().unwrap();
    let before = value.surface(NoticeStyle::CornerCard);
    let toast = value.surface_mut(NoticeStyle::Bubble);
    toast.opacity = 60;
    toast.width = 280;
    toast.border_color = Some("#12345680".into());
    assert_eq!(value.surface(NoticeStyle::CornerCard), before);
    assert_eq!(value.surface(NoticeStyle::Bubble).opacity, 60);
    let restored: NotificationAppearance =
        serde_json::from_str(&serde_json::to_string(&value).unwrap()).unwrap();
    assert_eq!(restored, value);
}
#[test]
fn component_geometry_and_colors_are_bounded_and_closed() {
    let mut surface = NoticeSurface {
        background: Some("url(javascript:bad)".into()),
        ..Default::default()
    };
    assert!(surface.validate().is_err());
    surface.background = Some("#12345678".into());
    surface.validate().unwrap();
    for opacity in [0, 19, 101, 255] {
        surface.opacity = opacity;
        assert!(surface.validate().is_err());
    }
    let appearance = NotificationAppearance {
        content: NoticeContent {
            title: false,
            summary: false,
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(appearance.validate().is_err());
}
#[test]
fn brief_and_detailed_presets_have_distinct_fields_and_queue_is_single_card() {
    assert!(!NoticeContent::brief().detailed);
    assert!(!NoticeContent::brief().event_id);
    assert!(NoticeContent::detailed().event_id);
    let mut appearance = NotificationAppearance {
        max_visible: 4,
        ..Default::default()
    };
    assert_eq!(appearance.visible_limit(), 4);
    appearance.stack = NoticeStack::Queue;
    assert_eq!(appearance.visible_limit(), 1);
}
#[test]
fn selection_customizations_are_serializable_and_legacy_safe() {
    let mut settings: AppearanceSettings = serde_json::from_str("{}").unwrap();
    settings.selection.selected_background = Some("#225577CC".into());
    settings.selection.idle_text = Some("#556677".into());
    settings.validate().unwrap();
    let restored: AppearanceSettings =
        serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
    assert_eq!(restored.selection, settings.selection);
    settings.selection.radius = 255;
    assert!(settings.validate().is_err());
}
