//! Standalone maintenance UI; confirmation precedes authenticated staged execution.
use crate::maintenance_runtime::{self, DiscoveryContext, InstallDiscovery};
use anyhow::{Result, ensure};
use gpui_kit::{
    component::{
        Disableable, Theme, ThemeMode,
        button::ButtonVariants,
        input::{Input, InputState},
    },
    *,
};
use studyapp_desktop::maintenance::*;
use studyapp_ui::{
    components::{BG, BORDER, GREEN, INK, MUTED, column, row},
    controls::{Button, Checkbox},
};

pub(crate) type MaintenanceExecutor = Box<dyn FnMut(ConfirmedUninstall) -> Result<String>>;
type WindowOpened = Box<dyn FnOnce(AnyWindowHandle, Entity<MaintenanceView>, &mut App)>;
#[path = "maintenance_result_ui.rs"]
mod result_ui;
#[cfg(all(windows, feature = "ui-test"))]
pub(crate) use result_ui::run_recovery_acceptance;
pub(crate) use result_ui::{run_recovery, run_result};
#[cfg(test)]
pub(crate) fn result_text_for_test(
    status: &crate::maintenance_execute::MaintenanceStatus,
) -> (&'static str, &'static str) {
    result_ui::result_text(status)
}

#[cfg(all(windows, feature = "ui-test"))]
#[path = "maintenance_smoke.rs"]
mod smoke;
#[cfg(all(windows, feature = "ui-test"))]
pub(crate) use smoke::run_if_requested as run_smoke_if_requested;
#[cfg(all(windows, feature = "ui-test"))]
#[path = "maintenance_cancel_acceptance.rs"]
mod cancel_acceptance;
#[cfg(all(windows, feature = "ui-test"))]
pub(crate) use cancel_acceptance::run_cancel_acceptance;

struct MaintenanceController {
    discovery: InstallDiscovery,
    selection: DataSelection,
    confirmation: Option<UninstallConfirmation>,
    consumed: bool,
}
impl MaintenanceController {
    fn new(discovery: InstallDiscovery) -> Self {
        Self {
            discovery,
            selection: DataSelection::default(),
            confirmation: None,
            consumed: false,
        }
    }
    fn stage(&self) -> ConfirmationStage {
        self.confirmation
            .as_ref()
            .map(UninstallConfirmation::stage)
            .unwrap_or(ConfirmationStage::Selection)
    }
    fn set_selection(&mut self, selection: DataSelection) {
        self.selection = selection;
        self.back();
    }
    fn back(&mut self) {
        self.confirmation = None;
        self.consumed = false;
    }
    fn advance(&mut self, phrase: &str) -> Result<()> {
        ensure!(!self.consumed, "此卸载操作已提交，请重新检查");
        if self.confirmation.is_none() {
            let receipt = self
                .discovery
                .receipt
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("安装所有权尚未验证"))?;
            self.confirmation = Some(UninstallConfirmation::new(preview_uninstall(
                receipt,
                self.selection,
                self.discovery.library.verified_database.clone(),
                self.discovery.library.configuration_files.clone(),
            )?));
        }
        let confirmation = self.confirmation.as_mut().expect("initialized above");
        match confirmation.stage() {
            ConfirmationStage::Selection => confirmation.confirm_selection(),
            ConfirmationStage::DatabaseReview => confirmation.confirm_database_review()?,
            ConfirmationStage::DatabaseFinal => confirmation.confirm_database_final(phrase)?,
            ConfirmationStage::Ready => anyhow::bail!("卸载操作已确认"),
        }
        Ok(())
    }
    fn take_confirmed(&mut self) -> Result<ConfirmedUninstall> {
        ensure!(
            !self.consumed && self.stage() == ConfirmationStage::Ready,
            "卸载尚未完成确认"
        );
        let token = self.confirmation.take().context("卸载尚未确认")?.finish()?;
        self.consumed = true;
        Ok(token)
    }
    fn execute(&mut self, executor: &mut MaintenanceExecutor) -> Result<String> {
        executor(self.take_confirmed()?)
    }
}
use anyhow::Context as _;

