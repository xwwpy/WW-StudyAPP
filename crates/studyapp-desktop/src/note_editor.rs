use crate::controls::Button;
use crate::{
    app::{Page, StudyApp},
    components::*,
    drafts::{DraftRecord, DraftStatus, DraftUpdate, NoteDraft},
};
use gpui_kit::{
    component::{
        Disableable, Sizable,
        button::ButtonVariants,
        input::Input,
        menu::{DropdownMenu, PopupMenuItem},
    },
    *,
};
use studyapp_core::Command;

impl StudyApp {
    pub(crate) fn refresh_draft_library(&mut self) {
        self.draft_library
            .refresh(self.draft_catalog.values().map(|record| {
                studyapp_desktop::draft_viewmodel::DraftRow {
                    id: &record.id,
                    revision: record.revision,
                    updated_at: record.updated_at,
                    active: record.active(),
                    title: &record.draft.title,
                    content: &record.draft.content,
                    note_id: record.draft.note_id.as_deref(),
                    folder_id: record.draft.folder_id.as_deref(),
                }
            }));
    }

    pub(crate) fn refresh_storage_draft_index(&mut self) {
        self.refresh_draft_library();
        let next = crate::drafts::protected_note_ids(self.draft_catalog.values());
        if next != self.storage_active_draft_notes {
            self.storage_active_draft_notes = next;
            self.storage_draft_revision = self.storage_draft_revision.wrapping_add(1);
        }
    }

    pub(crate) fn recoverable_draft_id(&self) -> Option<String> {
        self.draft_catalog
            .values()
            .filter(|r| r.active() && Some(&r.id) != self.active_draft_id.as_ref())
            .max_by_key(|r| r.updated_at)
            .map(|r| r.id.clone())
    }

    pub fn refresh_recoverable(&mut self) {
        self.refresh_storage_draft_index();
        self.recoverable_draft = self
            .draft_catalog
            .values()
            .filter(|r| r.active() && Some(&r.id) != self.active_draft_id.as_ref())
            .max_by_key(|r| r.updated_at)
            .map(|r| r.draft.clone());
    }

    pub fn capture_note_draft(&mut self, cx: &mut Context<Self>) {
        self.draft_dirty_since = None;
        if let Some(draft) = self.note_editor.as_mut() {
            draft.title = self.note_title.read(cx).value().to_string();
            draft.content = self.note_content.read(cx).value().to_string();
            if let Some(record) = self
                .active_draft_id
                .as_ref()
                .and_then(|id| self.draft_catalog.get_mut(id))
            {
                record.update(draft.clone(), chrono::Utc::now().timestamp_millis());
                self.drafts.save_record(&self.last_library, record.clone());
            }
        }
        self.refresh_draft_library();
    }

