//! A draft-only logging page. Filesystem changes begin only after a settings commit.
use crate::{
    app::{Page, StudyApp},
    components::*,
    controls::{Button, Checkbox},
    settings_components as form,
    settings_ui::SettingsSection,
};
use gpui_kit::{
    component::{
        Disableable, Sizable,
        button::ButtonVariants,
        input::{InputEvent, InputState},
    },
    prelude::FluentBuilder,
    *,
};
use studyapp_core::LogLevel;
impl StudyApp {
    pub(crate) fn poll_logging_health(&mut self, cx: &mut Context<Self>) {
        match crate::audit_log::health() {
            Some(crate::audit_log::Health::Unavailable) if !self.logging_health_notified => {
                self.logging_health_notified = true;
                let mut event = studyapp_core::NoticeEvent::new(
                    studyapp_core::NoticeCategory::OperationFailure,
                    studyapp_core::NoticeSeverity::Warning,
                    "日志暂时无法写入，笔记和通知仍可正常使用。请检查日志目录或在日志设置中选择其他位置。",
                );
                event.action = Some(studyapp_core::NoticeAction::OpenLogging);
                event.dedupe_key = Some("logging-health".into());
                // A logger-health notification must never feed its own failed writer.
                self.notifications
                    .update(cx, |center, cx| center.enqueue(event, cx));
            }
            Some(crate::audit_log::Health::Healthy | crate::audit_log::Health::Disabled) => {
                self.logging_health_notified = false
            }
            _ => {}
        }
    }

    pub(crate) fn fill_logging_fields(
        &mut self,
        value: &studyapp_core::LoggingSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preferences_ui.logging_picker.invalidate();
        let value = value.directory.clone().unwrap_or_default();
        if let Some(input) = &self.preferences_ui.logging_directory {
            input.update(cx, |input, cx| input.set_value(value, window, cx));
        } else {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("留空使用应用本地数据 logs 目录")
                    .default_value(value)
            });
            cx.subscribe(&input, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            })
            .detach();
            self.preferences_ui.logging_directory = Some(input);
        }
    }
    pub(crate) fn poll_logging_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.page != Page::Settings
            || self.preferences_ui.section != SettingsSection::Logging
            || self.preferences_ui.busy
            || self.preferences_ui.replacing
            || self.storage_busy
        {
            self.preferences_ui.logging_picker.invalidate();
            return;
        }
        if let Some(path) = self.preferences_ui.logging_picker.poll(&self.last_library) {
            if let Some(input) = &self.preferences_ui.logging_directory {
                input.update(cx, |input, cx| {
                    input.set_value(
                        studyapp_desktop::presentation::path_label(&path),
                        window,
                        cx,
                    )
                });
            }
            cx.notify();
        }
    }
    pub(crate) fn logging_controls(&self, cx: &mut Context<Self>) -> Div {
        let settings = self
            .preferences_ui
            .draft
            .as_ref()
            .map(|s| s.logging.clone())
            .unwrap_or_default();
        let busy = self.preferences_ui.busy || self.preferences_ui.replacing || self.storage_busy;
        let mut panel = form::section("本地日志")
            .child(
                Checkbox::new("logging-enabled")
                    .label("启用日志")
                    .checked(settings.enabled)
                    .disabled(busy)
                    .on_click(cx.listener(|this, checked, _, cx| {
                        if let Some(draft) = &mut this.preferences_ui.draft {
                            draft.logging.enabled = *checked;
                        }
                        cx.notify();
                    })),
            )
            .child(muted(
                "默认开启 Info。日志仅保存在本机；通知开关、免打扰和提示音不会改变日志输出。",
            ));
        let mut levels = row().flex_wrap().gap_1();
        for level in LogLevel::ALL {
            levels = levels.child(
                Button::new(SharedString::from(format!(
                    "logging-level-{}",
                    level.label()
                )))
                .small()
                .ghost()
                .label(level.label())
                .disabled(busy)
                .when(settings.minimum_level == level, |button| {
                    button.bg(crate::theme::selection_color())
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(draft) = &mut this.preferences_ui.draft {
                        draft.logging.minimum_level = level;
                    }
                    cx.notify();
                })),
            );
        }
        panel=panel.child(form::settings_row("输出等级",levels)).child(muted("Error：错误；Warn：警告及错误；Info：常规运行信息；Debug / Trace：更细的阶段与计数。任何等级都不记录正文、剪贴板、凭据、完整路径或技术堆栈。"));
        if let Some(input) = &self.preferences_ui.logging_directory {
            let expanded = self.preferences_ui.logging_directory_expanded;
            panel = panel.child(
                form::DirectoryField::new(
                    "logging-directory-detail",
                    "日志目录",
                    input.read(cx).value(),
                )
                .fallback("应用本地数据 / logs")
                .expanded(expanded)
                .editor(form::input(input, busy))
                .actions(
                    row()
                        .child(
                            Button::new("logging-directory-expand")
                                .small()
                                .ghost()
                                .label(if expanded { "收起" } else { "编辑路径" })
                                .disabled(busy)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.preferences_ui.logging_directory_expanded = !expanded;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("pick-logging-directory")
                                .small()
                                .label("选择文件夹")
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let current = this
                                        .preferences_ui
                                        .logging_directory
                                        .as_ref()
                                        .map(|input| input.read(cx).value().trim().to_owned())
                                        .filter(|s| !s.is_empty())
                                        .map(std::path::PathBuf::from);
                                    this.preferences_ui.logging_picker.choose(
                                        &this.last_library,
                                        current,
                                        "选择日志保存目录",
                                    );
                                })),
                        )
                        .child(
                            Button::new("logging-use-default")
                                .small()
                                .ghost()
                                .label("使用默认位置")
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.preferences_ui.logging_picker.invalidate();
                                    if let Some(input) = &this.preferences_ui.logging_directory {
                                        input.update(cx, |input, cx| {
                                            input.set_value("", window, cx)
                                        });
                                    }
                                    cx.notify();
                                })),
                        ),
                ),
            );
        }
        panel.child(muted("选择目录只修改草稿，保存全部设置后生效。更改位置不会迁移或删除旧日志。仅使用已登记的固定日志文件轮转，最多 5 个、每个 2 MiB；日志位置计入空间概览。"))
    }
}
