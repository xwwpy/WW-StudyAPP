use crate::{components::*, folders_ui::note_destinations};
use gpui_kit::{
    component::{
        Sizable,
        menu::{PopupMenu, PopupMenuItem},
    },
    *,
};
use std::rc::Rc;
use studyapp_core::{Note, Snapshot};

pub const LIBRARY_SORT_OPTIONS: [(studyapp_core::LibrarySort, &str); 4] = [
    (studyapp_core::LibrarySort::NameAscending, "名称 · A → Z"),
    (studyapp_core::LibrarySort::NameDescending, "名称 · Z → A"),
    (studyapp_core::LibrarySort::Newest, "修改时间 · 从新到旧"),
    (studyapp_core::LibrarySort::Oldest, "修改时间 · 从旧到新"),
];

/// Drag only carries logical identity; neither dragging nor dropping touches files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteDrag {
    pub note_id: String,
    pub library_path: String,
    pub title: String,
}

/// Unsaved document identity. The host rechecks its live catalog before moving.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftDrag {
    pub draft_id: String,
    pub library_path: String,
    pub folder_id: Option<String>,
    pub title: String,
}

pub struct DraftDragPreview {
    pub title: String,
    pub source_offset: Point<Pixels>,
}

impl Render for DraftDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        anchored()
            .position_mode(AnchoredPositionMode::Local)
            .position(self.source_offset + point(px(12.), px(12.)))
            .snap_to_window()
            .child(library_drag_card(
                "draft-drag-preview",
                self.title.clone(),
                gpui_kit::component::Icon::new(gpui_kit::component::IconName::FileText)
                    .small()
                    .text_color(crate::theme::color(MUTED)),
            ))
    }
}

impl NoteDrag {
    pub fn new(note: &Note, snapshot: &Snapshot) -> Self {
        Self {
            note_id: note.id.clone(),
            library_path: snapshot.data_path.clone(),
            title: note.title.clone(),
        }
    }

    pub fn move_action(&self, destination: Option<String>) -> NoteAction {
        NoteAction {
            note_id: self.note_id.clone(),
            library_path: self.library_path.clone(),
            kind: NoteActionKind::MoveTo(destination),
        }
    }
}

/// GPUI places a drag view at `pointer - source_offset`, even when the view is
/// smaller than the source. Restore that offset inside the view, then leave a
/// fixed gap so a lower-right grab stays just as close as a top-left grab.
pub struct NoteDragPreview {
    title: String,
    source_offset: Point<Pixels>,
}

impl NoteDragPreview {
    pub fn new(drag: &NoteDrag, source_offset: Point<Pixels>) -> Self {
        Self {
            title: drag.title.clone(),
            source_offset,
        }
    }

    fn content_offset(source_offset: Point<Pixels>) -> Point<Pixels> {
        source_offset + point(px(12.), px(12.))
    }
}

impl Render for NoteDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let offset = Self::content_offset(self.source_offset);
        anchored()
            .position_mode(AnchoredPositionMode::Local)
            .position(offset)
            .snap_to_window()
            .child(library_drag_card(
                "note-drag-preview",
                self.title.clone(),
                gpui_kit::component::Icon::new(gpui_kit::component::IconName::FileText)
                    .small()
                    .text_color(crate::theme::color(GREEN)),
            ))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoteActionKind {
    MoveTo(Option<String>),
    Rename { expected_revision: u64 },
    Reveal,
    CopyPath,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteAction {
    pub note_id: String,
    pub library_path: String,
    pub kind: NoteActionKind,
}

impl NoteAction {
    pub fn resolve<'a>(
        &self,
        snapshot: &'a Snapshot,
        busy: bool,
    ) -> Result<&'a Note, &'static str> {
        if self.library_path != snapshot.data_path {
            return Err("资料库已切换，请重新选择笔记。");
        }
        let note = snapshot
            .notes
            .iter()
            .find(|note| note.id == self.note_id)
            .ok_or("笔记已移除。")?;
        if let NoteActionKind::Rename { expected_revision } = &self.kind {
            if busy {
                return Err("笔记正在保存或资料库忙碌，请稍后重命名。");
            }
            if !note.managed || note.detached {
                return Err("仅支持重命名正在管理的托管笔记。");
            }
            if note.revision != *expected_revision {
                return Err("笔记已更新，请重新打开重命名对话框。");
            }
        }
        if let NoteActionKind::MoveTo(destination) = &self.kind {
            if busy {
                return Err("资料库维护中，请稍后移动笔记。");
            }
            if &note.folder_id == destination {
                return Err("笔记已在此目录。");
            }
            if destination
                .as_ref()
                .is_some_and(|id| !snapshot.folders.iter().any(|folder| &folder.id == id))
            {
                return Err("目标目录已移除，请重新选择。");
            }
        }
        Ok(note)
    }
}

