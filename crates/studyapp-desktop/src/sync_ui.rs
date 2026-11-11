use crate::controls::Button;
use crate::{
    components::*,
    settings_components as form,
    sync_runtime::{SyncSettings, SyncStatus, SyncTarget},
    worker::Work,
};
use gpui_kit::{
    component::{Disableable, input::InputState},
    *,
};
use std::sync::mpsc;

#[derive(Clone, Debug)]
pub enum SyncEvent {
    Notification(studyapp_core::NoticeEvent),
}

pub struct SyncPanel {
    sender: mpsc::Sender<Work>,
    pub(crate) notification_center: Option<Entity<crate::notifications::NotificationCenter>>,
    pub status: SyncStatus,
    pub editing_locked: bool,
    pub(crate) search_reveal: bool,
    pub target: usize,
    pub folder: Entity<InputState>,
    pub url: Entity<InputState>,
    pub username: Entity<InputState>,
    pub password: Entity<InputState>,
    credential: Entity<studyapp_ui::settings_secret::SecretSetting>,
    interval: Entity<InputState>,
    automatic: bool,
    initialized: bool,
    folder_picker: crate::folder_picker::FolderPicker,
}

impl EventEmitter<SyncEvent> for SyncPanel {}

impl SyncPanel {
    fn announce_error(&self, summary: &'static str, cx: &mut Context<Self>) {
        let mut event = studyapp_core::NoticeEvent::new(
            studyapp_core::NoticeCategory::SyncFailure,
            studyapp_core::NoticeSeverity::Error,
            summary,
        );
        event.action = Some(studyapp_core::NoticeAction::OpenSync);
        event.dedupe_key = Some("sync-panel-error".into());
        event.anchor = Some("sync-start".into());
        cx.emit(SyncEvent::Notification(event));
    }
    fn invalidate_folder_picker(&mut self) {
        self.folder_picker.invalidate();
    }

    /// Locking begins a new draft epoch, even if a failed save later unlocks
    /// without replacing settings. Its older native picker can no longer edit.
    pub fn set_editing_locked(&mut self, locked: bool, cx: &mut Context<Self>) {
        if self.editing_locked != locked {
            if locked {
                self.invalidate_folder_picker();
            }
            self.editing_locked = locked;
            cx.notify();
        }
    }

    #[cfg(all(test, feature = "ui-test"))]
    fn begin_folder_picker(&mut self) -> Option<crate::folder_picker::FolderRequest> {
        if self.editing_locked || self.status.busy || self.target != 1 {
            return None;
        }
        self.invalidate_folder_picker();
        Some(self.folder_picker.begin(&self.status.library))
    }

