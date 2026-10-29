//! Shared update card with release notes and explicit installation actions.
//!
//! Pure presentation: the caller owns the check/download state and wires the
//! install and "later" actions. The dialog never starts work by itself.
use crate::{
    components::{heading, row, scroll_body},
    controls::Button,
    theme::ResolvedTheme,
};
use gpui_kit::{
    assets::IconName,
    component::{Disableable, Icon, Sizable, button::ButtonVariants, text::TextView},
    *,
};

/// What the dialog shows while the caller owns the actual update work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateDialogPhase {
    /// Update known, nothing downloaded yet.
    Ready,
    /// Downloading in the background.
    Downloading { downloaded: u64, total: u64 },
    /// Installer handed over; the application is about to restart.
    Installing,
    /// The last attempt failed; the message stays visible with a retry action.
    Failed(SharedString),
}

impl UpdateDialogPhase {
    pub fn busy(&self) -> bool {
        matches!(self, Self::Downloading { .. } | Self::Installing)
    }

    /// Download progress in the 0..=1 range, if it can be estimated.
    pub fn fraction(&self) -> Option<f32> {
        match self {
            Self::Downloading { downloaded, total } if *total > 0 => {
                Some((*downloaded as f32 / *total as f32).clamp(0., 1.))
            }
            _ => None,
        }
    }

    pub fn headline(&self) -> &'static str {
        match self {
            Self::Ready => "发现新版本",
            Self::Downloading { .. } => "正在下载更新…",
            Self::Installing => "正在启动安装程序…",
            Self::Failed(_) => "更新失败",
        }
    }
}

/// Everything the dialog renders, owned by the caller.
#[derive(Clone, Debug)]
pub struct UpdateDialogContent {
    pub current_version: SharedString,
    pub latest_version: SharedString,
    pub published: SharedString,
    pub notes: SharedString,
    pub phase: UpdateDialogPhase,
}

impl UpdateDialogContent {
    pub fn new(
        current_version: impl Into<SharedString>,
        latest_version: impl Into<SharedString>,
        published: impl Into<SharedString>,
        notes: impl Into<SharedString>,
        phase: UpdateDialogPhase,
    ) -> Self {
        Self {
            current_version: current_version.into(),
            latest_version: latest_version.into(),
            published: published.into(),
            notes: notes.into(),
            phase,
        }
    }
}

/// Shared activation handler type for the dialog actions.
type ClickHandler = std::rc::Rc<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

#[derive(IntoElement)]
pub struct UpdateDialog {
    content: UpdateDialogContent,
    notes_scroll: ScrollHandle,
    on_install: Option<ClickHandler>,
    on_dismiss: Option<ClickHandler>,
}

impl UpdateDialog {
    pub fn new(content: UpdateDialogContent, notes_scroll: &ScrollHandle) -> Self {
        Self {
            content,
            notes_scroll: notes_scroll.clone(),
            on_install: None,
            on_dismiss: None,
        }
    }

    pub fn on_install(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_install = Some(std::rc::Rc::new(handler));
        self
    }

    /// Used by both the close affordance and "稍后再说".
    pub fn on_dismiss(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_dismiss = Some(std::rc::Rc::new(handler));
        self
    }
}

struct DialogColors {
    surface: u32,
    border: u32,
    text: u32,
    muted: u32,
    accent: u32,
    accent_text: u32,
    inset: u32,
    success_soft: u32,
    danger: u32,
}

impl DialogColors {
    fn resolve(theme: ResolvedTheme) -> Self {
        Self {
            surface: theme.surface,
            border: theme.border,
            text: theme.text,
            muted: theme.muted,
            accent: theme.accent,
            accent_text: theme.accent_text,
            inset: theme.inset,
            success_soft: theme.success_soft,
            danger: theme.danger,
        }
    }
}

impl RenderOnce for UpdateDialog {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let colors = DialogColors::resolve(crate::theme::palette());
        let phase = self.content.phase.clone();
        let busy = phase.busy();
        let failed = matches!(phase, UpdateDialogPhase::Failed(_));

        let later = Button::new("update-dialog-later")
            .label("稍后再说")
            .disabled(busy);
        let later = match self.on_dismiss.clone() {
            Some(handler) => later.on_click(move |event, window, cx| handler(event, window, cx)),
            None => later,
        };
        let close = Button::new("update-dialog-close")
            .ghost()
            .icon(IconName::Close)
            .disabled(busy);
        let close = match self.on_dismiss.clone() {
            Some(handler) => close.on_click(move |event, window, cx| handler(event, window, cx)),
            None => close,
        };

        let install = Button::new("update-dialog-install")
            .label(if busy {
                "正在更新"
            } else if failed {
                "重试更新"
            } else {
                "立即更新"
            })
            .disabled(busy)
            .icon(IconName::ArrowUp);
        let install = match self.on_install {
            Some(handler) => install.on_click(move |event, window, cx| handler(event, window, cx)),
            None => install,
        };

