use crate::{app::StudyApp, components::*, controls::Button, worker::Work};
use gpui_kit::{
    component::{Disableable, button::ButtonVariants},
    *,
};

impl StudyApp {
    pub(crate) fn startup_view(&self, cx: &mut Context<Self>) -> Div {
        let content = if let Some(failure) = &self.startup_failure {
            let restricted = matches!(
                failure.kind,
                crate::worker::startup::StartupFailureKind::RestrictedProcess
            );
            let executable = std::env::current_exe()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|_| "未知".to_owned());
            let diagnostics = format!(
                "WW-StudyApp 启动诊断\n程序：{}\n类型：{:?}\n阶段：{}\n资料库：{}\n失败路径：{}\n说明：{}\n建议：{}",
                executable,
                failure.kind,
                failure.stage.label(),
                failure.database.display(),
                failure.path.display(),
                failure.message.as_str(),
                failure.action
            );
            let actions = if restricted {
                row()
                    .flex_wrap()
                    .child(
                        Button::new("startup-copy-diagnostics")
                            .label("复制诊断信息")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    diagnostics.clone(),
                                ));
                                this.push_notice(Ok("诊断信息已复制".to_owned()), cx);
                            })),
                    )
                    .child(
                        Button::new("startup-exit")
                            .primary()
                            .label("退出应用")
                            .on_click(cx.listener(|_, _, window, _| window.remove_window())),
                    )
            } else {
                row()
                    .flex_wrap()
                    .child(
                        Button::new("startup-select-library")
                            .label("重新选择资料库")
                            .disabled(self.startup_busy)
                            .on_click(cx.listener(|this, _, _, _| this.picker("library", None))),
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
                    )
            };
            card()
                .w_full()
                .gap_4()
                .child(heading(match failure.kind {
                    crate::worker::startup::StartupFailureKind::InUse => "资料库正在使用中",
                    crate::worker::startup::StartupFailureKind::RestrictedProcess => {
                        "需要从普通用户会话启动"
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
                .child(actions)
                .children(
                    (failure.kind == crate::worker::startup::StartupFailureKind::InUse)
                        .then(|| self.owner_controls(cx)),
                )
                .child(muted(if restricted {
                    "资料库没有被修改；从普通会话重新启动后可直接使用原资料库。"
                } else {
                    "原目录中的笔记和学习记录会保留。选择已有资料库可继续使用其中的内容。"
                }))
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
        self.owner_recovery.reset();
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