/// Build a menu at opening time. The host must resolve the emitted action again
/// against its latest snapshot, since a menu can outlive a library switch.
pub fn note_context_menu(
    menu: PopupMenu,
    snapshot: &Snapshot,
    note: &Note,
    busy: bool,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
    handler: impl Fn(NoteAction, &mut Window, &mut App) + 'static,
) -> PopupMenu {
    let handler = Rc::new(handler);
    let destinations = note_destinations(&snapshot.folders, note.folder_id.as_deref());
    let can_move = !busy;
    let base = NoteAction {
        note_id: note.id.clone(),
        library_path: snapshot.data_path.clone(),
        kind: NoteActionKind::Reveal,
    };
    let move_action = base.clone();
    let move_handler = handler.clone();
    let menu = menu.submenu("移动到目录", window, cx, move |menu, _, _| {
        let mut menu = menu
            .scrollable(true)
            .max_h(px(320.))
            .min_w(px(210.))
            .max_w(px(480.));
        if destinations.is_empty() {
            return menu.item(PopupMenuItem::new("没有其他目录").disabled(true));
        }
        for destination in &destinations {
            let action = NoteAction {
                kind: NoteActionKind::MoveTo(destination.id.clone()),
                ..move_action.clone()
            };
            let handler = move_handler.clone();
            menu = menu.item(
                PopupMenuItem::new(destination.path.clone())
                    .disabled(!can_move)
                    .on_click(move |_, window, cx| handler(action.clone(), window, cx)),
            );
        }
        menu
    });
    let reveal = base.clone();
    let reveal_handler = handler.clone();
    let copy = NoteAction {
        kind: NoteActionKind::CopyPath,
        ..base.clone()
    };
    let copy_handler = handler.clone();
    let menu = menu
        .separator()
        .item(
            PopupMenuItem::new("在文件资源管理器中显示")
                .on_click(move |_, window, cx| reveal_handler(reveal.clone(), window, cx)),
        )
        .item(
            PopupMenuItem::new("复制文件路径")
                .on_click(move |_, window, cx| copy_handler(copy.clone(), window, cx)),
        );
    if note.managed && !note.detached {
        let rename = NoteAction {
            kind: NoteActionKind::Rename {
                expected_revision: note.revision,
            },
            ..base.clone()
        };
        let rename_handler = handler.clone();
        menu.item(
            PopupMenuItem::new("重命名")
                .disabled(busy)
                .on_click(move |_, window, cx| rename_handler(rename.clone(), window, cx)),
        )
    } else {
        menu
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use studyapp_core::{NoteFolder, Settings, TimerState};

    #[::core::prelude::v1::test]
    fn bottom_right_grab_keeps_compact_preview_near_pointer() {
        let pointer = point(px(720.), px(390.));
        // Includes the long/tall source row which exposed the old compact ghost.
        for source_offset in [
            point(px(3.), px(3.)),
            point(px(429.), px(137.)),
            point(px(-19.), px(-15.)),
        ] {
            let gpui_origin = pointer - source_offset;
            let preview_origin = gpui_origin + NoteDragPreview::content_offset(source_offset);
            assert_eq!(preview_origin - pointer, point(px(12.), px(12.)));
        }
    }

    fn snapshot() -> Snapshot {
        Snapshot {
            settings: Settings::default(),
            notes: vec![Note {
                id: "note".into(),
                path: "C:/notes/note.md".into(),
                title: "Note".into(),
                folder_id: None,
                title_override: false,
                managed: true,
                detached: false,
                missing: false,
                error: None,
                created_at: 0,
                updated_at: 0,
                revision: 1,
            }],
            folders: vec![NoteFolder {
                id: "folder".into(),
                name: "Folder".into(),
                parent_id: None,
            }],
            versions: Arc::new(Vec::new()),
            sessions: Vec::new(),
            reviews: Vec::new(),
            review_events: Vec::new(),
            timer: TimerState {
                paused: true,
                active_seconds: 0,
                target_seconds: 60,
                started_at: 0,
                spans: Vec::new(),
            },
            data_path: "C:/library/current".into(),
        }
    }

    fn action(kind: NoteActionKind) -> NoteAction {
        NoteAction {
            note_id: "note".into(),
            library_path: "C:/library/current".into(),
            kind,
        }
    }

    #[::core::prelude::v1::test]
    fn stale_library_payload_cannot_target_same_note_id_in_another_library() {
        let mut snapshot = snapshot();
        snapshot.data_path = "C:/library/other".into();
        for kind in [
            NoteActionKind::MoveTo(Some("folder".into())),
            NoteActionKind::Reveal,
            NoteActionKind::Rename {
                expected_revision: 1,
            },
            NoteActionKind::CopyPath,
        ] {
            assert!(action(kind).resolve(&snapshot, false).is_err());
        }
    }

    #[::core::prelude::v1::test]
    fn busy_notes_cannot_be_moved() {
        let snapshot = snapshot();
        let action = action(NoteActionKind::MoveTo(Some("folder".into())));
        assert!(action.resolve(&snapshot, true).is_err());
    }

    #[::core::prelude::v1::test]
    fn detached_notes_retain_metadata_move_support() {
        let mut snapshot = snapshot();
        snapshot.notes[0].detached = true;
        assert!(
            action(NoteActionKind::MoveTo(Some("folder".into())))
                .resolve(&snapshot, false)
                .is_ok()
        );
    }

    #[::core::prelude::v1::test]
    fn moves_reject_current_or_deleted_destination() {
        let snapshot = snapshot();
        assert!(
            action(NoteActionKind::MoveTo(None))
                .resolve(&snapshot, false)
                .is_err()
        );
        assert!(
            action(NoteActionKind::MoveTo(Some("gone".into())))
                .resolve(&snapshot, false)
                .is_err()
        );
    }

    #[::core::prelude::v1::test]
    fn valid_move_and_explicit_path_actions_resolve_latest_note() {
        let mut snapshot = snapshot();
        snapshot.notes[0].path = "C:/library/current/renamed.md".into();
        assert!(
            action(NoteActionKind::MoveTo(Some("folder".into())))
                .resolve(&snapshot, false)
                .is_ok()
        );
        for kind in [NoteActionKind::Reveal, NoteActionKind::CopyPath] {
            assert_eq!(
                action(kind).resolve(&snapshot, true).unwrap().path,
                "C:/library/current/renamed.md"
            );
        }
    }

    #[::core::prelude::v1::test]
    fn deleted_note_payload_is_rejected() {
        let mut snapshot = snapshot();
        snapshot.notes.clear();
        assert!(
            action(NoteActionKind::MoveTo(Some("folder".into())))
                .resolve(&snapshot, false)
                .is_err()
        );
    }
    #[::core::prelude::v1::test]
    fn rename_rechecks_busy_revision_eligibility_and_captured_note_identity() {
        let mut snapshot = snapshot();
        let action = action(NoteActionKind::Rename {
            expected_revision: 1,
        });
        assert!(action.resolve(&snapshot, false).is_ok());
        assert!(action.resolve(&snapshot, true).is_err());
        snapshot.notes[0].revision += 1;
        assert!(action.resolve(&snapshot, false).is_err());
        snapshot.notes[0].revision = 1;
        snapshot.notes[0].managed = false;
        assert!(action.resolve(&snapshot, false).is_err());
        snapshot.notes[0].managed = true;
        snapshot.notes[0].detached = true;
        assert!(action.resolve(&snapshot, false).is_err());
        snapshot.notes[0].detached = false;
        let mut other = snapshot.notes[0].clone();
        other.id = "other".into();
        other.title = "Other selection".into();
        snapshot.notes.insert(0, other);
        assert_eq!(action.resolve(&snapshot, false).unwrap().id, "note");
        snapshot.notes.retain(|n| n.id != "note");
        assert!(action.resolve(&snapshot, false).is_err());
    }
}