struct MaintenanceView {
    controller: MaintenanceController,
    phrase: Entity<InputState>,
    executor: MaintenanceExecutor,
    status: Option<String>,
    quit_after_handoff: bool,
}
impl MaintenanceView {
    fn new(
        discovery: InstallDiscovery,
        executor: MaintenanceExecutor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            controller: MaintenanceController::new(discovery),
            phrase: cx.new(|cx| InputState::new(window, cx).placeholder("输入：删除数据库")),
            executor,
            status: None,
            quit_after_handoff: false,
        }
    }
    fn next(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let executing = self.controller.stage() == ConfirmationStage::Ready;
        let result = if executing {
            self.controller.execute(&mut self.executor).map(Some)
        } else {
            self.controller
                .advance(&self.phrase.read(cx).value())
                .map(|_| None)
        };
        let handoff_ready = executing && result.is_ok();
        self.status = match result {
            Ok(message) => message,
            Err(_) => Some(
                if executing {
                    "操作未完成。请检查处理结果后重试；部分已完成的操作可能无法撤销。"
                } else {
                    "尚未完成确认。请检查安装记录、所选路径及确认文字。"
                }
                .into(),
            ),
        };
        if handoff_ready && self.quit_after_handoff {
            cx.quit();
        }
        cx.notify();
    }
}
impl Render for MaintenanceView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let stage = self.controller.stage();
        let selection = self.controller.selection;
        let database = self
            .controller
            .discovery
            .library
            .database_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "未能验证资料库位置".into());
        let verified = self.controller.discovery.receipt.is_some();
        let mut content = column()
            .gap(px(18.))
            .p(px(28.))
            .flex_1()
            .min_w_0()
            .child(
                div()
                    .text_size(px(24.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("卸载 WW-StudyApp"),
            )
            .child(
                div()
                    .text_color(rgb(MUTED))
                    .child("应用文件与个人数据分别处理。原始笔记和外部文件始终保留。"),
            )
            .child(
                column()
                    .gap(px(6.))
                    .p(px(16.))
                    .rounded(px(10.))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(0xFFFFFF))
                    .child("当前资料库")
                    .child(div().text_size(px(13.)).child(database.clone())),
            );
        content = match stage {
            ConfirmationStage::Selection => content
                .child(
                    Checkbox::new("maintenance-config")
                        .label("同时清除本应用配置（含保存的凭据）")
                        .checked(selection.configuration)
                        .disabled(
                            !verified
                                || (self
                                    .controller
                                    .discovery
                                    .library
                                    .configuration_files
                                    .is_empty()
                                    && self
                                        .controller
                                        .discovery
                                        .library
                                        .verified_database
                                        .is_none()),
                        )
                        .on_click(cx.listener(|this, checked, window, cx| {
                            this.controller.set_selection(DataSelection {
                                configuration: *checked,
                                ..this.controller.selection
                            });
                            this.phrase.update(cx, |s, cx| s.set_value("", window, cx));
                            this.status = None;
                            cx.notify();
                        })),
                )
                .child(
                    Checkbox::new("maintenance-database")
                        .label("同时删除当前资料库数据库")
                        .checked(selection.database)
                        .disabled(
                            !verified
                                || self
                                    .controller
                                    .discovery
                                    .library
                                    .verified_database
                                    .is_none(),
                        )
                        .on_click(cx.listener(|this, checked, window, cx| {
                            this.controller.set_selection(DataSelection {
                                database: *checked,
                                ..this.controller.selection
                            });
                            this.phrase.update(cx, |s, cx| s.set_value("", window, cx));
                            this.status = None;
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .text_color(rgb(MUTED))
                        .child("默认保留配置与数据库。删除数据库需要再完成两次独立确认。"),
                ),
            ConfirmationStage::DatabaseReview => {
                content.child(div().text_color(rgb(0xA63737)).child(
                    "第一次确认：数据库中的笔记记录、学习历史和会话将被删除。请确认以上路径。",
                ))
            }
            ConfirmationStage::DatabaseFinal => content
                .child(
                    div()
                        .text_color(rgb(0xA63737))
                        .child("第二次确认：此操作无法撤销。请输入“删除数据库”。"),
                )
                .child(Input::new(&self.phrase)),
            ConfirmationStage::Ready => content.child(if selection.database {
                "已完成两次数据库确认。执行前仍会重新验证文件身份。"
            } else {
                "确认卸载应用；配置和数据库按所选范围处理。"
            }),
        };
        for issue in self
            .controller
            .discovery
            .issues
            .iter()
            .chain(self.controller.discovery.library.issues.iter())
        {
            content = content.child(div().text_color(rgb(MUTED)).child(issue.clone()));
        }
        if let Some(status) = &self.status {
            content = content.child(div().text_color(rgb(0xA63737)).child(status.clone()));
        }
        let label = match stage {
            ConfirmationStage::Selection => "继续卸载",
            ConfirmationStage::DatabaseReview => "确认以上数据库路径",
            ConfirmationStage::DatabaseFinal => "确认删除数据库",
            ConfirmationStage::Ready => "执行卸载",
        };
        column()
            .size_full()
            .gap_0()
            .bg(rgb(BG))
            .text_color(rgb(INK))
            .font_family(studyapp_ui::design::UI_FONT)
            .child(studyapp_ui::window_chrome::title_bar(
                "WW-StudyApp 卸载",
                vec![],
                vec![],
                window,
                |window, _| window.remove_window(),
            ))
            .child(content)
            .child(
                row()
                    .justify_end()
                    .gap(px(10.))
                    .p(px(20.))
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .child(
                        Button::new("maintenance-cancel")
                            .label("取消")
                            .on_click(|_, window, _| window.remove_window()),
                    )
                    .child(
                        Button::new("maintenance-back")
                            .label("返回选择")
                            .disabled(stage == ConfirmationStage::Selection)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.controller.back();
                                this.phrase.update(cx, |s, cx| s.set_value("", window, cx));
                                this.status = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("maintenance-next")
                            .label(label)
                            .primary()
                            .disabled(!verified || self.controller.consumed)
                            .on_click(cx.listener(|this, _, window, cx| this.next(window, cx))),
                    ),
            )
    }
}

pub(crate) fn run_interactive(explicit_library: Option<std::path::PathBuf>) -> Result<()> {
    let executable = std::env::current_exe()?;
    let mut context = DiscoveryContext::current_user()?;
    context.explicit_library = explicit_library;
    let discovery = maintenance_runtime::discover(&executable, &context);
    let receipt = discovery.receipt.clone();
    run_window(
        discovery,
        Box::new(move |confirmed| {
            let receipt = receipt.as_ref().context("安装所有权尚未验证")?;
            let intent =
                crate::maintenance_execute::prepare_uninstall(confirmed, receipt, &context)?;
            crate::maintenance_handoff::begin_uninstall_handoff(intent)?;
            Ok("已交接至独立卸载程序。".into())
        }),
        Some(Box::new(|_, view, cx| {
            view.update(cx, |view, _| view.quit_after_handoff = true);
        })),
    )
}
fn run_window(
    discovery: InstallDiscovery,
    executor: MaintenanceExecutor,
    opened: Option<WindowOpened>,
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
                    title: Some("WW-StudyApp 卸载".into()),
                    appears_transparent: true,
                    ..Default::default()
                }),
                app_owns_titlebar_drag: true,
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(640.), px(600.)),
                    cx,
                ))),
                window_min_size: Some(size(px(580.), px(560.))),
                ..Default::default()
            };
            let (window, view) = open_window(options, cx, move |window, cx| {
                cx.new(|cx| MaintenanceView::new(discovery, executor, window, cx))
            })
            .expect("创建卸载窗口失败");
            if let Some(opened) = opened {
                opened(window, view, cx);
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::maintenance_runtime::{InstallDiscovery, LibraryDiscovery};
    use std::{cell::Cell, path::PathBuf, rc::Rc};
    fn fixture() -> InstallDiscovery {
        let identity = FileIdentity {
            volume: 1,
            file_id: 2,
            size: 4,
            sha256: "a".repeat(64),
        };
        InstallDiscovery {
            receipt: Some(InstallReceipt {
                schema: 1,
                install_id: "fixture".into(),
                owner: InstallOwner::Portable,
                install_root: PathBuf::from(r"C:\App"),
                root_identity: FileIdentity {
                    size: 0,
                    sha256: String::new(),
                    ..identity.clone()
                },
                version: "0.8.0".into(),
                source_package_sha256: "a".repeat(64),
                files: vec![OwnedFile {
                    relative_path: "WW-StudyApp.exe".into(),
                    role: FileRole::Application,
                    size: 4,
                    sha256: "a".repeat(64),
                }],
            }),
            library: LibraryDiscovery {
                database_path: Some(PathBuf::from(r"C:\Library\studyapp.sqlite3")),
                verified_database: Some((PathBuf::from(r"C:\Library\studyapp.sqlite3"), identity)),
                ..Default::default()
            },
            issues: vec![],
        }
    }
    #[::core::prelude::v1::test]
    fn maintenance_ui_controller_selection_and_back_reset_confirmation() {
        let mut controller = MaintenanceController::new(fixture());
        assert_eq!(controller.selection, DataSelection::default());
        controller.set_selection(DataSelection {
            configuration: false,
            database: true,
        });
        controller.advance("").unwrap();
        assert_eq!(controller.stage(), ConfirmationStage::DatabaseReview);
        controller.advance("").unwrap();
        assert_eq!(controller.stage(), ConfirmationStage::DatabaseFinal);
        assert!(controller.take_confirmed().is_err());
        controller.back();
        assert_eq!(controller.stage(), ConfirmationStage::Selection);
        controller.advance("").unwrap();
        controller.advance("").unwrap();
        assert!(controller.advance("yes").is_err());
        controller.advance("删除数据库").unwrap();
        assert!(controller.take_confirmed().is_ok());
    }
    #[::core::prelude::v1::test]
    fn maintenance_ui_injected_executor_only_receives_ready_consumed_intent() {
        let calls = Rc::new(Cell::new(0));
        let count = calls.clone();
        let mut executor: MaintenanceExecutor = Box::new(move |token| {
            assert!(!token.preview().selection.database);
            count.set(count.get() + 1);
            Ok("已记录测试意图".into())
        });
        let mut controller = MaintenanceController::new(fixture());
        assert!(controller.execute(&mut executor).is_err());
        assert_eq!(calls.get(), 0);
        controller.advance("").unwrap();
        assert_eq!(calls.get(), 0);
        assert_eq!(controller.execute(&mut executor).unwrap(), "已记录测试意图");
        assert_eq!(calls.get(), 1);
        assert!(controller.execute(&mut executor).is_err());
    }
}
