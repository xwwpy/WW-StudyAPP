use crate::{app::StudyApp, editor_lifecycle::NoteIntent};
use gpui_kit::*;

impl StudyApp {
    pub(crate) fn move_draft_to(
        &mut self,
        library: String,
        id: String,
        folder: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.storage_busy
            || self.note_saving
            || self.media_busy()
            || self.draft_loading
            || self.draft_discard.is_some()
            || self.draft_persist.is_some()
            || self.has_modal(cx)
        {
            return;
        }
        if !self.draft_move_valid(&library, &id, folder.as_deref()) {
            return;
        }
        if self.active_draft_id.as_ref() == Some(&id) && self.note_editor.is_some() {
            self.request_note_action(
                NoteIntent::MoveDraft {
                    library,
                    id,
                    folder,
                },
                window,
                cx,
            );
        } else {
            self.commit_draft_move(library, id, folder, cx);
        }
    }

    fn draft_move_valid(&self, library: &str, id: &str, folder: Option<&str>) -> bool {
        library == self.last_library
            && self.snapshot.as_ref().is_some_and(|snapshot| {
                folder.is_none_or(|id| snapshot.folders.iter().any(|f| f.id == id))
            })
            && self.draft_catalog.get(id).is_some_and(|record| {
                record.active() && record.draft.folder_id.as_deref() != folder
            })
    }

    pub(crate) fn commit_draft_move(
        &mut self,
        library: String,
        id: String,
        folder: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if !self.draft_move_valid(&library, &id, folder.as_deref()) {
            self.note_saving = false;
            if self.note_editor.is_some() {
                self.note_content
                    .update(cx, |editor, cx| editor.set_readonly(false, cx));
            }
            self.push_notice(Err("目标目录已变化，请重新选择。".into()), cx);
            return;
        }
        let mut record = self.draft_catalog[&id].clone();
        // A draft of an existing note shares its logical folder with that note.
        // The worker's durable snapshot reconciles all its drafts after the move.
        if let Some(note_id) = record.draft.note_id.clone() {
            self.note_saving = false;
            if self.note_editor.is_some() {
                self.note_content
                    .update(cx, |editor, cx| editor.set_readonly(false, cx));
            }
            self.send(
                studyapp_core::Command::MoveNote {
                    note_id,
                    folder_id: folder,
                },
                cx,
            );
            return;
        }
        record.draft.folder_id = folder.clone();
        record.touch(chrono::Utc::now().timestamp_millis());
        self.draft_catalog.insert(id.clone(), record.clone());
        if self.active_draft_id.as_ref() == Some(&id)
            && let Some(draft) = self.note_editor.as_mut()
        {
            draft.folder_id = folder;
        }
        self.persist_record_intent(record, NoteIntent::MovedDraft, cx);
    }
}

#[cfg(all(test, feature = "ui-test"))]
mod tests {
    use crate::{app::StudyApp, worker::Worker};
    use gpui_kit::TestAppContext;

    #[gpui_kit::test]
    fn active_move_flushes_and_keeps_the_same_editor(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let directory = tempfile::tempdir().unwrap().keep();
        let database = directory.join("studyapp.sqlite3");
        let library = database.to_string_lossy().into_owned();
        let mut engine = studyapp_core::Engine::open(&database, 1).unwrap();
        engine
            .apply(
                studyapp_core::Command::CreateFolder {
                    name: "课程".into(),
                    parent_id: None,
                },
                2,
            )
            .unwrap();
        let snapshot = engine.snapshot();
        let folder = snapshot.folders[0].id.clone();
        drop(engine);
        let worker = Worker::start(database, None);
        let (view, cx) = cx.add_window_view(|window, cx| StudyApp::new(worker, window, cx));
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.last_library = library.clone();
                app.snapshot = Some(std::sync::Arc::new(snapshot));
                app.draft_loading = false;
                app.start_new_draft(None, window, cx);
                app.note_content.update(cx, |editor, cx| {
                    editor.set_value("最新正文".into(), window, cx)
                });
                let id = app.active_draft_id.clone().unwrap();
                let generation = app.note_content.read(cx).generation();
                app.move_draft_to(library, id.clone(), Some(folder.clone()), window, cx);
                assert!(
                    app.note_intent.is_some(),
                    "moving the active draft must flush its current text first"
                );
                let request = app.note_intent.as_ref().unwrap().0;
                app.editor_event(
                    crate::markdown_editor::MarkdownEvent::Flushed(request),
                    window,
                    cx,
                );
                assert_eq!(app.draft_catalog[&id].draft.content, "最新正文");
                assert_eq!(
                    app.draft_catalog[&id].draft.folder_id.as_ref(),
                    Some(&folder)
                );
                let receipt = app
                    .draft_persist
                    .take()
                    .expect("folder change must be durably acknowledged");
                app.finish_draft_persist(receipt.intent, window, cx);
                assert_eq!(app.active_draft_id.as_ref(), Some(&id));
                assert_eq!(
                    app.note_editor.as_ref().unwrap().folder_id.as_ref(),
                    Some(&folder)
                );
                assert_eq!(app.note_content.read(cx).generation(), generation);
                assert_eq!(app.note_content.read(cx).value(), "最新正文");
                assert!(!app.note_saving);
                assert!(!app.note_content.read(cx).readonly());
            })
        });
    }
}