    pub fn has_unsaved_changes(&self, cx: &App) -> bool {
        let target_matches = match (&self.status.settings.target, self.target) {
            (SyncTarget::Disabled, 0) => true,
            (SyncTarget::Folder { path }, 1) => path == self.folder.read(cx).value().trim(),
            (SyncTarget::WebDav { url, username }, 2) => {
                url == self.url.read(cx).value().trim()
                    && username == self.username.read(cx).value().trim()
            }
            _ => false,
        };
        !target_matches
            || self.credential.read(cx).pending_clear()
            || !self.password.read(cx).value().is_empty()
            || self.automatic != self.status.settings.auto_minutes.is_some()
            || (self.automatic
                && self.interval.read(cx).value().trim().parse::<u32>().ok()
                    != self.status.settings.auto_minutes)
    }
    pub fn new(sender: mpsc::Sender<Work>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = |placeholder: &str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder.to_owned()))
        };
        let password = input("应用密码；留空保留已保存密码", window, cx);
        password.update(cx, |s, cx| s.set_masked(true, window, cx));
        let credential = cx.new(|cx| {
            studyapp_ui::settings_secret::SecretSetting::new(password.clone(), false, cx)
        });
        cx.subscribe(
            &credential,
            |_, _, _: &studyapp_ui::settings_secret::SecretChanged, cx| cx.notify(),
        )
        .detach();
        let panel = Self {
            sender,
            notification_center: None,
            status: SyncStatus::default(),
            editing_locked: false,
            search_reveal: false,
            target: 0,
            folder: input("选择本地文件夹、网盘同步目录或 NAS 路径", window, cx),
            url: input("https://dav.example.com/path/", window, cx),
            username: input("WebDAV 用户名", window, cx),
            password,
            credential,
            interval: input("5–1440 分钟", window, cx),
            automatic: false,
            initialized: false,
            folder_picker: Default::default(),
        };
        for input in [
            &panel.folder,
            &panel.url,
            &panel.username,
            &panel.password,
            &panel.interval,
        ] {
            cx.subscribe(
                input,
                |_, _, event: &gpui_kit::component::input::InputEvent, cx| {
                    if matches!(event, gpui_kit::component::input::InputEvent::Change) {
                        cx.notify();
                    }
                },
            )
            .detach();
        }
        panel
    }

    pub fn accept(
        &mut self,
        status: SyncStatus,
        reset: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let opening_library = reset || !self.initialized || status.library != self.status.library;
        // Loading has no worker result notice; surface its warning once for
        // this library. Routine status polls must not recreate a dismissed toast.
        if opening_library && status.failed {
            self.announce_error("同步配置无法读取，请重新配置；原配置文件保留。", cx);
        }
        if opening_library || status.settings != self.status.settings {
            self.invalidate_folder_picker();
            self.target = match &status.settings.target {
                SyncTarget::Disabled => 0,
                SyncTarget::Folder { path } => {
                    self.folder
                        .update(cx, |s, cx| s.set_value(path.clone(), window, cx));
                    1
                }
                SyncTarget::WebDav { url, username } => {
                    self.url
                        .update(cx, |s, cx| s.set_value(url.clone(), window, cx));
                    self.username
                        .update(cx, |s, cx| s.set_value(username.clone(), window, cx));
                    2
                }
            };
            self.credential
                .update(cx, |secret, cx| secret.reset(window, cx));
            self.automatic = status.settings.auto_minutes.is_some();
            self.interval.update(cx, |s, cx| {
                s.set_value(
                    status.settings.auto_minutes.unwrap_or(15).to_string(),
                    window,
                    cx,
                )
            });
            self.initialized = true;
        }
        self.status = status;
        cx.notify();
    }

    pub fn poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing_locked || self.status.busy || self.target != 1 {
            self.invalidate_folder_picker();
            return;
        }
        if let Some(path) = self.folder_picker.poll(&self.status.library) {
            self.folder.update(cx, |s, cx| {
                s.set_value(path.to_string_lossy().into_owned(), window, cx)
            });
            cx.notify();
        }
    }

    pub fn draft_settings(
        &self,
        cx: &App,
    ) -> anyhow::Result<(SyncSettings, Option<zeroize::Zeroizing<String>>)> {
        let target = match self.target {
            1 => SyncTarget::Folder {
                path: self.folder.read(cx).value().trim().into(),
            },
            2 => SyncTarget::WebDav {
                url: self.url.read(cx).value().trim().into(),
                username: self.username.read(cx).value().trim().into(),
            },
            _ => SyncTarget::Disabled,
        };
        let auto_minutes = if self.automatic {
            Some(
                self.interval
                    .read(cx)
                    .value()
                    .trim()
                    .parse()
                    .map_err(|_| anyhow::anyhow!("自动同步间隔必须为整数"))?,
            )
        } else {
            None
        };
        let raw = self.password.read(cx).value().to_string();
        let password = (self.credential.read(cx).pending_clear() || !raw.is_empty())
            .then(|| zeroize::Zeroizing::new(raw));
        Ok((
            SyncSettings {
                target,
                auto_minutes,
            },
            password,
        ))
    }

    pub fn reset_draft(
        &mut self,
        settings: SyncSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.invalidate_folder_picker();
        self.target = match &settings.target {
            SyncTarget::Disabled => 0,
            SyncTarget::Folder { path } => {
                self.folder
                    .update(cx, |input, cx| input.set_value(path.clone(), window, cx));
                1
            }
            SyncTarget::WebDav { url, username } => {
                self.url
                    .update(cx, |input, cx| input.set_value(url.clone(), window, cx));
                self.username.update(cx, |input, cx| {
                    input.set_value(username.clone(), window, cx)
                });
                2
            }
        };
        self.automatic = settings.auto_minutes.is_some();
        self.interval.update(cx, |input, cx| {
            input.set_value(settings.auto_minutes.unwrap_or(15).to_string(), window, cx)
        });
        self.credential
            .update(cx, |secret, cx| secret.reset(window, cx));
        cx.notify();
    }

    pub(crate) fn start(&mut self, test_only: bool, cx: &mut Context<Self>) {
        if self.editing_locked || self.status.busy {
            return;
        }
        if self.has_unsaved_changes(cx) {
            self.announce_error("同步设置有未保存的修改，请先保存。", cx);
            cx.notify();
            return;
        }
        if self
            .sender
            .send(Work::Scoped {
                library: self.status.library.clone(),
                work: Box::new(Work::StartSync { test_only }),
            })
            .is_err()
        {
            self.announce_error("同步后台服务已停止，请重新打开应用。", cx);
        }
        cx.notify();
    }
}

