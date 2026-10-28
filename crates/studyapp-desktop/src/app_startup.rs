use crate::{app::StudyApp, components::*, controls::Button, worker::Work};
use gpui_kit::{
    component::{Disableable, button::ButtonVariants},
    *,
};

impl StudyApp {
    pub(crate) fn startup_view(&self, cx: &mut Context<Self>) -> Div {
        let content = if let Some(failure) = &self.startup_failure {
            card()
                .w_full()
                .gap_4()
                .child(heading(match failure.kind {
                    crate::worker::startup::StartupFailureKind::InUse => "资料库正在使用中",
                    crate::worker::startup::StartupFailureKind::RestrictedProcess => {
                        "当前程序运行在受限环境"
                    }
                    _ => "暂时无法打开资料库",
                }))
                .child(div().child(failure.message.clone()))
                .child(muted(failure.action))
                .child(
                    column()
                        .gap_1()
                        .child(muted(format!("检查位置 · {}", failure.stage.label())))
                        .child(div().text_sm().overflow_hidden().child(
                            studyapp_desktop::presentation::display_path(
                                &failure.path.to_string_lossy(),
                            ),
                        )),
                )
                .child(
                    row()
                        .flex_wrap()
                        .child(
                            Button::new("startup-select-library")
                                .label("重新选择资料库")
                                .disabled(self.startup_busy)
                                .on_click(
                                    cx.listener(|this, _, _, _| this.picker("library", None)),
                                ),
                        )
                        .child(
                            Button::new("startup-retry")
                                .primary()
                                .label(if self.startup_busy {
                                    "正在重试…"
                                } else {
                                    "重试"
                                })
                                .disabled(self.startup_busy)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.startup_work(Work::RetryOpenLibrary, cx)
                                })),
                        ),
                )
                .child(muted(
                    "原目录中的笔记和学习记录会保留。选择已有资料库可继续使用其中的内容。",
                ))
        } else {
            studyapp_ui::components::skeleton("正在打开本地资料库")
        };
        column()
            .items_center()
            .justify_center()
            .min_h(px(320.))
            .child(column().w_full().max_w(px(560.)).child(content))
    }

    pub(crate) fn startup_work(&mut self, work: Work, cx: &mut Context<Self>) {
        if self.startup_busy {
            return;
        }
        match self.worker.sender.send(work) {
            Ok(()) => self.startup_busy = true,
            Err(_) => {
                if let Some(failure) = &mut self.startup_failure {
                    failure.message = "资料库服务暂不可用".into();
                    failure.action = "请关闭应用后重新打开。原有资料会保留。";
                }
            }
        }
        cx.notify();
    }
}
