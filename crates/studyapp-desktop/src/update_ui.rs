use crate::{components::*, controls::Button, update_backend::*};
use gpui_kit::{
    component::{Disableable, button::ButtonVariants},
    *,
};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};
use studyapp_desktop::update_policy::{CheckOrigin, UpdateCheckPolicy, startup_check_allowed};

static STARTUP_CHECK_CLAIMED: AtomicBool = AtomicBool::new(false);

/// Called once during normal application construction, after subscriptions exist.
pub fn schedule_startup(panel: Entity<UpdatePanel>, cx: &mut App) {
    if !panel.read(cx).checks_allowed() || STARTUP_CHECK_CLAIMED.swap(true, Ordering::Relaxed) {
        return;
    }
    cx.defer(move |cx| panel.update(cx, |panel, cx| panel.check(CheckOrigin::Startup, cx)));
}

pub enum UpdateEvent {
    InstallRequested,
    AvailabilityChanged,
    NsisReady,
}

enum UpdateReply {
    Checked(CheckOrigin, UpdateResult<UpdateCheck>),
    NsisChecked(
        CheckOrigin,
        UpdateResult<Option<crate::update_manifest::UpdateManifest>>,
    ),
    NsisProgress(u64, u64),
    NsisDownloaded(Box<Result<crate::update_install::VerifiedPackage, String>>),
    Progress(UpdateProgress),
    Prepared(String, Box<UpdateResult<PreparedUpdate>>),
    Previous(UpdateResult<Option<UpdateStatus>>),
    RepairState(UpdateResult<bool>),
    Repaired(UpdateResult<()>),
}

/// Background checks and downloads invalidate only this panel, never the editor.
pub struct UpdatePanel {
    library: String,
    isolated_launch: bool,
    busy: bool,
    check_policy: UpdateCheckPolicy,
    check_epoch: std::time::Instant,
    release: Option<UpdateRelease>,
    prepared: Option<(String, PreparedUpdate)>,
    status: String,
    error: Option<String>,
    previous: Option<UpdateStatus>,
    repair_needed: bool,
    dialog: Option<String>,
    progress: Option<UpdateProgress>,
    nsis: Option<crate::update_manifest::UpdateManifest>,
    nsis_version: Option<String>,
    nsis_progress: Option<(u64, u64)>,
    nsis_setup: Option<crate::update_install::VerifiedPackage>,
    nsis_error: Option<String>,
    notes_scroll: ScrollHandle,
    receiver: mpsc::Receiver<UpdateReply>,
    sender: mpsc::Sender<UpdateReply>,
    _poll: Task<()>,
    #[cfg(feature = "ui-test")]
    fixture: Option<CheckFixture>,
    #[cfg(feature = "ui-test")]
    repair_fixture: Option<(PathBuf, RepairTransport)>,
}

#[cfg(feature = "ui-test")]
#[derive(Clone)]
struct CheckFixture {
    result: UpdateResult<UpdateCheck>,
    checks: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    downloads: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

fn signed_check_reply(
    origin: CheckOrigin,
    result: UpdateResult<Option<crate::update_manifest::UpdateManifest>>,
) -> UpdateReply {
    UpdateReply::NsisChecked(origin, result)
}

impl EventEmitter<UpdateEvent> for UpdatePanel {}

impl UpdatePanel {
    pub fn available_version(&self) -> Option<&str> {
        self.release
            .as_ref()
            .map(|release| release.version.as_str())
            .or(self.nsis_version.as_deref())
    }