impl Render for SyncPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.status.busy || self.editing_locked;
        let unsaved = self.has_unsaved_changes(cx);
        let mut panel = form::section("笔记文件同步")
            .child(muted("连接自己的 WebDAV 或文件夹/NAS。同步 Markdown 文件，完整历史和学习记录由本地资料库与备份管理。"))
            .child(form::settings_row("同步目标", form::choice_with_default(
                "sync-target", (self.target, 0usize),
                vec![(0usize, "关闭同步".into()), (1usize, "文件夹 / NAS".into()), (2usize, "WebDAV".into())],
                busy, cx.weak_entity(), |this, target, _, cx| {
                    if this.editing_locked || this.status.busy { return; }
                    if this.target != target { this.invalidate_folder_picker(); }
                    this.target = target; cx.notify();
                },
            )));
        if self.target == 1 || self.search_reveal {
            let busy = busy || self.target != 1;
            panel = panel.child(
                form::DirectoryField::new(
                    "sync-directory-detail",
                    "同步文件夹",
                    self.folder.read(cx).value(),
                )
                .fallback("尚未选择")
                .editor(form::input(&self.folder, busy))
                .actions(
                    row()
                        .flex_wrap()
                        .child(
                            Button::new("reset-sync-folder")
                                .label("重置")
                                .disabled(busy || self.folder.read(cx).value().is_empty())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.invalidate_folder_picker();
                                    this.folder
                                        .update(cx, |input, cx| input.set_value("", window, cx));
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("pick-sync-folder")
                                .action("选择文件夹")
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if this.editing_locked || this.status.busy || this.target != 1 {
                                        return;
                                    }
                                    let current = std::path::PathBuf::from(
                                        this.folder.read(cx).value().trim(),
                                    );
                                    this.folder_picker.choose(
                                        &this.status.library,
                                        Some(current),
                                        "选择同步根目录",
                                    );
                                })),
                        ),
                ),
            );
        }
        if self.target == 2 || self.search_reveal {
            let busy = busy || self.target != 2;
            panel = panel
                .child(form::field(
                    "WebDAV 地址",
                    div()
                        .id("sync-url-field")
                        .test_support()
                        .child(form::input(&self.url, busy)),
                ))
                .child(
                    row()
                        .flex_wrap()
                        .child(
                            form::field("账户", form::input(&self.username, busy))
                                .min_w(px(180.))
                                .flex_1(),
                        )
                        .child(
                            form::field("凭据", {
                                self.credential.update(cx, |secret, cx| {
                                    secret.configure(self.status.credential_saved, busy, cx)
                                });
                                self.credential.clone()
                            })
                            .min_w(px(180.))
                            .flex_1(),
                        ),
                )
                .child(muted(if self.status.credential_saved {
                    "密码已由当前 Windows 用户保护；此字段留空可继续使用。"
                } else {
                    "建议使用服务商提供的应用密码；凭据不进入学习数据库或备份。"
                }));
        }
        if self.search_reveal {
            panel = panel.child(muted("搜索仅展开相关配置；不属于当前同步目标的字段仅供查看。选择对应目标后才能编辑，搜索不会切换目标或修改草稿。"));
        }
        let sync_now = Button::new("sync-now")
            .icon_action(
                gpui_kit::assets::IconName::RefreshCw,
                if busy {
                    "正在同步…"
                } else {
                    "立即同步"
                },
            )
            .disabled(
                busy || unsaved || matches!(self.status.settings.target, SyncTarget::Disabled),
            )
            .on_click(cx.listener(|this, _, _, cx| this.start(false, cx)));
        let sync_now = if let Some(center) = &self.notification_center {
            studyapp_ui::notification_anchor::NotificationAnchor::new(
                "sync-start",
                center.clone(),
                sync_now,
            )
            .into_any_element()
        } else {
            sync_now.into_any_element()
        };
        panel.child(column().child(studyapp_ui::settings_switch::SettingSwitch::new("auto-sync").default_value(false).label("自动同步").checked(self.automatic).disabled(busy)
            .on_click(cx.listener(|this, checked, _, cx| {
                if this.editing_locked || this.status.busy { return; }
                this.automatic = *checked; cx.notify();
            })))
            .child(form::settings_row("自动同步间隔", form::NumericSetting::new("auto-sync-interval", &self.interval, "分钟", 30, 5, 1440, busy || !self.automatic)))
            .children((!self.automatic).then(|| muted("启用自动同步后才能调整间隔；当前间隔草稿保留。"))))
            .child(row().child(Button::new("test-sync").action("测试连接").disabled(busy || unsaved || matches!(self.status.settings.target, SyncTarget::Disabled)).on_click(cx.listener(|this, _, _, cx| this.start(true, cx))))
                .child(sync_now))
            .child(muted(if unsaved { "同步设置有未保存的修改，请先保存。" } else { "使用已保存的同步设置" }))
            .child(muted(if busy { "正在处理同步任务…" }
                else if matches!(self.status.settings.target, SyncTarget::Disabled) { "同步已关闭" }
                else { "同步已就绪" }))
            .child(muted(format!("最近成功：{}", self.status.last_success.map(studyapp_desktop::presentation::stamp).unwrap_or_else(|| "尚未完成".into()))))
            .child(muted("远端专用目录：StudyApp/notes；不同修改会保留冲突副本。不传播删除，导入的外部原文件不会被同步改写。"))
    }
}

