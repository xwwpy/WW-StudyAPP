use studyapp_core::{SelectionAppearance, SelectionCategory, SelectionOverride};

#[test]
fn old_selection_settings_default_to_inheriting_categories() {
    let selection: SelectionAppearance =
        serde_json::from_value(serde_json::json!({"radius": 12, "selected_text": "#123456"}))
            .unwrap();
    assert!(selection.categories.is_empty());
    for category in SelectionCategory::ALL {
        assert_eq!(
            selection.effective(category).selected_text.as_deref(),
            Some("#123456")
        );
        assert_eq!(selection.effective(category).radius, 12);
    }
    selection.validate().unwrap();
}
#[test]
fn category_edits_override_only_their_fields_and_reset_restores_live_inheritance() {
    let mut global = SelectionAppearance::default();
    global.categories.insert(
        SelectionCategory::Library,
        SelectionOverride {
            selected_text: Some("#FF0000".into()),
            ..Default::default()
        },
    );
    global.radius = 17;
    global.idle_text = Some("#00FF00".into());
    let library = global.effective(SelectionCategory::Library);
    assert_eq!(library.radius, 17);
    assert_eq!(library.idle_text, global.idle_text);
    assert_eq!(library.selected_text.as_deref(), Some("#FF0000"));
    assert_eq!(
        global.effective(SelectionCategory::Directory).selected_text,
        None
    );
    global
        .categories
        .get_mut(&SelectionCategory::Library)
        .unwrap()
        .selected_text = None;
    global.selected_text = Some("#0000FF".into());
    assert_eq!(
        global.effective(SelectionCategory::Library).selected_text,
        global.selected_text
    );
    global.validate().unwrap();
    let restored: SelectionAppearance =
        serde_json::from_str(&serde_json::to_string(&global).unwrap()).unwrap();
    assert_eq!(restored, global);
}
#[test]
fn invalid_category_tokens_are_rejected_instead_of_applied() {
    let mut global = SelectionAppearance::default();
    global.categories.insert(
        SelectionCategory::Navigation,
        SelectionOverride {
            radius: Some(25),
            ..Default::default()
        },
    );
    assert!(global.validate().is_err());
    global
        .categories
        .get_mut(&SelectionCategory::Navigation)
        .unwrap()
        .radius = Some(8);
    global
        .categories
        .get_mut(&SelectionCategory::Navigation)
        .unwrap()
        .idle_background = Some("url(javascript:alert(1))".into());
    assert!(global.validate().is_err());
}
