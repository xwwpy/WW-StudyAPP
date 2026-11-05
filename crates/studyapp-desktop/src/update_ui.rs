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
    ReadStateFailed,
    NsisReady,
    Notification(studyapp_core::NoticeEvent),
}

enum UpdateReply {
    Checked(CheckOrigin, UpdateResult<UpdateCheck>),
    NsisChecked(
        CheckOrigin,
        UpdateResult<Option<crate::update_manifest::UpdateManifest>>,
    ),
    NsisProgress(u64, u64),
    NsisDownloaded(Box<Result<crate::update_install::VerifiedPackage, String>>),
    NsisPrepared(
        u64,
        String,
        Box<Result<crate::update_install::PreparedPackage, String>>,
    ),
    Progress(UpdateProgress),
    Prepared(String, Box<UpdateResult<PreparedUpdate>>),
    Previous(UpdateResult<Option<UpdateStatus>>),
    Repaired(UpdateResult<()>),
}

/// Background checks and downloads invalidate only this panel, never the editor.
pub struct UpdatePanel {
    library: String,
    isolated_launch: bool,
    busy: bool,
    relocating: bool,
    check_policy: UpdateCheckPolicy,
    check_epoch: std::time::Instant,
    release: Option<UpdateRelease>,
    prepared: Option<(String, PreparedUpdate)>,
    status: String,
    error: Option<String>,
    previous: Option<UpdateStatus>,
    repair_needed: bool,
    dialog: Option<String>,
    badge_enabled: bool,
    read_state: crate::update_read_state::UpdateReadState,
    read_state_path: Option<PathBuf>,
    progress: Option<UpdateProgress>,
    nsis: Option<crate::update_manifest::UpdateManifest>,
    nsis_version: Option<String>,
    nsis_progress: Option<(u64, u64)>,
    nsis_setup: Option<crate::update_install::VerifiedPackage>,
    install_preparation: crate::update_install::PreparationGate,
    install_prepared: Option<(String, crate::update_install::PreparedPackage)>,
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