    fn settings_notes(&self) -> Option<&str> {
        self.nsis
            .as_ref()
            .map(|manifest| manifest.notes.as_str())
            .or_else(|| self.release.as_ref().map(|release| release.notes.as_str()))
    }
    pub fn nsis_setup(&self) -> Option<crate::update_install::VerifiedPackage> {
        self.nsis_setup.clone()
    }
    pub fn new(cx: &mut Context<Self>) -> Self {
        let (sender, receiver) = mpsc::channel();
        let status_sender = sender.clone();
        std::thread::spawn(move || {
            let _ = status_sender.send(UpdateReply::Previous(read_last_update_status()));
            if cfg!(windows) && !cfg!(test) && startup_check_allowed(std::env::args_os()) {
                let _ = status_sender.send(UpdateReply::RepairState(portable_repair_needed()));
            }
        });
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(200))
                    .await;
                if this.update(cx, |this, cx| this.poll(cx)).is_err() {
                    break;
                }
            }
        });
        Self {
            library: String::new(),
            busy: false,
            check_policy: UpdateCheckPolicy::default(),
            check_epoch: std::time::Instant::now(),
            release: None,
            prepared: None,
            isolated_launch: std::env::args_os().any(|arg| arg == "--data-dir"),
            status: "手动检查最新正式版本".into(),
            error: None,
            previous: None,
            repair_needed: false,
            dialog: None,
            progress: None,
            nsis: None,
            nsis_version: None,
            nsis_progress: None,
            nsis_setup: None,
            nsis_error: None,
            notes_scroll: ScrollHandle::new(),
            receiver,
            sender,
            _poll: poll,
            #[cfg(feature = "ui-test")]
            fixture: None,
            #[cfg(feature = "ui-test")]
            repair_fixture: None,
        }
    }

    pub fn dialog_visible(&self) -> bool {
        self.dialog.is_some() && (self.release.is_some() || self.nsis.is_some())
    }

    pub fn dialog_notes_scroll(&self) -> ScrollHandle {
        self.notes_scroll.clone()
    }

    /// The dialog mirrors the panel's own state; it never starts work by itself.
    pub fn dialog_content(&self) -> Option<studyapp_ui::update_dialog::UpdateDialogContent> {
        self.dialog.as_ref()?;
        if let Some(manifest) = &self.nsis {
            let phase = if let Some(error) = self.nsis_error.clone() {
                UpdateDialogPhase::Failed(error.into())
            } else if let Some((downloaded, total)) = self.nsis_progress {
                UpdateDialogPhase::Downloading { downloaded, total }
            } else if self.busy {
                UpdateDialogPhase::Installing
            } else {
                UpdateDialogPhase::Ready
            };
            return Some(UpdateDialogContent::new(
                env!("CARGO_PKG_VERSION"),
                manifest.version.to_string(),
                manifest.pub_date.clone().unwrap_or_default(),
                manifest.notes.clone(),
                phase,
            ));
        }
        let release = self.release.as_ref()?;
        self.dialog.as_ref()?;
        use studyapp_ui::update_dialog::{UpdateDialogContent, UpdateDialogPhase};
        let phase = if let Some(error) = self.error.clone() {
            UpdateDialogPhase::Failed(error.into())
        } else if let Some(progress) = &self.progress {
            match progress {
                UpdateProgress::Downloading { downloaded, total } => {
                    UpdateDialogPhase::Downloading {
                        downloaded: *downloaded,
                        total: *total,
                    }
                }
                UpdateProgress::Verifying | UpdateProgress::Preparing => {
                    UpdateDialogPhase::Installing
                }
            }
        } else if self.busy {
            UpdateDialogPhase::Installing
        } else {
            UpdateDialogPhase::Ready
        };
        Some(UpdateDialogContent::new(
            env!("CARGO_PKG_VERSION"),
            release.version.clone(),
            String::new(),
            release.notes.clone(),
            phase,
        ))
    }

    pub fn show_dialog(&mut self, cx: &mut Context<Self>) {
        if let Some(release) = &self.release {
            self.dialog = Some(release.version.clone());
            cx.notify();
        } else if let Some(version) = &self.nsis_version {
            self.dialog = Some(version.clone());
            cx.notify();
        }
    }

    pub fn dismiss_dialog(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if let Some(version) = &self.dialog {
            self.check_policy.dismiss(version);
        }
        self.dialog = None;
        cx.notify();
    }

    pub fn install_from_dialog(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if self.nsis.is_some() {
            if self.nsis_setup.is_some() {
                cx.emit(UpdateEvent::NsisReady);
            } else {
                self.start_nsis_download(cx);
            }
            return;
        }
        if self.prepared.is_some() {
            cx.emit(UpdateEvent::InstallRequested);
        } else {
            self.download(cx);
        }
    }

    fn finish_nsis_check(
        &mut self,
        origin: CheckOrigin,
        result: UpdateResult<Option<crate::update_manifest::UpdateManifest>>,
    ) {
        self.busy = false;
        self.check_policy.finish();
        match result {
            Ok(Some(manifest)) => {
                self.error = None;
                self.nsis_error = None;
                self.nsis_progress = None;
                self.nsis_version = Some(manifest.version.to_string());
                self.dialog = self
                    .check_policy
                    .should_prompt(origin, &manifest.version.to_string())
                    .then(|| manifest.version.to_string());
                self.status = format!("发现新版本 {}", manifest.version);
                self.nsis = Some(manifest);
                self.release = None;
            }
            Ok(None) => {
                self.error = None;
                self.nsis = None;
                self.nsis_version = None;
                self.nsis_error = None;
                self.nsis_progress = None;
                self.dialog = None;
                self.release = None;
                self.status = format!("当前 {} 已是最新正式版本", env!("CARGO_PKG_VERSION"));
            }
            Err(error) if origin == CheckOrigin::Manual => self.show_error(error),
            Err(_) => {}
        }
    }

    fn start_nsis_download(&mut self, cx: &mut Context<Self>) {
        let Some(manifest) = self.nsis.clone() else {
            return;
        };
        self.busy = true;
        self.error = None;
        self.nsis_error = None;
        self.nsis_progress = Some((0, 0));
        self.status = "正在下载更新…".into();
        let sender = self.sender.clone();
        let directory = std::env::temp_dir();
        std::thread::spawn(move || {
            let result = crate::update_install::VerifiedPackage::download(
                &manifest.signed_envelope,
                &directory,
                |downloaded, total| {
                    let _ = sender.send(UpdateReply::NsisProgress(downloaded, total));
                },
            );
            let _ = sender.send(UpdateReply::NsisDownloaded(Box::new(result)));
        });
        cx.notify();
    }

    pub fn set_library(&mut self, library: String, cx: &mut Context<Self>) {
        if self.library == library {
            return;
        }
        self.library = library;
        if self.prepared.take().is_some() {
            self.status = "资料库已切换，请重新准备更新以保留当前打开位置。".into();
        }
        cx.notify();
    }

    pub fn prepared(&self, library: &str) -> Option<PreparedUpdate> {
        self.prepared
            .as_ref()
            .filter(|(saved, _)| saved == library)
            .map(|(_, update)| update.clone())
    }

    pub fn install_failed(&mut self, message: String, cx: &mut Context<Self>) {
        self.nsis_error = Some(message.clone());
        self.error = Some(message);
        cx.notify();
    }

    fn check(&mut self, origin: CheckOrigin, cx: &mut Context<Self>) {
        if !self.checks_allowed() {
            return;
        }
        // Claim startup even while a manual request is running: never queue a retry.
        if origin != CheckOrigin::Periodic {
            self.check_policy
                .periodic_due(self.check_epoch.elapsed().as_secs(), false);
        }
        let started = self.check_policy.start(origin);
        if !started {
            return;
        }
        if self.busy {
            self.check_policy.finish();
            return;
        }
        self.busy = true;
        self.error = None;
        if origin == CheckOrigin::Manual {
            self.status = "正在检查正式版本…".into();
        }
        let sender = self.sender.clone();
        #[cfg(feature = "ui-test")]
        let fixture = self.fixture.clone();
        std::thread::spawn(move || {
            #[cfg(feature = "ui-test")]
            if let Some(fixture) = fixture {
                fixture.checks.fetch_add(1, Ordering::Relaxed);
                let _ = sender.send(UpdateReply::Checked(origin, fixture.result));
                return;
            }
            let current = env!("CARGO_PKG_VERSION");
            let reply = signed_check_reply(origin, crate::update_nsis::check(current));
            let _ = sender.send(reply);
        });
        cx.notify();
    }

    fn download(&mut self, cx: &mut Context<Self>) {
        #[cfg(feature = "ui-test")]
        if let Some(fixture) = &self.fixture {
            fixture.downloads.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if !self.checks_allowed() {
            return;
        }
        if self.busy || self.library.is_empty() {
            return;
        }
        let Some(release) = self.release.clone() else {
            return;
        };
        self.busy = true;
        self.error = None;
        self.prepared = None;
        self.progress = Some(UpdateProgress::Downloading {
            downloaded: 0,
            total: release.package_size,
        });
        self.status = "正在准备下载…".into();
        let sender = self.sender.clone();
        let library = self.library.clone();
        let isolated = self.isolated_launch;
        std::thread::spawn(move || {
            let directory = restart_data_directory(&library, isolated);
            let result = prepare_update(&release, directory, |progress| {
                let _ = sender.send(UpdateReply::Progress(progress));
            });
            let _ = sender.send(UpdateReply::Prepared(library, Box::new(result)));
        });
        cx.notify();
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        if self.checks_allowed()
            && self
                .check_policy
                .periodic_due(self.check_epoch.elapsed().as_secs(), self.busy)
        {
            self.check(CheckOrigin::Periodic, cx);
        }
        let mut changed = false;
        while let Ok(reply) = self.receiver.try_recv() {
            changed = true;
            match reply {
                UpdateReply::Checked(origin, result) => {
                    self.finish_check(origin, result);
                    cx.emit(UpdateEvent::AvailabilityChanged);
                }
                UpdateReply::Progress(progress) => {
                    self.status = progress_text(&progress);
                    self.progress = Some(progress);
                }
                UpdateReply::NsisChecked(origin, result) => {
                    self.finish_nsis_check(origin, result);
                    cx.emit(UpdateEvent::AvailabilityChanged);
                }
                UpdateReply::NsisProgress(downloaded, total) => {
                    self.nsis_progress = Some((downloaded, total));
                    self.status = format!(
                        "正在下载更新 · {:.1} / {:.1} MB",
                        downloaded as f64 / 1_048_576.,
                        total as f64 / 1_048_576.
                    );
                }
                UpdateReply::NsisDownloaded(result) => {
                    self.busy = false;
                    self.nsis_progress = None;
                    match *result {
                        Ok(path) => {
                            self.nsis_setup = Some(path);
                            self.status = "已下载并校验签名，正在启动安装程序…".into();
                            cx.emit(UpdateEvent::NsisReady);
                        }
                        Err(error) => {
                            self.nsis_error = Some(error.clone());
                            self.error = Some(error);
                        }
                    }
                }
                UpdateReply::Prepared(library, result) => {
                    self.busy = false;
                    match *result {
                        Ok(prepared) if library == self.library => {
                            self.status =
                                format!("{} 已下载并校验，可以重启安装", prepared.version);
                            self.prepared = Some((library, prepared));
                            self.progress = None;
                        }
                        Ok(_) => self.status = "资料库已切换，请重新准备更新。".into(),
                        Err(error) => {
                            self.progress = None;
                            self.show_error(error);
                        }
                    }
                }
                UpdateReply::Previous(Ok(status)) => self.previous = status,
                UpdateReply::RepairState(Ok(needed)) => self.repair_needed = needed,
                UpdateReply::RepairState(Err(_)) => {}
                UpdateReply::Repaired(result) => self.finish_repair(result),
                UpdateReply::Previous(Err(_)) => {
                    // An unreadable old receipt must not block checks or show a
                    // recurring launch-time error unrelated to the current task.
                }
            }
        }
        if changed {
            cx.notify();
        }
    }

    fn finish_check(&mut self, origin: CheckOrigin, result: UpdateResult<UpdateCheck>) {
        self.busy = false;
        self.check_policy.finish();
        match result {
            Ok(UpdateCheck::Available(release)) => {
                self.error = None;
                if self
                    .release
                    .as_ref()
                    .is_none_or(|old| !old.same_artifacts(&release))
                {
                    self.prepared = None;
                }
                self.status = format!(
                    "发现新版本 {} · {:.1} MB",
                    release.version,
                    release.package_size as f64 / 1_048_576.
                );
                self.dialog = self
                    .check_policy
                    .should_prompt(origin, &release.version)
                    .then(|| release.version.clone());
                self.progress = None;
                self.release = Some(*release);
            }
            Ok(UpdateCheck::UpToDate { current_version }) => {
                self.error = None;
                self.status = format!("当前 {current_version} 已是最新正式版本");
                self.release = None;
                self.prepared = None;
                self.dialog = None;
                self.progress = None;
            }
            Err(error) if origin == CheckOrigin::Manual => self.show_error(error),
            Err(_) => {}
        }
    }

    fn repair(&mut self, cx: &mut Context<Self>) {
        if !self.checks_allowed() || self.busy || !self.repair_needed || self.library.is_empty() {
            return;
        }
        self.busy = true;
        self.prepared = None;
        self.error = None;
        self.status = "正在验证当前版本并修复安装文件…".into();
        let sender = self.sender.clone();
        let library = self.library.clone();
        let isolated = self.isolated_launch;
        #[cfg(feature = "ui-test")]
        let repair_fixture = self.repair_fixture.clone();
        std::thread::spawn(move || {
            #[cfg(feature = "ui-test")]
            if let Some((target, transport)) = repair_fixture {
                let result = repair_fixture_installation(
                    &target,
                    restart_data_directory(&library, isolated),
                    &transport,
                    |progress| {
                        let _ = sender.send(UpdateReply::Progress(progress));
                    },
                );
                let _ = sender.send(UpdateReply::Repaired(result));
                return;
            }
            let result = repair_current_installation(
                restart_data_directory(&library, isolated),
                |progress| {
                    let _ = sender.send(UpdateReply::Progress(progress));
                },
            );
            let _ = sender.send(UpdateReply::Repaired(result));
        });
        cx.notify();
    }

    fn finish_repair(&mut self, result: UpdateResult<()>) {
        self.busy = false;
        match result {
            Ok(()) => {
                self.repair_needed = false;
                self.error = None;
                self.status = "安装文件已修复，可使用程序目录中的 Uninstall.exe 卸载。".into();
            }
            Err(error) => self.show_error(error),
        }
    }

    fn show_error(&mut self, error: UpdateError) {
        if matches!(error.kind, UpdateErrorKind::RepairRequired) {
            self.repair_needed = true;
        }
        if matches!(error.kind, UpdateErrorKind::ManualUpdate) {
            self.status = "请使用手动更新".into();
        }
        self.error = Some(error.to_string());
    }

    fn checks_allowed(&self) -> bool {
        #[cfg(feature = "ui-test")]
        if self.fixture.is_some() {
            return true;
        }
        cfg!(any(windows, target_os = "linux", target_os = "macos"))
            && !cfg!(test)
            && startup_check_allowed(std::env::args_os())
    }

    /// Remote smoke only: reuse strict release parsing while replacing all IO.
    #[cfg(feature = "ui-test")]
    pub(crate) fn fixture_response(&mut self, json: Option<&str>) {
        let result = json.map(fixture_release_from_json).unwrap_or_else(|| {
            Err(UpdateError {
                kind: UpdateErrorKind::Network,
                message: "更新服务暂不可用",
            })
        });
        if let Some(fixture) = &mut self.fixture {
            fixture.result = result;
        } else {
            self.fixture = Some(CheckFixture {
                result,
                checks: Default::default(),
                downloads: Default::default(),
            });
        }
    }

    #[cfg(feature = "ui-test")]
    pub(crate) fn fixture_manual_check(&mut self, cx: &mut Context<Self>) {
        assert!(self.fixture.is_some());
        self.check(CheckOrigin::Manual, cx);
    }

    #[cfg(feature = "ui-test")]
    pub(crate) fn fixture_automatic_prompt_allowed(&self, version: &str) -> bool {
        self.check_policy
            .should_prompt(CheckOrigin::Periodic, version)
    }
    #[cfg(feature = "ui-test")]
    pub(crate) fn fixture_state(&self) -> (usize, usize, bool, bool) {
        let fixture = self.fixture.as_ref().expect("isolated update fixture");
        (
            fixture.checks.load(Ordering::Relaxed),
            fixture.downloads.load(Ordering::Relaxed),
            self.busy,
            self.error.is_some(),
        )
    }
}

