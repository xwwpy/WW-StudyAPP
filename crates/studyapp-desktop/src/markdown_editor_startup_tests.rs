//! Only the editor state is created. The fixture never mounts a native WebView.
use super::MarkdownEditor;
use gpui_kit::{AppContext, Context, Entity, IntoElement, Render, TestAppContext, Window, div};

struct Fixture(Entity<MarkdownEditor>);
impl Render for Fixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}
fn fixture(window: &mut Window, cx: &mut Context<Fixture>) -> Fixture {
    Fixture(cx.new(|cx| {
        let mut editor = MarkdownEditor::new(window, cx);
        editor.set_value_with_readonly("# 首次阅读\n\n保留中文与草稿。".into(), true, window, cx);
        editor
    }))
}

#[gpui_kit::test]
fn ready_during_native_construction_waits_without_poisoning_the_document(cx: &mut TestAppContext) {
    let (fixture, visual) = cx.add_window_view(fixture);
    let editor = fixture.read_with(visual, |fixture, _| fixture.0.clone());
    editor.update(visual, |editor, cx| {
        let original = (
            editor.value().to_owned(),
            editor.generation(),
            editor.revision(),
        );
        editor.creating = true;
        editor.visible = true;
        editor.sender.send(r#"{"type":"ready"}"#.into()).unwrap();
        editor.poll(cx);
        assert!(
            editor.error().is_none(),
            "early Ready must not evaluate against a missing host: {:?}",
            editor.error()
        );
        assert!(!editor.ready());
        assert_eq!(editor.receiver.try_recv().unwrap(), r#"{"type":"ready"}"#);
        assert_eq!(
            (
                editor.value().to_owned(),
                editor.generation(),
                editor.revision()
            ),
            original
        );
        assert!(editor.readonly());
        assert!(editor.theme.is_null());
        assert!(editor.host.is_none());
    });
}

#[gpui_kit::test]
fn migration_revokes_the_old_host_channel_before_a_late_ready_or_error(cx: &mut TestAppContext) {
    let (fixture, visual) = cx.add_window_view(fixture);
    let editor = fixture.read_with(visual, |fixture, _| fixture.0.clone());
    editor.update(visual, |editor, cx| {
        let old = editor.sender.clone();
        old.send(r#"{"type":"ready"}"#.into()).unwrap();
        let generation = editor.generation();
        editor.suspend_for_migration(true, cx);
        assert!(old.send(r#"{"type":"ready"}"#.into()).is_err(), "a retired WebView must lose access to the active receiver");
        assert!(old.send(serde_json::json!({"type":"error","generation":generation,"message":"retired host"}).to_string()).is_err());
        editor.poll(cx);
        assert!(!editor.ready());
        assert!(editor.error().is_none());
        assert!(editor.receiver.try_recv().is_err());
        assert_eq!(editor.generation(), generation);
        assert_eq!(editor.value(), "# 首次阅读\n\n保留中文与草稿。");
    });
}

#[gpui_kit::test]
fn queued_ready_keeps_the_latest_document_theme_and_readonly_intent(cx: &mut TestAppContext) {
    let (fixture, visual) = cx.add_window_view(fixture);
    let editor = fixture.read_with(visual, |fixture, _| fixture.0.clone());
    for (text, readonly, dark) in [
        ("# 正文\n\n首次打开。", true, false),
        ("# 草稿\n\n尚未保存。", false, true),
        (crate::appearance_preview::PREVIEW_SAMPLE, true, true),
    ] {
        visual.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.reset_host_messages();
                editor.creating = true;
                editor.sender.send(r#"{"type":"ready"}"#.into()).unwrap();
                // set_value itself polls; an early Ready must not turn that
                // operation into an evaluate() against a not-yet-attached host.
                editor.set_value_with_readonly(text.into(), readonly, window, cx);
                let generation = editor.generation();
                let theme = crate::theme::resolve(&Default::default(), dark).editor_tokens();
                editor.set_theme(theme.clone(), cx);
                editor.set_visible(true, cx);
                editor.set_visible(false, cx);
                editor.set_visible(true, cx);
                editor.poll(cx);
                assert_eq!(editor.value(), text);
                assert_eq!(editor.generation(), generation);
                assert_eq!(editor.revision(), 0);
                assert_eq!(editor.readonly(), readonly);
                assert_eq!(editor.theme, theme);
                assert!(editor.error().is_none());
                assert!(!editor.ready());
                assert_eq!(editor.receiver.try_recv().unwrap(), r#"{"type":"ready"}"#);
                assert!(editor.receiver.try_recv().is_err());
                assert!(editor.host.is_none());
            });
        });
    }
}

#[gpui_kit::test]
fn a_never_shown_constructing_editor_can_flush_without_losing_the_draft(cx: &mut TestAppContext) {
    let (fixture, visual) = cx.add_window_view(fixture);
    let editor = fixture.read_with(visual, |fixture, _| fixture.0.clone());
    visual.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            editor.set_value_with_readonly("草稿：输入之前保留原文".into(), false, window, cx);
            editor.creating = true;
            editor.visible = true;
            editor.sender.send(r#"{"type":"ready"}"#.into()).unwrap();
            editor.poll(cx);
            let generation = editor.generation();
            let request = editor.request_flush(cx).unwrap();
            assert!(request > 0);
            assert!(editor.pending.is_none());
            assert!(!editor.ready());
            assert!(editor.readonly());
            assert!(editor.error().is_none());
            assert_eq!(editor.generation(), generation);
            assert_eq!(editor.value(), "草稿：输入之前保留原文");
            assert_eq!(editor.receiver.try_recv().unwrap(), r#"{"type":"ready"}"#);
        });
    });
}

#[gpui_kit::test]
fn retiring_a_host_discards_queued_changes_without_resetting_the_document(cx: &mut TestAppContext) {
    let (fixture, visual) = cx.add_window_view(fixture);
    let editor = fixture.read_with(visual, |fixture, _| fixture.0.clone());
    editor.update(visual, |editor, cx| {
        let original = (
            editor.value().to_owned(),
            editor.generation(),
            editor.revision(),
        );
        let old = editor.sender.clone();
        old.send(
            serde_json::json!({"type":"change","generation":editor.generation(),
            "baseRevision":0,"revision":1,"changes":[{"from":0,"to":0,"insert":"旧窗口"}]})
            .to_string(),
        )
        .unwrap();
        old.send(r#"{"type":"ready"}"#.into()).unwrap();
        editor.suspend_for_migration(true, cx);
        editor.suspend_for_migration(false, cx);
        assert!(editor.receiver.try_recv().is_err());
        assert!(old.send(r#"{"type":"ready"}"#.into()).is_err());
        editor.sender.send(r#"{"type":"ready"}"#.into()).unwrap();
        editor.poll(cx);
        assert_eq!(editor.receiver.try_recv().unwrap(), r#"{"type":"ready"}"#);
        assert_eq!(
            (
                editor.value().to_owned(),
                editor.generation(),
                editor.revision()
            ),
            original
        );
        assert!(editor.error().is_none());
        assert!(!editor.ready());
    });
}
