use crate::{
    app::StudyApp,
    note_actions::{NoteAction, NoteActionKind},
};
use gpui_kit::*;
use studyapp_core::Command;
use studyapp_desktop::presentation::display_path;

impl StudyApp {
    pub(crate) fn note_rename_busy(&self, cx: &App) -> bool {
        self.storage_busy
            || self.media_busy()
            || self.note_saving
            || self.saving
            || self.preferences_ui.busy
            || self.preferences_ui.replacing
            || self.editor.is_some()
            || self.draft_discard.is_some()
            || self.has_modal(cx)
            || self.sync_panel.read(cx).status.busy
    }

    pub(crate) fn handle_note_rename_command(
        &mut self,
        library_path: &str,
        command: Command,
        cx: &mut Context<Self>,
    ) {
        let Command::RenameNote {
            note_id,
            title,
            expected_revision,
        } = command
        else {
            return;
        };
        let action = NoteAction {
            note_id: note_id.clone(),
            library_path: library_path.into(),
            kind: NoteActionKind::Rename { expected_revision },
        };
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        if library_path != self.last_library {
            return;
        }
        match action.resolve(snapshot, self.note_rename_busy(cx)) {
            Ok(_) => self.send(
                Command::RenameNote {
                    note_id,
                    title,
                    expected_revision,
                },
                cx,
            ),
            Err(message) => self.push_notice(Err(message.into()), cx),
        }
        cx.notify();
    }

    pub fn handle_note_action(
        &mut self,
        action: NoteAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if action.library_path != self.last_library {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let busy = if matches!(action.kind, NoteActionKind::Rename { .. }) {
            self.note_rename_busy(cx)
        } else {
            self.storage_busy
        };
        let note = match action.resolve(snapshot, busy) {
            Ok(note) => note,
            Err(message) => {
                self.push_notice(Err(message.into()), cx);
                cx.notify();
                return;
            }
        };
        match action.kind.clone() {
            NoteActionKind::Rename { .. } => {
                self.folders
                    .update(cx, |panel, cx| panel.begin_rename_note(action, window, cx));
            }
            NoteActionKind::MoveTo(folder_id) => {
                if !self.storage_busy && note.folder_id != folder_id {
                    self.send(
                        Command::MoveNote {
                            note_id: note.id.clone(),
                            folder_id,
                        },
                        cx,
                    );
                }
            }
            NoteActionKind::CopyPath => {
                cx.write_to_clipboard(ClipboardItem::new_string(display_path(&note.path)));
                self.push_notice(Ok("已复制文件路径".into()), cx);
            }
            NoteActionKind::Reveal => {
                let path = note.path.clone();
                let note_id = note.id.clone();
                let library = self.last_library.clone();
                cx.spawn(async move |this, cx| {
                    let checked_path = path.clone();
                    let exists =
                        cx.background_executor()
                            .spawn(async move {
                                std::fs::metadata(checked_path).is_ok_and(|m| m.is_file())
                            })
                            .await;
                    let _ = this.update(cx, |this, cx| {
                        if this.last_library != library
                            || !this.snapshot.as_ref().is_some_and(|s| {
                                s.notes.iter().any(|n| n.id == note_id && n.path == path)
                            })
                        {
                            return;
                        }
                        if exists {
                            cx.reveal_path(std::path::Path::new(&display_path(&path)));
                        } else {
                            this.push_notice(
                                Err("原文件不存在或暂时无法访问；笔记历史仍可查看和导出。".into()),
                                cx,
                            );
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
        }
        cx.notify();
    }
}