fn restart_data_directory(library: &str, isolated: bool) -> Option<&Path> {
    isolated.then(|| Path::new(library).parent()).flatten()
}

fn progress_text(progress: &UpdateProgress) -> String {
    match progress {
        UpdateProgress::Downloading { downloaded, total } => format!(
            "正在下载 · {:.1} / {:.1} MB",
            *downloaded as f64 / 1_048_576.,
            *total as f64 / 1_048_576.
        ),
        UpdateProgress::Verifying => "正在校验下载文件…".into(),
        UpdateProgress::Preparing => "正在准备更新并检查安装位置…".into(),
    }
}

impl Render for UpdatePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !cfg!(any(windows, target_os = "linux", target_os = "macos")) {
            return card()
                .id("application-updates")
                .child(heading("软件更新"))
                .child(muted(format!("WW-StudyApp {}", env!("CARGO_PKG_VERSION"))))
                .child(muted(
                    "下载适合当前系统的安装包后安装更新，原有资料库会保留。",
                ))
                .child(
                    Button::new("open-releases")
                        .label("打开发布页面")
                        .on_click(|_, _, cx| {
                            cx.open_url("https://github.com/xwwpy/WW-StudyAPP/releases/latest")
                        }),
                )
                .into_any_element();
        }
        let release_url = self
            .release
            .as_ref()
            .map(|release| release.page_url.clone())
            .unwrap_or_else(|| "https://github.com/xwwpy/WW-StudyAPP/releases".into());
        card().id("application-updates").test_support()
            .child(heading("软件更新"))
            .child(muted(format!("WW-StudyApp {}", env!("CARGO_PKG_VERSION"))))
            .child(div().child(self.status.clone()))
            .children(self.error.as_ref().map(|error| div().text_color(crate::theme::color(0xB45142)).child(error.clone())))
            .child(row().flex_wrap()
                .child(Button::new("check-for-updates").label("检查更新").disabled(self.busy)
                    .on_click(cx.listener(|this, _, _, cx| this.check(CheckOrigin::Manual, cx))))
                .children(self.repair_needed.then(|| Button::new("repair-installation")
                    .label("修复安装文件").disabled(self.busy)
                    .on_click(cx.listener(|this, _, _, cx| this.repair(cx)))))
                .children((self.release.is_some() && self.prepared.is_none()).then(||
                    Button::new("download-update").label("下载更新").disabled(self.busy)
                        .on_click(cx.listener(|this, _, _, cx| this.download(cx)))))
                .children(self.prepared.as_ref().map(|_| Button::new("restart-to-update")
                    .label("重启并安装").disabled(self.busy)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(UpdateEvent::InstallRequested)))))
                .child(Button::new("open-releases").ghost().label("发布页面")
                    .on_click(move |_, _, cx| cx.open_url(&release_url))))
            .children(self.settings_notes().map(|notes|
                scroll_body("update-release-notes", &self.notes_scroll).max_h(px(180.)).pr_3()
                    .child(gpui_kit::component::text::TextView::markdown("update-settings-markdown", studyapp_ui::markdown_content::release_notes(notes)))))
            .children(self.prepared.as_ref().map(|(_, update)| {
                let status = update.status_path.clone();
                column().gap_1().child(muted(format!("安装位置：{}", studyapp_desktop::presentation::display_path(&update.installation_directory.to_string_lossy()))))
                    .child(Button::new("reveal-update-record").ghost().label("查看更新记录")
                        .on_click(move |_, _, cx| cx.reveal_path(&status)))
            }))
            .children(self.previous.as_ref().map(|status| column().gap_1()
                .child(muted(format!("{} · {}", status.version, status.user_message())))
                .children((!status.record_path.is_empty()).then(|| {
                    let path = PathBuf::from(&status.record_path);
                    Button::new("reveal-maintenance-record").ghost().label("查看维护记录")
                        .on_click(move |_, _, cx| cx.reveal_path(&path))
                }))
                .children((!status.backup_path.is_empty()).then(|| {
                    let path = PathBuf::from(&status.backup_path);
                    Button::new("reveal-update-backup").ghost().label("定位旧版本备份")
                        .on_click(move |_, _, cx| cx.reveal_path(&path))
                }))
                .children((!status.distribution_path.is_empty()).then(|| {
                    let path = PathBuf::from(&status.distribution_path);
                    Button::new("reveal-update-distribution").ghost().label("查看发行文件")
                        .on_click(move |_, _, cx| cx.reveal_path(&path))
                }))))
            .children(self.repair_needed.then(|| muted("修复会下载当前版本并验证程序文件，补齐卸载程序及安装记录；不会改变笔记和资料库。")))
            .child(muted("安装前会保存草稿并处理未保存的设置。更新只处理属于当前安装的程序文件，保留资料库；安装目录不可写时可从发布页面手动更新。"))
            .into_any_element()
    }
}

