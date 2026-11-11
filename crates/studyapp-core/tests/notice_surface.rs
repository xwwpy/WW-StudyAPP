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

#[test]
fn legacy_full_surface_migrates_without_freezing_inherited_or_losing_auto_colors() {
    let mut appearance = studyapp_core::NotificationAppearance {
        background: Some("#112233".into()),
        ..Default::default()
    };
    let style = studyapp_core::NoticeStyle::CornerCard;
    let mut legacy = appearance.surface(style);
    legacy.background = None;
    legacy.width = 420;
    appearance.surfaces.insert(style, legacy.clone());
    let patch = appearance.surface_patch_mut(style);
    assert_eq!(patch.background.as_deref(), Some(""));
    patch.radius = None;
    assert!(!appearance.surfaces.contains_key(&style));
    assert_eq!(appearance.surface(style).background, None);
    assert_eq!(appearance.surface(style).width, 420);
    appearance.radius = 24;
    assert_eq!(appearance.surface(style).radius, 24);
    appearance.surface_patch_mut(style).background = None;
    assert_eq!(
        appearance.surface(style).background.as_deref(),
        Some("#112233")
    );
    appearance.validate().unwrap();
}

#[test]
fn component_density_is_optional_and_legacy_reset_preserves_other_overrides() {
    use studyapp_core::{NoticeDensity, NoticeStyle, NotificationAppearance};
    for style in NoticeStyle::ALL {
        assert_eq!(
            style.supports_density(),
            matches!(style, NoticeStyle::CornerCard | NoticeStyle::PopupMenu)
        );
    }
    for style in [NoticeStyle::CornerCard, NoticeStyle::PopupMenu] {
        let mut appearance = NotificationAppearance::default();
        assert_eq!(
            appearance.surface(style).density,
            NoticeDensity::Comfortable
        );
        appearance.density = NoticeDensity::Compact;
        assert_eq!(appearance.surface(style).density, NoticeDensity::Compact);
        let mut legacy = appearance.surface(style);
        legacy.density = NoticeDensity::Comfortable;
        legacy.width = 444;
        appearance.surfaces.insert(style, legacy);
        appearance.surface_patch_mut(style).density = None;
        assert_eq!(appearance.surface(style).density, NoticeDensity::Compact);
        assert_eq!(appearance.surface(style).width, 444);
        appearance.density = NoticeDensity::Comfortable;
        assert_eq!(
            appearance.surface(style).density,
            NoticeDensity::Comfortable
        );
        appearance.surface_patch_mut(style).density = Some(NoticeDensity::Compact);
        assert_eq!(appearance.surface(style).density, NoticeDensity::Compact);
    }
}
