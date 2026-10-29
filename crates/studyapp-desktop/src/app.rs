use crate::controls::Button;
use crate::{
    components::*,
    drafts::{DraftRecord, DraftStore, NoteDraft},
    worker::{Work, Worker},
};
use chrono::{Local, Utc};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    Disableable, Selectable, Sizable,
    button::ButtonVariants,
    input::{InputState, TextareaState},
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::{collections::HashSet, sync::Arc, time::Duration};
use studyapp_core::{Command, SessionStatus, Snapshot};
use studyapp_desktop::presentation::parse_local_time;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Today,
    Notes,
    Reviews,
    Stats,
    Settings,
}

#[derive(Clone)]
pub enum EditorMode {
    Pending(String),
    Manual,
    Edit(String),
}

pub struct StudyApp {
    // Drain queued recovery snapshots before Worker releases the library lock.
    pub drafts: DraftStore,
    pub worker: Worker,
    pub snapshot: Option<Arc<Snapshot>>,
    pub startup_failure: Option<crate::worker::startup::StartupFailure>,
    pub startup_busy: bool,
    pub read_models: Arc<studyapp_desktop::read_models::SnapshotReadModels>,
    pub page: Page,
    pub notice: Option<Result<String, String>>,
    pub search: Entity<InputState>,
    pub summary: Entity<TextareaState>,
    pub summary_editor: Entity<studyapp_ui::compact_markdown::CompactMarkdown>,
    pub minutes: Entity<InputState>,
    pub start: Entity<InputState>,
    pub interval: Entity<InputState>,
    pub deadline: Entity<InputState>,
    pub review_days: Entity<InputState>,
    pub editor: Option<EditorMode>,
    pub chosen_notes: HashSet<String>,
    pub chosen_versions: HashSet<String>,
    pub saving: bool,
    pub selected_note: Option<String>,
    pub selected_version: Option<String>,
    pub history: bool,
    pub show_detached: bool,
    pub stats_days: usize,
    pub record_filter: usize,
    pub record_limit: usize,
    pub review_filter: usize,
    pub last_library: String,
    pub note_title: Entity<InputState>,
    pub note_content: Entity<crate::markdown_editor::MarkdownEditor>,
    pub note_editor: Option<NoteDraft>,
    pub recoverable_draft: Option<NoteDraft>,
    pub draft_catalog: std::collections::BTreeMap<String, DraftRecord>,
    pub draft_library: studyapp_desktop::draft_viewmodel::DraftLibraryIndex,
    pub active_draft_id: Option<String>,
    pub draft_durable: std::collections::BTreeMap<String, u64>,
    pub draft_errors: std::collections::BTreeMap<String, String>,
    pub draft_close_drain: Option<u64>,
    pub draft_drain_intent: Option<crate::editor_lifecycle::NoteIntent>,
    pub draft_deferred_work: Option<(String, Work)>,
    pub draft_work_ready: bool,
    pub pending_note_save: Option<(String, String, u64)>,
    pub draft_loading: bool,
    pub draft_failed: bool,
    pub record_date: Entity<InputState>,
    pub note_saving: bool,
    pub note_intent: Option<(u64, crate::editor_lifecycle::NoteIntent)>,
    pub pending_library: Option<(
        Arc<Snapshot>,
        Arc<studyapp_desktop::read_models::SnapshotReadModels>,
        bool,
    )>,
    pub draft_persist: Option<crate::editor_lifecycle::PersistPending>,
    pub draft_discard: Option<crate::editor_lifecycle::DiscardPending>,
    pub confirmation: Option<crate::editor_lifecycle::NoteConfirmation>,
    pub reader_key: String,
    pub draft_dirty_since: Option<std::time::Instant>,
    pub close_requested: bool,
    pub close_allowed: bool,
    pub sync_panel: Entity<crate::sync_ui::SyncPanel>,
    pub folders: Entity<crate::folders_ui::FolderPanel>,
    pub history_preview: crate::history_preview::HistoryPreview,
    pub storage_report: Option<crate::storage_backend::StorageReport>,
    pub storage_busy: bool,
    pub storage_confirm: Option<String>,
    pub storage_ui: crate::storage_ui::StorageUi,
    pub storage_active_draft_notes: HashSet<String>,
    pub storage_draft_revision: u64,
    pub page_scroll: ScrollHandle,
    pub settings_scroll: ScrollHandle,
    pub learning_versions_scroll: ScrollHandle,
    pub history_versions_scroll: ScrollHandle,
    pub statistics_scroll: ScrollHandle,
    pub active_note_menu: Option<u64>,
    pub note_menu_sequence: u64,
    pub layout: studyapp_core::LayoutSettings,
    pub resizing: Option<crate::workspace_layout::ResizeState>,
    pub layout_dirty: bool,
    pub preferences_ui: crate::settings_ui::PreferencesUi,
    pub(crate) picker_sender: std::sync::mpsc::Sender<(String, Work)>,
    pub(crate) picker_results: std::sync::mpsc::Receiver<(String, Work)>,
    pub modal_focus: crate::feedback::ModalFocus,
    pub notifications: Entity<crate::notifications::NotificationCenter>,
    pub bubble_registry: Entity<studyapp_ui::bubble::BubbleRegistry>,
    #[cfg(feature = "ui-test")]
    pub smoke_pointer_bubble: Option<AnyView>,
    pub update_panel: Entity<crate::update_ui::UpdatePanel>,
    pub pending_update: Option<crate::update_backend::PreparedUpdate>,
    pub pending_nsis_update: Option<crate::update_install::PreparedPackage>,
    pub image_cloud_busy: bool,
    pub applied_appearance: Option<crate::theme::ResolvedTheme>,
    pub appearance_source: Option<(studyapp_core::AppearanceSettings, bool)>,
    pub external_drop: crate::external_drop::ExternalDropState,
    pub external_drop_paths: Vec<std::path::PathBuf>,
    pub media_ui: crate::media_ui::MediaUi,
    notified: HashSet<String>,
    _poll: Task<()>,
}