impl crate::app::StudyApp {
    pub fn request_install_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.guard_settings(crate::settings_ui::SettingsContinuation::InstallUpdate, cx) {
            return;
        }
        let Some(prepared) = self.update_panel.read(cx).prepared(&self.last_library) else {
            self.push_notice(Err("请先下载更新；切换资料库后需要重新准备。".into()), cx);
            return;
        };
        self.pending_update = Some(prepared);
        self.close_requested = true;
        self.continue_close(window, cx);
    }

    /// Called only after settings and the active editor's durable-close barrier.
    pub fn complete_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(prepared) = self.pending_update.take()
            && let Err(error) = launch_prepared_update(&prepared)
        {
            self.close_requested = false;
            self.clear_pending_updates();
            self.update_panel
                .update(cx, |panel, cx| panel.install_failed(error.to_string(), cx));
            self.push_notice(Err(error.to_string()), cx);
            return;
        }
        if let Some(setup) = self.pending_nsis_update.take()
            && let Err(error) = crate::update_install::launch(&setup)
        {
            self.close_requested = false;
            self.clear_pending_updates();
            self.update_panel
                .update(cx, |panel, cx| panel.install_failed(error.to_string(), cx));
            self.push_notice(Err(error.to_string()), cx);
            return;
        }
        // GPUI drops its platform window before its view entities. Tear down
        // every Linux WebKit host first, after all close barriers have passed.
        #[cfg(target_os = "linux")]
        {
            self.note_content
                .update(cx, |editor, _| editor.close_native_host());
            if let Some(preview) = &self.preferences_ui.appearance_preview {
                preview.update(cx, |preview, cx| preview.close_native_host(cx));
            }
        }
        self.close_allowed = true;
        window.remove_window();
    }

    /// Hands the verified setup to the installer after settings are saved.
    pub fn install_nsis_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.guard_settings(crate::settings_ui::SettingsContinuation::InstallPackage, cx) {
            return;
        }
        let Some(setup) = self.update_panel.read(cx).nsis_setup() else {
            self.push_notice(Err("请先下载更新。".into()), cx);
            return;
        };
        let data = restart_data_directory(
            &self.last_library,
            self.update_panel.read(cx).isolated_launch,
        )
        .map(Path::to_owned);
        let prepared = match crate::update_install::prepare(&setup, data) {
            Ok(prepared) => prepared,
            Err(error) => {
                self.update_panel
                    .update(cx, |panel, cx| panel.install_failed(error.clone(), cx));
                self.push_notice(Err(error), cx);
                return;
            }
        };
        self.pending_nsis_update = Some(prepared);
        self.close_requested = true;
        self.continue_close(window, cx);
    }

    /// Renders the update card; the panel owns all state.
    pub fn update_dialog_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let (content, scroll) = {
            let panel = self.update_panel.read(cx);
            let Some(content) = panel.dialog_content() else {
                return div().into_any_element();
            };
            (content, panel.dialog_notes_scroll())
        };
        let install_panel = self.update_panel.clone();
        let dismiss_panel = self.update_panel.clone();
        studyapp_ui::update_dialog::UpdateDialog::new(content, &scroll)
            .on_install(move |_, _, cx| {
                install_panel.update(cx, |panel, cx| panel.install_from_dialog(cx));
            })
            .on_dismiss(move |_, _, cx| {
                dismiss_panel.update(cx, |panel, cx| panel.dismiss_dialog(cx));
            })
            .into_any_element()
    }
}