    pub fn poll_drafts(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let mut changed = false;
        while let Ok(update) = self.drafts.receiver.try_recv() {
            match update {
                DraftUpdate::Catalog {
                    library,
                    records,
                    errors,
                } if library == self.last_library => {
                    self.update_health.catalog_result(&errors);
                    self.draft_loading = false;
                    for mut record in records {
                        // A persisted save attempt is also a restart receipt: a source write
                        // can succeed just before the terminal-draft acknowledgement is lost.
                        if record.active()
                            && let Some(target) = record.save_target.as_ref()
                            && self.snapshot.as_ref().is_some_and(|s| {
                                s.versions
                                    .iter()
                                    .rev()
                                    .find(|v| &v.note_id == target)
                                    .is_some_and(|v| v.content == record.draft.content)
                            })
                        {
                            record.retire(
                                DraftStatus::Saved {
                                    note_id: target.clone(),
                                    hash: Self::draft_hash(&record.draft.content),
                                },
                                chrono::Utc::now().timestamp_millis(),
                            );
                            self.drafts.save_record(&library, record.clone());
                        }
                        let id = record.id.clone();
                        if self
                            .draft_catalog
                            .get(&id)
                            .is_none_or(|old| old.revision <= record.revision)
                        {
                            self.draft_durable.insert(id.clone(), record.revision);
                            self.draft_catalog.insert(id, record);
                        }
                    }
                    for error in errors {
                        self.push_notice(Err(error), cx);
                    }
                    self.refresh_recoverable();
                }
                DraftUpdate::Written {
                    library,
                    id,
                    revision,
                    token,
                } if library == self.last_library => {
                    self.draft_durable
                        .entry(id.clone())
                        .and_modify(|r| *r = (*r).max(revision))
                        .or_insert(revision);
                    if self
                        .draft_catalog
                        .get(&id)
                        .is_some_and(|r| r.revision <= revision)
                    {
                        self.draft_errors.remove(&id);
                    }
                    if self.draft_persist.as_ref().is_some_and(|p| {
                        p.library == library
                            && p.id == id
                            && p.revision == revision
                            && Some(p.token) == token
                    }) {
                        let pending = self.draft_persist.take().unwrap();
                        self.finish_draft_persist(pending.intent, window, cx);
                    }
                    if self.draft_discard.as_ref().is_some_and(|p| {
                        p.library == library
                            && p.id == id
                            && p.revision == revision
                            && Some(p.token) == token
                    }) {
                        self.draft_discard = None;
                        self.draft_failed = false;
                        if self.active_draft_id.as_ref() == Some(&id) {
                            self.note_editor = None;
                            self.active_draft_id = None;
                            self.note_content.update(cx, |editor, cx| {
                                editor.set_visible(false, cx);
                                editor.set_value(String::new(), window, cx);
                            });
                            self.reader_key.clear();
                        } else if self.note_editor.is_some() {
                            self.note_content
                                .update(cx, |editor, cx| editor.set_readonly(false, cx));
                        }
                        self.refresh_recoverable();
                        self.push_notice(
                            Ok("这一份草稿已删除，原笔记、历史及其他草稿均保留。".into()),
                            cx,
                        );
                        self.finish_library_transition(window, cx);
                    }
                }
                DraftUpdate::Error {
                    library,
                    id,
                    revision,
                    token,
                    message,
                } if library == self.last_library => {
                    if let Some(id) = id.as_ref() {
                        self.draft_errors.insert(id.clone(), message.clone());
                    }
                    self.clear_pending_updates();
                    if self.draft_persist.as_ref().is_some_and(|p| {
                        p.library == library
                            && Some(&p.id) == id.as_ref()
                            && p.revision == revision
                            && Some(p.token) == token
                    }) {
                        let pending = self.draft_persist.take().unwrap();
                        if let crate::editor_lifecycle::NoteIntent::Saved(note_id) = pending.intent
                            && let Some(record) = self.draft_catalog.get_mut(&pending.id)
                        {
                            // The formal note is already durable. Keep a usable editing
                            // session if only retiring its recovery record failed.
                            record.status = DraftStatus::Active;
                            record.draft.note_id = Some(note_id);
                            record.draft.expected_hash =
                                Some(Self::draft_hash(&record.draft.content));
                            record.touch(chrono::Utc::now().timestamp_millis());
                            self.note_editor = Some(record.draft.clone());
                            self.drafts.save_record(&library, record.clone());
                        }
                        self.note_saving = false;
                        self.draft_failed = true;
                        self.close_requested = false;
                        self.clear_pending_updates();
                        if self.note_editor.is_some() {
                            self.note_content
                                .update(cx, |editor, cx| editor.set_readonly(false, cx));
                        }
                    }
                    if self.draft_discard.as_ref().is_some_and(|p| {
                        p.library == library
                            && Some(&p.id) == id.as_ref()
                            && p.revision == revision
                            && Some(p.token) == token
                    }) {
                        let pending = self.draft_discard.take().unwrap();
                        let mut original = pending.original;
                        original.revision = revision;
                        original.touch(chrono::Utc::now().timestamp_millis());
                        self.drafts.save_record(&library, original.clone());
                        self.draft_catalog.insert(original.id.clone(), original);
                        self.close_requested = false;
                        self.clear_pending_updates();
                        if self.note_editor.is_some() {
                            self.note_content
                                .update(cx, |editor, cx| editor.set_readonly(false, cx));
                        }
                    }
                    self.push_notice(Err(message), cx);
                }
                DraftUpdate::Drained {
                    library,
                    token,
                    result,
                } if library == self.last_library && self.draft_close_drain == Some(token) => {
                    self.draft_close_drain = None;
                    let intent = self.draft_drain_intent.take();
                    match result {
                        Ok(()) => {
                            if let Some(intent) = intent {
                                self.finish_draft_drain(intent, window, cx);
                            }
                        }
                        Err(error) => {
                            self.close_requested = false;
                            self.clear_pending_updates();
                            self.push_notice(Err(error), cx);
                        }
                    }
                }
                _ => {}
            }
            changed = true;
        }
        if changed {
            self.refresh_storage_draft_index();
            cx.notify();
        }
        changed
    }