impl StudyApp {
    pub fn new(worker: Worker, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (picker_sender, picker_results) = std::sync::mpsc::channel();
        let input =
            |placeholder: &str, value: &str, window: &mut Window, cx: &mut Context<Self>| {
                cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(placeholder.to_owned())
                        .default_value(value.to_owned())
                })
            };
        let poll = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                if this
                    .update_in(cx, |this, window, cx| this.poll(window, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        cx.observe_window_appearance(window, |this, window, cx| {
            this.refresh_appearance(window, cx);
            cx.notify();
        })
        .detach();
        let search = input("搜索标题、路径与正文", "", window, cx);
        cx.observe(&search, |this, search, cx| {
            // Observe actual text so programmatic restore/clear also updates
            // the model; focus and caret notifications do no catalog work.
            if this
                .draft_library
                .set_query(search.read(cx).value().as_ref())
            {
                this.refresh_draft_library();
                cx.notify();
            }
        })
        .detach();
        let record_date = input("学习日期 YYYY-MM-DD", "", window, cx);
        cx.subscribe(
            &record_date,
            |this, _, _: &gpui_kit::component::input::InputEvent, cx| {
                this.record_limit = 50;
                cx.notify();
            },
        )
        .detach();
        let note_title = input("笔记标题", "", window, cx);
        let note_content = cx.new(|cx| crate::markdown_editor::MarkdownEditor::new(window, cx));
        let notifications = cx.new(|_| crate::notifications::NotificationCenter::new());
        let bubble_registry = studyapp_ui::bubble::window_bubble_registry(window, cx);
        let bubble_editor = note_content.clone();
        cx.subscribe(
            &bubble_registry,
            move |this, _, event: &studyapp_ui::bubble::BubbleRegionsChanged, cx| {
                bubble_editor.update(cx, |editor, cx| {
                    editor.set_overlay_shapes("pointer-tooltip", event.0.clone(), cx)
                });
                if let Some(preview) = &this.preferences_ui.appearance_preview {
                    preview.update(cx, |preview, cx| {
                        preview.set_overlay_shapes("pointer-tooltip", event.0.clone(), cx)
                    });
                }
            },
        )
        .detach();
        let update_panel = cx.new(crate::update_ui::UpdatePanel::new);
        cx.subscribe_in(
            &update_panel,
            window,
            |this, _, event: &crate::update_ui::UpdateEvent, window, cx| match event {
                crate::update_ui::UpdateEvent::InstallRequested => {
                    this.request_install_update(window, cx)
                }
                crate::update_ui::UpdateEvent::NsisReady => this.install_nsis_update(window, cx),
                crate::update_ui::UpdateEvent::AvailabilityChanged => cx.notify(),
            },
        )
        .detach();
        crate::update_ui::schedule_startup(update_panel.clone(), cx);
        let editor_occlusions = note_content.clone();
        cx.subscribe(
            &notifications,
            move |this, _, event: &crate::notifications::NotificationOcclusions, cx| {
                editor_occlusions
                    .update(cx, |editor, cx| editor.set_occlusions(event.0.clone(), cx));
                if let Some(preview) = &this.preferences_ui.appearance_preview {
                    preview.update(cx, |preview, cx| {
                        preview.set_occlusions(event.0.clone(), cx)
                    });
                }
            },
        )
        .detach();
        cx.subscribe(
            &note_title,
            |this, _, _: &gpui_kit::component::input::InputEvent, _| this.mark_note_dirty(),
        )
        .detach();
        cx.subscribe(
            &note_content,
            |this, _, _: &crate::markdown_editor::MarkdownChanged, _| this.mark_note_dirty(),
        )
        .detach();
        cx.subscribe_in(
            &note_content,
            window,
            |this, _, event: &crate::markdown_editor::MarkdownEvent, window, cx| {
                this.editor_event(event.clone(), window, cx);
            },
        )
        .detach();
        let weak = cx.weak_entity();
        let shortcut_owner = weak.clone();
        let shortcut_window = window.window_handle();
        cx.intercept_keystrokes(move |event, window, cx| {
            if window.window_handle() != shortcut_window {
                return;
            }
            let _ = shortcut_owner.update(cx, |this, cx| {
                if this.capture_settings_keystroke(&event.keystroke, window, cx) {
                    cx.stop_propagation();
                    return;
                }
                if event.input_preference == InputPreference::CharacterInput {
                    return;
                }
                if this.external_drop.active() && event.keystroke.key.eq_ignore_ascii_case("escape")
                {
                    this.cancel_external_drag(cx);
                    cx.stop_active_drag(window);
                    cx.stop_propagation();
                    return;
                }
                if this.is_layout_resizing() && event.keystroke.key.eq_ignore_ascii_case("escape") {
                    this.cancel_resize(cx);
                    cx.stop_propagation();
                } else if this.handle_shortcut(&event.keystroke, window, cx) {
                    cx.stop_propagation();
                }
            });
        })
        .detach();
        window.on_window_should_close(cx, move |window, cx| {
            weak.update(cx, |this, cx| this.handle_close(window, cx))
                .unwrap_or(true)
        });
        let sync_panel =
            cx.new(|cx| crate::sync_ui::SyncPanel::new(worker.sender.clone(), window, cx));
        cx.subscribe(
            &sync_panel,
            |this, _, event: &crate::sync_ui::SyncEvent, cx| {
                match event {
                    crate::sync_ui::SyncEvent::Feedback(message) => {
                        this.push_notice(message.clone(), cx);
                    }
                }
                cx.notify();
            },
        )
        .detach();
        cx.observe(&sync_panel, |_, _, cx| cx.notify()).detach();
        let folders = cx.new(|cx| crate::folders_ui::FolderPanel::new(window, cx));
        let interval = input("5–240 分钟", "60", window, cx);
        let deadline = input("1–30 分钟", "5", window, cx);
        let review_days = input("用逗号分隔天数", "1, 3, 7, 14, 30", window, cx);
        for setting in [&interval, &deadline, &review_days] {
            cx.subscribe(
                setting,
                |_, _, event: &gpui_kit::component::input::InputEvent, cx| {
                    if matches!(event, gpui_kit::component::input::InputEvent::Change) {
                        cx.notify();
                    }
                },
            )
            .detach();
        }
        cx.observe(&folders, |_, _, cx| cx.notify()).detach();
        cx.subscribe_in(
            &folders,
            window,
            |this, _, event: &crate::folders_ui::FolderEvent, window, cx| {
                match event {
                    crate::folders_ui::FolderEvent::Filter(filter) => {
                        this.select_folder_document(filter.clone(), window, cx);
                    }
                    crate::folders_ui::FolderEvent::Command(command) => {
                        this.send(command.as_ref().clone(), cx)
                    }
                    crate::folders_ui::FolderEvent::RenameNote {
                        library_path,
                        command,
                    } => {
                        this.handle_note_rename_command(library_path, command.as_ref().clone(), cx)
                    }
                    crate::folders_ui::FolderEvent::MoveDraft {
                        library_path,
                        draft_id,
                        folder_id,
                    } => {
                        this.move_draft_to(
                            library_path.clone(),
                            draft_id.clone(),
                            folder_id.clone(),
                            window,
                            cx,
                        );
                    }
                    crate::folders_ui::FolderEvent::OverlayChanged(open) => {
                        if *open && this.folders.read(cx).dialog_open() {
                            this.note_content
                                .update(cx, |editor, cx| editor.set_visible(false, cx));
                        }
                    }
                    crate::folders_ui::FolderEvent::MenuRegions(bounds) => {
                        this.note_content.update(cx, |editor, cx| {
                            editor.set_overlay_regions("folder-menu", bounds.clone(), cx);
                        });
                    }
                }
                cx.notify();
            },
        )
        .detach();
        let summary = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("记录你学到了什么，支持 Markdown，也可以使用笔记变更摘要…")
        });
        let summary_editor =
            cx.new(|cx| studyapp_ui::compact_markdown::CompactMarkdown::new(summary.clone(), cx));
        Self {
            sync_panel,
            folders,
            history_preview: Default::default(),
            storage_report: None,
            storage_busy: false,
            storage_confirm: None,
            storage_ui: Default::default(),
            storage_active_draft_notes: Default::default(),
            storage_draft_revision: 0,
            page_scroll: ScrollHandle::new(),
            settings_scroll: ScrollHandle::new(),
            learning_versions_scroll: ScrollHandle::new(),
            history_versions_scroll: ScrollHandle::new(),
            statistics_scroll: ScrollHandle::new(),
            active_note_menu: None,
            note_menu_sequence: 0,
            layout: Default::default(),
            resizing: None,
            layout_dirty: false,
            preferences_ui: Default::default(),
            applied_appearance: None,
            appearance_source: None,
            external_drop: Default::default(),
            external_drop_paths: Vec::new(),
            media_ui: crate::media_ui::MediaUi::new(),
            picker_sender,
            picker_results,
            modal_focus: crate::feedback::ModalFocus::new(cx),
            notifications,
            bubble_registry,
            #[cfg(feature = "ui-test")]
            smoke_pointer_bubble: None,
            update_panel,
            pending_update: None,
            pending_nsis_update: None,
            image_cloud_busy: false,
            drafts: DraftStore::new(),
            note_title,
            note_content,
            record_date,
            draft_failed: false,
            note_editor: None,
            recoverable_draft: None,
            draft_catalog: Default::default(),
            draft_library: Default::default(),
            active_draft_id: None,
            draft_durable: Default::default(),
            draft_errors: Default::default(),
            draft_close_drain: None,
            draft_drain_intent: None,
            draft_deferred_work: None,
            draft_work_ready: false,
            pending_note_save: None,
            draft_loading: true,
            note_saving: false,
            note_intent: None,
            pending_library: None,
            draft_persist: None,
            draft_discard: None,
            confirmation: None,
            reader_key: String::new(),
            draft_dirty_since: None,
            close_requested: false,
            close_allowed: false,
            worker,
            snapshot: None,
            startup_failure: None,
            startup_busy: true,
            read_models: Arc::new(Default::default()),
            page: Page::Today,
            notice: crate::update_install::previous_failure().map(Err),
            search,
            summary,
            summary_editor,
            minutes: input("分钟", "30", window, cx),
            start: input(
                "YYYY-MM-DD HH:MM",
                &Local::now().format("%Y-%m-%d %H:%M").to_string(),
                window,
                cx,
            ),
            interval,
            deadline,
            review_days,
            editor: None,
            chosen_notes: HashSet::new(),
            chosen_versions: HashSet::new(),
            saving: false,
            selected_note: None,
            selected_version: None,
            history: false,
            show_detached: false,
            stats_days: 30,
            record_filter: 0,
            record_limit: 50,
            review_filter: 0,
            last_library: String::new(),
            notified: HashSet::new(),
            _poll: poll,
        }
    }

    fn poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if crate::update_install::startup_ready(
            self.startup_busy,
            self.draft_loading,
            self.snapshot.is_some(),
        ) {
            crate::update_install::acknowledge_health();
        }
        self.poll_image_settings(window, cx);
        while !self.storage_busy
            && !self.media_jobs_active()
            && !self.preferences_ui.busy
            && !self.preferences_ui.replacing
            && !self.has_modal(cx)
        {
            let Ok((library, work)) = self.picker_results.try_recv() else {
                break;
            };
            self.accept_picker_result(library, work, cx);
        }
        self.sync_panel
            .update(cx, |panel, cx| panel.poll(window, cx));
        let mut changed = self.poll_drafts(window, cx);
        self.poll_media(window, cx);
        if self
            .draft_dirty_since
            .is_some_and(|at| at.elapsed() >= Duration::from_millis(200))
        {
            self.capture_note_draft(cx);
        }
        while let Ok(update) = self.worker.receiver.try_recv() {
            let startup_failed = update.startup_failure.is_some();
            if let Some(failure) = update.startup_failure {
                self.startup_failure = Some(failure);
                self.startup_busy = false;
                self.draft_loading = false;
            }
            if let Some(busy) = update.image_cloud_busy {
                self.image_cloud_busy = busy;
            }
            if let Some(response) = update.media_result {
                self.accept_media_result(response, window, cx);
            }
            let storage_finished = update.storage_event.is_some();
            let preferences_finished = update.preferences_result.is_some();
            if let Some(crate::storage_backend::StorageEvent::Finished(result)) =
                update.storage_event
            {
                self.storage_busy = false;
                self.folders
                    .update(cx, |panel, cx| panel.set_busy(false, cx));
                match result {
                    Ok(report) => self.storage_report = Some(report),
                    Err(error) => self.push_notice(Err(error), cx),
                }
            }
            if let Some(status) = update.sync_status {
                self.sync_panel.update(cx, |panel, cx| {
                    panel.accept(status, update.library_replaced, window, cx)
                });
            }
            if let Some(result) = update.note_result {
                self.accept_note_save_result(result, window, cx);
            }
            if update.editor_result {
                self.saving = false;
                if update.notice.as_ref().is_some_and(Result::is_ok) {
                    self.editor = None;
                }
            }
            if let Some(notice) = update.notice {
                // preferences_finished emits one contextual outcome below.
                if !preferences_finished && !startup_failed {
                    self.push_notice(notice, cx);
                }
            }
            if let Some((snapshot, read_models)) = update.snapshot.zip(update.read_models) {
                crate::maintenance_handoff::acknowledge_storage_ready();
                self.startup_failure = None;
                self.startup_busy = false;
                if (update.library_replaced || self.last_library != snapshot.data_path)
                    && (self.note_editor.is_some()
                        || self.draft_discard.is_some()
                        || self.draft_persist.is_some())
                {
                    self.pending_library = Some((snapshot, read_models, update.library_replaced));
                    if !self.draft_failed {
                        self.request_note_action(
                            crate::editor_lifecycle::NoteIntent::SwitchLibrary,
                            window,
                            cx,
                        );
                    }
                } else {
                    self.accept_prepared_snapshot(
                        snapshot,
                        read_models,
                        update.library_replaced,
                        window,
                        cx,
                    );
                }
            }
            if let Some(preferences) = update.preferences {
                self.accept_preferences(preferences, update.library_replaced, window, cx);
            }
            if let Some(result) = update.preferences_result {
                self.preferences_finished(result, window, cx);
            }
            if update
                .library_result
                .as_ref()
                .is_some_and(|result| result.is_err() || self.pending_library.is_none())
                || (storage_finished
                    && self.preferences_ui.replacing
                    && self.pending_library.is_none())
            {
                self.preferences_ui.replacing = false;
                self.sync_panel.update(cx, |panel, cx| {
                    panel.set_editing_locked(false, cx);
                });
            }
            changed = true;
        }
        if self.draft_deferred_work.is_some()
            && !self.close_requested
            && !self.note_saving
            && !self.media_busy()
            && self.draft_persist.is_none()
            && self.draft_close_drain.is_none()
            && !self.image_cloud_busy
        {
            if self.note_editor.is_some() {
                self.request_note_action(
                    crate::editor_lifecycle::NoteIntent::DeferredWork,
                    window,
                    cx,
                );
            } else {
                self.begin_draft_drain(crate::editor_lifecycle::NoteIntent::DeferredWork, cx);
            }
        }
        if let Some(snapshot) = self.snapshot.clone() {
            self.reconcile_draft_folders(&snapshot);
        }
        self.continue_close(window, cx);
        if changed {
            cx.notify();
        }
    }

    #[cfg(any(test, feature = "ui-test"))]
    pub fn accept_snapshot(
        &mut self,
        snapshot: Arc<Snapshot>,
        library_replaced: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let read_models = studyapp_desktop::read_models::ReadModelCache::default().refresh(
            &snapshot,
            &Local,
            Utc::now().timestamp(),
        );
        self.accept_prepared_snapshot(snapshot, read_models, library_replaced, window, cx);
    }

    pub fn accept_prepared_snapshot(
        &mut self,
        snapshot: Arc<Snapshot>,
        read_models: Arc<studyapp_desktop::read_models::SnapshotReadModels>,
        library_replaced: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if library_replaced || self.last_library != snapshot.data_path {
            // A successful switch may wait for the old editor's durable flush.
            // Unlock only when this snapshot actually becomes the current library.
            self.preferences_ui.replacing = false;
            self.sync_panel.update(cx, |panel, cx| {
                panel.set_editing_locked(false, cx);
            });
            self.layout = snapshot.settings.layout.clone();
            self.layout_dirty = false;
            self.resizing = None;
            self.show_detached = false;
            self.storage_report = None;
            self.storage_confirm = None;
            self.storage_ui = Default::default();
            self.storage_active_draft_notes.clear();
            self.storage_draft_revision = self.storage_draft_revision.wrapping_add(1);
            self.page_scroll = ScrollHandle::new();
            self.settings_scroll = ScrollHandle::new();
            self.learning_versions_scroll = ScrollHandle::new();
            self.history_versions_scroll = ScrollHandle::new();
            self.statistics_scroll = ScrollHandle::new();
            self.active_note_menu = None;
            self.note_editor = None;
            self.recoverable_draft = None;
            self.active_draft_id = None;
            self.draft_catalog.clear();
            self.refresh_draft_library();
            self.draft_durable.clear();
            self.draft_errors.clear();
            self.draft_close_drain = None;
            self.draft_drain_intent = None;
            self.pending_note_save = None;
            self.note_saving = false;
            self.note_content.update(cx, |editor, cx| {
                editor.set_readonly(false, cx);
                editor.set_overlay_regions("note-menu", vec![], cx);
                editor.set_overlay_regions("folder-menu", vec![], cx);
            });
            self.draft_loading = true;
            self.draft_failed = false;
            self.last_library = snapshot.data_path.clone();
            self.update_panel.update(cx, |panel, cx| {
                panel.set_library(snapshot.data_path.clone(), cx)
            });
            self.pending_update = None;
            self.drafts.load(&self.last_library);
            self.selected_note = None;
            self.selected_version = None;
            self.reader_key.clear();
            self.confirmation = None;
            self.editor = None;
            self.saving = false;
            self.chosen_notes.clear();
            self.chosen_versions.clear();
            self.notified.clear();
            self.interval.update(cx, |s, cx| {
                s.set_value(snapshot.settings.interval_minutes.to_string(), window, cx)
            });
            self.deadline.update(cx, |s, cx| {
                s.set_value(snapshot.settings.answer_minutes.to_string(), window, cx)
            });
            self.review_days.update(cx, |s, cx| {
                s.set_value(
                    snapshot
                        .settings
                        .review_days
                        .iter()
                        .map(u32::to_string)
                        .collect::<Vec<_>>()
                        .join(", "),
                    window,
                    cx,
                )
            });
        }
        if self.page == Page::Settings && self.settings_dirty(cx) {
            // Layout changes made on the settings page are part of its draft.
        } else if !self.is_layout_resizing() && !self.layout_dirty {
            self.layout = snapshot.settings.layout.clone();
        } else if self.layout == snapshot.settings.layout {
            self.layout_dirty = false;
        }
        for session in &snapshot.sessions {
            if session.status == SessionStatus::Pending && self.notified.insert(session.id.clone())
            {
                cx.show_system_notification(SystemNotification {
                    tag: session.id.clone().into(),
                    title: "该回顾刚才的学习了".into(),
                    body: format!(
                        "请在 {} 分钟内记录学到了什么，未回答将记为未学习。",
                        snapshot.settings.answer_minutes
                    )
                    .into(),
                    actions: vec![],
                });
                if self.editor.is_none() {
                    self.editor = Some(EditorMode::Pending(session.id.clone()));
                    self.summary_editor
                        .update(cx, |editor, cx| editor.reset(cx));
                    self.summary.update(cx, |s, cx| {
                        s.set_value(session.suggested_summary.clone(), window, cx)
                    });
                    self.chosen_notes = session.note_ids.iter().cloned().collect();
                    self.chosen_versions = session.version_ids.iter().cloned().collect();
                    self.seed_versions(&snapshot, session.ended_at);
                    self.navigate(Page::Today, window, cx);
                }
            }
        }
        if self.selected_note.as_ref().is_none_or(|id| {
            !snapshot.notes.iter().any(|n| {
                &n.id == id
                    && (!n.detached || self.show_detached || self.selected_version.is_some())
            })
        }) {
            self.selected_note = snapshot
                .notes
                .iter()
                .find(|n| {
                    n.detached == self.show_detached && self.folders.read(cx).filter().matches(n)
                })
                .map(|n| n.id.clone());
            self.selected_version = None;
        }
        if self.note_editor.is_none()
            && self.selected_version.is_none()
            && self.selected_note.as_ref().is_none_or(|id| {
                !snapshot
                    .notes
                    .iter()
                    .any(|n| &n.id == id && n.detached == self.show_detached)
            })
        {
            self.selected_note = snapshot
                .notes
                .iter()
                .find(|n| {
                    n.detached == self.show_detached && self.folders.read(cx).filter().matches(n)
                })
                .map(|n| n.id.clone());
        }
        // Folder selection is an explicit navigation action. A later snapshot
        // must not replace the document just because that note was moved.
        let moved_selection = self
            .selected_note
            .as_ref()
            .filter(|id| {
                self.snapshot
                    .as_ref()
                    .and_then(|old| old.notes.iter().find(|n| &n.id == *id))
                    .zip(snapshot.notes.iter().find(|n| &n.id == *id))
                    .is_some_and(|(old, new)| old.folder_id != new.folder_id)
            })
            .cloned();
        self.folders.update(cx, |panel, cx| {
            panel.accept_with_index(snapshot.clone(), read_models.library.clone(), window, cx)
        });
        if self.selected_note.is_none() && self.note_editor.is_none() {
            self.selected_note = snapshot
                .notes
                .iter()
                .find(|note| {
                    note.detached == self.show_detached
                        && self.folders.read(cx).filter().matches(note)
                })
                .map(|note| note.id.clone());
        }
        if let Some(id) = moved_selection {
            self.folders.update(cx, |panel, cx| {
                panel.align_note_context(&id, cx);
            });
        }
        self.reconcile_draft_folders(&snapshot);
        self.read_models = read_models;
        self.snapshot = Some(snapshot);
    }

    pub(crate) fn reconcile_draft_folders(&mut self, snapshot: &Snapshot) {
        let now = chrono::Utc::now().timestamp_millis();
        let mut changed = false;
        for record in self.draft_catalog.values_mut().filter(|r| r.active()) {
            // A flush/persist/media receipt names an exact revision. Reconcile
            // metadata on the next poll after that transaction has settled.
            if self.note_saving && self.active_draft_id.as_ref() == Some(&record.id) {
                continue;
            }
            let folder = snapshot
                .notes
                .iter()
                .find(|n| Some(&n.id) == record.draft.note_id.as_ref())
                .map(|n| n.folder_id.clone())
                .unwrap_or_else(|| {
                    record
                        .draft
                        .folder_id
                        .clone()
                        .filter(|id| snapshot.folders.iter().any(|f| &f.id == id))
                });
            if record.draft.folder_id != folder {
                changed = true;
                record.draft.folder_id = folder;
                record.touch(now);
                self.drafts.save_record(&self.last_library, record.clone());
            }
        }
        if let Some(record) = self
            .active_draft_id
            .as_ref()
            .and_then(|id| self.draft_catalog.get(id))
            && let Some(draft) = self.note_editor.as_mut()
        {
            draft.folder_id.clone_from(&record.draft.folder_id);
        }
        if changed {
            self.refresh_recoverable();
        }
    }

    pub fn send(&mut self, command: Command, cx: &mut Context<Self>) {
        if self.storage_busy {
            self.saving = false;
            self.note_saving = false;
            if self.note_editor.is_some() {
                self.note_content
                    .update(cx, |editor, cx| editor.set_readonly(false, cx));
            }
            self.push_notice(Err("资料库维护进行中，请完成后重试。".into()), cx);
            cx.notify();
            return;
        }
        if self
            .worker
            .sender
            .send(Work::Scoped {
                library: self.last_library.clone(),
                work: Box::new(Work::Apply(command)),
            })
            .is_err()
        {
            self.push_notice(Err("后台服务已停止，请重新打开应用".into()), cx);
            self.saving = false;
            self.note_saving = false;
            self.note_content
                .update(cx, |editor, cx| editor.set_readonly(false, cx));
        }
        cx.notify();
    }

    pub fn picker(&self, action: &'static str, note: Option<String>) {
        if self.note_saving || self.draft_discard.is_some() || self.storage_busy {
            return;
        }
        let sender = self.picker_sender.clone();
        let library = self.last_library.clone();
        let library_directory = self
            .startup_failure
            .as_ref()
            .and_then(|failure| failure.database.parent().map(std::path::Path::to_path_buf))
            .unwrap_or_else(|| std::path::PathBuf::from(&library));
        std::thread::spawn(move || {
            let work = match action {
                "import" => rfd::FileDialog::new().set_title("导入 Markdown 笔记").add_filter("Markdown", &["md", "markdown"]).pick_files().map(Work::ImportPaths),
                "import-folder" => crate::folder_picker::pick_folder("导入目录中的 Markdown", Some(std::path::PathBuf::from(&library))).map(|p| Work::ImportPaths(vec![p])),
                "relink" => rfd::FileDialog::new().add_filter("Markdown", &["md", "markdown"]).pick_file().zip(note).map(|(path, note_id)| Work::Apply(Command::Relink { note_id, path })),
                "backup" => rfd::FileDialog::new().set_file_name(format!("WW-StudyApp-{}.sqlite3", Local::now().format("%Y%m%d-%H%M%S"))).save_file().map(|p| Work::Apply(Command::Backup(p))),
                "restore" => rfd::FileDialog::new().set_title("选择 WW-StudyApp 备份（恢复前自动保留当前资料）").add_filter("SQLite 备份", &["sqlite3", "sqlite", "db"]).pick_file().and_then(|p| {
                    let confirmed = rfd::MessageDialog::new().set_title("恢复资料库").set_description("将用选中的备份恢复当前资料库。恢复前会自动保存当前资料的备份；源 Markdown 文件不会改变。").set_buttons(rfd::MessageButtons::OkCancel).show();
                    (confirmed == rfd::MessageDialogResult::Ok).then_some(Work::Apply(Command::Restore(p)))
                }),
                "library" => crate::folder_picker::pick_folder("选择本地资料库文件夹", Some(library_directory)).map(Work::SwitchLibrary),
                _ => None,
            };
            if let Some(work) = work {
                let _ = sender.send((library, work));
            }
        });
    }

    pub fn open_editor(&mut self, mode: EditorMode, window: &mut Window, cx: &mut Context<Self>) {
        if self.guard_settings(
            crate::settings_ui::SettingsContinuation::Record(mode.clone()),
            cx,
        ) {
            return;
        }
        let mut text = String::new();
        self.chosen_notes.clear();
        self.chosen_versions.clear();
        let mut ended_at = Utc::now().timestamp();
        if let EditorMode::Pending(id) | EditorMode::Edit(id) = &mode {
            if let Some(session) = self
                .snapshot
                .as_ref()
                .and_then(|s| s.sessions.iter().find(|s| &s.id == id))
            {
                text = if matches!(mode, EditorMode::Pending(_)) {
                    session.suggested_summary.clone()
                } else {
                    session.summary.clone()
                };
                self.chosen_notes = session.note_ids.iter().cloned().collect();
                self.chosen_versions = session.version_ids.iter().cloned().collect();
                ended_at = session.ended_at;
            }
        } else {
            let start = Local::now() - chrono::Duration::minutes(30);
            self.start.update(cx, |s, cx| {
                s.set_value(start.format("%Y-%m-%d %H:%M").to_string(), window, cx)
            });
            self.minutes
                .update(cx, |s, cx| s.set_value("30", window, cx));
            if let Some(note) = &self.selected_note {
                self.chosen_notes.insert(note.clone());
            }
        }
        if let Some(snapshot) = self.snapshot.clone() {
            self.seed_versions(&snapshot, ended_at);
        }
        self.summary
            .update(cx, |s, cx| s.set_value(text, window, cx));
        self.summary_editor
            .update(cx, |editor, cx| editor.reset(cx));
        self.editor = Some(mode);
        self.page = Page::Today;
        self.saving = false;
        cx.notify();
    }

    pub fn save_editor(&mut self, cx: &mut Context<Self>) {
        let summary = self.summary.read(cx).value().to_string();
        let note_ids = self.chosen_notes.iter().cloned().collect();
        let version_ids = Some(self.snapshot.as_ref().map_or_else(Vec::new, |snapshot| {
            snapshot
                .versions
                .iter()
                .filter(|v| self.chosen_versions.contains(&v.id))
                .map(|v| v.id.clone())
                .collect()
        }));
        let result = (|| -> anyhow::Result<Command> {
            Ok(
                match self
                    .editor
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("没有待保存记录"))?
                {
                    EditorMode::Pending(session_id) => Command::Submit {
                        session_id,
                        summary,
                        note_ids,
                        version_ids,
                    },
                    EditorMode::Edit(session_id) => Command::EditSession {
                        session_id,
                        summary,
                        learned: true,
                        note_ids,
                        version_ids,
                    },
                    EditorMode::Manual => Command::Manual {
                        started_at: parse_local_time(&self.start.read(cx).value())?,
                        minutes: self
                            .minutes
                            .read(cx)
                            .value()
                            .trim()
                            .parse()
                            .map_err(|_| anyhow::anyhow!("学习分钟数必须是整数"))?,
                        summary,
                        note_ids,
                        version_ids,
                    },
                },
            )
        })();
        match result {
            Ok(command) => {
                self.saving = true;
                self.send(command, cx);
            }
            Err(error) => {
                self.push_notice(Err(error.to_string()), cx);
                cx.notify();
            }
        }
    }

    fn seed_versions(&mut self, snapshot: &Snapshot, ended_at: i64) {
        self.chosen_versions.retain(|id| {
            snapshot
                .versions
                .iter()
                .any(|v| &v.id == id && self.chosen_notes.contains(&v.note_id))
        });
        for note in &self.chosen_notes {
            if !snapshot
                .versions
                .iter()
                .any(|v| &v.note_id == note && self.chosen_versions.contains(&v.id))
            {
                let version = snapshot
                    .versions
                    .iter()
                    .rev()
                    .find(|v| &v.note_id == note && v.captured_at <= ended_at)
                    .or_else(|| snapshot.versions.iter().rev().find(|v| &v.note_id == note));
                if let Some(version) = version {
                    self.chosen_versions.insert(version.id.clone());
                }
            }
        }
    }

    pub fn toggle_note(&mut self, id: &str, selected: bool) {
        if selected {
            self.chosen_notes.insert(id.to_owned());
        } else {
            self.chosen_notes.remove(id);
        }
        if let Some(snapshot) = self.snapshot.clone() {
            let ended_at = match &self.editor {
                Some(EditorMode::Pending(id) | EditorMode::Edit(id)) => snapshot
                    .sessions
                    .iter()
                    .find(|s| &s.id == id)
                    .map_or(Utc::now().timestamp(), |s| s.ended_at),
                _ => Utc::now().timestamp(),
            };
            self.seed_versions(&snapshot, ended_at);
        }
    }

    pub fn show_version(
        &mut self,
        note: String,
        version: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_note_document(note, Some(version), false, window, cx);
    }

    pub fn has_modal(&self, cx: &App) -> bool {
        self.confirmation.is_some()
            || self.storage_confirm.is_some()
            || self.folders.read(cx).modal_open()
            || self.preferences_ui.leave.is_some()
            || self.preferences_ui.reset_confirmation
            || self.update_panel.read(cx).dialog_visible()
    }

    fn cancel_modal(&mut self, cx: &mut Context<Self>) {
        if self.preferences_ui.busy {
            return;
        }
        if self.update_panel.read(cx).dialog_visible() {
            self.update_panel
                .update(cx, |panel, cx| panel.dismiss_dialog(cx));
            return;
        }
        self.preferences_ui.leave = None;
        self.preferences_ui.reset_confirmation = false;
        self.confirmation = None;
        self.storage_confirm = None;
        self.folders.update(cx, |panel, cx| panel.cancel_action(cx));
        cx.notify();
    }

    fn accept_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.update_panel.read(cx).dialog_visible() {
            self.update_panel
                .update(cx, |panel, cx| panel.install_from_dialog(cx));
            return;
        }
        if self.preferences_ui.leave.is_some() {
            self.save_preferences(cx);
        } else if self.preferences_ui.reset_confirmation {
            // Reset is deliberately activated by its explicit dialog button.
        } else if self.confirmation.is_some() || self.storage_confirm.is_some() {
            // Destructive actions require activation of their explicit button.
            // Enter on the dialog scope itself must never remove user content.
        } else {
            self.folders
                .update(cx, |panel, cx| panel.submit_action(window, cx));
        }
    }

    fn navigation_toggle(&self, cx: &mut Context<Self>) -> Button {
        Button::new("toggle-navigation")
            .disabled(
                self.storage_busy || self.preferences_ui.busy || self.preferences_ui.replacing,
            )
            .small()
            .ghost()
            .icon(IconName::PanelLeft)
            .accessibility_label("展开或收起主导航")
            .tooltip(if self.layout.navigation_visible {
                "收起主导航"
            } else {
                "展开主导航"
            })
            .on_click(cx.listener(|this, _, _, cx| this.toggle_navigation(cx)))
    }

    fn navigation(&self, width: Pixels, cx: &mut Context<Self>) -> impl IntoElement {
        let items = [
            (Page::Today, "今日学习", IconName::House),
            (Page::Notes, "笔记库", IconName::BookOpen),
            (Page::Reviews, "复习计划", IconName::CircleCheck),
            (Page::Stats, "学习统计", IconName::ChartBar),
            (Page::Settings, "设置", IconName::Settings),
        ];
        column()
            .id("navigation-pane")
            .test_support()
            .w(width)
            .h_full()
            .flex_shrink_0()
            .bg(crate::theme::color(BG).opacity(crate::design::NAVIGATION_TINT_ALPHA))
            .border_r_1()
            .border_color(crate::theme::color(BORDER))
            .p_3()
            .child(
                column().gap_1().py_2().child(
                    row().gap_1().px_1().child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .truncate()
                            .font_weight(FontWeight::BOLD)
                            .text_color(crate::theme::color(INK))
                            .child("WW-StudyApp"),
                    ),
                ),
            )
            .children(
                items
                    .into_iter()
                    .enumerate()
                    .map(|(i, (page, label, icon))| {
                        navigation_item(
                            Button::new(("nav", i))
                                .label(label)
                                .icon(icon)
                                .ghost()
                                .selected(self.page == page)
                                .disabled(self.snapshot.is_none())
                                .h(px(36.))
                                .rounded(px(crate::design::CONTROL_RADIUS))
                                .w_full()
                                .justify_start()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.navigate(page, window, cx);
                                })),
                            self.page == page,
                        )
                    }),
            )
            .child(div().flex_1())
            .children(
                self.update_panel
                    .read(cx)
                    .available_version()
                    .map(|version| {
                        studyapp_ui::update_available::AvailableUpdateButton::new(
                            "sidebar-update",
                            version,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.update_panel
                                .update(cx, |panel, cx| panel.show_dialog(cx));
                        }))
                    }),
            )
            .child(
                row()
                    .id("sidebar-version")
                    .test_support()
                    .gap_1()
                    .px_1()
                    .child(muted(format!("v{} · 本地", env!("CARGO_PKG_VERSION")))),
            )
    }
}

