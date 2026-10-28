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
        if self.applied_appearance != Some(next) {
            crate::theme::apply(&settings, system_dark, cx);
            self.applied_appearance = Some(next);
            self.note_content
                .update(cx, |editor, cx| editor.set_theme(next.editor_tokens(), cx));
            cx.notify();
        }
    }
}