    pub(crate) fn draft_hash(content: &str) -> String {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(content.as_bytes()))
    }

    pub(crate) fn start_note_draft(
        &mut self,
        mut draft: NoteDraft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if draft.asset_group.is_empty() {
            draft.asset_group = crate::media::asset_group(draft.note_id.as_deref(), &draft.content);
        }
        if draft.folder_id.as_ref().is_some_and(|id| {
            self.snapshot
                .as_ref()
                .is_none_or(|s| !s.folders.iter().any(|f| &f.id == id))
        }) {
            draft.folder_id = None;
        }
        if self.active_draft_id.is_none() {
            let mut record = DraftRecord::new(draft.clone(), chrono::Utc::now().timestamp_millis());
            record.source_path = self
                .snapshot
                .as_ref()
                .and_then(|s| {
                    s.notes.iter().find(|n| {
                        Some(&n.id) == draft.note_id.as_ref().or(self.selected_note.as_ref())
                    })
                })
                .map(|n| std::path::PathBuf::from(&n.path));
            self.active_draft_id = Some(record.id.clone());
            self.draft_catalog.insert(record.id.clone(), record);
        }
        self.confirmation = None;
        self.note_title
            .update(cx, |s, cx| s.set_value(draft.title.clone(), window, cx));
        let saving = self.note_saving;
        let same_reader = !self.reader_key.is_empty()
            && self.snapshot.as_ref().is_some_and(|snapshot| {
                snapshot.versions.iter().any(|version| {
                    version.id == self.reader_key
                        && Some(&version.note_id) == draft.note_id.as_ref()
                })
            })
            && self.note_content.read(cx).value() == draft.content;
        if saving {
            if let Err(error) = self.note_content.update(cx, |editor, cx| {
                editor.rebase_saved_value(draft.content.clone(), cx)
            }) {
                self.note_saving = false;
                self.close_requested = false;
                self.clear_pending_updates();
                self.push_notice(Err(error.to_string()), cx);
                return false;
            }
        } else if same_reader {
            self.note_content
                .update(cx, |editor, cx| editor.set_readonly(false, cx));
        } else {
            self.note_content
                .update(cx, |s, cx| s.set_value(draft.content.clone(), window, cx));
        }
        self.note_editor = Some(draft);
        self.reader_key.clear();
        self.history = false;
        self.page = Page::Notes;
        self.capture_note_draft(cx);
        self.refresh_recoverable();
        cx.notify();
        true
    }

    pub fn open_draft(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_draft_id.as_ref() == Some(&id) && self.note_editor.is_some() {
            return;
        }
        if self.note_editor.is_some() {
            self.request_note_action(
                crate::editor_lifecycle::NoteIntent::OpenDraft(id),
                window,
                cx,
            );
            return;
        }
        if self.storage_busy
            || self.media_busy()
            || self.draft_loading
            || self.draft_discard.is_some()
        {
            return;
        }
        if let Some(record) = self.draft_catalog.get(&id).filter(|r| r.active()).cloned() {
            self.active_draft_id = Some(id);
            self.selected_note = record.draft.note_id.clone();
            self.selected_version = None;
            self.start_note_draft(record.draft, window, cx);
        }
    }

    pub fn open_note_editor(&mut self, copy: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.confirmation.is_some() || self.storage_busy || self.media_busy() || self.note_saving
        {
            return;
        }
        self.page = Page::Notes;
        if self.note_editor.is_some() {
            return;
        }
        if self.draft_loading || self.draft_discard.is_some() {
            return;
        }
        if !copy
            && let Some(id) = self
                .draft_catalog
                .values()
                .filter(|r| {
                    r.active() && r.draft.note_id.is_some() && r.draft.note_id == self.selected_note
                })
                .max_by_key(|r| r.updated_at)
                .map(|r| r.id.clone())
        {
            self.open_draft(id, window, cx);
            return;
        }
        let draft = self.snapshot.as_ref().and_then(|s| {
            let note = s
                .notes
                .iter()
                .find(|n| Some(&n.id) == self.selected_note.as_ref() && (!n.detached || copy))?;
            let version = s.versions.iter().rev().find(|v| {
                v.note_id == note.id
                    && (!copy || self.selected_version.as_ref().is_none_or(|id| id == &v.id))
            })?;
            Some(NoteDraft {
                asset_group: crate::media::asset_group(
                    (!copy).then_some(note.id.as_str()),
                    &version.content,
                ),
                note_id: (!copy).then(|| note.id.clone()),
                expected_hash: (!copy).then(|| version.hash.clone()),
                folder_id: note.folder_id.clone(),
                title: if copy {
                    format!("{}（副本）", note.title)
                } else {
                    note.title.clone()
                },
                content: version.content.clone(),
            })
        });
        if let Some(draft) = draft {
            if copy {
                let source = self
                    .snapshot
                    .as_ref()
                    .and_then(|s| {
                        s.notes
                            .iter()
                            .find(|n| Some(&n.id) == self.selected_note.as_ref())
                    })
                    .map(|n| std::path::PathBuf::from(&n.path));
                self.begin_media_adoption(draft, source, window, cx);
            } else {
                self.start_note_draft(draft, window, cx);
            }
        }
    }

    pub fn new_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.guard_settings(crate::settings_ui::SettingsContinuation::NewNote, cx) {
            return;
        }
        if self.confirmation.is_some()
            || self.storage_busy
            || self.media_busy()
            || self.note_saving
            || self.draft_loading
            || self.draft_discard.is_some()
        {
            return;
        }
        let folder = self
            .folders
            .read(cx)
            .filter()
            .folder_id()
            .map(str::to_owned);
        if self.note_editor.is_some() {
            self.request_note_action(crate::editor_lifecycle::NoteIntent::New(folder), window, cx);
            return;
        }
        self.start_new_draft(folder, window, cx);
    }
    pub(crate) fn start_new_draft(
        &mut self,
        folder_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.active_draft_id = None;
        self.selected_note = None;
        self.selected_version = None;
        self.start_note_draft(
            NoteDraft {
                asset_group: crate::media::new_group(),
                note_id: None,
                expected_hash: None,
                folder_id,
                title: "新笔记".into(),
                content: String::new(),
            },
            window,
            cx,
        );
    }
    pub fn recover_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self
            .draft_catalog
            .values()
            .filter(|r| r.active() && Some(&r.id) != self.active_draft_id.as_ref())
            .max_by_key(|r| r.updated_at)
            .map(|r| r.id.clone())
        {
            self.open_draft(id, window, cx);
        }
    }
    pub fn save_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.request_note_action(
            crate::editor_lifecycle::NoteIntent::Save { copy: false },
            window,
            cx,
        );
    }

    pub fn commit_note(&mut self, cx: &mut Context<Self>) {
        self.capture_note_draft(cx);
        let Some(id) = self.active_draft_id.clone() else {
            return;
        };
        let Some(record) = self.draft_catalog.get_mut(&id) else {
            return;
        };
        record.save_target = Some(
            record
                .draft
                .note_id
                .clone()
                .unwrap_or_else(|| record.draft.asset_group.clone()),
        );
        record.touch(chrono::Utc::now().timestamp_millis());
        let record = record.clone();
        self.refresh_storage_draft_index();
        self.persist_record_intent(record, crate::editor_lifecycle::NoteIntent::Commit, cx);
    }
    pub(crate) fn finish_commit_note(&mut self, cx: &mut Context<Self>) {
        let Some(record) = self
            .active_draft_id
            .as_ref()
            .and_then(|id| self.draft_catalog.get(id))
            .cloned()
        else {
            return;
        };
        let draft = record.draft;
        let command = if let Some(note_id) = draft.note_id {
            Command::SaveNote {
                note_id,
                expected_hash: draft.expected_hash.unwrap_or_default(),
                content: draft.content,
            }
        } else {
            Command::CreateNoteWithId {
                note_id: draft.asset_group,
                folder_id: draft.folder_id,
                title: draft.title,
                content: draft.content,
            }
        };
        self.pending_note_save = Some((self.last_library.clone(), record.id, record.revision));
        self.note_saving = true;
        self.note_content
            .update(cx, |editor, cx| editor.set_readonly(true, cx));
        self.send(command, cx);
    }
    pub(crate) fn complete_note_save(
        &mut self,
        note_id: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((library, id, revision)) = self.pending_note_save.take() else {
            return;
        };
        if library != self.last_library {
            return;
        }
        let Some(record) = self
            .draft_catalog
            .get_mut(&id)
            .filter(|r| r.revision == revision)
        else {
            return;
        };
        record.retire(
            DraftStatus::Saved {
                note_id: note_id.clone(),
                hash: Self::draft_hash(&record.draft.content),
            },
            chrono::Utc::now().timestamp_millis(),
        );
        let record = record.clone();
        self.refresh_storage_draft_index();
        self.persist_record_intent(
            record,
            crate::editor_lifecycle::NoteIntent::Saved(note_id),
            cx,
        );
    }

    pub(crate) fn accept_note_save_result(
        &mut self,
        result: Result<String, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match result {
            // The editor remains frozen until the terminal draft record is
            // acknowledged. Its recovery/error path explicitly restores input.
            Ok(note_id) => self.complete_note_save(note_id, window, cx),
            Err(_) => {
                self.note_saving = false;
                self.pending_note_save = None;
                self.note_content
                    .update(cx, |editor, cx| editor.set_readonly(false, cx));
            }
        }
    }

    pub fn note_editor_view(&self, cx: &mut Context<Self>) -> Div {
        let draft = self.note_editor.as_ref().expect("visible note editor");
        let busy = self.note_saving || self.storage_busy || self.media_busy();
        let palette = crate::theme::palette();
        let title_size =
            studyapp_ui::design::type_size(palette.font_size, studyapp_ui::design::TypeRole::Title);
        let record = self
            .active_draft_id
            .as_ref()
            .and_then(|id| self.draft_catalog.get(id));
        let durable = record.is_some_and(|record| {
            self.draft_durable
                .get(&record.id)
                .is_some_and(|revision| *revision >= record.revision)
        });
        let failed = self.draft_failed
            || self
                .active_draft_id
                .as_ref()
                .is_some_and(|id| self.draft_errors.contains_key(id));
        let (status, tone) = if failed {
            ("草稿保存失败", Tone::Danger)
        } else if self.note_saving {
            ("保存中…", Tone::Neutral)
        } else if self.draft_dirty_since.is_some() || !durable {
            ("草稿保存中…", Tone::Warning)
        } else {
            ("草稿已保存", Tone::Success)
        };
        let owner = cx.weak_entity();
        let overflow = Button::new("note-editor-menu")
            .small()
            .ghost()
            .label("⋯")
            .accessibility_label("笔记更多操作")
            .tooltip("笔记更多操作")
            .disabled(busy)
            .dropdown_menu(move |mut menu, _, cx| {
                let Some((_, _, busy)) = Self::library_menu_state(&owner, cx) else {
                    return menu;
                };
                let can_copy = owner
                    .update(cx, |this, _| {
                        this.note_editor
                            .as_ref()
                            .is_some_and(|draft| draft.note_id.is_some())
                    })
                    .unwrap_or(false);
                if can_copy {
                    let copy = owner.clone();
                    menu = menu.item(PopupMenuItem::new("另存新笔记").disabled(busy).on_click(
                        move |_, window, cx| {
                            let _ = copy.update(cx, |this, cx| {
                                this.request_note_action(
                                    crate::editor_lifecycle::NoteIntent::Save { copy: true },
                                    window,
                                    cx,
                                )
                            });
                        },
                    ));
                }
                let keep = owner.clone();
                let discard = owner.clone();
                menu.min_w(px(188.))
                    .item(
                        PopupMenuItem::new("保留草稿并返回")
                            .disabled(busy)
                            .on_click(move |_, window, cx| {
                                let _ = keep.update(cx, |this, cx| {
                                    this.request_note_action(
                                        crate::editor_lifecycle::NoteIntent::Keep,
                                        window,
                                        cx,
                                    )
                                });
                            }),
                    )
                    .separator()
                    .item(PopupMenuItem::new("删除草稿").disabled(busy).on_click(
                        move |_, _, cx| {
                            let _ = discard.update(cx, |this, cx| {
                                if !this.note_saving && !this.storage_busy && !this.media_busy() {
                                    this.confirmation = this.active_draft_id.clone().map(
                                        crate::editor_lifecycle::NoteConfirmation::DiscardDraft,
                                    );
                                    cx.notify();
                                }
                            });
                        },
                    ))
            });
        let view = document_surface()
            .id("note-editor")
            .flex_1()
            .min_h_0()
            .h_full()
            .min_w_0()
            .child(
                document_toolbar()
                    .id("note-editor-toolbar")
                    .flex_shrink_0()
                    .child(
                        div()
                            .id("note-title")
                            .test_support()
                            .flex_1()
                            .min_w_0()
                            .child(
                                Input::new(&self.note_title)
                                    .large()
                                    .text_size(title_size)
                                    .line_height(
                                        title_size
                                            * studyapp_ui::design::line_height(
                                                studyapp_ui::design::TypeRole::Title,
                                            ),
                                    )
                                    .disabled(busy || draft.note_id.is_some()),
                            ),
                    )
                    .child(
                        Button::new("save-note")
                            .loading(self.note_saving || self.media_busy())
                            .small()
                            .primary()
                            .disabled(busy)
                            .label(if self.note_saving {
                                "保存中…"
                            } else if self.media_busy() {
                                "图片处理中…"
                            } else {
                                "保存"
                            })
                            .on_click(
                                cx.listener(|this, _, window, cx| this.save_note(window, cx)),
                            ),
                    )
                    .child(overflow),
            )
            .child(
                row()
                    .px(studyapp_ui::design::space(palette.spacing, 1.5))
                    .py(studyapp_ui::design::space(palette.spacing, 0.5))
                    .gap(studyapp_ui::design::space(palette.spacing, 1.))
                    .child(
                        pill_tone(status, tone)
                            .id("note-draft-status")
                            .test_support(),
                    )
                    .child(
                        muted("草稿自动保留，点击保存后写入笔记。")
                            .min_w_0()
                            .truncate(),
                    ),
            )
            .children(self.snapshot.as_ref().and_then(|snapshot| {
                if !snapshot.settings.show_file_paths {
                    return None;
                }
                draft.note_id.as_ref().and_then(|id| {
                    snapshot
                        .notes
                        .iter()
                        .find(|note| &note.id == id)
                        .map(|note| {
                            muted(studyapp_desktop::presentation::display_path(&note.path))
                                .id("editor-file-path")
                                .test_support()
                                .truncate()
                                .flex_shrink_0()
                        })
                })
            }))
            .child(self.note_content.clone());
        column().flex_1().min_h_0().h_full().min_w_0().child(view)
    }
    pub fn export_note(&self, note_id: String, version_id: Option<String>) {
        let sender = self.worker.sender.clone();
        let library = self.last_library.clone();
        std::thread::spawn(move || {
            if let Some(path) = rfd::FileDialog::new()
                .set_title("导出 Markdown（请选择新文件名）")
                .add_filter("Markdown", &["md"])
                .set_file_name("笔记快照.md")
                .save_file()
            {
                let _ = sender.send(crate::worker::Work::Scoped {
                    library,
                    work: Box::new(crate::worker::Work::Apply(Command::ExportNote {
                        note_id,
                        version_id,
                        path,
                    })),
                });
            }
        });
    }
}

