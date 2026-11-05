//! Window policy and semantic navigation for the shared notification controller.
use crate::{
    app::{Page, StudyApp},
    settings_ui::{SettingsSection, StoragePage},
};
use gpui_kit::*;
use studyapp_core::{NoticeAction, NoticeEnvironment, NoticeEvent, NoticeEventId};

impl StudyApp {
    pub fn sync_notification_preferences(&self, window: &Window, cx: &mut Context<Self>) {
        let current = self
            .preferences_ui
            .saved
            .as_ref()
            .map(|s| &s.settings)
            .or_else(|| self.snapshot.as_ref().map(|s| &s.settings));
        let defaults = studyapp_core::Settings::default();
        let current = current.unwrap_or(&defaults);
        self.configure_notification_settings(current, window, cx);
    }
    pub fn configure_notification_settings(
        &self,
        current: &studyapp_core::Settings,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        use chrono::Timelike;
        let clock = chrono::Local::now();
        let environment = NoticeEnvironment {
            foreground: window.is_window_active(),
            desktop_available: !self.notification_desktop_failed
                && !self.close_requested
                && !self.close_allowed
                && !cx.displays().is_empty(),
            anchor_available: false,
            minute: (clock.hour() * 60 + clock.minute()) as u16,
        };
        self.update_panel.update(cx, |panel, cx| {
            panel.set_badge_enabled(current.notifications.update_badge, cx)
        });
        let blocked = self.has_required_modal(cx);
        self.notifications.update(cx, |center, cx| {
            center.configure(
                &current.notifications,
                &current.appearance.notifications,
                environment,
                blocked,
                cx,
            )
        });
    }
    pub fn emit_notification(
        &mut self,
        event: NoticeEvent,
        cx: &mut Context<Self>,
    ) -> Option<NoticeEventId> {
        self.notice = Some(
            if event.severity >= studyapp_core::NoticeSeverity::Warning {
                Err(event.summary.clone())
            } else {
                Ok(event.summary.clone())
            },
        );
        let source = event.id;
        crate::audit_log::record(crate::audit_log::Record::notice(&event));
        let displayed = self
            .notifications
            .update(cx, |center, cx| center.enqueue(event, cx));
        if let Some(target) = displayed.filter(|target| *target != source) {
            crate::audit_log::record(crate::audit_log::Record {
                event: Some(source),
                related_event: Some(target),
                ..crate::audit_log::Record::new(
                    studyapp_core::LogLevel::Info,
                    crate::audit_log::Phase::NotificationMerged,
                    crate::audit_log::Outcome::Success,
                )
            });
        }
        displayed
    }

    pub fn handle_notification_action(
        &mut self,
        action: NoticeAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = match action {
            NoticeAction::OpenUpdate | NoticeAction::RetryUpdate => {
                Some(SettingsSection::AboutAccount)
            }
            NoticeAction::OpenSync | NoticeAction::RetrySync => Some(SettingsSection::Sync),
            NoticeAction::OpenPreferences => Some(self.preferences_ui.section),
            NoticeAction::OpenLogging => Some(SettingsSection::Logging),
            NoticeAction::OpenStorage | NoticeAction::OpenIntegrity | NoticeAction::ShowDetails => {
                Some(SettingsSection::Storage)
            }
            _ => None,
        };
        if let Some(section) = target {
            self.navigate(Page::Settings, window, cx);
            if self.page != Page::Settings {
                return;
            }
            self.select_settings_section(section, cx);
            match action {
                NoticeAction::OpenUpdate => self.update_panel.update(cx, |panel, cx| {
                    if panel.available_version().is_some() {
                        panel.show_dialog(cx);
                    }
                }),
                NoticeAction::RetryUpdate => self
                    .update_panel
                    .update(cx, |panel, cx| panel.retry_from_notice(cx)),
                NoticeAction::RetrySync => self
                    .sync_panel
                    .update(cx, |panel, cx| panel.start(false, cx)),
                NoticeAction::OpenIntegrity => {
                    self.preferences_ui.storage_page = StoragePage::Overview;
                    self.storage_ui.integrity_dialog = self.storage_ui.integrity_result.is_some();
                }
                _ => {}
            }
        } else {
            let page = match action {
                NoticeAction::OpenTimer => Page::Today,
                NoticeAction::OpenReviews => Page::Reviews,
                _ => Page::Notes,
            };
            self.navigate(page, window, cx);
        }
        cx.notify();
    }
}