#[cfg(all(test, feature = "ui-test"))]
mod feedback_tests {
    use super::{SyncEvent, SyncPanel};
    use crate::sync_runtime::SyncStatus;
    use gpui_kit::TestAppContext;
    use std::sync::mpsc;

    #[gpui_kit::test]
    fn search_reveal_keeps_inactive_target_fields_disabled_and_preserves_raw_drafts(
        cx: &mut TestAppContext,
    ) {
        use gpui_kit::test::TestWindowExt;
        cx.update(gpui_kit::init);
        let (sender, _receiver) = mpsc::channel();
        let (panel, visual) = cx.add_window_view(|window, cx| SyncPanel::new(sender, window, cx));
        visual.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.folder.update(cx, |input, cx| {
                    input.set_value("preserved-folder", window, cx)
                });
                panel.url.update(cx, |input, cx| {
                    input.set_value("https://dav.invalid/", window, cx)
                });
                panel.search_reveal = true;
                assert!(!panel.has_unsaved_changes(cx));
                cx.notify();
            })
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
            assert!(window.try_find("sync-url-field").is_some());
            assert!(window.try_find("sync-directory-detail").is_some());
            window.click("reset-sync-folder", cx);
            panel.update(cx, |panel, cx| {
                assert_eq!(panel.target, 0);
                assert_eq!(panel.folder.read(cx).value(), "preserved-folder");
                assert_eq!(panel.url.read(cx).value(), "https://dav.invalid/");
                assert!(!panel.has_unsaved_changes(cx));
                assert!(panel.password.read(cx).value().is_empty());
            });
        });
    }

    #[gpui_kit::test]
    fn stopped_worker_emits_one_floating_error_instead_of_inline_state(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (sender, receiver) = mpsc::channel();
        drop(receiver);
        let (panel, cx) = cx.add_window_view(|window, cx| SyncPanel::new(sender, window, cx));
        let feedback = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let received = feedback.clone();
        let subscription = cx.update(|_, cx| {
            cx.subscribe(&panel, move |_, event: &SyncEvent, _| {
                received.borrow_mut().push(event.clone());
            })
        });
        panel.update(cx, |panel, cx| panel.start(false, cx));
        cx.run_until_parked();
        assert!(
            matches!(feedback.borrow().as_slice(), [SyncEvent::Notification(event)] if event.category == studyapp_core::NoticeCategory::SyncFailure && event.summary.contains("后台服务已停止"))
        );
        cx.update(|window, cx| panel.update(cx, |panel, cx| panel.poll(window, cx)));
        cx.run_until_parked();
        assert_eq!(feedback.borrow().len(), 1, "poll repeated the same error");
        drop(subscription);
    }

    #[gpui_kit::test]
    fn unreadable_sync_configuration_is_reported_once_when_library_opens(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (sender, _receiver) = mpsc::channel();
        let (panel, cx) = cx.add_window_view(|window, cx| SyncPanel::new(sender, window, cx));
        let feedback = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let received = feedback.clone();
        let subscription = cx.update(|_, cx| {
            cx.subscribe(&panel, move |_, event: &SyncEvent, _| {
                received.borrow_mut().push(event.clone())
            })
        });
        let status = SyncStatus {
            library: "isolated-test-library".into(),
            failed: true,
            message: "同步配置无法读取，请重新配置；原配置文件保留".into(),
            ..Default::default()
        };
        for _ in 0..2 {
            cx.update(|window, cx| {
                panel.update(cx, |panel, cx| {
                    panel.accept(status.clone(), false, window, cx)
                })
            });
            cx.run_until_parked();
        }
        assert!(
            matches!(feedback.borrow().as_slice(), [SyncEvent::Notification(event)] if event.category == studyapp_core::NoticeCategory::SyncFailure && event.summary.contains("同步配置无法读取"))
        );
        drop(subscription);
    }
}

