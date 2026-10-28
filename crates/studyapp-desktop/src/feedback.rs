use crate::app::StudyApp;
use gpui_kit::*;
pub use studyapp_ui::feedback::{ModalFocus, modal_frame_shell};

impl StudyApp {
    pub fn push_notice(&mut self, message: Result<String, String>, cx: &mut Context<Self>) {
        // Retain the latest outcome for diagnostic assertions; event delivery
        // goes to the queue, never through text equality or render timing.
        self.notice = Some(message.clone());
        self.notifications
            .update(cx, |center, cx| center.push(message, cx));
    }

    #[cfg(feature = "ui-test")]
    pub fn clear_notices(&mut self, cx: &mut Context<Self>) {
        self.notice = None;
        self.notifications.update(cx, |center, cx| center.clear(cx));
    }
}