#[cfg(all(windows, feature = "ui-test"))]
#[path = "update_repair_acceptance.rs"]
mod repair_acceptance;
#[cfg(all(windows, feature = "ui-test"))]
pub(crate) use repair_acceptance::run_repair_acceptance;

#[cfg(test)]
mod tests {
    use super::*;
    use studyapp_desktop::update_policy::CheckOrigin;

    #[::core::prelude::v1::test]
    fn signed_check_failure_remains_a_failure() {
        let reply = signed_check_reply(
            CheckOrigin::Manual,
            Err(UpdateError {
                kind: UpdateErrorKind::InvalidRelease,
                message: "invalid manifest",
            }),
        );
        assert!(matches!(
            reply,
            UpdateReply::NsisChecked(CheckOrigin::Manual, Err(_))
        ));
    }

    fn panel() -> UpdatePanel {
        let (sender, receiver) = mpsc::channel();
        UpdatePanel {
            library: String::new(),
            isolated_launch: true,
            busy: true,
            release: None,
            prepared: None,
            status: "已准备的更新状态".into(),
            error: None,
            previous: None,
            repair_needed: false,
            dialog: None,
            progress: None,
            nsis: None,
            nsis_version: None,
            nsis_progress: None,
            nsis_setup: None,
            nsis_error: None,
            notes_scroll: ScrollHandle::new(),
            receiver,
            sender,
            check_policy: Default::default(),
            check_epoch: std::time::Instant::now(),
            _poll: Task::ready(()),
            #[cfg(feature = "ui-test")]
            fixture: None,
            #[cfg(feature = "ui-test")]
            repair_fixture: None,
        }
    }