#[cfg(all(test, feature = "ui-test"))]
mod picker_tests {
    use super::SyncPanel;
    use crate::sync_runtime::{SyncSettings, SyncStatus, SyncTarget};
    use gpui_kit::TestAppContext;
    use std::sync::mpsc;

    fn status(library: &str, path: &str) -> SyncStatus {
        SyncStatus {
            library: library.into(),
            settings: SyncSettings {
                target: SyncTarget::Folder { path: path.into() },
                auto_minutes: None,
            },
            ..Default::default()
        }
    }

    #[gpui_kit::test]
    fn only_the_latest_picker_can_change_the_current_draft(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (sender, _receiver) = mpsc::channel();
        let (panel, cx) = cx.add_window_view(|window, cx| SyncPanel::new(sender, window, cx));
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.accept(status("library-a", "saved"), false, window, cx);
                let old = panel.begin_folder_picker().unwrap();
                let current = panel.begin_folder_picker().unwrap();
                panel.folder_picker.reply_test(old, "stale");
                panel.folder_picker.reply_test(current.clone(), "chosen");
                panel.folder_picker.reply_test(current, "duplicate");
                panel.poll(window, cx);
                assert_eq!(panel.folder.read(cx).value(), "chosen");
            })
        });
    }

    #[gpui_kit::test]
    fn reset_or_library_round_trip_invalidates_delayed_picker_results(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (sender, _receiver) = mpsc::channel();
        let (panel, cx) = cx.add_window_view(|window, cx| SyncPanel::new(sender, window, cx));
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.accept(status("library-a", "saved"), false, window, cx);
                let before_reset = panel.begin_folder_picker().unwrap();
                panel.reset_draft(status("library-a", "reset").settings, window, cx);
                panel.folder_picker.reply_test(before_reset, "stale-reset");
                panel.poll(window, cx);
                assert_eq!(panel.folder.read(cx).value(), "reset");
                let before_switch = panel.begin_folder_picker().unwrap();
                panel.accept(status("library-b", "other"), false, window, cx);
                panel.accept(status("library-a", "restored"), false, window, cx);
                panel
                    .folder_picker
                    .reply_test(before_switch, "stale-library");
                panel.poll(window, cx);
                assert_eq!(panel.folder.read(cx).value(), "restored");
            })
        });
    }

    #[gpui_kit::test]
    fn editing_lock_retires_open_picker_even_if_reply_arrives_after_unlock(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (sender, _receiver) = mpsc::channel();
        let (panel, cx) = cx.add_window_view(|window, cx| SyncPanel::new(sender, window, cx));
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.accept(status("library-a", "saved"), false, window, cx);
                let before_save = panel.begin_folder_picker().unwrap();
                panel.set_editing_locked(true, cx);
                assert!(panel.begin_folder_picker().is_none());
                panel
                    .folder_picker
                    .reply_test(before_save.clone(), "while-locked");
                panel.poll(window, cx);
                assert_eq!(panel.folder.read(cx).value(), "saved");
                panel.set_editing_locked(false, cx);
                panel.folder_picker.reply_test(before_save, "after-unlock");
                panel.poll(window, cx);
                assert_eq!(panel.folder.read(cx).value(), "saved");
                let valid = panel.begin_folder_picker().unwrap();
                panel.folder_picker.reply_test(valid, "new-picker");
                panel.poll(window, cx);
                assert_eq!(panel.folder.read(cx).value(), "new-picker");
            })
        });
    }
}
