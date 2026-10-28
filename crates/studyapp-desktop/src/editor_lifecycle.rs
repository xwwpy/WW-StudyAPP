use crate::controls::Button;
use crate::{
    app::{Page, StudyApp},
    components::*,
    drafts::{DraftRecord, DraftStatus},
    markdown_editor::MarkdownEvent,
};
use gpui_kit::{
    component::{Disableable, Sizable, button::ButtonVariants},
    prelude::FluentBuilder,
    *,
};
use std::sync::atomic::{AtomicU64, Ordering};
use studyapp_core::Command;
use studyapp_ui::modal_frame::{ModalFrame, ModalTone, ModalWidth};

#[derive(Clone)]
pub enum NoteIntent {
    Save {
        copy: bool,
    },
    Keep,
    New(Option<String>),
    OpenDraft(String),
    ViewNote(String, Option<String>, bool),
    Filter(crate::folders_ui::FolderFilter),
    Navigate(Page),
    Commit,
    Saved(String),
    MoveDraft {
        library: String,
        id: String,
        folder: Option<String>,
    },
    MovedDraft,
    Discard,
    SwitchLibrary,
    PrepareMigration,
    DeferredWork,
    Close,
}

#[derive(Clone)]
pub enum NoteConfirmation {
    DiscardDraft(String),
    Unmanage(String),
}

pub struct DiscardPending {
    pub token: u64,
    pub library: String,
    pub id: String,
    pub revision: u64,
    pub original: DraftRecord,
}
pub struct PersistPending {
    pub token: u64,
    pub library: String,
    pub id: String,
    pub revision: u64,
    pub intent: NoteIntent,
}

impl StudyApp {
    pub fn mark_note_dirty(&mut self) {
        if self.note_editor.is_some()
            && self
                .draft_discard
                .as_ref()
                .is_none_or(|pending| self.active_draft_id.as_ref() != Some(&pending.id))
        {
            self.draft_dirty_since
                .get_or_insert_with(std::time::Instant::now);
        }
    }

    pub fn request_note_action(
        &mut self,
        intent: NoteIntent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.note_editor.is_none()
            || self.storage_busy
            || (self.note_saving && !matches!(intent, NoteIntent::Commit))
            || self.media_busy()
            || self.draft_discard.is_some()
            || (self.confirmation.is_some()
                && !matches!(
                    intent,
                    NoteIntent::Discard | NoteIntent::Close | NoteIntent::SwitchLibrary
                ))
        {
            return;
        }
        self.note_saving = true;
        match self
            .note_content
            .update(cx, |editor, cx| editor.request_flush(cx))
        {
            Ok(request) => self.note_intent = Some((request, intent)),
            Err(error) => {
                self.note_saving = false;
                self.close_requested = false;
                self.pending_update = None;
                self.push_notice(Err(error.to_string()), cx);
            }
        }
        cx.notify();
    }

