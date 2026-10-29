use crate::app::StudyApp;
use gpui_kit::*;
pub use studyapp_ui::feedback::{ModalFocus, modal_frame_shell};

fn update_recovery_message(reason: &str) -> &'static str {
    if reason.contains("System authorization cancelled or denied") {
        "系统授权已取消或被拒绝，本次更新未安装。可在设置中重试更新。"
    } else {
        "上次更新未完成，应用已重新打开。请在设置中重试更新。"
    }
}

impl StudyApp {
    pub(crate) fn deliver_update_notice(&mut self, cx: &mut Context<Self>) {
        if !self.startup_busy
            && !self.draft_loading
            && let Some(message) = self.pending_update_notice.take()
        {
            self.push_notice(Err(update_recovery_message(&message).into()), cx);
        }
    }
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

#[cfg(all(test, feature = "ui-test"))]
mod tests {
    use gpui_kit::{AppContext, Context, IntoElement, Render, TestAppContext, Window, div};
    #[test]
    fn recovery_messages_keep_internal_errors_and_paths_out_of_the_notice() {
        let cancelled = super::update_recovery_message(
            "System authorization cancelled or denied; /tmp/studyapp-install-private",
        );
        assert!(cancelled.contains("系统授权已取消或被拒绝"));
        assert!(!cancelled.contains("/tmp"));
        let failed = super::update_recovery_message("NSIS failed: C:\\private\\transaction");
        assert!(failed.contains("设置中重试更新"));
        assert!(!failed.contains("NSIS"));
        assert!(!failed.contains("private"));
    }
    #[gpui_kit::test]
    fn recovered_update_enqueues_a_visible_notification_once_after_loading(
        cx: &mut TestAppContext,
    ) {
        struct Blank;
        impl Render for Blank {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div()
            }
        }
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::apply(&Default::default(), false, cx);
        });
        let root = tempfile::tempdir().unwrap().keep();
        let worker = crate::worker::Worker::start(root.join("studyapp.sqlite3"), None);
        let mut app = None;
        let (_, visual) = cx.add_window_view(|window, cx| {
            app = Some(cx.new(|cx| crate::app::StudyApp::new(worker, window, cx)));
            Blank
        });
        let app = app.unwrap();
        visual.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.pending_update_notice = Some("System authorization denied".into());
                app.startup_busy = true;
                app.draft_loading = true;
                app.deliver_update_notice(cx);
                assert_eq!(app.notifications.read(cx).visible_count(), 0);
                app.startup_busy = false;
                app.draft_loading = false;
                app.deliver_update_notice(cx);
                assert_eq!(app.notifications.read(cx).visible_count(), 1);
                assert!(app.pending_update_notice.is_none());
                app.deliver_update_notice(cx);
                assert_eq!(app.notifications.read(cx).visible_count(), 1);
            });
        });
    }
}