    pub fn set_badge_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.badge_enabled != enabled {
            self.badge_enabled = enabled;
            cx.emit(UpdateEvent::AvailabilityChanged);
            cx.notify();
        }
    }

    pub fn unread_update(&self) -> bool {
        self.badge_enabled
            && self
                .available_version()
                .is_some_and(|version| self.read_state.unread(version))
    }

    fn release_sections(&self) -> Vec<studyapp_ui::release_notes::ReleaseNotesSection> {
        use studyapp_ui::release_notes::ReleaseNotesSection;
        if let Some(manifest) = &self.nsis {
            return manifest
                .releases_since(env!("CARGO_PKG_VERSION"))
                .into_iter()
                .map(|entry| ReleaseNotesSection::new(entry.version.to_string(), entry.notes))
                .collect();
        }
        self.release
            .as_ref()
            .map(|release| ReleaseNotesSection::new(release.version.clone(), release.notes.clone()))
            .into_iter()
            .collect()
    }
    pub(crate) fn relocation_blocked(&self) -> bool {
        self.busy
            || self.prepared.is_some()
            || self.nsis_setup.is_some()
            || self.install_prepared.is_some()
    }
    pub(crate) fn set_relocating(&mut self, value: bool) {
        self.relocating = value;
    }
    pub(crate) fn is_relocating(&self) -> bool {
        self.relocating
    }
    pub(crate) fn cancel_install_preparation(&mut self) {
        self.install_preparation.invalidate();
        self.install_prepared = None;
    }
    fn take_install_prepared(
        &mut self,
        identity: &str,
    ) -> Option<crate::update_install::PreparedPackage> {
        self.install_prepared
            .take()
            .filter(|(saved, _)| saved == identity)
            .map(|(_, prepared)| prepared)
    }
    fn prepare_install(&mut self, data: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.relocating || self.busy {
            return;
        }
        let Some(package) = self.nsis_setup.clone() else {
            return;
        };
        let Some(token) = self.install_preparation.begin() else {
            return;
        };
        self.busy = true;
        self.error = None;
        self.nsis_error = None;
        self.status = "正在验证安装环境；您可以继续编辑…".into();
        let sender = self.sender.clone();
        std::thread::spawn(move || {
            let identity = package.identity();
            let result = crate::update_install::prepare(&package, data);
            let _ = sender.send(UpdateReply::NsisPrepared(token, identity, Box::new(result)));
        });
        cx.notify();
    }
    pub fn nsis_setup(&self) -> Option<crate::update_install::VerifiedPackage> {
        self.nsis_setup.clone()
    }
    pub fn new(cx: &mut Context<Self>) -> Self {
        let (sender, receiver) = mpsc::channel();
        let status_sender = sender.clone();
        std::thread::spawn(move || {
            let _ = status_sender.send(UpdateReply::Previous(read_last_update_status()));
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
        let read_state_path = if !cfg!(test) && startup_check_allowed(std::env::args_os()) {
            crate::app_usage::runtime_paths()
                .ok()
                .map(|paths| paths.effective.config.join("update-read.json"))
        } else {
            None
        };
        // This cache controls only the unread dot, never update availability or
        // installed version. An unreadable marker safely reverts to unread.
        let read_state = read_state_path
            .as_deref()
            .and_then(|path| crate::update_read_state::UpdateReadState::load(path).ok())
            .unwrap_or_default();
        Self {
            library: String::new(),
            busy: false,
            relocating: false,
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
            badge_enabled: true,
            read_state,
            read_state_path,
            progress: None,
            nsis: None,
            nsis_version: None,
            nsis_progress: None,
            nsis_setup: None,
            install_preparation: Default::default(),
            install_prepared: None,
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
            let phase = if self.nsis_error.is_some() {
                UpdateDialogPhase::Failed(
                    "更新未完成，当前资料保留。请重试或从官方发布页面手动安装。".into(),
                )
            } else if let Some((downloaded, total)) = self.nsis_progress {
                UpdateDialogPhase::Downloading { downloaded, total }
            } else if self.busy {
                UpdateDialogPhase::Installing
            } else {
                UpdateDialogPhase::Ready
            };
            return Some(
                UpdateDialogContent::new(
                    env!("CARGO_PKG_VERSION"),
                    manifest.version.to_string(),
                    manifest.pub_date.clone().unwrap_or_default(),
                    String::new(),
                    phase,
                )
                .with_releases(self.release_sections()),
            );
        }
        let release = self.release.as_ref()?;
        self.dialog.as_ref()?;
        use studyapp_ui::update_dialog::{UpdateDialogContent, UpdateDialogPhase};
        let phase = if self.error.is_some() {
            UpdateDialogPhase::Failed(
                "更新未完成，当前资料保留。请重试或从官方发布页面手动安装。".into(),
            )
        } else if let Some(progress) = &self.progress {
            match progress {
                UpdateProgress::Downloading { downloaded, total } => {
                    UpdateDialogPhase::Downloading {
                        downloaded: *downloaded,
                        total: *total,
                    }
                }
                UpdateProgress::Verifying | UpdateProgress::Preparing => {
                    crate::audit_log::stage(
                        studyapp_core::LogLevel::Debug,
                        crate::audit_log::Phase::UpdateVerify,
                        crate::audit_log::Outcome::Attempt,
                    );
                    UpdateDialogPhase::Installing
                }
            }
        } else if self.busy {
            UpdateDialogPhase::Installing
        } else {
            UpdateDialogPhase::Ready
        };
        Some(
            UpdateDialogContent::new(
                env!("CARGO_PKG_VERSION"),
                release.version.clone(),
                String::new(),
                String::new(),
                phase,
            )
            .with_releases(self.release_sections()),
        )
    }

    pub fn show_dialog(&mut self, cx: &mut Context<Self>) {
        let Some(version) = self.available_version().map(str::to_owned) else {
            return;
        };
        self.dialog = Some(version.clone());
        self.read_state.mark_read(&version);
        if let Some(path) = &self.read_state_path
            && self.read_state.save(path).is_err()
        {
            cx.emit(UpdateEvent::ReadStateFailed);
        }
        cx.emit(UpdateEvent::AvailabilityChanged);
        cx.notify();
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
        if self.relocating || self.busy {
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
                if self.nsis.as_ref().is_none_or(|old| {
                    old.version != manifest.version || old.sha256 != manifest.sha256
                }) {
                    self.cancel_install_preparation();
                    self.nsis_setup = None;
                    self.notes_scroll.set_offset(point(px(0.), px(0.)));
                }
                self.error = None;
                self.nsis_error = None;
                self.nsis_progress = None;
                self.nsis_version = Some(manifest.version.to_string());
                self.dialog = self.dialog.take().map(|_| manifest.version.to_string());
                self.status = format!("发现新版本 {}", manifest.version);
                self.nsis = Some(manifest);
                self.release = None;
            }
            Ok(None) => {
                self.cancel_install_preparation();
                self.nsis_setup = None;
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
        crate::audit_log::stage(
            studyapp_core::LogLevel::Info,
            crate::audit_log::Phase::UpdateDownload,
            crate::audit_log::Outcome::Attempt,
        );
        self.cancel_install_preparation();
        self.nsis_setup = None;
        let Some(manifest) = self.nsis.clone() else {
            return;
        };
        self.busy = true;
        self.error = None;
        self.nsis_error = None;
        self.nsis_progress = Some((0, 0));
        self.status = "正在下载更新…".into();
        let sender = self.sender.clone();
        std::thread::spawn(move || {
            let directory = match crate::app_usage::runtime_paths() {
                Ok(paths) => paths.effective.updates,
                Err(error) => {
                    let _ = sender.send(UpdateReply::NsisDownloaded(Box::new(Err(
                        error.to_string()
                    ))));
                    return;
                }
            };
            let _guards = match crate::storage_backend::guard_directories(&directory, true) {
                Ok(guards) => guards,
                Err(error) => {
                    let _ = sender.send(UpdateReply::NsisDownloaded(Box::new(Err(
                        error.to_string()
                    ))));
                    return;
                }
            };
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
        self.install_preparation.invalidate();
        self.install_prepared = None;
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
        self.busy = false;
        self.install_prepared = None;
        self.status = "更新安装未完成，可重试；原有资料保留。".into();
        self.nsis_error = Some(message.clone());
        self.error = Some(message);
        self.announce(
            studyapp_core::NoticeCategory::UpdateInstallFailure,
            studyapp_core::NoticeSeverity::Error,
            "更新安装未完成，原有资料保留。可重试或从官方发布页面手动安装。",
            studyapp_core::NoticeAction::OpenUpdate,
            cx,
        );
        cx.notify();
    }

    pub(crate) fn retry_from_notice(&mut self, cx: &mut Context<Self>) {
        self.check(CheckOrigin::Manual, cx);
    }

    fn check(&mut self, origin: CheckOrigin, cx: &mut Context<Self>) {
        crate::audit_log::stage(
            studyapp_core::LogLevel::Info,
            crate::audit_log::Phase::UpdateCheck,
            crate::audit_log::Outcome::Attempt,
        );
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
        crate::audit_log::stage(
            studyapp_core::LogLevel::Info,
            crate::audit_log::Phase::UpdateDownload,
            crate::audit_log::Outcome::Attempt,
        );
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

    fn announce(
        &self,
        category: studyapp_core::NoticeCategory,
        severity: studyapp_core::NoticeSeverity,
        summary: impl Into<String>,
        action: studyapp_core::NoticeAction,
        cx: &mut Context<Self>,
    ) {
        let mut event = studyapp_core::NoticeEvent::new(category, severity, summary);
        event.dedupe_key = Some(format!("update-{category:?}"));
        event.action = Some(action);
        cx.emit(UpdateEvent::Notification(event));
    }
    fn announce_offer(&self, previous: Option<&str>, cx: &mut Context<Self>) {
        if let Some(version) = self.available_version()
            && previous != Some(version)
            && self.read_state.unread(version)
        {
            let mut event = studyapp_core::NoticeEvent::new(
                studyapp_core::NoticeCategory::UpdateAvailable,
                studyapp_core::NoticeSeverity::Information,
                format!("发现新版本 {version}，可查看更新内容。"),
            );
            event.dedupe_key = Some(format!("update-offer-{version}"));
            event.action = Some(studyapp_core::NoticeAction::OpenUpdate);
            cx.emit(UpdateEvent::Notification(event));
        }
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
            let previous_offer = self.available_version().map(str::to_owned);
            changed = true;
            match reply {
                UpdateReply::Checked(origin, result) => {
                    let failure = if origin == CheckOrigin::Manual {
                        result.as_ref().err().map(ToString::to_string)
                    } else {
                        None
                    };
                    self.finish_check(origin, result);
                    if let Some(message) = failure {
                        self.announce(
                            studyapp_core::NoticeCategory::UpdateCheckFailure,
                            studyapp_core::NoticeSeverity::Error,
                            message,
                            studyapp_core::NoticeAction::RetryUpdate,
                            cx,
                        );
                    } else {
                        self.announce_offer(previous_offer.as_deref(), cx);
                    }
                    cx.emit(UpdateEvent::AvailabilityChanged);
                }
                UpdateReply::Progress(progress) => {
                    self.status = progress_text(&progress);
                    self.progress = Some(progress);
                }
                UpdateReply::NsisChecked(origin, result) => {
                    let failure = if origin == CheckOrigin::Manual {
                        result.as_ref().err().map(ToString::to_string)
                    } else {
                        None
                    };
                    self.finish_nsis_check(origin, result);
                    if let Some(message) = failure {
                        self.announce(
                            studyapp_core::NoticeCategory::UpdateCheckFailure,
                            studyapp_core::NoticeSeverity::Error,
                            message,
                            studyapp_core::NoticeAction::RetryUpdate,
                            cx,
                        );
                    } else {
                        self.announce_offer(previous_offer.as_deref(), cx);
                    }
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
                            self.announce(
                                studyapp_core::NoticeCategory::UpdateReady,
                                studyapp_core::NoticeSeverity::Success,
                                "更新已下载并通过签名校验。",
                                studyapp_core::NoticeAction::OpenUpdate,
                                cx,
                            );
                            self.status = "已下载并校验签名，正在启动安装程序…".into();
                            cx.emit(UpdateEvent::NsisReady);
                        }
                        Err(error) => {
                            self.status = "更新下载未完成，可稍后重试。".into();
                            self.nsis_error = Some(error.clone());
                            self.error = Some(error);
                            self.announce(
                                studyapp_core::NoticeCategory::UpdateDownloadFailure,
                                studyapp_core::NoticeSeverity::Error,
                                "更新下载或校验未完成；当前程序和资料未更改。请重试。",
                                studyapp_core::NoticeAction::RetryUpdate,
                                cx,
                            );
                        }
                    }
                }
                UpdateReply::NsisPrepared(token, identity, result) => {
                    self.busy = false;
                    if !self.install_preparation.finish(token)
                        || self
                            .nsis_setup
                            .as_ref()
                            .is_none_or(|package| package.identity() != identity)
                    {
                        self.status = "资料库已切换，请重新准备更新。".into();
                        continue;
                    }
                    match *result {
                        Ok(prepared) => {
                            self.install_prepared = Some((identity, prepared));
                            cx.emit(UpdateEvent::NsisReady);
                        }
                        Err(error) => self.install_failed(error, cx),
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
                            self.announce(
                                studyapp_core::NoticeCategory::UpdateReady,
                                studyapp_core::NoticeSeverity::Success,
                                "更新已下载并校验，可以重启安装。",
                                studyapp_core::NoticeAction::OpenUpdate,
                                cx,
                            );
                        }
                        Ok(_) => self.status = "资料库已切换，请重新准备更新。".into(),
                        Err(error) => {
                            self.progress = None;
                            let category = if matches!(
                                error.kind,
                                UpdateErrorKind::Network | UpdateErrorKind::RateLimited
                            ) {
                                studyapp_core::NoticeCategory::UpdateDownloadFailure
                            } else {
                                studyapp_core::NoticeCategory::UpdateInstallFailure
                            };
                            self.announce(
                                category,
                                studyapp_core::NoticeSeverity::Error,
                                error.to_string(),
                                studyapp_core::NoticeAction::RetryUpdate,
                                cx,
                            );
                            self.show_error(error);
                        }
                    }
                }
                UpdateReply::Previous(Ok(status)) => self.previous = status,
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
                    self.notes_scroll.set_offset(point(px(0.), px(0.)));
                }
                self.status = format!(
                    "发现新版本 {} · {:.1} MB",
                    release.version,
                    release.package_size as f64 / 1_048_576.
                );
                self.dialog = self.dialog.take().map(|_| release.version.clone());
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

    #[allow(dead_code)] // Internal legacy maintenance acceptance, never a user action.
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
        self.status = if matches!(error.kind, UpdateErrorKind::ManualUpdate) {
            "请使用手动更新"
        } else {
            "本次更新未完成，可稍后重试。"
        }
        .into();
        self.error = Some(error.to_string());
    }

    #[cfg(all(feature = "update-acceptance", target_os = "macos"))]
    pub(crate) fn acceptance_install_error(&self) -> Option<String> {
        self.nsis_error.clone()
    }
    #[cfg(feature = "update-acceptance")]
    pub(crate) fn acceptance_check(&mut self, cx: &mut Context<Self>) {
        self.check(CheckOrigin::Manual, cx);
    }
    fn checks_allowed(&self) -> bool {
        if self.relocating || crate::app_migration::startup_pending() {
            return false;
        }
        #[cfg(feature = "update-acceptance")]
        if crate::update_acceptance::active() {
            return crate::update_acceptance::checks_allowed();
        }
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
                        .action("打开发布页面")
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
            .child(studyapp_ui::update_available::version_entry(
                "about-version", env!("CARGO_PKG_VERSION"), self.available_version(), self.unread_update(),
            ).on_click(cx.listener(|this, _, _, cx| {
                if this.available_version().is_some() { this.show_dialog(cx); }
                else { this.check(CheckOrigin::Manual, cx); }
            })))
            .child(div().child(self.status.clone()))

            .child(row().flex_wrap()
                .child(Button::new("check-for-updates").action("检查更新").disabled(self.busy)
                    .on_click(cx.listener(|this, _, _, cx| this.check(CheckOrigin::Manual, cx))))
                .children((self.release.is_some() && self.prepared.is_none()).then(||
                    Button::new("download-update").action("下载更新").disabled(self.busy)
                        .on_click(cx.listener(|this, _, _, cx| this.download(cx)))))
                .children(self.prepared.as_ref().map(|_| Button::new("restart-to-update")
                    .action("重启并安装").disabled(self.busy)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(UpdateEvent::InstallRequested)))))
                .child(Button::new("open-releases").ghost().action("发布页面")
                    .on_click(move |_, _, cx| cx.open_url(&release_url))))
            .children(self.available_version().map(|_| Button::new("view-update-notes")
                .action("查看更新内容")
                .on_click(cx.listener(|this, _, _, cx| this.show_dialog(cx)))))
            .children(self.prepared.as_ref().map(|(_, update)| {
                let status = update.status_path.clone();
                column().gap_1().child(muted(format!("安装位置：{}", studyapp_desktop::presentation::display_path(&update.installation_directory.to_string_lossy()))))
                    .child(Button::new("reveal-update-record").ghost().action("查看更新记录")
                        .on_click(move |_, _, cx| cx.reveal_path(&status)))
            }))
            .children(self.previous.as_ref().map(|status| column().gap_1()
                .child(muted(format!("{} · {}", status.version, status.user_message())))
                .children((!status.record_path.is_empty()).then(|| {
                    let path = PathBuf::from(&status.record_path);
                    Button::new("reveal-maintenance-record").ghost().action("查看维护记录")
                        .on_click(move |_, _, cx| cx.reveal_path(&path))
                }))
                .children((!status.backup_path.is_empty()).then(|| {
                    let path = PathBuf::from(&status.backup_path);
                    Button::new("reveal-update-backup").ghost().action("定位旧版本备份")
                        .on_click(move |_, _, cx| cx.reveal_path(&path))
                }))
                .children((!status.distribution_path.is_empty()).then(|| {
                    let path = PathBuf::from(&status.distribution_path);
                    Button::new("reveal-update-distribution").ghost().action("查看发行文件")
                        .on_click(move |_, _, cx| cx.reveal_path(&path))
                }))))
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
        if self.pending_update.is_some() || self.pending_nsis_update.is_some() {
            crate::audit_log::stage(
                studyapp_core::LogLevel::Info,
                crate::audit_log::Phase::UpdateInstall,
                crate::audit_log::Outcome::Attempt,
            );
            let _ = crate::audit_log::flush();
        }

        #[cfg(feature = "update-acceptance")]
        if self.pending_nsis_update.is_some() {
            crate::update_acceptance::before_helper();
        }
        if let Some(prepared) = self.pending_update.take()
            && let Err(error) = launch_prepared_update(&prepared)
        {
            self.close_requested = false;
            self.clear_pending_updates();
            self.update_panel
                .update(cx, |panel, cx| panel.install_failed(error.to_string(), cx));
            return;
        }
        if let Some(setup) = self.pending_nsis_update.take()
            && let Err(error) = crate::update_install::launch(&setup)
        {
            self.close_requested = false;
            self.clear_pending_updates();
            self.update_panel
                .update(cx, |panel, cx| panel.install_failed(error.to_string(), cx));
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
        if self.timer_widget.is_some() {
            self.close_widget(true, cx);
        }
        self.close_notification_desktop(cx);
        if let Some(control) = &self.owner_close {
            control.mark(crate::library_owner::Phase::Closing);
        }
        // Window removal need not drop its view immediately. Release the real
        // database owner here, only after durable close and successful handoff.
        // A helper-launch failure above leaves the original worker usable.
        self.worker.shutdown();
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
        let identity = setup.identity();
        let prepared = self.update_panel.update(cx, |panel, cx| {
            if let Some(prepared) = panel.take_install_prepared(&identity) {
                return Some(prepared);
            }
            panel.prepare_install(data, cx);
            None
        });
        let Some(prepared) = prepared else {
            return;
        };
        #[cfg(feature = "update-acceptance")]
        crate::update_acceptance::before_close(self, &prepared, cx);
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
    fn dialog_release_notes_keep_one_scroll_state_across_card_renders() {
        let panel = panel();
        panel.notes_scroll.set_offset(point(px(0.), px(-80.)));
        assert_eq!(
            panel.dialog_notes_scroll().offset(),
            point(px(0.), px(-80.))
        );
        panel
            .dialog_notes_scroll()
            .set_offset(point(px(0.), px(-120.)));
        assert_eq!(panel.notes_scroll.offset(), point(px(0.), px(-120.)));
    }

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

    pub(super) fn panel() -> UpdatePanel {
        let (sender, receiver) = mpsc::channel();
        UpdatePanel {
            library: String::new(),
            isolated_launch: true,
            busy: true,
            relocating: false,
            release: None,
            prepared: None,
            status: "已准备的更新状态".into(),
            error: None,
            previous: None,
            repair_needed: false,
            dialog: None,
            badge_enabled: true,
            read_state: Default::default(),
            read_state_path: None,
            progress: None,
            nsis: None,
            nsis_version: None,
            nsis_progress: None,
            nsis_setup: None,
            install_preparation: Default::default(),
            install_prepared: None,
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
    fn cancelled_preparation_cannot_install_under_a_new_offer() {
        let mut panel = panel();
        let token = panel.install_preparation.begin().unwrap();
        panel.install_prepared = Some((
            "A".into(),
            crate::update_install::PreparedPackage::Windows(PathBuf::from("A")),
        ));
        panel.cancel_install_preparation();
        assert!(panel.take_install_prepared("A").is_none());
        assert!(!panel.install_preparation.finish(token));
        panel.install_prepared = Some((
            "A".into(),
            crate::update_install::PreparedPackage::Windows(PathBuf::from("A")),
        ));
        let manifest = crate::update_manifest::UpdateManifest {
            version: semver::Version::parse("0.9.1").unwrap(),
            notes: String::new(),
            history: Vec::new(),
            pub_date: None,
            setup_url: String::new(),
            signature: String::new(),
            signed_envelope: "B".into(),
            size: 2,
            sha256: "b".repeat(64),
        };
        panel.finish_nsis_check(CheckOrigin::Manual, Ok(Some(manifest)));
        assert!(
            panel.install_prepared.is_none(),
            "A must not remain prepared under B's card"
        );
        panel.install_prepared = Some((
            "A".into(),
            crate::update_install::PreparedPackage::Windows(PathBuf::from("A")),
        ));
        assert!(
            panel.take_install_prepared("B").is_none(),
            "digest/version binding rejects stale cache even before invalidation"
        );
        panel.install_prepared = Some((
            "B".into(),
            crate::update_install::PreparedPackage::Windows(PathBuf::from("B")),
        ));
        assert!(
            matches!(panel.take_install_prepared("B"), Some(crate::update_install::PreparedPackage::Windows(path)) if path == Path::new("B"))
        );
    }
    #[::core::prelude::v1::test]
    fn signed_history_is_filtered_before_any_card_consumer() {
        use crate::update_manifest::{ReleaseNotes, UpdateManifest};
        let mut panel = panel();
        let manifest = UpdateManifest {
            version: semver::Version::parse("0.9.0").unwrap(),
            notes: "already-installed aggregate must never be displayed".into(),
            history: vec![
                ReleaseNotes {
                    version: semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap(),
                    notes: "already-installed".into(),
                },
                ReleaseNotes {
                    version: semver::Version::parse("0.8.0").unwrap(),
                    notes: "intermediate".into(),
                },
                ReleaseNotes {
                    version: semver::Version::parse("0.9.0").unwrap(),
                    notes: "target".into(),
                },
            ],
            pub_date: None,
            setup_url: String::new(),
            signature: String::new(),
            signed_envelope: String::new(),
            size: 10,
            sha256: "a".repeat(64),
        };
        panel.finish_nsis_check(CheckOrigin::Manual, Ok(Some(manifest)));
        panel.dialog = Some("0.9.0".into());
        let content = panel.dialog_content().unwrap();
        assert_eq!(content.releases.len(), 2);
        assert_eq!(content.releases[0].version.as_ref(), "0.9.0");
        assert_eq!(content.releases[0].notes.as_ref(), "target");
        assert_eq!(content.releases[1].notes.as_ref(), "intermediate");
        assert!(
            !content
                .releases
                .iter()
                .any(|entry| entry.notes.contains("already-installed"))
        );
    }

    #[::core::prelude::v1::test]
    fn nsis_manifest_marks_unread_without_opening_and_clears_when_up_to_date() {
        use crate::update_manifest::UpdateManifest;
        use studyapp_ui::update_dialog::UpdateDialogPhase;
        let mut panel = panel();
        let manifest = UpdateManifest {
            version: semver::Version::parse("0.9.0").unwrap(),
            notes: "- 新版本".into(),
            history: Vec::new(),
            pub_date: Some("2026-10-28T00:00:00Z".into()),
            setup_url: "https://github.com/xwwpy/WW-StudyAPP/releases/download/v0.9.0/WW-StudyApp-0.9.0-windows-x86_64-Setup.exe".into(),
            signature: "c2lnbmF0dXJl".into(),
            signed_envelope: String::new(),
            size: 10,
            sha256: "a".repeat(64),
        };
        panel.finish_nsis_check(CheckOrigin::Startup, Ok(Some(manifest)));
        assert_eq!(panel.release_sections()[0].notes.as_ref(), "- 新版本");
        assert!(!panel.busy);
        assert!(!panel.dialog_visible());
        assert!(panel.unread_update());
        assert_eq!(panel.available_version(), Some("0.9.0"));
        panel.dialog = Some("0.9.0".into());
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
    fn checks_never_open_the_card_and_unread_marker_tracks_each_target() {
        let mut panel = panel();
        let manifest = crate::update_manifest::UpdateManifest {
            version: semver::Version::parse("0.9.0").unwrap(),
            notes: "# Changes\n- **New**".into(),
            history: Vec::new(),
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
        assert!(!panel.dialog_visible());
        assert!(panel.unread_update());
        panel.badge_enabled = false;
        assert!(!panel.unread_update());
        assert!(
            panel.read_state.unread("0.9.0"),
            "hiding a badge must not mark a release as read"
        );
        panel.badge_enabled = true;
        assert!(panel.unread_update());
        panel.read_state.mark_read("0.9.0");
        assert!(!panel.unread_update());
        assert_eq!(panel.available_version(), Some("0.9.0"));
        let mut newer = manifest;
        newer.version = semver::Version::parse("0.9.1").unwrap();
        panel.finish_nsis_check(CheckOrigin::Periodic, Ok(Some(newer)));
        assert!(!panel.dialog_visible());
        assert!(panel.unread_update());
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

#[cfg(all(test, feature = "ui-test"))]
mod modal_visibility_tests {
    use gpui_kit::{AppContext, Context, IntoElement, Render, TestAppContext, Window, div};
    use studyapp_desktop::update_policy::CheckOrigin;
    #[gpui_kit::test]
    fn update_modal_hides_and_restores_persistent_editor(cx: &mut TestAppContext) {
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
        let mut entity = None;
        let (_, visual) = cx.add_window_view(|window, cx| {
            entity = Some(cx.new(|cx| crate::app::StudyApp::new(worker, window, cx)));
            Blank
        });
        let entity = entity.unwrap();
        visual.update(|window, cx| {
            entity.update(cx, |app, cx| {
                app.page = crate::app::Page::Notes;
                app.note_editor = Some(crate::drafts::NoteDraft {
                    note_id: None,
                    expected_hash: None,
                    folder_id: None,
                    asset_group: String::new(),
                    title: String::new(),
                    content: String::new(),
                });
                app.prepare_document_view(window, cx);
                assert!(app.note_content.read(cx).requested_visible_for_test());
                app.update_panel.update(cx, |panel, _| {
                    panel.finish_nsis_check(
                        CheckOrigin::Manual,
                        Ok(Some(crate::update_manifest::UpdateManifest {
                            version: semver::Version::parse("9.0.0").unwrap(),
                            notes: String::new(),
                            history: Vec::new(),
                            pub_date: None,
                            setup_url: String::new(),
                            signature: String::new(),
                            signed_envelope: String::new(),
                            size: 1,
                            sha256: "a".repeat(64),
                        })),
                    );
                });
                assert!(
                    !app.has_modal(cx),
                    "discovery must leave the current editor visible"
                );
                app.prepare_document_view(window, cx);
                assert!(app.note_content.read(cx).requested_visible_for_test());
                app.update_panel
                    .update(cx, |panel, cx| panel.show_dialog(cx));
                assert!(app.has_modal(cx));
                app.prepare_document_view(window, cx);
                assert!(
                    !app.note_content.read(cx).requested_visible_for_test(),
                    "native surface must not cover update card"
                );
                app.update_panel
                    .update(cx, |panel, cx| panel.dismiss_dialog(cx));
                app.prepare_document_view(window, cx);
                assert!(app.note_content.read(cx).requested_visible_for_test());
            })
        });
    }
}

#[cfg(all(test, feature = "ui-test"))]
#[path = "notification_update_tests.rs"]
mod notification_tests;