    pub fn editor_event(
        &mut self,
        event: MarkdownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            MarkdownEvent::RequestInsertImage => self.request_insert_image(window, cx),
            MarkdownEvent::RequestNativeImagePaste { generation } => {
                self.paste_native_image(generation, window, cx);
            }
            MarkdownEvent::PasteImage {
                generation,
                name,
                mime,
                base64,
            } => {
                self.paste_editor_image(generation, name, mime, base64, window, cx);
            }
            MarkdownEvent::ExternalDrag { phase, paths } => {
                self.handle_external_drag(phase, paths, window, cx);
            }
            MarkdownEvent::Save => self.save_note(window, cx),
            MarkdownEvent::RequestEdit(position) => {
                let allowed = self.page == Page::Notes
                    && !self.history
                    && self.selected_version.is_none()
                    && self.note_editor.is_none()
                    && !self.has_modal(cx)
                    && self.active_note_menu.is_none()
                    && !self.storage_busy
                    && !self.external_drop.active()
                    && self.snapshot.as_ref().is_some_and(|snapshot| {
                        snapshot.notes.iter().any(|note| {
                            Some(&note.id) == self.selected_note.as_ref()
                                && note.managed
                                && !note.detached
                                && !note.missing
                        })
                    });
                if allowed {
                    self.open_note_editor(false, window, cx);
                    if self.note_editor.is_some() {
                        self.note_content
                            .update(cx, |editor, cx| editor.restore_selection(position, cx));
                    }
                }
            }
            MarkdownEvent::CancelResize => self.cancel_resize(cx),
            MarkdownEvent::Shortcut(action) => {
                self.dispatch_shortcut(&action, window, cx);
            }
            MarkdownEvent::Error(error) => {
                self.note_intent = None;
                self.note_saving = self.pending_note_save.is_some();
                self.close_requested = false;
                self.pending_update = None;
                self.cancel_draft_retention();
                self.push_notice(Err(error), cx);
                cx.notify();
            }
            MarkdownEvent::Flushed(request) => {
                if self
                    .note_intent
                    .as_ref()
                    .is_none_or(|(id, _)| *id != request)
                {
                    return;
                }
                let (_, intent) = self.note_intent.take().expect("matching editor intent");
                self.capture_note_draft(cx);
                match intent {
                    NoteIntent::Save { copy } => {
                        self.begin_save_images(copy, window, cx);
                    }
                    NoteIntent::Commit => self.commit_note(cx),
                    NoteIntent::Discard => {
                        self.note_saving = false;
                        self.begin_discard(window, cx);
                    }
                    NoteIntent::MoveDraft {
                        library,
                        id,
                        folder,
                    } => {
                        self.commit_draft_move(library, id, folder, cx);
                    }
                    intent => self.begin_draft_retention(intent, window, cx),
                }
                cx.notify();
            }
        }
    }

    pub(crate) fn persist_record_intent(
        &mut self,
        record: DraftRecord,
        intent: NoteIntent,
        cx: &mut Context<Self>,
    ) {
        static NEXT_PERSIST: AtomicU64 = AtomicU64::new(1);
        self.refresh_draft_library();
        let token = NEXT_PERSIST.fetch_add(1, Ordering::Relaxed);
        self.draft_persist = Some(PersistPending {
            token,
            library: self.last_library.clone(),
            id: record.id.clone(),
            revision: record.revision,
            intent,
        });
        self.note_saving = true;
        self.drafts
            .persist_record(&self.last_library, record, token);
        cx.notify();
    }

    pub fn finish_draft_persist(
        &mut self,
        intent: NoteIntent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.note_saving = false;
        self.draft_failed = false;
        self.confirmation = None;
        if matches!(intent, NoteIntent::MovedDraft) {
            if self.note_editor.is_some() {
                self.note_content
                    .update(cx, |editor, cx| editor.set_readonly(false, cx));
            }
            self.refresh_recoverable();
            self.push_notice(Ok("草稿所属目录已更新".into()), cx);
            cx.notify();
            return;
        }
        if matches!(intent, NoteIntent::Close) && !self.close_requested {
            self.note_content
                .update(cx, |editor, cx| editor.set_readonly(false, cx));
            return;
        }
        if matches!(intent, NoteIntent::Close)
            && self.guard_settings(crate::settings_ui::SettingsContinuation::Close, cx)
        {
            self.close_requested = false;
            self.pending_update = None;
            self.note_content
                .update(cx, |editor, cx| editor.set_readonly(false, cx));
            return;
        }
        if matches!(intent, NoteIntent::Commit) {
            self.finish_commit_note(cx);
            return;
        }
        if let NoteIntent::Saved(note_id) = intent {
            self.note_editor = None;
            self.active_draft_id = None;
            self.selected_note = Some(note_id.clone());
            self.selected_version = None;
            self.show_detached = false;
            self.reader_key = self
                .snapshot
                .as_ref()
                .and_then(|snapshot| {
                    snapshot
                        .versions
                        .iter()
                        .rev()
                        .find(|version| {
                            version.note_id == note_id
                                && version.content == self.note_content.read(cx).value()
                        })
                        .map(|version| version.id.clone())
                })
                .unwrap_or_default();
            self.refresh_recoverable();
            if self.page != Page::Settings {
                self.page = Page::Notes;
            }
            self.finish_library_transition(window, cx);
            return;
        }
        self.note_editor = None;
        self.active_draft_id = None;
        self.reader_key.clear();
        self.refresh_recoverable();
        match intent {
            NoteIntent::Keep => self.finish_library_transition(window, cx),
            NoteIntent::New(folder) => self.start_new_draft(folder, window, cx),
            NoteIntent::OpenDraft(id) => self.open_draft(id, window, cx),
            NoteIntent::ViewNote(id, version, history) => {
                self.select_note_document(id, version, history, window, cx)
            }
            NoteIntent::Filter(filter) => self.select_folder_document(filter, window, cx),
            NoteIntent::Navigate(page) => self.navigate(page, window, cx),
            NoteIntent::SwitchLibrary
            | NoteIntent::PrepareMigration
            | NoteIntent::DeferredWork
            | NoteIntent::Close => self.begin_draft_drain(intent, cx),
            _ => {}
        }
    }
    pub(crate) fn begin_draft_drain(&mut self, intent: NoteIntent, cx: &mut Context<Self>) {
        if self.draft_close_drain.is_some() {
            return;
        }
        static NEXT_DRAIN: AtomicU64 = AtomicU64::new(1);
        let token = NEXT_DRAIN.fetch_add(1, Ordering::Relaxed);
        self.draft_close_drain = Some(token);
        self.draft_drain_intent = Some(intent);
        self.drafts.drain(&self.last_library, token);
        cx.notify();
    }
    pub(crate) fn finish_draft_drain(
        &mut self,
        intent: NoteIntent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match intent {
            NoteIntent::Close => {
                if !self.close_requested {
                    return;
                }
                if self.guard_settings(crate::settings_ui::SettingsContinuation::Close, cx) {
                    self.close_requested = false;
                    self.pending_update = None;
                } else {
                    self.complete_close(window, cx);
                }
            }
            NoteIntent::SwitchLibrary => self.finish_library_transition(window, cx),
            NoteIntent::PrepareMigration => self.pick_migration(window, cx),
            NoteIntent::DeferredWork => {
                if let Some((library, work)) = self.draft_deferred_work.take() {
                    self.draft_work_ready = true;
                    self.accept_picker_result(library, work, cx);
                    self.draft_work_ready = false;
                }
            }
            _ => {}
        }
    }
    pub(crate) fn select_note_document(
        &mut self,
        id: String,
        version: Option<String>,
        history: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.note_saving || self.storage_busy || self.media_busy() {
            return;
        }
        if self.note_editor.is_some() {
            if self
                .note_editor
                .as_ref()
                .is_some_and(|d| d.note_id.as_ref() == Some(&id))
                && !history
                && version.is_none()
            {
                return;
            }
            self.request_note_action(NoteIntent::ViewNote(id, version, history), window, cx);
            return;
        }
        self.folders.update(cx, |panel, cx| {
            panel.align_note_context(&id, cx);
        });
        self.selected_note = Some(id);
        self.selected_version = version;
        self.history = history;
        self.page = Page::Notes;
        cx.notify();
    }
    pub(crate) fn select_folder_document(
        &mut self,
        filter: crate::folders_ui::FolderFilter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.note_editor.is_some() {
            self.request_note_action(NoteIntent::Filter(filter), window, cx);
            return;
        }
        self.selected_note = if matches!(filter, crate::folders_ui::FolderFilter::Drafts) {
            None
        } else {
            self.snapshot.as_ref().and_then(|s| {
                s.notes
                    .iter()
                    .find(|n| n.detached == self.show_detached && filter.matches(n))
                    .map(|n| n.id.clone())
            })
        };
        self.selected_version = None;
        self.history = false;
        cx.notify();
    }

    pub fn finish_library_transition(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.note_editor.is_none()
            && self.draft_discard.is_none()
            && self.pending_update.is_none()
            && self.draft_persist.is_none()
            && let Some((snapshot, read_models, replaced)) = self.pending_library.take()
        {
            self.accept_prepared_snapshot(snapshot, read_models, replaced, window, cx);
        }
    }

    pub fn handle_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.close_allowed {
            return true;
        }
        if self.guard_settings(crate::settings_ui::SettingsContinuation::Close, cx) {
            return false;
        }
        self.close_requested = true;
        self.continue_close(window, cx);
        false
    }

    pub fn continue_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.close_requested
            || self.close_allowed
            || self.note_saving
            || self.draft_persist.is_some()
            || self.draft_close_drain.is_some()
            || self.media_busy()
            || self.draft_discard.is_some()
            || self.note_content.read(cx).creating()
            || self.storage_busy
            || self.image_cloud_busy
            || self.preferences_ui.busy
            || self.preferences_ui.leave.is_some()
        {
            return;
        }
        if self.note_editor.is_some() {
            self.request_note_action(NoteIntent::Close, window, cx);
        } else {
            if self.guard_settings(crate::settings_ui::SettingsContinuation::Close, cx) {
                self.close_requested = false;
                return;
            }
            self.begin_draft_drain(NoteIntent::Close, cx);
        }
    }

    pub fn begin_discard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self
            .active_draft_id
            .clone()
            .or_else(|| self.recoverable_draft_id());
        if let Some(id) = id {
            self.begin_discard_id(id, window, cx);
        }
    }

    pub(crate) fn begin_discard_id(
        &mut self,
        id: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.storage_busy
            || self.note_saving
            || self.media_busy()
            || self.draft_loading
            || self.draft_discard.is_some()
        {
            return;
        }
        let Some(original) = self
            .draft_catalog
            .get(&id)
            .filter(|record| record.active())
            .cloned()
        else {
            return;
        };
        let mut deleted = original.clone();
        deleted.retire(DraftStatus::Deleted, chrono::Utc::now().timestamp_millis());
        static NEXT_DISCARD: AtomicU64 = AtomicU64::new(1);
        let token = NEXT_DISCARD.fetch_add(1, Ordering::Relaxed);
        self.draft_discard = Some(DiscardPending {
            token,
            library: self.last_library.clone(),
            id: id.clone(),
            revision: deleted.revision,
            original,
        });
        self.draft_catalog.insert(id, deleted.clone());
        self.refresh_storage_draft_index();
        if self.active_draft_id.as_ref() == Some(&deleted.id) {
            self.draft_dirty_since = None;
            self.note_content
                .update(cx, |editor, cx| editor.set_readonly(true, cx));
        }
        self.drafts
            .persist_record(&self.last_library, deleted, token);
        cx.notify();
    }

    pub fn confirm_note_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.storage_busy || self.note_saving {
            return;
        }
        match self.confirmation.take() {
            Some(NoteConfirmation::DiscardDraft(id))
                if self.note_editor.is_some() && self.active_draft_id.as_ref() == Some(&id) =>
            {
                self.request_note_action(NoteIntent::Discard, window, cx)
            }
            Some(NoteConfirmation::DiscardDraft(id)) => self.begin_discard_id(id, window, cx),
            Some(NoteConfirmation::Unmanage(note_id)) => {
                self.send(Command::UnmanageNote { note_id }, cx);
            }
            None => {}
        }
        cx.notify();
    }

    /// Dialog content only; the window-level shell owns blocking and focus.
    pub fn confirmation_view(&self, cx: &mut Context<Self>) -> Div {
        let message = match self.confirmation.as_ref() {
            Some(NoteConfirmation::DiscardDraft(id)) => {
                let title = self
                    .draft_catalog
                    .get(id)
                    .map(|record| record.draft.title.as_str())
                    .unwrap_or("未命名草稿");
                format!("删除草稿「{title}」？原笔记文件和已保存的历史版本会保留。")
            }
            Some(NoteConfirmation::Unmanage(_)) => {
                "取消管理这份笔记？将停止扫描和同步，磁盘文件、历史版本及学习复习记录会保留。"
                    .into()
            }
            None => return div(),
        };
        let deleting = matches!(self.confirmation, Some(NoteConfirmation::DiscardDraft(_)));
        let title = if deleting {
            "删除草稿"
        } else {
            "取消管理笔记"
        };
        div().child(
            ModalFrame::new(title, div().child(message))
                .tone(if deleting {
                    ModalTone::Danger
                } else {
                    ModalTone::Warning
                })
                .width(ModalWidth::Compact)
                .scroll_handle(self.modal_focus.scroll())
                .footer(
                    row()
                        .child(
                            Button::new("cancel-note-action")
                                .small()
                                .ghost()
                                .label("取消")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.confirmation = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("confirm-note-action")
                                .disabled(self.storage_busy || self.note_saving)
                                .small()
                                .when(deleting, |button| button.danger())
                                .when(!deleting, |button| button.primary())
                                .label(if deleting {
                                    "删除草稿"
                                } else {
                                    "取消管理"
                                })
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.confirm_note_action(window, cx)
                                })),
                        ),
                ),
        )
    }

    /// Load a read-only document into the same persistent browser surface.
    /// Reading and writing therefore share highlighting and viewport limits.
    pub fn prepare_document_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Other pages only need to hide the persistent native editor. Do not
        // search note history or prepare a diff on their input/scroll frames.
        if self.page != Page::Notes {
            self.note_content.update(cx, |editor, cx| {
                editor.set_can_request_edit(false, cx);
                editor.set_visible(false, cx);
            });
            return;
        }
        let mut show = self.confirmation.is_none()
            && self.storage_confirm.is_none()
            && !self.folders.read(cx).dialog_open()
            && self.preferences_ui.leave.is_none()
            && !self.preferences_ui.reset_confirmation
            && self.draft_discard.is_none();
        show &= !self.external_drop.active();
        if self.note_editor.is_none() {
            let version = self.selected_note.as_ref().and_then(|note_id| {
                let snapshot = self.snapshot.as_ref()?;
                self.read_models
                    .version_positions(note_id)
                    .iter()
                    .rev()
                    .map(|&position| &snapshot.versions[position])
                    .find(|version| {
                        self.selected_version
                            .as_ref()
                            .is_none_or(|id| id == &version.id)
                    })
            });
            if self.history {
                self.history_preview.prepare(version);
            }
            show &= version.is_some() && !self.history;
            if show
                && let Some(version) = version
                && self.reader_key != version.id
            {
                let text = version.content.clone();
                self.reader_key = version.id.clone();
                self.note_content.update(cx, |editor, cx| {
                    editor.set_value(text, window, cx);
                    editor.set_readonly(true, cx);
                });
            }
        }
        let can_edit = show
            && self.note_editor.is_none()
            && !self.history
            && self.selected_version.is_none()
            && !self.storage_busy
            && !self.draft_loading
            && !self.draft_failed
            && self.snapshot.as_ref().is_some_and(|snapshot| {
                self.selected_note
                    .as_ref()
                    .and_then(|id| self.read_models.note_position(id))
                    .is_some_and(|position| {
                        let note = &snapshot.notes[position];
                        note.managed && !note.detached && !note.missing
                    })
            });
        self.note_content.update(cx, |editor, cx| {
            editor.set_can_request_edit(can_edit, cx);
            editor.set_visible(show, cx);
        });
    }
}