        let notes = self.content.notes.clone();
        let mut card = gpui_kit::div()
            .id("update-dialog")
            .test_support()
            .flex()
            .flex_col()
            .w((window.viewport_size().width - px(32.)).min(px(440.)))
            .rounded(px(28.))
            .border_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.surface))
            .p_6()
            .gap_4()
            .child(
                row()
                    .items_start()
                    .justify_between()
                    .child(
                        gpui_kit::div()
                            .size(px(44.))
                            .rounded(px(12.))
                            .bg(rgb(colors.accent))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(rgb(colors.accent_text))
                            .child(Icon::new(IconName::ArrowUp)),
                    )
                    .child(
                        gpui_kit::div()
                            .flex_1()
                            .px_3()
                            .child(heading(phase.headline()))
                            .child(
                                gpui_kit::div()
                                    .text_size(px(12.))
                                    .text_color(rgb(colors.muted))
                                    .child(format!(
                                        "v{} → v{}",
                                        self.content.current_version, self.content.latest_version
                                    )),
                            ),
                    )
                    .child(close),
            )
            .child(
                row()
                    .justify_between()
                    .child(
                        gpui_kit::div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(colors.text))
                            .child("更新内容"),
                    )
                    .child(
                        gpui_kit::div()
                            .text_size(px(11.))
                            .text_color(rgb(colors.muted))
                            .child("来自 GitHub Releases"),
                    ),
            )
            .child(
                gpui_kit::div()
                    .rounded(px(10.))
                    .border_1()
                    .border_color(rgb(colors.border))
                    .bg(rgb(colors.inset))
                    .max_h(px(240.))
                    .overflow_hidden()
                    .child(
                        scroll_body("update-dialog-notes", &self.notes_scroll)
                            .p_3()
                            .child(
                                gpui_kit::div()
                                    .text_size(px(13.))
                                    .text_color(rgb(colors.text))
                                    .child(TextView::markdown(
                                        "update-dialog-markdown",
                                        crate::markdown_content::release_notes(&notes),
                                    )),
                            ),
                    ),
            );

        card = card.child(match &phase {
            UpdateDialogPhase::Ready => gpui_kit::div(),
            UpdateDialogPhase::Failed(message) => gpui_kit::div()
                .text_size(px(12.))
                .text_color(rgb(colors.danger))
                .child(message.clone()),
            UpdateDialogPhase::Installing => gpui_kit::div()
                .text_size(px(12.))
                .text_color(rgb(colors.muted))
                .child("安装前会保存草稿并关闭应用，安装完成后重新打开。"),
            UpdateDialogPhase::Downloading { downloaded, total } => {
                let fraction = phase.fraction().unwrap_or(0.);
                gpui_kit::div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        row()
                            .justify_between()
                            .child(
                                gpui_kit::div()
                                    .text_size(px(12.))
                                    .text_color(rgb(colors.text))
                                    .child("正在下载更新…"),
                            )
                            .child(
                                gpui_kit::div()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors.muted))
                                    .child(if *total > 0 {
                                        format!(
                                            "{:.1} / {:.1} MB",
                                            *downloaded as f64 / 1_048_576.,
                                            *total as f64 / 1_048_576.
                                        )
                                    } else {
                                        String::new()
                                    }),
                            ),
                    )
                    .child(
                        gpui_kit::div()
                            .h(px(5.))
                            .w_full()
                            .rounded_full()
                            .bg(rgb(colors.inset))
                            .child(
                                gpui_kit::div()
                                    .h_full()
                                    .w(relative(fraction))
                                    .rounded_full()
                                    .bg(rgb(colors.accent)),
                            ),
                    )
                    .child(
                        gpui_kit::div()
                            .text_size(px(11.))
                            .text_color(rgb(colors.muted))
                            .child("安装前会保存草稿并关闭应用，安装完成后重新打开。"),
                    )
            }
        });

        if !failed {
            card = card.child(
                row()
                    .items_start()
                    .gap_2()
                    .p_3()
                    .rounded(px(10.))
                    .bg(rgb(colors.success_soft))
                    .child(Icon::new(IconName::Info).small())
                    .child(
                        gpui_kit::div()
                            .text_size(px(12.))
                            .child("更新会覆盖安装当前版本，已保存的设置与资料库保持不变。"),
                    ),
            );
        }

        card.child(row().justify_end().gap_2().child(later).child(install))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studyapp_core::{AppearanceSettings, ThemeMode, theme_contrast};

    #[::core::prelude::v1::test]
    fn dialog_phases_report_progress_and_busy_state() {
        assert!(!UpdateDialogPhase::Ready.busy());
        assert_eq!(UpdateDialogPhase::Ready.fraction(), None);
        let downloading = UpdateDialogPhase::Downloading {
            downloaded: 25,
            total: 100,
        };
        assert!(downloading.busy());
        assert_eq!(downloading.fraction(), Some(0.25));
        assert_eq!(UpdateDialogPhase::Installing.fraction(), None);
        assert!(UpdateDialogPhase::Installing.busy());
        assert!(!UpdateDialogPhase::Failed("网络错误".into()).busy());
        assert_eq!(
            UpdateDialogPhase::Failed("网络错误".into()).headline(),
            "更新失败"
        );
        assert_eq!(
            UpdateDialogPhase::Downloading {
                downloaded: 0,
                total: 0
            }
            .fraction(),
            None
        );
    }

    #[::core::prelude::v1::test]
    fn dialog_text_stays_readable_on_surface_across_themes() {
        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            let theme = crate::theme::resolve(
                &AppearanceSettings {
                    mode,
                    ..Default::default()
                },
                false,
            );
            let colors = DialogColors::resolve(theme);
            assert!(theme_contrast(colors.text, colors.surface) >= 4.5);
            assert!(theme_contrast(colors.muted, colors.surface) >= 4.5);
            assert!(theme_contrast(colors.accent, colors.surface) >= 3.);
            assert!(theme_contrast(colors.accent_text, colors.accent) >= 4.5);
        }
    }
}