impl Render for StudyApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.refresh_appearance(window, cx);
        self.external_drop.set_blocked(self.external_drop_block(cx));
        let modal_open = self.has_modal(cx);
        let initial_focus = self.folders.read(cx).modal_initial_focus(cx);
        self.modal_focus
            .sync_with_initial(modal_open, initial_focus, window, cx);
        self.prepare_document_view(window, cx);
        if let Some(preview) = self.preferences_ui.appearance_preview.clone() {
            let occlusions = self.notifications.read(cx).occlusions().to_vec();
            let popup = self.appearance_color_popup_shapes(cx);
            let bubbles = self.bubble_registry.read(cx).shapes();
            preview.update(cx, |preview, cx| {
                preview.set_occlusions(occlusions, cx);
                preview.set_overlay_shapes("appearance-color-picker", popup, cx);
                preview.set_overlay_shapes("pointer-tooltip", bubbles, cx);
            });
            let visible = self.appearance_preview_visible(window, cx)
                && !modal_open
                && !self.external_drop.active();
            if visible {
                let theme = self.appearance_preview_theme(cx);
                preview.update(cx, |preview, cx| preview.configure(theme, true, window, cx));
            } else {
                preview.update(cx, |preview, cx| preview.hide(cx));
            }
        }
        let layout = self.effective_layout(window);
        let continuous = self.page == Page::Notes;
        let title = match self.page {
            Page::Today => "今日学习",
            Page::Notes => "笔记库",
            Page::Reviews => "复习计划",
            Page::Stats => "学习统计",
            Page::Settings => "设置",
        };
        let body = if let Some(snapshot) = self.snapshot.clone() {
            match self.page {
                Page::Today => self.today(&snapshot, window, cx).into_any_element(),
                Page::Notes => self.notes(&snapshot, window, cx).into_any_element(),
                Page::Reviews => self.reviews(&snapshot, cx).into_any_element(),
                Page::Stats => self.statistics(&snapshot, cx).into_any_element(),
                Page::Settings => self.settings(&snapshot, window, cx).into_any_element(),
            }
        } else {
            self.startup_view(cx).into_any_element()
        };
        let modal_content = if let Some(settings) = self.settings_modal(cx) {
            Some(settings)
        } else if self.confirmation.is_some() {
            Some(self.confirmation_view(cx).into_any_element())
        } else if self.storage_confirm.is_some() {
            self.storage_confirmation_view(cx)
                .map(IntoElement::into_any_element)
        } else if self.update_panel.read(cx).dialog_visible() {
            Some(self.update_dialog_view(cx))
        } else {
            self.folders.update(cx, |panel, cx| panel.modal_content(cx))
        };
        let modal = modal_content.map(|content| {
            let cancel_owner = cx.weak_entity();
            let accept_owner = cx.weak_entity();
            crate::feedback::modal_frame_shell(content, &self.modal_focus, window, cx)
                .on_cancel(move |_, _, cx| {
                    let _ = cancel_owner.update(cx, |this, cx| this.cancel_modal(cx));
                    true
                })
                .on_ok(move |_, window, cx| {
                    let _ = accept_owner.update(cx, |this, cx| this.accept_modal(window, cx));
                    true
                })
        });
        let mut leading = vec![self.navigation_toggle(cx).rounded_full().into_any_element()];
        if self.page == Page::Notes && !self.layout.folders_visible {
            leading.push(
                Button::new("toggle-folders")
                    .small()
                    .ghost()
                    .rounded_full()
                    .icon(IconName::PanelLeft)
                    .accessibility_label("展开笔记库")
                    .tooltip("展开笔记库")
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_folders(cx)))
                    .into_any_element(),
            );
        }
        let mut actions = if self.page == Page::Notes || self.snapshot.is_none() {
            vec![]
        } else {
            vec![self.library_add_button(cx)]
        };
        if self.snapshot.is_some() {
            let syncing = self.sync_panel.read(cx).status.busy;
            let sync_failed = self.sync_panel.read(cx).status.failed;
            let label = if self.note_saving || self.preferences_ui.busy {
                "正在保存"
            } else if syncing {
                "正在同步"
            } else if sync_failed {
                "同步失败"
            } else if self.page == Page::Settings && self.settings_dirty(cx) {
                "设置未保存"
            } else if self.note_editor.is_some() {
                "草稿编辑中"
            } else {
                "本地已保存"
            };
            actions.insert(
                0,
                Button::new("titlebar-save-status")
                    .small()
                    .ghost()
                    .label(label)
                    .tooltip("查看保存与同步设置")
                    .text_color(rgb(if sync_failed {
                        crate::theme::palette().danger
                    } else {
                        crate::theme::palette().muted
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.navigate(Page::Settings, window, cx);
                        this.select_settings_section(
                            if syncing || sync_failed {
                                crate::settings_ui::SettingsSection::Sync
                            } else {
                                crate::settings_ui::SettingsSection::Storage
                            },
                            cx,
                        );
                    }))
                    .into_any_element(),
            );
        }
        let window_title = format!("WW-StudyApp  ·  {title}");
        let close_owner = cx.weak_entity();
        column()
            .relative()
            .on_drag_move::<ExternalPaths>(cx.listener(
                |this, event: &DragMoveEvent<ExternalPaths>, _, cx| {
                    let paths = event.drag(cx).paths();
                    if this.external_drop_paths != paths {
                        this.external_drop_paths = paths.to_vec();
                    }
                    let count = paths.len();
                    this.hover_external_file_count(count, cx);
                },
            ))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.drop_external_files(paths.paths().to_vec(), window, cx);
            }))
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.external_drop.active() && event.keystroke.key == "escape" {
                    this.cancel_external_drag(cx);
                    cx.stop_active_drag(window);
                    cx.stop_propagation();
                } else if this.is_layout_resizing() && event.keystroke.key == "escape" {
                    this.cancel_resize(cx);
                    cx.stop_propagation();
                }
            }))
            .gap_0()
            .size_full()
            .bg(rgba(0))
            .text_color(crate::theme::color(INK))
            .font_family(crate::design::UI_FONT)
            .text_size(px(crate::theme::palette().font_size as f32))
            .child(crate::window_chrome::title_bar(
                window_title,
                leading,
                actions,
                window,
                move |window, cx| {
                    let _ = close_owner.update(cx, |this, cx| {
                        if this.handle_close(window, cx) {
                            window.remove_window();
                        }
                    });
                },
            ))
            .child(
                row()
                    .gap_0()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .when(self.layout.navigation_visible, |view| {
                        view.child(self.navigation(layout.navigation, cx))
                    })
                    .when(
                        self.layout.navigation_visible
                            || self.resizing.as_ref().is_some_and(|resize| {
                                resize.pane == crate::workspace_layout::Pane::Navigation
                            }),
                        |view| {
                            view.child(
                                self.resize_handle(crate::workspace_layout::Pane::Navigation, cx),
                            )
                        },
                    )
                    .child(
                        column()
                            .gap_0()
                            .bg(crate::theme::color(BG))
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .h_full()
                            .child(
                                column()
                                    .id("page-scroll")
                                    .relative()
                                    .track_scroll(&self.page_scroll)
                                    .when(!continuous && self.page != Page::Settings, |c| {
                                        use gpui_kit::component::scroll::ScrollableElement;
                                        c.vertical_scrollbar(&self.page_scroll)
                                    })
                                    .test_support()
                                    .flex_1()
                                    .min_h_0()
                                    .when(!continuous && self.page != Page::Settings, |c| {
                                        c.overflow_y_scroll()
                                    })
                                    .when(continuous || self.page == Page::Settings, |c| {
                                        c.overflow_hidden()
                                    })
                                    .when(continuous || self.page == Page::Settings, |c| c.p_4())
                                    .when(!continuous && self.page != Page::Settings, |c| c.p_8())
                                    .when(!continuous && self.page != Page::Settings, |view| {
                                        view.items_center()
                                    })
                                    .child(if !continuous && self.page != Page::Settings {
                                        column()
                                            .w_full()
                                            .max_w(px(1120.))
                                            .child(body)
                                            .into_any_element()
                                    } else {
                                        body
                                    }),
                            ),
                    ),
            )
            .when(self.external_drop.active(), |view| {
                view.child(crate::notes_view::external_drop_overlay(
                    &self.external_drop,
                    cx,
                ))
            })
            .child(self.external_drop_event_layer(cx))
            .child(self.notifications.clone())
            .child(self.resize_event_layer(cx))
            .children(modal)
            .map(|root| {
                #[cfg(feature = "ui-test")]
                {
                    root.children(self.smoke_pointer_bubble.clone())
                }
                #[cfg(not(feature = "ui-test"))]
                {
                    root
                }
            })
    }
}
