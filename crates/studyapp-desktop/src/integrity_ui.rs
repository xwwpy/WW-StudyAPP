use crate::{app::StudyApp, components::*, controls::Button, installation_integrity::Report};
use gpui_kit::{
    component::{Disableable, Sizable, button::ButtonVariants},
    prelude::FluentBuilder,
    *,
};

impl StudyApp {
    pub(crate) fn request_integrity_check(&mut self, cx: &mut Context<Self>) {
        if self.storage_ui.integrity_busy
            || self.storage_busy
            || self.update_panel.read(cx).relocation_blocked()
        {
            return;
        }
        self.storage_ui.integrity_busy = true;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async { crate::installation_integrity::check_current() })
                .await;
            let _ = this.update(cx, |this, cx| this.finish_integrity_check(result, cx));
        })
        .detach();
        cx.notify();
    }
    pub(crate) fn finish_integrity_check(
        &mut self,
        result: Result<Report, String>,
        cx: &mut Context<Self>,
    ) {
        self.storage_ui.integrity_busy = false;
        let notice = match &result {
            Ok(report) if report.findings.is_empty() => Ok(report.summary()),
            Ok(report) => Err(report.summary()),
            Err(message) => Err(message.clone()),
        };
        self.storage_ui.integrity_result = Some(result);
        self.push_notice(notice, cx);
        cx.notify();
    }
    pub(crate) fn integrity_controls(&self, cx: &mut Context<Self>) -> Div {
        column().gap_2().child(heading("应用诊断"))
            .child(muted("对照当前版本的已签名发行基线，只读检查程序文件。需要连接发布服务获取基线，不会下载安装包。"))
            .child(row().flex_wrap()
                .child(Button::new("check-file-integrity").small()
                    .label(if self.storage_ui.integrity_busy { "正在检查…" } else { "检查文件完整性" })
                    .disabled(self.storage_ui.integrity_busy || self.storage_busy || self.update_panel.read(cx).relocation_blocked())
                    .on_click(cx.listener(|this, _, _, cx| this.request_integrity_check(cx))))
                .when(self.storage_ui.integrity_result.is_some(), |row| row.child(
                    Button::new("view-integrity-result").small().ghost().label("查看检查结果")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if !this.has_modal(cx) { this.storage_ui.integrity_dialog = true; cx.notify(); }
                        })))))
    }
    pub(crate) fn integrity_dialog_view(&self, cx: &mut Context<Self>) -> Option<Div> {
        if !self.storage_ui.integrity_dialog {
            return None;
        }
        let result = self.storage_ui.integrity_result.as_ref()?;
        let (summary, findings, warning) = match result {
            Ok(report) => (
                report.summary(),
                report
                    .findings
                    .iter()
                    .map(|finding| studyapp_ui::file_integrity::FileFinding {
                        path: finding.path.clone().into(),
                        state: finding.state.label().into(),
                    })
                    .collect::<Vec<_>>(),
                !report.findings.is_empty(),
            ),
            Err(message) => (message.clone(), vec![], true),
        };
        Some(
            div().child(
                studyapp_ui::modal_frame::ModalFrame::new(
                    "文件完整性检查",
                    studyapp_ui::file_integrity::details(summary.into(), &findings),
                )
                .tone(if warning {
                    studyapp_ui::modal_frame::ModalTone::Warning
                } else {
                    studyapp_ui::modal_frame::ModalTone::Neutral
                })
                .scroll_handle(self.modal_focus.scroll())
                .footer(
                    row().child(
                        Button::new("close-integrity-result")
                            .label("关闭")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.storage_ui.integrity_dialog = false;
                                cx.notify();
                            })),
                    ),
                ),
            ),
        )
    }
}

#[cfg(all(test, feature = "ui-test"))]
mod tests {
    use crate::app::StudyApp;
    use gpui_kit::{
        AppContext, Context, Entity, IntoElement, Render, TestAppContext, Window, base::Root,
        test::TestWindowExt,
    };
    struct Fixture(Entity<StudyApp>);
    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.0.update(cx, |app, cx| {
                if app.storage_ui.integrity_dialog {
                    app.integrity_dialog_view(cx).unwrap()
                } else {
                    app.storage_view(cx)
                }
            })
        }
    }
    #[gpui_kit::test]
    fn integrity_results_use_global_notices_and_explicit_details_in_storage(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::apply(&Default::default(), false, cx);
        });
        let directory = tempfile::tempdir().unwrap().keep();
        let worker = crate::worker::Worker::start(directory.join("studyapp.sqlite3"), None);
        let mut app = None;
        let (_, visual) = cx.add_window_view(|window, cx| {
            let entity = cx.new(|cx| StudyApp::new(worker, window, cx));
            app = Some(entity.clone());
            let content = cx.new(|_| Fixture(entity));
            Root::new(content, window, cx)
        });
        let app = app.unwrap();
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("check-file-integrity").is_some());
            assert!(window.try_find("repair-installation").is_none());
            assert!(window.try_find("view-integrity-result").is_none());
            app.update(cx, |app, cx| {
                app.finish_integrity_check(
                    Err("没有可信发行基线；未判断文件是否完整。".into()),
                    cx,
                );
                assert_eq!(app.notifications.read(cx).visible_count(), 1);
                assert!(!app.storage_ui.integrity_dialog);
                assert!(app.notice.as_ref().unwrap().is_err());
            });
            window.render_frame(cx);
            assert!(window.try_find("view-integrity-result").is_some());
            assert!(window.try_find("integrity-result").is_none());
            window.click("view-integrity-result", cx);
            window.render_frame(cx);
            assert!(app.read(cx).has_modal(cx));
            assert!(window.try_find("integrity-result").is_some());
            window.click("close-integrity-result", cx);
            window.render_frame(cx);
            assert!(!app.read(cx).storage_ui.integrity_dialog);
            assert!(app.read(cx).storage_ui.integrity_result.is_some());
            assert!(window.try_find("view-integrity-result").is_some());
        });
    }
}