#[cfg(all(test, feature = "ui-test"))]
mod deletion_tests {
    use crate::{
        app::StudyApp,
        drafts::{DraftRecord, DraftStatus},
        editor_lifecycle::NoteConfirmation,
        worker::Worker,
    };
    use gpui_kit::TestAppContext;

    #[gpui_kit::test]
    fn deleting_recoverable_draft_preserves_the_other_active_editor(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let directory = tempfile::tempdir().unwrap().keep();
        let database = directory.join("studyapp.sqlite3");
        let library = database.to_string_lossy().into_owned();
        let worker = Worker::start(database, None);
        let (view, cx) = cx.add_window_view(|window, cx| StudyApp::new(worker, window, cx));
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.last_library = library.clone();
                app.draft_loading = false;
                app.start_new_draft(None, window, cx);
                app.note_content.update(cx, |editor, cx| {
                    editor.set_value("A 的未保存正文".into(), window, cx)
                });
                app.capture_note_draft(cx);
                let active = app.active_draft_id.clone().unwrap();
                let before = app.draft_catalog[&active].clone();
                let mut recovered = DraftRecord::new(before.draft.clone(), before.updated_at + 1);
                recovered.draft.title = "B 的恢复草稿".into();
                recovered.draft.content = "只应删除 B".into();
                let target = recovered.id.clone();
                app.draft_catalog.insert(target.clone(), recovered);
                app.refresh_recoverable();
                app.confirmation = app
                    .recoverable_draft_id()
                    .map(NoteConfirmation::DiscardDraft);
                app.confirm_note_action(window, cx);
                assert_eq!(app.draft_discard.as_ref().unwrap().id, target);
                assert_eq!(app.active_draft_id.as_ref(), Some(&active));
                app.mark_note_dirty();
                assert!(
                    app.draft_dirty_since.is_some(),
                    "deleting another draft must not suspend active autosave"
                );
                let started = std::time::Instant::now();
                while app.draft_discard.is_some() {
                    app.poll_drafts(window, cx);
                    assert!(
                        started.elapsed() < std::time::Duration::from_secs(5),
                        "draft deletion was not acknowledged"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                assert_eq!(app.active_draft_id.as_ref(), Some(&active));
                assert_eq!(app.draft_catalog[&active], before);
                assert_eq!(app.note_content.read(cx).value(), "A 的未保存正文");
                assert!(!app.note_content.read(cx).readonly());
                let deleted: DraftRecord = serde_json::from_slice(
                    &std::fs::read(
                        crate::drafts::DraftStore::record_path(&library, &target).unwrap(),
                    )
                    .unwrap(),
                )
                .unwrap();
                assert_eq!(deleted.status, DraftStatus::Deleted);
                assert_eq!(deleted.draft.content, "只应删除 B");
            });
        });
    }
}