    #[::core::prelude::v1::test]
    fn nsis_manifest_opens_dialog_and_clears_when_up_to_date() {
        use crate::update_manifest::UpdateManifest;
        use studyapp_ui::update_dialog::UpdateDialogPhase;
        let mut panel = panel();
        let manifest = UpdateManifest {
            version: semver::Version::parse("0.9.0").unwrap(),
            notes: "- 新版本".into(),
            pub_date: Some("2026-10-28T00:00:00Z".into()),
            setup_url: "https://github.com/xwwpy/WW-StudyAPP/releases/download/v0.9.0/WW-StudyApp-0.9.0-windows-x86_64-Setup.exe".into(),
            signature: "c2lnbmF0dXJl".into(),
            signed_envelope: String::new(),
            size: 10,
            sha256: "a".repeat(64),
        };
        panel.finish_nsis_check(CheckOrigin::Startup, Ok(Some(manifest)));
        assert_eq!(panel.settings_notes(), Some("- 新版本"));
        assert!(!panel.busy);
        assert!(panel.dialog_visible());
        assert_eq!(panel.available_version(), Some("0.9.0"));
        let content = panel.dialog_content().unwrap();
        assert_eq!(content.latest_version.as_ref(), "0.9.0");
        assert!(matches!(content.phase, UpdateDialogPhase::Ready));
        assert!(panel.nsis_setup().is_none());
        panel.finish_nsis_check(CheckOrigin::Manual, Ok(None));
        assert!(!panel.dialog_visible());
        assert!(panel.available_version().is_none());
        assert!(!panel.busy);
    }

