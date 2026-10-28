//! Standalone application recovery and final results; no normal app/Worker startup.
use super::*;
use crate::maintenance_execute::{MaintenanceState, MaintenanceStatus, RecoveryRequest};
use std::path::PathBuf;
#[cfg(all(windows, feature = "ui-test"))]
#[path = "maintenance_recovery_acceptance.rs"]
mod recovery_acceptance;
#[cfg(all(windows, feature = "ui-test"))]
pub(crate) use recovery_acceptance::run_recovery_acceptance;

pub(super) fn result_text(status: &MaintenanceStatus) -> (&'static str, &'static str) {
    match status.code.as_str() {
        "application_removed_data_retained" => (
            "应用已卸载，数据清理未完成",
            "部分所选数据已保留。请查看下方路径，重新确认后再处理。",
        ),
        "reboot_required_data_retained" | "reboot_required" => (
            "需要重启 Windows",
            "不会自动重启；个人数据未因重启要求而删除。",
        ),
        "partial_recovery" => (
            "恢复尚未完成",
            "检测到文件身份变化或访问限制。已保留可恢复资源，请勿删除维护记录。",
        ),
        "msi_installed_startup_failed" => (
            "安装已完成，启动验证未通过",
            "Windows Installer 管理的程序文件已保留，可使用原安装包修复。",
        ),
        _ => match status.state {
            MaintenanceState::Succeeded => (
                "操作已完成",
                "下方列出仍保留的文件；临时维护资源将在后续维护时重试清理。",
            ),
            MaintenanceState::RolledBack => (
                "已恢复原有应用文件",
                "此次更新没有完成。个人资料库未通过恢复流程清理。",
            ),
            _ => (
                "操作尚未完成",
                "请保留维护记录并检查下方路径，排除占用后再重试。",
            ),
        },
    }
}
enum Mode {
    Result(MaintenanceStatus),
    Recover(Option<Box<RecoveryRequest>>),
}
#[cfg(all(windows, feature = "ui-test"))]
type HandoffObserver = Box<dyn FnMut(&crate::maintenance_handoff::AcceptanceHandoff) -> Result<()>>;
struct ResultView {
    mode: Mode,
    error: bool,
    #[cfg(all(windows, feature = "ui-test"))]
    handoff_observer: Option<HandoffObserver>,
}
impl Render for ResultView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pending_only = matches!(&self.mode,Mode::Recover(Some(request)) if !request.prepared.plan_path.parent().is_some_and(|stage|stage.join("journal.jsonl").exists()));
        let (title, description) = match &self.mode {
            Mode::Result(status) => result_text(status),
            Mode::Recover(_) if pending_only => (
                "更新尚未开始",
                "应用文件尚未更改，无需恢复。关闭本窗口后可返回原应用；退出或取消更新后会清理已验证的准备资源。",
            ),
            Mode::Recover(_) => (
                "发现未完成的应用更新",
                "恢复只处理此事务记录的应用文件，不会清理配置、数据库或原始笔记。",
            ),
        };
        let mut content = column()
            .id("maintenance-result-content")
            .p(px(26.))
            .gap(px(16.))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(
                div()
                    .text_size(px(24.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            )
            .child(div().text_color(rgb(MUTED)).child(description));
        if let Mode::Result(status) = &self.mode {
            for path in &status.retained_paths {
                content = content.child(div().text_size(px(13.)).child(path.display().to_string()));
            }
        }
        if self.error {
            content = content.child(
                div()
                    .text_color(rgb(0xA63737))
                    .child("恢复交接未完成；所有用户数据均保留，请关闭占用程序后重试。"),
            );
        }
        let mut actions = row()
            .justify_end()
            .gap(px(10.))
            .p(px(20.))
            .border_t_1()
            .border_color(rgb(BORDER))
            .child(
                Button::new("maintenance-result-close")
                    .label("关闭")
                    .on_click(|_, window, _| window.remove_window()),
            );
        if matches!(self.mode, Mode::Recover(_)) && !pending_only {
            actions = actions.child(
                Button::new("maintenance-recover")
                    .label("恢复应用文件")
                    .primary()
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Mode::Recover(request) = &this.mode
                            && let Some(request) = request
                        {
                            if let Ok(handoff) = crate::maintenance_handoff::begin_recovery_handoff(
                                request.as_ref().clone(),
                            ) {
                                #[cfg(all(windows, feature = "ui-test"))]
                                if let Some(observer) = &mut this.handoff_observer
                                    && observer(&handoff).is_err()
                                {
                                    this.error = true;
                                    cx.notify();
                                    return;
                                }
                                #[cfg(not(all(windows, feature = "ui-test")))]
                                let _ = handoff;
                                cx.quit();
                            } else {
                                this.error = true;
                                cx.notify();
                            }
                        }
                    })),
            );
        }
        column()
            .size_full()
            .gap_0()
            .bg(rgb(BG))
            .text_color(rgb(INK))
            .font_family(studyapp_ui::design::UI_FONT)
            .child(studyapp_ui::window_chrome::title_bar(
                "WW-StudyApp 维护",
                vec![],
                vec![],
                window,
                |window, _| window.remove_window(),
            ))
            .child(content)
            .child(actions)
    }
}
pub(crate) fn run_recovery(request: RecoveryRequest) -> Result<()> {
    run(Mode::Recover(Some(Box::new(request))), None)
}
pub(crate) fn run_result(status: MaintenanceStatus, stage: PathBuf) -> Result<()> {
    run(Mode::Result(status), Some(stage))
}
fn run(mode: Mode, stage: Option<PathBuf>) -> Result<()> {
    run_with_opened(mode, stage, None)
}
type RecoveryOpened = Box<dyn FnOnce(AnyWindowHandle, Entity<ResultView>, &mut App)>;
fn run_with_opened(
    mode: Mode,
    stage: Option<PathBuf>,
    opened: Option<RecoveryOpened>,
) -> Result<()> {
    application()
        .with_assets(crate::assets::StudyAssets)
        .run(move |cx| {
            init(cx);
            Theme::change(ThemeMode::Light, None, cx);
            let theme = Theme::global_mut(cx);
            theme.font_family = studyapp_ui::design::UI_FONT.into();
            theme.font_size = px(14.);
            theme.colors.primary = rgb(GREEN).into();
            theme.colors.primary_foreground = rgb(0xFFFFFF).into();
            theme.colors.ring = rgb(GREEN).into();
            let options = WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some("WW-StudyApp 维护".into()),
                    appears_transparent: true,
                    ..Default::default()
                }),
                app_owns_titlebar_drag: true,
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(680.), px(560.)),
                    cx,
                ))),
                window_min_size: Some(size(px(580.), px(450.))),
                ..Default::default()
            };
            let (handle, view) = open_window(options, cx, move |_, cx| {
                cx.new(|_| ResultView {
                    mode,
                    error: false,
                    #[cfg(all(windows, feature = "ui-test"))]
                    handoff_observer: None,
                })
            })
            .expect("创建维护结果窗口失败");
            if let Some(opened) = opened {
                opened(handle, view, cx);
            }
            #[cfg(all(windows, feature = "ui-test"))]
            if let Some(stage) = stage
                && capture_allowed(&stage)
            {
                use gpui_kit::test::TestWindowExt;
                cx.spawn(async move |cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(500))
                        .await;
                    handle
                        .update(cx, |_, window, cx| {
                            window.render_frame(cx);
                            let image = window.render_to_image().expect("维护结果截图失败");
                            let mut file = crate::maintenance_handoff::create_stage_file(
                                &stage,
                                "maintenance-result.png",
                            )
                            .expect("维护截图文件创建失败");
                            image
                                .write_to(&mut file, image::ImageFormat::Png)
                                .expect("维护结果截图保存失败");
                            file.seal().expect("维护结果截图记录失败");
                            window.remove_window();
                        })
                        .expect("维护结果窗口已关闭");
                })
                .detach();
            }
            #[cfg(not(all(windows, feature = "ui-test")))]
            let _ = (handle, stage);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });
    Ok(())
}
#[cfg(all(windows, feature = "ui-test"))]
fn capture_allowed(stage: &std::path::Path) -> bool {
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true") {
        return false;
    }
    let Some(root) = std::env::var_os("RUNNER_TEMP") else {
        return false;
    };
    let (Ok(stage), Ok(root)) = (
        absolute_components(stage),
        absolute_components(std::path::Path::new(&root)),
    ) else {
        return false;
    };
    stage.len() > root.len() && stage.starts_with(&root)
}
