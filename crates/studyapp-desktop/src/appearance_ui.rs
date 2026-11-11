//! Resolves one shared palette for the native shell and persistent editor.
use crate::app::StudyApp;
use gpui_kit::*;

impl StudyApp {
    pub fn refresh_appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let settings = self.effective_appearance();
        let system_dark = crate::theme::system_is_dark(window.appearance());
        if self
            .appearance_source
            .as_ref()
            .is_some_and(|(previous, dark)| previous == &settings && *dark == system_dark)
        {
            return;
        }
        let next = crate::theme::resolve(&settings, system_dark);
        self.appearance_source = Some((settings.clone(), system_dark));
        let palette_changed = self.applied_appearance != Some(next);
        // Menu/Popover appearance also lives in the shared theme, but most of
        // it is deliberately absent from the native/editor palette tokens.
        if palette_changed || crate::theme::notification_appearance() != settings.notifications {
            crate::theme::apply(&settings, system_dark, cx);
            self.applied_appearance = Some(next);
            if palette_changed {
                self.note_content
                    .update(cx, |editor, cx| editor.set_theme(next.editor_tokens(), cx));
            }
            cx.notify();
        }
    }
}