    #[::core::prelude::v1::test]
    fn automatic_checks_respect_dismissal_and_manual_reopens() {
        let mut panel = panel();
        let manifest = crate::update_manifest::UpdateManifest {
            version: semver::Version::parse("0.9.0").unwrap(),
            notes: "# Changes\n- **New**".into(),
            pub_date: None,
            setup_url: "https://example.com/setup.exe".into(),
            signature: "fixture".into(),
            signed_envelope: String::new(),
            size: 10,
            sha256: "a".repeat(64),
        };
        panel.check_policy.dismiss("0.9.0");
        panel.finish_nsis_check(CheckOrigin::Periodic, Ok(Some(manifest.clone())));
        assert!(!panel.dialog_visible());
        panel.finish_nsis_check(CheckOrigin::Manual, Ok(Some(manifest.clone())));
        assert!(panel.dialog_visible());
        panel.dialog = None;
        let mut newer = manifest;
        newer.version = semver::Version::parse("0.9.1").unwrap();
        panel.finish_nsis_check(CheckOrigin::Periodic, Ok(Some(newer)));
        assert!(panel.dialog_visible());
    }
    #[::core::prelude::v1::test]
    fn update_startup_failure_is_quiet_and_manual_retry_is_visible() {
        let mut panel = panel();
        let error = UpdateError {
            kind: UpdateErrorKind::Network,
            message: "网络暂不可用",
        };
        panel.finish_check(CheckOrigin::Startup, Err(error.clone()));
        assert!(!panel.busy);
        assert!(panel.error.is_none());
        assert_eq!(panel.status, "已准备的更新状态");
        panel.finish_check(CheckOrigin::Manual, Err(error));
        assert_eq!(panel.error.as_deref(), Some("网络暂不可用"));
    }

    #[::core::prelude::v1::test]
    fn update_success_clears_previous_error_and_allows_another_manual_check() {
        let mut panel = panel();
        assert!(panel.check_policy.start(CheckOrigin::Startup));
        panel.error = Some("上一次检查失败".into());
        panel.finish_check(
            CheckOrigin::Startup,
            Ok(UpdateCheck::UpToDate {
                current_version: "0.7.2".into(),
            }),
        );
        assert!(!panel.busy);
        assert!(panel.error.is_none());
        assert!(panel.available_version().is_none());
        assert!(panel.check_policy.start(CheckOrigin::Manual));
    }

    #[::core::prelude::v1::test]
    fn update_restart_preserves_normal_and_isolated_launch_modes() {
        let library = std::path::Path::new("C:/fixtures/library/studyapp.sqlite3");
        let text = library.to_string_lossy();
        assert_eq!(super::restart_data_directory(&text, false), None);
        assert_eq!(super::restart_data_directory(&text, true), library.parent());
    }

    #[::core::prelude::v1::test]
    fn update_same_version_repair_is_separate_from_available_update_and_retryable() {
        let mut panel = panel();
        panel.repair_needed = true;
        panel.finish_repair(Err(UpdateError {
            kind: UpdateErrorKind::Network,
            message: "暂时离线",
        }));
        assert!(panel.repair_needed);
        assert!(!panel.busy);
        assert!(panel.available_version().is_none());
        assert!(panel.prepared.is_none());
        assert_eq!(panel.error.as_deref(), Some("暂时离线"));
        panel.finish_repair(Ok(()));
        assert!(!panel.repair_needed);
        assert!(panel.available_version().is_none());
        assert!(panel.prepared.is_none());
        assert!(panel.error.is_none());
    }
}
