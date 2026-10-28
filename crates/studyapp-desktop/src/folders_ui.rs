use crate::controls::Button;
use crate::{
    components::*,
    note_actions::{DraftDrag, NoteAction, NoteActionKind, NoteDrag},
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    component::{
        Disableable, Icon, IconName, Selectable, Sizable,
        button::ButtonVariants,
        input::{Input, InputEvent, InputState},
        menu::{ContextMenuExt, DropdownMenu, PopupMenu, PopupMenuItem},
    },
    *,
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use studyapp_core::{Command, MAX_FOLDER_DEPTH, Note, NoteFolder, Snapshot};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum FolderFilter {
    #[default]
    All,
    Root,
    Drafts,
    Folder(String),
}

impl FolderFilter {
    /// Folder membership only. The caller keeps its usual detached/search filters.
    pub fn matches(&self, note: &Note) -> bool {
        match self {
            Self::All => true,
            Self::Root => note.folder_id.is_none(),
            Self::Drafts => false,
            Self::Folder(id) => note.folder_id.as_ref() == Some(id),
        }
    }

    pub fn folder_id(&self) -> Option<&str> {
        match self {
            Self::Folder(id) => Some(id),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub enum FolderEvent {
    Filter(FolderFilter),
    Command(Box<Command>),
    RenameNote {
        library_path: String,
        command: Box<Command>,
    },
    MoveDraft {
        library_path: String,
        draft_id: String,
        folder_id: Option<String>,
    },
    OverlayChanged(bool),
    MenuRegions(Vec<Bounds<Pixels>>),
}

#[derive(Clone, Debug)]
struct FolderRow {
    id: String,
    name: String,
    path: String,
    depth: usize,
    ancestors: Vec<String>,
    has_children: bool,
}

/// Logical identity only. Drop resolves it against the latest library snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderDrag {
    pub folder_id: String,
    pub library_path: String,
    pub title: String,
}

struct FolderDragPreview {
    title: String,
    source_offset: Point<Pixels>,
}

impl Render for FolderDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        anchored()
            .position_mode(AnchoredPositionMode::Local)
            .position(self.source_offset + point(px(12.), px(12.)))
            .snap_to_window()
            .child(library_drag_card(
                "folder-drag-preview",
                self.title.clone(),
                Icon::new(IconName::Folder)
                    .small()
                    .text_color(crate::theme::color(GREEN)),
            ))
    }
}

#[derive(Clone, Copy)]
enum FolderMenuAction {
    Create,
    Rename,
    Move,
    Delete,
}

#[derive(Clone)]
enum FolderAction {
    Create {
        parent_id: Option<String>,
    },
    Rename {
        folder_id: String,
    },
    MoveFolder {
        folder_id: String,
        destination: Option<String>,
    },
    RenameNote {
        action: NoteAction,
    },
    MoveNote {
        note_id: String,
        destination: Option<String>,
    },
    Delete {
        folder_id: String,
    },
}

/// A bounded native sidebar. Give its parent a height; the tree scrolls within it.
///
/// `begin_move_note` lets the host's selected-note toolbar reuse this panel's
/// destination picker. All mutations are events; this component performs no IO.
pub struct FolderPanel {
    snapshot: Option<Arc<Snapshot>>,
    index: Arc<studyapp_desktop::library_tree::LibraryIndex>,
    revealed_note: Option<String>,
    row_sort: studyapp_core::LibrarySort,
    search_query: Option<String>,
    search_folders: HashSet<String>,
    visible_cache: Option<VisibleLibrary>,
    rows: Vec<FolderRow>,
    counts: HashMap<String, usize>,
    all_count: usize,
    root_count: usize,
    draft_count: usize,
    draft_destinations: bool,
    draft_return_scroll: Option<ListOffset>,
    flat_cache: Option<FlatLibrary>,
    filter: FolderFilter,
    collapsed: HashSet<String>,
    name: Entity<InputState>,
    action: Option<FolderAction>,
    error: Option<String>,
    busy: bool,
    active_menu: Option<u64>,
    menu_sequence: u64,
    tree_scroll: ListState,
    action_scroll: ScrollHandle,
    destination_scroll: ScrollHandle,
}

struct VisibleLibrary {
    query: String,
    sort: studyapp_core::LibrarySort,
    detached: bool,
    collapsed: HashSet<String>,
    positions: Arc<Vec<usize>>,
    count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LibraryRow {
    System(usize),
    Folder {
        id: String,
        position: usize,
    },
    Note {
        id: String,
        position: usize,
        depth: usize,
    },
    Draft(String),
    Empty(bool),
}
impl LibraryRow {
    fn identity(&self) -> (u8, &str, usize) {
        match self {
            Self::System(index) => (0, "", *index),
            Self::Folder { id, .. } => (1, id, 0),
            Self::Note { id, .. } => (2, id, 0),
            Self::Draft(id) => (3, id, 0),
            Self::Empty(drafts) => (4, "", usize::from(*drafts)),
        }
    }
    fn same_identity(&self, other: &Self) -> bool {
        self.identity() == other.identity()
    }
}

fn relocate_anchor(old: &[LibraryRow], next: &[LibraryRow], index: usize) -> Option<(usize, bool)> {
    let target = old.get(index)?;
    if let Some(position) = next.iter().position(|row| row.same_identity(target)) {
        return Some((position, true));
    }
    let positions: HashMap<_, _> = next
        .iter()
        .enumerate()
        .map(|(position, row)| (row.identity(), position))
        .collect();
    old.iter()
        .skip(index + 1)
        .chain(old[..index].iter().rev())
        .find_map(|target| {
            positions
                .get(&target.identity())
                .copied()
                .map(|position| (position, false))
        })
}
struct FlatLibrary {
    visible: Arc<Vec<usize>>,
    drafts_only: bool,
    draft_ids: Arc<Vec<String>>,
    rows: Arc<Vec<LibraryRow>>,
}

impl EventEmitter<FolderEvent> for FolderPanel {}

impl FolderPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("目录名称"));
        cx.subscribe(&name, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::PressEnter { .. })
                && matches!(
                    this.action,
                    Some(
                        FolderAction::Create { .. }
                            | FolderAction::Rename { .. }
                            | FolderAction::RenameNote { .. }
                    )
                )
            {
                // submit clears the action before emitting, so a repeated
                // Enter cannot submit the same form twice.
                this.submit(cx);
            } else {
                cx.notify();
            }
        })
        .detach();
        Self {
            snapshot: None,
            index: Arc::new(Default::default()),
            revealed_note: None,
            row_sort: Default::default(),
            search_query: None,
            search_folders: HashSet::new(),
            visible_cache: None,
            rows: Vec::new(),
            counts: HashMap::new(),
            all_count: 0,
            root_count: 0,
            draft_count: 0,
            draft_destinations: false,
            draft_return_scroll: None,
            flat_cache: None,
            filter: FolderFilter::All,
            collapsed: HashSet::new(),
            name,
            action: None,
            error: None,
            busy: false,
            active_menu: None,
            menu_sequence: 0,
            tree_scroll: ListState::new(0, ListAlignment::Top, px(LIBRARY_ROW_HEIGHT)),
            action_scroll: ScrollHandle::new(),
            destination_scroll: ScrollHandle::new(),
        }
    }

    pub fn library_index(&self) -> Arc<studyapp_desktop::library_tree::LibraryIndex> {
        self.index.clone()
    }

    pub fn filter(&self) -> &FolderFilter {
        &self.filter
    }

    pub fn modal_open(&self) -> bool {
        self.action.is_some()
    }

    pub fn can_create_folder(&self) -> bool {
        !self.busy
            && self.action.is_none()
            && self.snapshot.is_some()
            && self.filter.folder_id().is_none_or(|id| {
                self.rows
                    .iter()
                    .find(|row| row.id == id)
                    .is_none_or(|row| row.depth + 1 < MAX_FOLDER_DEPTH)
            })
    }

    /// The library plus menu uses the exact selected folder as the destination.
    /// Recheck depth and busy state at activation, since a menu may remain open.
    pub fn begin_create_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.can_create_folder() {
            let parent_id = self.filter.folder_id().map(str::to_owned);
            self.edit_name(
                FolderAction::Create { parent_id },
                String::new(),
                window,
                cx,
            );
        }
    }

    pub fn overlay_open(&self) -> bool {
        self.action.is_some() || self.active_menu.is_some()
    }

    pub fn dialog_open(&self) -> bool {
        self.action.is_some()
    }

    pub fn modal_initial_focus(&self, cx: &App) -> Option<FocusHandle> {
        matches!(
            self.action,
            Some(
                FolderAction::Create { .. }
                    | FolderAction::Rename { .. }
                    | FolderAction::RenameNote { .. }
            )
        )
        .then(|| self.name.focus_handle(cx))
    }

    pub fn cancel_action(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if self.action.take().is_some() {
            self.error = None;
            cx.emit(FolderEvent::OverlayChanged(false));
            cx.notify();
        }
    }

    pub fn submit_action(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        // Scope-level Enter is for forms. Deletion requires the explicit
        // danger action, so opening the dialog cannot accept it accidentally.
        if matches!(self.action, Some(FolderAction::Delete { .. })) {
            return;
        }
        self.submit(cx);
    }

    pub fn modal_content(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.action.as_ref()?;
        Some(self.action_view(cx))
    }

    fn opened_action(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        cx.emit(FolderEvent::OverlayChanged(true));
        cx.notify();
    }

    pub fn set_filter(&mut self, filter: FolderFilter, cx: &mut Context<Self>) {
        let filter = match filter {
            FolderFilter::Folder(id) if !self.rows.iter().any(|row| row.id == id) => {
                FolderFilter::All
            }
            filter => filter,
        };
        if let FolderFilter::Folder(id) = &filter
            && let Some(row) = self.rows.iter().find(|row| &row.id == id)
        {
            for ancestor in &row.ancestors {
                self.collapsed.remove(ancestor);
            }
        }
        let had_action = self.action.take().is_some();
        self.error = None;
        if had_action {
            cx.emit(FolderEvent::OverlayChanged(false));
        }
        if self.filter != filter {
            self.filter = filter;
            cx.emit(FolderEvent::Filter(self.filter.clone()));
        }
        cx.notify();
    }

    /// Note clicks already choose an exact note in the host. Align destination
    /// context silently: emitting Filter here would asynchronously replace that
    /// exact choice with the first note in the folder.
    pub fn align_note_context(&mut self, note_id: &str, cx: &mut Context<Self>) -> bool {
        let Some(note) = self
            .snapshot
            .as_ref()
            .and_then(|s| s.notes.iter().find(|n| n.id == note_id))
        else {
            return false;
        };
        self.filter = match &note.folder_id {
            Some(id) if self.rows.iter().any(|f| &f.id == id) => FolderFilter::Folder(id.clone()),
            Some(_) => FolderFilter::All,
            None => FolderFilter::Root,
        };
        cx.notify();
        true
    }

    pub fn set_busy(&mut self, busy: bool, cx: &mut Context<Self>) {
        if self.busy != busy {
            self.busy = busy;
            cx.notify();
        }
    }

    #[cfg(test)]
    pub fn accept(&mut self, snapshot: Arc<Snapshot>, window: &mut Window, cx: &mut Context<Self>) {
        let index = if self.index.needs_update(&snapshot) {
            Arc::new(studyapp_desktop::library_tree::LibraryIndex::build(
                &snapshot,
            ))
        } else {
            self.index.clone()
        };
        self.accept_with_index(snapshot, index, window, cx);
    }

    pub fn accept_with_index(
        &mut self,
        snapshot: Arc<Snapshot>,
        index: Arc<studyapp_desktop::library_tree::LibraryIndex>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let index_changed = !Arc::ptr_eq(&self.index, &index);
        if index_changed {
            self.search_query = None;
            self.visible_cache = None;
            self.index = index;
            self.rows = folder_rows_sorted(&snapshot.folders, self.row_sort, &self.index);
        }
        let had_overlay = self.overlay_open();
        let library_changed = self
            .snapshot
            .as_ref()
            .is_some_and(|old| old.data_path != snapshot.data_path);
        let folders_changed = index_changed
            && (library_changed
                || self.snapshot.as_ref().is_none_or(|old| {
                    old.folders.len() != snapshot.folders.len()
                        || old.folders.iter().zip(&snapshot.folders).any(|(a, b)| {
                            a.id != b.id || a.name != b.name || a.parent_id != b.parent_id
                        })
                }));
        if folders_changed {
            self.collapsed
                .retain(|id| snapshot.folders.iter().any(|folder| &folder.id == id));
        }
        if index_changed {
            self.counts.clear();
            self.all_count = 0;
            self.root_count = 0;
            for note in snapshot.notes.iter().filter(|note| !note.detached) {
                self.all_count += 1;
                if let Some(folder_id) = &note.folder_id {
                    *self.counts.entry(folder_id.clone()).or_default() += 1;
                } else {
                    self.root_count += 1;
                }
            }
        }
        if library_changed {
            self.collapsed.clear();
            self.draft_destinations = false;
            self.draft_return_scroll = None;
            self.flat_cache = None;
            self.tree_scroll.reset(0);
            self.action = None;
            self.active_menu = None;
            cx.emit(FolderEvent::MenuRegions(vec![]));
            self.error = None;
            self.name
                .update(cx, |input, cx| input.set_value("", window, cx));
        }
        if self.action.as_ref().is_some_and(|action| {
            let folder_id = match action {
                FolderAction::Rename { folder_id }
                | FolderAction::MoveFolder { folder_id, .. }
                | FolderAction::Delete { folder_id } => Some(folder_id),
                FolderAction::Create { parent_id } => parent_id.as_ref(),
                FolderAction::RenameNote { action } => {
                    return action.resolve(&snapshot, false).is_err();
                }
                FolderAction::MoveNote { note_id, .. } => {
                    return !snapshot.notes.iter().any(|note| &note.id == note_id);
                }
            };
            folder_id.is_some_and(|id| !snapshot.folders.iter().any(|folder| &folder.id == id))
        }) {
            self.action = None;
            self.error = None;
        }
        self.snapshot = Some(snapshot);
        if had_overlay && !self.overlay_open() {
            cx.emit(FolderEvent::OverlayChanged(false));
        }
        if library_changed
            || self
                .filter
                .folder_id()
                .is_some_and(|id| !self.rows.iter().any(|row| row.id == id))
        {
            self.filter = FolderFilter::All;
            // A removed folder is background reconciliation, not a user
            // navigation request: parking a draft here would close its editor.
            if library_changed {
                cx.emit(FolderEvent::Filter(self.filter.clone()));
            }
        }
        cx.notify();
    }

    pub fn begin_move_note(&mut self, note_id: String, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if let Some(note) = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.notes.iter().find(|note| note.id == note_id))
        {
            self.action = Some(FolderAction::MoveNote {
                note_id,
                destination: note.folder_id.clone(),
            });
            self.opened_action(cx);
        }
    }

    pub fn begin_rename_note(
        &mut self,
        action: NoteAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        if !matches!(action.kind, NoteActionKind::Rename { .. }) {
            return;
        }
        let Ok(note) = action.resolve(snapshot, self.busy) else {
            return;
        };
        let title = note.title.clone();
        self.edit_name(FolderAction::RenameNote { action }, title, window, cx);
    }

    fn edit_name(
        &mut self,
        action: FolderAction,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }
        self.action = Some(action);
        self.name
            .update(cx, |input, cx| input.set_value(name, window, cx));
        self.opened_action(cx);
    }

    fn folder_name(&self, id: &str) -> String {
        self.rows
            .iter()
            .find(|row| row.id == id)
            .map(|row| row.name.clone())
            .unwrap_or_else(|| "目录已移除".into())
    }

    fn destination_name(&self, id: Option<&str>) -> String {
        id.and_then(|id| self.rows.iter().find(|row| row.id == id))
            .map(|row| row.path.clone())
            .unwrap_or_else(|| "未分类 / 根目录".into())
    }

    fn choose_destination(&mut self, destination: Option<String>, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        match &mut self.action {
            Some(FolderAction::MoveFolder {
                destination: target,
                ..
            })
            | Some(FolderAction::MoveNote {
                destination: target,
                ..
            }) => *target = destination,
            _ => return,
        }
        self.error = None;
        cx.notify();
    }

    fn can_drop_note(&self, drag: &NoteDrag, destination: Option<String>) -> bool {
        !self.busy
            && !self.overlay_open()
            && self.snapshot.as_ref().is_some_and(|snapshot| {
                drag.library_path == snapshot.data_path
                    && destination
                        .as_deref()
                        .is_none_or(|id| self.index.folder_exists(id))
                    && self
                        .index
                        .note(&drag.note_id)
                        .and_then(|metadata| snapshot.notes.get(metadata.position))
                        .is_some_and(|note| {
                            note.id == drag.note_id && note.folder_id != destination
                        })
            })
    }

    pub fn set_draft_destinations(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if enabled && !self.draft_destinations && self.filter == FolderFilter::Drafts {
            self.draft_return_scroll = Some(self.tree_scroll.logical_scroll_top());
        }
        self.draft_destinations = enabled;
        cx.notify();
    }

    fn can_drop_draft(&self, drag: &DraftDrag, destination: Option<&str>) -> bool {
        !self.busy
            && !self.overlay_open()
            && self
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.data_path == drag.library_path)
            && destination != drag.folder_id.as_deref()
            && destination.is_none_or(|id| self.index.folder_exists(id))
    }

    fn drop_note(&mut self, drag: &NoteDrag, destination: Option<String>, cx: &mut Context<Self>) {
        if !self.can_drop_note(drag, destination.clone()) {
            return;
        }
        cx.emit(FolderEvent::Command(Box::new(Command::MoveNote {
            note_id: drag.note_id.clone(),
            folder_id: destination,
        })));
        cx.notify();
    }

    fn can_drop_folder(&self, drag: &FolderDrag, destination: Option<&str>) -> bool {
        self.snapshot.as_ref().is_some_and(|snapshot| {
            !self.busy
                && !self.overlay_open()
                && drag.library_path == snapshot.data_path
                && self.index.can_move_folder(&drag.folder_id, destination)
        })
    }

    fn drop_folder(
        &mut self,
        drag: &FolderDrag,
        destination: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if !self.can_drop_folder(drag, destination.as_deref()) {
            return;
        }
        cx.emit(FolderEvent::Command(Box::new(Command::MoveFolder {
            folder_id: drag.folder_id.clone(),
            parent_id: destination,
        })));
        cx.notify();
    }

    fn folder_drop_target(
        &self,
        id: SharedString,
        destination: Option<String>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let hover_panel = cx.entity().downgrade();
        let hover_destination = destination.clone();
        let check_panel = cx.entity().downgrade();
        let check_destination = destination.clone();
        let folder_hover_panel = cx.weak_entity();
        let folder_hover_destination = destination.clone();
        let folder_destination = destination.clone();
        let draft_hover_panel = cx.weak_entity();
        let draft_hover_destination = destination.clone();
        let draft_destination = destination.clone();
        div()
            .id(id)
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .rounded_md()
            .border_1()
            .border_color(rgba(0))
            .drag_over::<NoteDrag>(move |style, drag, _, cx| {
                if hover_panel.upgrade().is_some_and(|panel| {
                    panel
                        .read(cx)
                        .can_drop_note(drag, hover_destination.clone())
                }) {
                    style
                        .bg(crate::theme::color(LIGHT))
                        .border_color(crate::theme::color(GREEN))
                } else {
                    style
                }
            })
            .drag_over::<FolderDrag>(move |style, drag, _, cx| {
                if folder_hover_panel.upgrade().is_some_and(|panel| {
                    panel
                        .read(cx)
                        .can_drop_folder(drag, folder_hover_destination.as_deref())
                }) {
                    style
                        .bg(crate::theme::color(LIGHT))
                        .border_color(crate::theme::color(GREEN))
                } else {
                    style
                }
            })
            .drag_over::<DraftDrag>(move |style, drag, _, cx| {
                if draft_hover_panel.upgrade().is_some_and(|panel| {
                    panel
                        .read(cx)
                        .can_drop_draft(drag, draft_hover_destination.as_deref())
                }) {
                    style
                        .bg(crate::theme::color(LIGHT))
                        .border_color(crate::theme::color(GREEN))
                } else {
                    style
                }
            })
            .can_drop(move |value, _, cx| {
                check_panel.upgrade().is_some_and(|panel| {
                    let panel = panel.read(cx);
                    if let Some(drag) = value.downcast_ref::<NoteDrag>() {
                        panel.can_drop_note(drag, check_destination.clone())
                    } else if let Some(drag) = value.downcast_ref::<FolderDrag>() {
                        panel.can_drop_folder(drag, check_destination.as_deref())
                    } else if let Some(drag) = value.downcast_ref::<DraftDrag>() {
                        panel.can_drop_draft(drag, check_destination.as_deref())
                    } else {
                        false
                    }
                })
            })
            .on_drop(cx.listener(move |this, drag: &NoteDrag, _, cx| {
                this.drop_note(drag, destination.clone(), cx);
            }))
            .on_drop(cx.listener(move |this, drag: &FolderDrag, _, cx| {
                this.drop_folder(drag, folder_destination.clone(), cx);
            }))
            .on_drop(cx.listener(move |this, drag: &DraftDrag, _, cx| {
                if this.can_drop_draft(drag, draft_destination.as_deref()) {
                    cx.emit(FolderEvent::MoveDraft {
                        library_path: drag.library_path.clone(),
                        draft_id: drag.draft_id.clone(),
                        folder_id: draft_destination.clone(),
                    });
                }
            }))
    }

    fn begin_menu_action(
        &mut self,
        library: &str,
        id: &str,
        action: FolderMenuAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy || self.action.is_some() {
            return;
        }
        let Some(snapshot) = self
            .snapshot
            .as_ref()
            .filter(|snapshot| snapshot.data_path == library)
        else {
            return;
        };
        let Some(folder) = snapshot
            .folders
            .iter()
            .find(|folder| folder.id == id)
            .cloned()
        else {
            return;
        };
        match action {
            FolderMenuAction::Create => {
                if self
                    .rows
                    .iter()
                    .any(|row| row.id == id && row.depth + 1 < MAX_FOLDER_DEPTH)
                {
                    self.edit_name(
                        FolderAction::Create {
                            parent_id: Some(id.to_owned()),
                        },
                        String::new(),
                        window,
                        cx,
                    );
                }
            }
            FolderMenuAction::Rename => self.edit_name(
                FolderAction::Rename {
                    folder_id: id.to_owned(),
                },
                folder.name,
                window,
                cx,
            ),
            FolderMenuAction::Move => {
                self.action = Some(FolderAction::MoveFolder {
                    folder_id: id.to_owned(),
                    destination: folder.parent_id,
                });
                self.opened_action(cx);
            }
            FolderMenuAction::Delete if empty_folder(snapshot, id) => {
                self.action = Some(FolderAction::Delete {
                    folder_id: id.to_owned(),
                });
                self.opened_action(cx);
            }
            FolderMenuAction::Delete => {}
        }
    }

    fn close_menu(&mut self, token: u64, cx: &mut Context<Self>) {
        if self.active_menu == Some(token) {
            self.active_menu = None;
            cx.emit(FolderEvent::MenuRegions(vec![]));
            cx.emit(FolderEvent::OverlayChanged(self.overlay_open()));
            cx.notify();
        }
    }

    fn row_menu(
        owner: WeakEntity<Self>,
        library: String,
        id: String,
        mut menu: PopupMenu,
        cx: &mut Context<PopupMenu>,
    ) -> PopupMenu {
        let opened = owner.update(cx, |this, cx| {
            let snapshot = this
                .snapshot
                .as_ref()
                .filter(|snapshot| snapshot.data_path == library)?;
            let row = this.rows.iter().find(|row| row.id == id)?;
            if this.busy || this.action.is_some() {
                return None;
            }
            let can_create = row.depth + 1 < MAX_FOLDER_DEPTH;
            let can_delete = empty_folder(snapshot, &id);
            this.menu_sequence = this.menu_sequence.wrapping_add(1);
            let token = this.menu_sequence;
            this.active_menu = Some(token);
            cx.emit(FolderEvent::OverlayChanged(true));
            cx.notify();
            Some((token, can_create, can_delete))
        });
        let Ok(Some((token, can_create, can_delete))) = opened else {
            return menu;
        };
        let layout_owner = owner.clone();
        cx.subscribe_self(
            move |_, event: &gpui_kit::component::menu::PopupMenuLayout, cx| {
                let _ = layout_owner.update(cx, |this, cx| {
                    if this.active_menu == Some(token) {
                        cx.emit(FolderEvent::MenuRegions(event.0.clone()));
                    }
                });
            },
        )
        .detach();
        let dismissed = owner.clone();
        cx.subscribe_self(move |_, _: &DismissEvent, cx| {
            let _ = dismissed.update(cx, |this, cx| this.close_menu(token, cx));
        })
        .detach();
        let released = owner.clone();
        cx.on_release(move |_, cx| {
            let _ = released.update(cx, |this, cx| this.close_menu(token, cx));
        })
        .detach();
        for (label, action, enabled) in [
            ("新建子目录", FolderMenuAction::Create, can_create),
            ("重命名", FolderMenuAction::Rename, true),
            ("移动目录", FolderMenuAction::Move, true),
            ("删除空目录", FolderMenuAction::Delete, can_delete),
        ] {
            let action_owner = owner.clone();
            let action_library = library.clone();
            let action_id = id.clone();
            menu = menu.item(PopupMenuItem::new(label).disabled(!enabled).on_click(
                move |_, window, cx| {
                    let _ = action_owner.update(cx, |this, cx| {
                        this.begin_menu_action(&action_library, &action_id, action, window, cx)
                    });
                },
            ));
        }
        menu
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let (Some(snapshot), Some(action)) = (&self.snapshot, self.action.clone()) else {
            return;
        };
        let result = (|| -> Result<Command, String> {
            Ok(match action.clone() {
                FolderAction::RenameNote { action } => {
                    action.resolve(snapshot, self.busy).map_err(str::to_owned)?;
                    let NoteActionKind::Rename { expected_revision } = action.kind else {
                        unreachable!()
                    };
                    let raw = self.name.read(cx).value();
                    if raw
                        .chars()
                        .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
                        || raw.trim().is_empty()
                        || raw.trim().chars().count() > 200
                    {
                        return Err("笔记名称须为 1 至 200 个字符，不能包含换行或控制字符。".into());
                    }
                    Command::RenameNote {
                        note_id: action.note_id,
                        title: raw.trim().into(),
                        expected_revision,
                    }
                }
                FolderAction::Create { parent_id } => {
                    let name = validated_name(
                        self.name.read(cx).value().as_ref(),
                        &snapshot.folders,
                        parent_id.as_deref(),
                        None,
                    )?;
                    Command::CreateFolder { name, parent_id }
                }
                FolderAction::Rename { folder_id } => {
                    let folder = snapshot
                        .folders
                        .iter()
                        .find(|folder| folder.id == folder_id)
                        .ok_or("目录已移除，请重新选择。")?;
                    let name = validated_name(
                        self.name.read(cx).value().as_ref(),
                        &snapshot.folders,
                        folder.parent_id.as_deref(),
                        Some(&folder_id),
                    )?;
                    Command::RenameFolder { folder_id, name }
                }
                FolderAction::MoveFolder {
                    folder_id,
                    destination,
                } => {
                    if !valid_destination(&self.rows, &folder_id, destination.as_deref()) {
                        return Err("该位置无法作为上级目录，请重新选择。".into());
                    }
                    if snapshot
                        .folders
                        .iter()
                        .find(|folder| folder.id == folder_id)
                        .is_none_or(|folder| folder.parent_id == destination)
                    {
                        return Err("目录已在此位置，请选择其他目录。".into());
                    }
                    Command::MoveFolder {
                        folder_id,
                        parent_id: destination,
                    }
                }
                FolderAction::MoveNote {
                    note_id,
                    destination,
                } => {
                    let note = snapshot
                        .notes
                        .iter()
                        .find(|note| note.id == note_id)
                        .ok_or("笔记已移除，请重新选择。")?;
                    NoteDrag::new(note, snapshot)
                        .move_action(destination.clone())
                        .resolve(snapshot, self.busy)
                        .map_err(str::to_owned)?;
                    Command::MoveNote {
                        note_id,
                        folder_id: destination,
                    }
                }
                FolderAction::Delete { folder_id } => {
                    if !empty_folder(snapshot, &folder_id) {
                        return Err("此目录仍有笔记或子目录，请先将它们移出。".into());
                    }
                    Command::DeleteFolder { folder_id }
                }
            })
        })();
        match result {
            Ok(command) => {
                self.action = None;
                self.error = None;
                cx.emit(FolderEvent::OverlayChanged(false));
                if let FolderAction::RenameNote { action } = action {
                    cx.emit(FolderEvent::RenameNote {
                        library_path: action.library_path,
                        command: Box::new(command),
                    });
                } else {
                    cx.emit(FolderEvent::Command(Box::new(command)));
                }
            }
            Err(message) => self.error = Some(message),
        }
        cx.notify();
    }

    fn action_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(action) = &self.action else {
            return div().into_any_element();
        };
        // Root presents this body in its shared modal layer, above every pane.
        let title = match action {
            FolderAction::Create { .. } => "新建目录",
            FolderAction::Rename { .. } => "重命名目录",
            FolderAction::RenameNote { .. } => "重命名笔记",
            FolderAction::MoveFolder { .. } => "移动目录",
            FolderAction::MoveNote { .. } => "移动笔记",
            FolderAction::Delete { .. } => "删除空目录",
        };
        let deleting = matches!(action, FolderAction::Delete { .. });
        let mut form = column()
            .id("folder-action-scroll")
            .test_support()
            .gap_2()
            .min_h_0()
            .min_w_0();
        let mut submit_label = "保存";
        let mut can_submit = !self.busy;
        match action {
            FolderAction::Create { parent_id } => {
                submit_label = "创建目录";
                form = form
                    .child(muted(format!(
                        "新建于：{}",
                        self.destination_name(parent_id.as_deref())
                    )))
                    .child(
                        div()
                            .id("folder-name-input")
                            .test_support()
                            .child(Input::new(&self.name).small().disabled(self.busy)),
                    );
                can_submit &= !self.name.read(cx).value().trim().is_empty();
            }
            FolderAction::RenameNote { .. } => {
                form = form
                    .child(muted("输入笔记显示名称（1 至 200 个字符）"))
                    .child(
                        div()
                            .id("note-rename-input")
                            .test_support()
                            .child(Input::new(&self.name).small().disabled(self.busy)),
                    );
                can_submit &= !self.name.read(cx).value().trim().is_empty();
            }
            FolderAction::Rename { folder_id } => {
                form = form
                    .child(muted(format!("重命名：{}", self.folder_name(folder_id))))
                    .child(
                        div()
                            .id("folder-name-input")
                            .test_support()
                            .child(Input::new(&self.name).small().disabled(self.busy)),
                    );
                can_submit &= !self.name.read(cx).value().trim().is_empty();
            }
            FolderAction::Delete { folder_id } => {
                submit_label = "确认删除空目录";
                form = form
                    .child(muted(format!("删除「{}」？", self.folder_name(folder_id))))
                    .child(muted("仅移除这个空的逻辑目录。"));
                can_submit &= self
                    .snapshot
                    .as_ref()
                    .is_some_and(|snapshot| empty_folder(snapshot, folder_id));
            }
            FolderAction::MoveFolder {
                folder_id,
                destination,
            }
            | FolderAction::MoveNote {
                note_id: folder_id,
                destination,
            } => {
                submit_label = "移到此处";
                let moving_folder = matches!(action, FolderAction::MoveFolder { .. });
                let current = self.snapshot.as_ref().and_then(|snapshot| {
                    if moving_folder {
                        snapshot
                            .folders
                            .iter()
                            .find(|folder| &folder.id == folder_id)
                            .map(|folder| folder.parent_id.as_ref())
                    } else {
                        snapshot
                            .notes
                            .iter()
                            .find(|note| &note.id == folder_id)
                            .map(|note| note.folder_id.as_ref())
                    }
                });
                can_submit &= current.is_some_and(|current| current != destination.as_ref());
                let title = if moving_folder {
                    format!("移动目录：{}", self.folder_name(folder_id))
                } else {
                    self.snapshot
                        .as_ref()
                        .and_then(|snapshot| {
                            snapshot.notes.iter().find(|note| &note.id == folder_id)
                        })
                        .map(|note| format!("移动笔记：{}", note.title))
                        .unwrap_or_else(|| "移动笔记".into())
                };
                let choices = self
                    .snapshot
                    .as_ref()
                    .map(|snapshot| {
                        if moving_folder {
                            folder_destinations(&snapshot.folders, folder_id)
                        } else {
                            note_destinations(
                                &snapshot.folders,
                                current.flatten().map(String::as_str),
                            )
                        }
                    })
                    .unwrap_or_default();
                let mut destinations =
                    scroll_body("folder-destination-scroll", &self.destination_scroll)
                        .test_support()
                        .gap_1()
                        .h(px(132.))
                        .flex_shrink_0()
                        .min_h_0()
                        .overflow_y_scroll();
                for choice in choices {
                    let id = choice.id;
                    let element_id = id
                        .as_deref()
                        .map(|id| format!("folder-destination-{id}"))
                        .unwrap_or_else(|| "folder-destination-root".into());
                    let tooltip_id = SharedString::from(format!("{element_id}-tip"));
                    let tip = choice.path.clone();
                    destinations = destinations.child(
                        div()
                            .id(tooltip_id)
                            .w_full()
                            .tooltip(move |window, cx| sidebar_tooltip(tip.clone(), window, cx))
                            .child(
                                Button::new(SharedString::from(element_id))
                                    .small()
                                    .ghost()
                                    .w_full()
                                    .label(choice.path.clone())
                                    .selected(destination == &id)
                                    .disabled(self.busy)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.choose_destination(id.clone(), cx)
                                    })),
                            ),
                    );
                }
                form = form
                    .child(muted(title).truncate().flex_shrink_0())
                    .child(destinations)
                    .child(muted(if moving_folder {
                        "仅调整逻辑目录层级，源文件位置保持不变。"
                    } else {
                        "只改变笔记分类，源文件位置保持不变。"
                    }));
            }
        }
        let body =
            form.children(self.error.as_ref().map(|message| {
                muted(message.clone()).text_color(rgb(crate::theme::palette().danger))
            }));
        studyapp_ui::modal_frame::ModalFrame::new(title, body)
            .tone(if deleting {
                studyapp_ui::modal_frame::ModalTone::Danger
            } else {
                studyapp_ui::modal_frame::ModalTone::Neutral
            })
            .width(studyapp_ui::modal_frame::ModalWidth::Regular)
            .scroll_handle(&self.action_scroll)
            .footer(
                row()
                    .gap_1()
                    .child(
                        Button::new("folder-cancel")
                            .small()
                            .ghost()
                            .label("取消")
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.cancel_action(cx);
                            })),
                    )
                    .child(
                        Button::new("folder-submit")
                            .small()
                            .when(deleting, |button| button.danger())
                            .when(!deleting, |button| button.primary())
                            .label(submit_label)
                            .disabled(!can_submit)
                            .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                    ),
            )
            .into_any_element()
    }
}

impl FolderPanel {
    fn prepare_library(
        &mut self,
        query: &str,
        sort: studyapp_core::LibrarySort,
        selected_note: Option<&str>,
    ) {
        if self.search_query.as_deref() != Some(query) {
            self.search_folders = if query.is_empty() {
                HashSet::new()
            } else {
                self.index.matching_folders(query)
            };
            self.search_query = Some(query.to_owned());
        }
        if let Some(snapshot) = &self.snapshot {
            if self.row_sort != sort {
                self.row_sort = sort;
                self.rows = folder_rows_sorted(&snapshot.folders, sort, &self.index);
            }
            if selected_note != self.revealed_note.as_deref() {
                if let Some(note) = snapshot
                    .notes
                    .iter()
                    .find(|n| Some(n.id.as_str()) == selected_note)
                    && let Some(folder) = self
                        .rows
                        .iter()
                        .find(|f| Some(&f.id) == note.folder_id.as_ref())
                {
                    self.collapsed.remove(&folder.id);
                    for ancestor in &folder.ancestors {
                        self.collapsed.remove(ancestor);
                    }
                }
                self.revealed_note = selected_note.map(str::to_owned);
            }
        }
    }

    pub fn visible_notes(
        &mut self,
        query: &str,
        sort: studyapp_core::LibrarySort,
        selected_note: Option<&str>,
        detached: bool,
    ) -> Arc<Vec<usize>> {
        self.prepare_library(query, sort, selected_note);
        if self.filter == FolderFilter::Drafts && !self.draft_destinations {
            return Arc::new(Vec::new());
        }
        let Some(snapshot) = &self.snapshot else {
            return Arc::new(Vec::new());
        };
        if let Some(cache) = &self.visible_cache
            && cache.query == query
            && cache.sort == sort
            && cache.detached == detached
            && cache.collapsed == self.collapsed
        {
            return cache.positions.clone();
        }
        let mut count = 0;
        let positions = Arc::new(
            self.index
                .ordered_notes(sort)
                .iter()
                .copied()
                .filter(|&position| {
                    let note = &snapshot.notes[position];
                    if note.detached != detached || !self.index.matches(&note.id, query) {
                        return false;
                    }
                    count += 1;
                    self.index.note_visible(&note.id, &self.collapsed, query)
                })
                .collect(),
        );
        self.visible_cache = Some(VisibleLibrary {
            query: query.to_owned(),
            sort,
            detached,
            collapsed: self.collapsed.clone(),
            positions: Arc::clone(&positions),
            count,
        });
        positions
    }

    pub fn matching_note_count(&self) -> usize {
        self.visible_cache.as_ref().map_or(0, |cache| cache.count)
    }

    pub fn prepare_tree(
        &mut self,
        query: &str,
        sort: studyapp_core::LibrarySort,
        selected_note: Option<&str>,
        detached: bool,
        draft_count: usize,
        draft_ids: Arc<Vec<String>>,
    ) -> Arc<Vec<LibraryRow>> {
        self.draft_count = draft_count;
        let visible = self.visible_notes(query, sort, selected_note, detached);
        let drafts_only = self.filter == FolderFilter::Drafts && !self.draft_destinations;
        if let Some(cache) = &self.flat_cache
            && cache.drafts_only == drafts_only
            && ((drafts_only && Arc::ptr_eq(&cache.draft_ids, &draft_ids))
                || (!drafts_only && Arc::ptr_eq(&cache.visible, &visible)))
        {
            return cache.rows.clone();
        }
        let top = self.tree_scroll.logical_scroll_top();
        let previous = self
            .flat_cache
            .as_ref()
            .filter(|cache| cache.drafts_only == drafts_only);
        let mut rows = vec![
            LibraryRow::System(0),
            LibraryRow::System(1),
            LibraryRow::System(2),
        ];
        if drafts_only {
            rows.extend(draft_ids.iter().cloned().map(LibraryRow::Draft));
        } else if let Some(snapshot) = &self.snapshot {
            let folder_ids: HashSet<_> =
                self.rows.iter().map(|folder| folder.id.as_str()).collect();
            let mut grouped: HashMap<Option<&str>, Vec<usize>> = HashMap::new();
            for &position in visible.iter() {
                let note = &snapshot.notes[position];
                let folder = note
                    .folder_id
                    .as_deref()
                    .filter(|id| folder_ids.contains(id));
                grouped.entry(folder).or_default().push(position);
            }
            let note_row = |position: usize, depth| LibraryRow::Note {
                id: snapshot.notes[position].id.clone(),
                position,
                depth,
            };
            rows.extend(
                grouped
                    .remove(&None)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|position| note_row(position, 0)),
            );
            for (position, folder) in
                self.rows.iter().enumerate().filter(|(_, folder)| {
                    folder_is_visible(folder, &self.collapsed, !query.is_empty())
                })
            {
                rows.push(LibraryRow::Folder {
                    id: folder.id.clone(),
                    position,
                });
                if folder_is_expanded(
                    &folder.id,
                    &self.collapsed,
                    &self.search_folders,
                    !query.is_empty(),
                ) || !query.is_empty()
                {
                    rows.extend(
                        grouped
                            .remove(&Some(folder.id.as_str()))
                            .unwrap_or_default()
                            .into_iter()
                            .map(|position| note_row(position, folder.depth + 1)),
                    );
                }
            }
        }
        if rows.len() == 3 {
            rows.push(LibraryRow::Empty(drafts_only));
        }
        let next_anchor = previous.and_then(|old| relocate_anchor(&old.rows, &rows, top.item_ix));
        self.tree_scroll
            .reset_with_uniform_height(rows.len(), px(LIBRARY_ROW_HEIGHT));
        if let Some((item_ix, exact)) = next_anchor {
            self.tree_scroll.scroll_to(ListOffset {
                item_ix,
                offset_in_item: if exact { top.offset_in_item } else { px(0.) },
            });
        } else if drafts_only && let Some(origin) = self.draft_return_scroll.take() {
            self.tree_scroll.scroll_to(ListOffset {
                item_ix: origin.item_ix.min(rows.len() - 1),
                ..origin
            });
        }
        let rows = Arc::new(rows);
        self.flat_cache = Some(FlatLibrary {
            visible,
            drafts_only,
            draft_ids,
            rows: rows.clone(),
        });
        rows
    }

    pub fn tree_view(
        &self,
        render: impl FnMut(usize, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Stateful<Div> {
        column()
            .id("note-folders")
            .flex_1()
            .gap_0()
            .h_full()
            .min_h_0()
            .min_w_0()
            .child(
                div()
                    .id("note-list-scroll")
                    .test_support()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .px_2()
                    .pr(px(18.))
                    .overflow_hidden()
                    .child(list(self.tree_scroll.clone(), render).size_full())
                    .child(studyapp_ui::virtual_list_scrollbar::virtual_list_scrollbar(
                        "note-list-scrollbar",
                        &self.tree_scroll,
                        px(18.),
                    )),
            )
    }

    pub fn render_tree_row(&self, item: &LibraryRow, cx: &mut Context<Self>) -> AnyElement {
        match item {
            LibraryRow::Folder { position, .. } => self.folder_row(*position, cx),
            LibraryRow::System(index) => self.system_row(*index, cx),
            LibraryRow::Empty(drafts) => muted(if *drafts {
                "还没有草稿"
            } else {
                "使用 ＋ 新建或导入笔记"
            })
            .text_xs()
            .px_2()
            .h(px(LIBRARY_ROW_HEIGHT))
            .into_any_element(),
            _ => div().h(px(LIBRARY_ROW_HEIGHT)).into_any_element(),
        }
    }

    fn system_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let (id, label, icon, count, filter) = match index {
            0 => (
                "folders-all",
                "全部笔记",
                IconName::BookOpen,
                self.all_count,
                FolderFilter::All,
            ),
            1 => (
                "folders-root",
                "未分类",
                IconName::Inbox,
                self.root_count,
                FolderFilter::Root,
            ),
            _ => (
                "folders-drafts",
                "草稿箱",
                IconName::FileText,
                self.draft_count,
                FolderFilter::Drafts,
            ),
        };
        let chosen = self.filter == filter;
        let entry = library_interaction(library_entry().id(id), chosen, self.busy)
            .test_support()
            .aria_selected(chosen)
            .w_full()
            .child(div().w(px(12.)).flex_shrink_0())
            .child(
                Icon::new(icon)
                    .small()
                    .text_color(crate::theme::color(MUTED)),
            )
            .child(div().flex_1().min_w_0().truncate().child(label))
            .child(muted(count.to_string()).text_xs())
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.busy {
                    this.set_filter(filter.clone(), cx);
                }
            }));
        if index == 1 {
            self.folder_drop_target("folder-drop-root".into(), None, cx)
                .border_0()
                .child(entry)
                .into_any_element()
        } else {
            entry.into_any_element()
        }
    }

    #[cfg(test)]
    pub fn library_tree(
        &mut self,
        _: HashMap<Option<String>, Vec<AnyElement>>,
        query: &str,
        sort: studyapp_core::LibrarySort,
        selected_note: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let rows = self.prepare_tree(
            query,
            sort,
            selected_note,
            false,
            self.draft_count,
            Arc::default(),
        );
        let owner = cx.entity();
        self.tree_view(move |index, _, cx| {
            owner.update(cx, |panel, cx| panel.render_tree_row(&rows[index], cx))
        })
    }

    fn folder_row(&self, position: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(folder) = self.rows.get(position) else {
            return div().into_any_element();
        };
        #[cfg(test)]
        FOLDER_ROW_RENDERS.with(|rows| {
            rows.borrow_mut().insert(folder.id.clone());
        });
        let id = folder.id.clone();
        let expanded = folder_is_expanded(
            &id,
            &self.collapsed,
            &self.search_folders,
            self.search_query
                .as_deref()
                .is_some_and(|query| !query.is_empty()),
        );
        let count = self.counts.get(&id).copied().unwrap_or(0);
        let chosen = self.filter.folder_id() == Some(id.as_str());
        let library = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.data_path.clone())
            .unwrap_or_default();
        let drag = FolderDrag {
            folder_id: id.clone(),
            library_path: library.clone(),
            title: folder.name.clone(),
        };
        let menu_owner = cx.weak_entity();
        let menu_id = id.clone();
        let menu_library = library.clone();
        let keyboard_owner = cx.weak_entity();
        let keyboard_id = id.clone();
        let mut entry = row().gap_0().min_w_0().flex_shrink_0().children(
            (0..folder.depth.min(MAX_FOLDER_DEPTH)).map(|_| {
                div()
                    .w(px(14.))
                    .h(px(32.))
                    .flex_shrink_0()
                    .border_l_1()
                    .border_color(crate::theme::color(BORDER))
            }),
        );
        if folder.has_children || self.index.folder_has_notes(&id) {
            let collapse_id = id.clone();
            entry = entry.child(
                Button::new(SharedString::from(format!("folder-expand-{id}")))
                    .small()
                    .ghost()
                    .w(px(20.))
                    .flex_shrink_0()
                    .px_0()
                    .icon(if expanded {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .disabled(self.busy)
                    .accessibility_label(format!(
                        "{}{}",
                        if expanded { "折叠" } else { "展开" },
                        folder.name
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        if !this.collapsed.remove(&collapse_id) {
                            this.collapsed.insert(collapse_id.clone());
                        }
                        cx.notify();
                    })),
            );
        } else {
            entry = entry.child(div().w(px(20.)).flex_shrink_0());
        }
        let tip = format!("{} · 直接包含 {count} 份笔记", folder.path);

        self.folder_drop_target(format!("folder-drop-{id}").into(), Some(id.clone()), cx)
            .h(px(LIBRARY_ROW_HEIGHT))
            .px_2()
            .test_support()
            .debug_selector({
                let selector = format!("folder-drop-{id}");
                move || selector
            })
            .aria_selected(chosen)
            .when(chosen, |style| style.bg(crate::theme::selection_color()))
            .tooltip(move |window, cx| sidebar_tooltip(tip.clone(), window, cx))
            .child(
                entry
                    .child(
                        Button::new(SharedString::from(format!("folder-row-{id}")))
                            .small()
                            .h(px(LIBRARY_ROW_HEIGHT - 2.))
                            .ghost()
                            .flex_1()
                            .min_w_0()
                            .px_0()
                            .accessibility_label(format!("{}，{count} 份笔记", folder.name))
                            .child(
                                row()
                                    .w_full()
                                    .min_w_0()
                                    .gap_2()
                                    .child(
                                        Icon::new(if expanded {
                                            IconName::FolderOpen
                                        } else {
                                            IconName::FolderClosed
                                        })
                                        .small()
                                        .text_color(crate::theme::color(MUTED)),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_left()
                                            .child(folder.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .flex_shrink_0()
                                            .text_xs()
                                            .text_color(crate::theme::color(MUTED))
                                            .child(count.to_string()),
                                    ),
                            )
                            .when(!self.busy && self.action.is_none(), |button| {
                                button.on_drag(drag, |drag, source_offset, _, cx| {
                                    cx.new(|_| FolderDragPreview {
                                        title: drag.title.clone(),
                                        source_offset,
                                    })
                                })
                            })
                            .disabled(self.busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_filter(FolderFilter::Folder(id.clone()), cx)
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("folder-menu-{keyboard_id}")))
                            .small()
                            .ghost()
                            .icon(IconName::Ellipsis)
                            .w(px(18.))
                            .px_0()
                            .flex_shrink_0()
                            .accessibility_label(format!("{}的目录操作", folder.name))
                            .tooltip("目录操作（也可右键目录）")
                            .disabled(self.busy)
                            .dropdown_menu(move |menu, _, cx| {
                                Self::row_menu(
                                    keyboard_owner.clone(),
                                    library.clone(),
                                    keyboard_id.clone(),
                                    menu,
                                    cx,
                                )
                            }),
                    ),
            )
            .context_menu(move |menu, _, cx| {
                Self::row_menu(
                    menu_owner.clone(),
                    menu_library.clone(),
                    menu_id.clone(),
                    menu,
                    cx,
                )
            })
            .into_any_element()
    }
}

impl Render for FolderPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.prepare_tree(
            "",
            Default::default(),
            None,
            false,
            self.draft_count,
            Arc::default(),
        );
        let owner = cx.entity();
        self.tree_view(move |index, _, cx| {
            owner.update(cx, |panel, cx| panel.render_tree_row(&rows[index], cx))
        })
    }
}

/// Search narrows notes, while all folder destinations stay available for
/// organization. User collapse state is restored as soon as search is cleared.
fn folder_is_visible(folder: &FolderRow, collapsed: &HashSet<String>, searching: bool) -> bool {
    searching || !folder.ancestors.iter().any(|id| collapsed.contains(id))
}

fn folder_is_expanded(
    id: &str,
    collapsed: &HashSet<String>,
    matching: &HashSet<String>,
    searching: bool,
) -> bool {
    !collapsed.contains(id) || (searching && matching.contains(id))
}

fn empty_folder(snapshot: &Snapshot, id: &str) -> bool {
    snapshot.folders.iter().any(|folder| folder.id == id)
        && !snapshot
            .folders
            .iter()
            .any(|folder| folder.parent_id.as_deref() == Some(id))
        && !snapshot
            .notes
            .iter()
            .any(|note| note.folder_id.as_deref() == Some(id))
}

fn validated_name(
    raw: &str,
    folders: &[NoteFolder],
    parent: Option<&str>,
    except: Option<&str>,
) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() || name.chars().count() > 200 {
        return Err("请输入 1–200 个字符的目录名称。".into());
    }
    if matches!(name, "." | "..")
        || name
            .chars()
            .any(|c| c == '/' || c == '\\' || c.is_control())
    {
        return Err("目录名称不能含路径分隔符或控制字符。".into());
    }
    if folders.iter().any(|folder| {
        Some(folder.id.as_str()) != except
            && folder.parent_id.as_deref() == parent
            && folder.name.to_lowercase() == name.to_lowercase()
    }) {
        return Err("同一上级目录中已有这个名称。".into());
    }
    Ok(name.to_owned())
}

fn folder_rows(folders: &[NoteFolder]) -> Vec<FolderRow> {
    folder_rows_sorted(
        folders,
        studyapp_core::LibrarySort::default(),
        &Default::default(),
    )
}
fn folder_rows_sorted(
    folders: &[NoteFolder],
    sort: studyapp_core::LibrarySort,
    index: &studyapp_desktop::library_tree::LibraryIndex,
) -> Vec<FolderRow> {
    #[cfg(test)]
    FOLDER_ROWS_BUILDS.with(|count| count.set(count.get() + 1));
    let ids: HashSet<&str> = folders.iter().map(|folder| folder.id.as_str()).collect();
    let mut children: HashMap<Option<&str>, Vec<&NoteFolder>> = HashMap::new();
    for folder in folders {
        let parent = folder.parent_id.as_deref().filter(|id| ids.contains(id));
        children.entry(parent).or_default().push(folder);
    }
    for siblings in children.values_mut() {
        siblings.sort_by(|a, b| {
            studyapp_desktop::library_tree::compare_entries(
                sort,
                (&a.name, index.folder_updated(&a.id), &a.id),
                (&b.name, index.folder_updated(&b.id), &b.id),
            )
        });
    }
    let mut rows = Vec::with_capacity(folders.len());
    let mut seen = HashSet::new();
    let mut stack: Vec<(&NoteFolder, Vec<String>, String)> = children
        .get(&None)
        .into_iter()
        .flatten()
        .rev()
        .map(|folder| (*folder, Vec::new(), String::new()))
        .collect();
    // Core rejects cycles. The fallback also keeps a corrupt/stale tree finite
    // and visible so selection and rendering cannot recurse forever.
    let mut remaining = folders.iter();
    loop {
        if stack.is_empty() {
            if let Some(folder) = remaining.find(|folder| !seen.contains(&folder.id)) {
                stack.push((folder, Vec::new(), String::new()));
            } else {
                break;
            }
        }
        let (folder, ancestors, parent_path) = stack.pop().unwrap();
        if !seen.insert(folder.id.clone()) {
            continue;
        }
        let path = if parent_path.is_empty() {
            folder.name.clone()
        } else {
            format!("{parent_path} / {}", folder.name)
        };
        let descendants = children.get(&Some(folder.id.as_str()));
        rows.push(FolderRow {
            id: folder.id.clone(),
            name: folder.name.clone(),
            path: path.clone(),
            depth: ancestors.len(),
            ancestors: ancestors.clone(),
            has_children: descendants.is_some_and(|children| !children.is_empty()),
        });
        if let Some(descendants) = descendants {
            let mut next_ancestors = ancestors;
            next_ancestors.push(folder.id.clone());
            for child in descendants.iter().rev() {
                stack.push((child, next_ancestors.clone(), path.clone()));
            }
        }
    }
    rows
}

#[cfg(test)]
thread_local! {
    static FOLDER_ROWS_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static FOLDER_ROW_RENDERS: std::cell::RefCell<std::collections::BTreeSet<String>> = const { std::cell::RefCell::new(std::collections::BTreeSet::new()) };
}

fn moving_subtree_height(rows: &[FolderRow], moving: &str) -> Option<usize> {
    let source = rows.iter().find(|row| row.id == moving)?;
    rows.iter()
        .filter(|row| row.id == moving || row.ancestors.iter().any(|id| id == moving))
        .map(|row| row.depth.saturating_sub(source.depth) + 1)
        .max()
}

fn accepts_destination(row: &FolderRow, moving: &str, subtree_height: usize) -> bool {
    row.id != moving
        && !row.ancestors.iter().any(|ancestor| ancestor == moving)
        && row.depth + 1 + subtree_height <= MAX_FOLDER_DEPTH
}

fn valid_destination(rows: &[FolderRow], moving: &str, destination: Option<&str>) -> bool {
    let Some(subtree_height) = moving_subtree_height(rows, moving) else {
        return false;
    };
    match destination {
        None => subtree_height <= MAX_FOLDER_DEPTH,
        Some(id) => rows
            .iter()
            .find(|row| row.id == id)
            .is_some_and(|row| accepts_destination(row, moving, subtree_height)),
    }
}

#[cfg(test)]
fn valid_folder_drop(
    drag: &FolderDrag,
    library: &str,
    folders: &[NoteFolder],
    destination: Option<&str>,
    blocked: bool,
) -> bool {
    if blocked || drag.library_path != library {
        return false;
    }
    let Some(source) = folders.iter().find(|folder| folder.id == drag.folder_id) else {
        return false;
    };
    source.parent_id.as_deref() != destination
        && valid_destination(&folder_rows(folders), &drag.folder_id, destination)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderDestination {
    pub id: Option<String>,
    pub path: String,
}

pub fn note_destinations(folders: &[NoteFolder], current: Option<&str>) -> Vec<FolderDestination> {
    let mut destinations = Vec::new();
    if current.is_some() {
        destinations.push(FolderDestination {
            id: None,
            path: "未分类 / 根目录".into(),
        });
    }
    destinations.extend(
        folder_rows(folders)
            .into_iter()
            .filter(|row| Some(row.id.as_str()) != current)
            .map(|row| FolderDestination {
                id: Some(row.id),
                path: row.path,
            }),
    );
    destinations
}

fn folder_destinations(folders: &[NoteFolder], moving: &str) -> Vec<FolderDestination> {
    let Some(source) = folders.iter().find(|folder| folder.id == moving) else {
        return Vec::new();
    };
    let rows = folder_rows(folders);
    note_destinations(folders, source.parent_id.as_deref())
        .into_iter()
        .filter(|destination| valid_destination(&rows, moving, destination.id.as_deref()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(id: &str, name: &str, parent: Option<&str>) -> NoteFolder {
        NoteFolder {
            id: id.into(),
            name: name.into(),
            parent_id: parent.map(str::to_owned),
        }
    }

    #[::core::prelude::v1::test]
    fn search_preserves_empty_drop_destinations_and_temporarily_expands_matching_ancestors() {
        let rows = folder_rows(&[
            folder("matching", "Matching", None),
            folder("matching-child", "Child", Some("matching")),
            folder("destination", "Empty destination", None),
            folder(
                "nested-destination",
                "Nested empty destination",
                Some("destination"),
            ),
        ]);
        let collapsed = ["matching".to_owned(), "destination".to_owned()]
            .into_iter()
            .collect();
        let matching = ["matching".to_owned(), "matching-child".to_owned()]
            .into_iter()
            .collect();
        let visible = rows
            .iter()
            .filter(|row| folder_is_visible(row, &collapsed, true))
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(visible.len(), 4);
        assert!(visible.contains(&"destination"));
        assert!(
            visible.contains(&"nested-destination"),
            "Search keeps every logical drop target reachable"
        );
        assert!(folder_is_expanded("matching", &collapsed, &matching, true));
        assert!(!folder_is_expanded(
            "destination",
            &collapsed,
            &matching,
            true
        ));
        assert!(
            !folder_is_expanded("matching", &collapsed, &matching, false),
            "Clearing search restores user collapse state"
        );
        assert_eq!(
            rows.iter()
                .filter(|row| folder_is_visible(row, &collapsed, false))
                .count(),
            2
        );
    }

    #[::core::prelude::v1::test]
    fn tree_orders_siblings_and_retains_hierarchy() {
        let rows = folder_rows(&[
            folder("b", "Beta", None),
            folder("child", "Child", Some("a")),
            folder("a", "Alpha", None),
        ]);
        assert_eq!(
            rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
            ["a", "child", "b"]
        );
        assert_eq!(rows[1].path, "Alpha / Child");
        assert_eq!(rows[1].ancestors, ["a"]);
        assert!(rows[0].has_children);
    }

    #[::core::prelude::v1::test]
    fn stale_cycles_and_orphans_cannot_loop_or_hide_rows() {
        let rows = folder_rows(&[
            folder("a", "A", Some("b")),
            folder("b", "B", Some("a")),
            folder("c", "C", Some("missing")),
        ]);
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows.iter().map(|row| &row.id).collect::<HashSet<_>>().len(),
            3
        );
    }

    #[::core::prelude::v1::test]
    fn move_choices_exclude_self_descendants_and_depth_overflow() {
        let folders: Vec<_> = (0..8)
            .map(|level| {
                folder(
                    &level.to_string(),
                    &format!("Level {level}"),
                    (level > 0).then(|| (level - 1).to_string()).as_deref(),
                )
            })
            .chain([folder("other", "Other", None)])
            .collect();
        let rows = folder_rows(&folders);
        assert!(!valid_destination(&rows, "0", Some("0")));
        assert!(!valid_destination(&rows, "0", Some("7")));
        assert!(!valid_destination(&rows, "other", Some("7")));
        assert!(valid_destination(&rows, "other", Some("6")));
        assert!(valid_destination(&rows, "0", None));
    }

    #[::core::prelude::v1::test]
    fn note_move_destinations_include_root_and_nested_paths_but_not_current_folder() {
        let folders = [
            folder("parent", "课程", None),
            folder("child", "数学", Some("parent")),
            folder("other", "研究", None),
        ];
        let destinations = note_destinations(&folders, Some("other"));
        assert!(
            destinations
                .iter()
                .any(|d| d.id.is_none() && d.path == "未分类 / 根目录")
        );
        assert!(
            destinations
                .iter()
                .any(|d| d.id.as_deref() == Some("child") && d.path == "课程 / 数学")
        );
        assert!(
            !destinations
                .iter()
                .any(|d| d.id.as_deref() == Some("other"))
        );
        assert_eq!(destinations.len(), 3);
    }

    #[::core::prelude::v1::test]
    fn unclassified_notes_do_not_offer_their_current_destination() {
        let destinations = note_destinations(&[folder("a", "课程", None)], None);
        assert_eq!(
            destinations,
            [FolderDestination {
                id: Some("a".into()),
                path: "课程".into()
            }]
        );
        assert!(note_destinations(&[], None).is_empty());
    }

    #[::core::prelude::v1::test]
    fn folder_picker_excludes_current_parent_self_and_descendants() {
        let folders = [
            folder("parent", "Parent", None),
            folder("moving", "Moving", Some("parent")),
            folder("child", "Child", Some("moving")),
            folder("other", "Other", None),
        ];
        let destinations = folder_destinations(&folders, "moving");
        assert_eq!(
            destinations
                .iter()
                .map(|d| d.id.as_deref())
                .collect::<Vec<_>>(),
            [None, Some("other")]
        );
        assert!(folder_destinations(&folders, "missing").is_empty());
    }

    #[::core::prelude::v1::test]
    fn directory_drop_revalidates_identity_hierarchy_and_latest_parent() {
        let folders = vec![
            folder("parent", "Parent", None),
            folder("moving", "Moving", Some("parent")),
            folder("child", "Child", Some("moving")),
            folder("other", "Other", None),
        ];
        let drag = FolderDrag {
            folder_id: "moving".into(),
            library_path: "current".into(),
            title: "Moving".into(),
        };
        for destination in [Some("parent"), Some("moving"), Some("child"), Some("gone")] {
            assert!(!valid_folder_drop(
                &drag,
                "current",
                &folders,
                destination,
                false
            ));
        }
        assert!(!valid_folder_drop(
            &drag,
            "other-library",
            &folders,
            Some("other"),
            false
        ));
        assert!(!valid_folder_drop(
            &drag,
            "current",
            &folders,
            Some("other"),
            true
        ));
        assert!(!valid_folder_drop(
            &drag,
            "current",
            &[],
            Some("other"),
            false
        ));
        assert!(valid_folder_drop(
            &drag,
            "current",
            &folders,
            Some("other"),
            false
        ));
        assert!(valid_folder_drop(&drag, "current", &folders, None, false));
        let mut latest = folders;
        latest[1].parent_id = Some("other".into());
        assert!(!valid_folder_drop(
            &drag,
            "current",
            &latest,
            Some("other"),
            false
        ));
    }

    #[::core::prelude::v1::test]
    fn directory_drop_checks_the_whole_subtree_depth() {
        let folders: Vec<_> = (0..8)
            .map(|level| {
                folder(
                    &level.to_string(),
                    "Level",
                    (level > 0).then(|| (level - 1).to_string()).as_deref(),
                )
            })
            .chain([
                folder("moving", "Moving", None),
                folder("child", "Child", Some("moving")),
            ])
            .collect();
        let drag = FolderDrag {
            folder_id: "moving".into(),
            library_path: "current".into(),
            title: "Moving".into(),
        };
        assert!(!valid_folder_drop(
            &drag,
            "current",
            &folders,
            Some("6"),
            false
        ));
        assert!(valid_folder_drop(
            &drag,
            "current",
            &folders,
            Some("5"),
            false
        ));
    }

    #[::core::prelude::v1::test]
    fn names_are_trimmed_and_unique_only_among_siblings() {
        let folders = [
            folder("a", "Research", None),
            folder("b", "Research", Some("a")),
        ];
        assert!(validated_name(" research ", &folders, None, None).is_err());
        assert_eq!(
            validated_name(" Research ", &folders, None, Some("a")).unwrap(),
            "Research"
        );
        assert!(validated_name("../folder", &folders, None, None).is_err());
        assert!(validated_name("..", &folders, None, None).is_err());
        assert!(validated_name(&"中".repeat(201), &folders, None, None).is_err());
        assert_eq!(
            validated_name("  新目录  ", &folders, None, None).unwrap(),
            "新目录"
        );
    }
}

#[cfg(all(test, feature = "ui-test"))]
mod ui_tests {
    use super::{
        FolderAction, FolderDrag, FolderEvent, FolderFilter, FolderMenuAction, FolderPanel,
    };
    use gpui_kit::{
        AppContext, Context, Entity, Focusable, IntoElement, ParentElement, Render, TestAppContext,
        VisualTestContext, Window,
    };
    use std::sync::Arc;
    use studyapp_core::{Command, NoteFolder, Snapshot};

    /// The production host mounts folder forms in its global modal layer.
    /// Keep the input mounted here too so Enter exercises actual input events.
    struct TestHost {
        panel: Entity<FolderPanel>,
    }

    impl TestHost {
        fn new(name: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
            let panel = cx.new(|cx| {
                let mut panel = FolderPanel::new(window, cx);
                panel.accept(panel_snapshot(), window, cx);
                panel.edit_name(
                    FolderAction::Create { parent_id: None },
                    name.into(),
                    window,
                    cx,
                );
                panel
            });
            cx.observe(&panel, |_, _, cx| cx.notify()).detach();
            Self { panel }
        }
    }

    impl Render for TestHost {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let body = self.panel.update(cx, |panel, cx| panel.modal_content(cx));
            crate::components::column()
                .child(self.panel.clone())
                .children(body)
        }
    }

    fn focus_form(panel: &Entity<FolderPanel>, cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            if let Some(focus) = panel.read(cx).modal_initial_focus(cx) {
                focus.focus(window, cx);
            }
        });
    }

    fn panel_snapshot() -> Arc<Snapshot> {
        Arc::new(Snapshot {
            settings: studyapp_core::Settings::default(),
            notes: Vec::new(),
            folders: vec![NoteFolder {
                id: "existing".into(),
                name: "Existing".into(),
                parent_id: None,
            }],
            versions: Arc::new(Vec::new()),
            sessions: Vec::new(),
            reviews: Vec::new(),
            review_events: Vec::new(),
            timer: studyapp_core::TimerState {
                paused: true,
                active_seconds: 0,
                target_seconds: 60,
                started_at: 0,
                spans: Vec::new(),
            },
            data_path: "C:/test-library".into(),
        })
    }

    fn selection_snapshot() -> Arc<Snapshot> {
        let mut snapshot = (*panel_snapshot()).clone();
        snapshot.folders.push(NoteFolder {
            id: "other".into(),
            name: "Other".into(),
            parent_id: None,
        });
        snapshot.notes = [
            ("a", Some("existing")),
            ("b", Some("other")),
            ("root-note", None),
        ]
        .into_iter()
        .map(|(id, folder)| studyapp_core::Note {
            id: id.into(),
            path: format!("/{id}.md"),
            title: id.into(),
            folder_id: folder.map(str::to_owned),
            title_override: false,
            managed: true,
            detached: false,
            missing: false,
            error: None,
            created_at: 1,
            updated_at: 1,
            revision: 1,
        })
        .collect();
        Arc::new(snapshot)
    }

    #[gpui_kit::test]
    fn draft_collection_excludes_formal_note_positions(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (host, cx) = cx.add_window_view(|window, cx| TestHost::new("New", window, cx));
        let panel = cx.read(|cx| host.read(cx).panel.clone());
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.cancel_action(cx);
                panel.accept(selection_snapshot(), window, cx);
                assert_eq!(
                    panel
                        .visible_notes("", Default::default(), None, false)
                        .len(),
                    3
                );
                panel.set_filter(FolderFilter::Drafts, cx);
                assert!(
                    panel
                        .visible_notes("", Default::default(), None, false)
                        .is_empty(),
                    "The draft collection must never construct formal note rows"
                );
                panel.set_filter(FolderFilter::All, cx);
                assert_eq!(
                    panel
                        .visible_notes("", Default::default(), None, false)
                        .len(),
                    3
                );
            })
        });
    }

    #[gpui_kit::test]
    fn folder_drag_queries_do_not_rebuild_or_sort_the_tree(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (host, cx) = cx.add_window_view(|window, cx| TestHost::new("New", window, cx));
        let panel = cx.read(|cx| host.read(cx).panel.clone());
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.cancel_action(cx);
                panel.accept(selection_snapshot(), window, cx);
                let drag = FolderDrag {
                    folder_id: "existing".into(),
                    library_path: "C:/test-library".into(),
                    title: "Existing".into(),
                };
                let before = super::FOLDER_ROWS_BUILDS.with(|count| count.get());
                for _ in 0..1000 {
                    assert!(panel.can_drop_folder(&drag, Some("other")));
                }
                assert_eq!(
                    super::FOLDER_ROWS_BUILDS.with(|count| count.get()),
                    before,
                    "Pointer hit tests must read the prepared relationships, not rebuild folders"
                );
            })
        });
    }

    #[gpui_kit::test]
    fn clicking_note_aligns_folder_context_without_emitting_a_reselection(cx: &mut TestAppContext) {
        use std::{cell::RefCell, rc::Rc};
        cx.update(gpui_kit::init);
        let (host, cx) = cx.add_window_view(|window, cx| TestHost::new("New", window, cx));
        let panel = cx.read(|cx| host.read(cx).panel.clone());
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.cancel_action(cx);
                panel.accept(selection_snapshot(), window, cx);
                panel.set_filter(FolderFilter::Folder("existing".into()), cx);
            })
        });
        cx.run_until_parked();
        let events = Rc::new(RefCell::new(0));
        let observed = events.clone();
        cx.update(|_, cx| {
            cx.subscribe(&panel, move |_, event: &FolderEvent, _| {
                if matches!(event, FolderEvent::Filter(_)) {
                    *observed.borrow_mut() += 1;
                }
            })
            .detach();
        });
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                assert!(panel.align_note_context("b", cx));
                assert_eq!(
                    panel.filter().folder_id(),
                    Some("other"),
                    "New notes use clicked note's parent"
                );
                panel.accept(selection_snapshot(), window, cx);
                let note = &panel.snapshot.as_ref().unwrap().notes[1];
                assert!(
                    panel.filter().matches(note),
                    "Next worker snapshot must retain clicked B"
                );
                assert!(panel.align_note_context("root-note", cx));
                assert_eq!(panel.filter(), &FolderFilter::Root);
                assert!(!panel.align_note_context("missing", cx));
                assert_eq!(panel.filter(), &FolderFilter::Root);
            })
        });
        cx.run_until_parked();
        assert_eq!(
            *events.borrow(),
            0,
            "Folder reselection would override the clicked note"
        );
    }

    #[gpui_kit::test]
    fn deleted_folder_reconciles_without_navigating_away_from_an_open_draft(
        cx: &mut TestAppContext,
    ) {
        use std::{cell::RefCell, rc::Rc};
        cx.update(gpui_kit::init);
        let (host, cx) = cx.add_window_view(|window, cx| TestHost::new("New", window, cx));
        let panel = cx.read(|cx| host.read(cx).panel.clone());
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.cancel_action(cx);
                panel.accept(selection_snapshot(), window, cx);
                panel.set_filter(FolderFilter::Folder("existing".into()), cx);
            })
        });
        cx.run_until_parked();
        let navigations = Rc::new(RefCell::new(0));
        let observed = navigations.clone();
        cx.update(|_, cx| {
            cx.subscribe(&panel, move |_, event: &FolderEvent, _| {
                if matches!(event, FolderEvent::Filter(_)) {
                    *observed.borrow_mut() += 1;
                }
            })
            .detach();
        });
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                let mut snapshot = (*selection_snapshot()).clone();
                snapshot.folders.retain(|folder| folder.id != "existing");
                for note in &mut snapshot.notes {
                    if note.folder_id.as_deref() == Some("existing") {
                        note.folder_id = None;
                    }
                }
                panel.accept(Arc::new(snapshot), window, cx);
                assert_eq!(panel.filter(), &FolderFilter::All);
            })
        });
        cx.run_until_parked();
        assert_eq!(
            *navigations.borrow(),
            0,
            "Background reconciliation must not park the active draft"
        );
        cx.update(|_, cx| panel.update(cx, |panel, cx| panel.set_filter(FolderFilter::Root, cx)));
        cx.run_until_parked();
        assert_eq!(
            *navigations.borrow(),
            1,
            "User folder clicks still navigate"
        );
    }

    #[gpui_kit::test]
    fn library_selection_invalidates_for_search_sort_collapse_and_snapshot(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (host, cx) = cx.add_window_view(|window, cx| TestHost::new("New", window, cx));
        let panel = cx.read(|cx| host.read(cx).panel.clone());
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.accept(selection_snapshot(), window, cx);
                let initial = panel.visible_notes("", Default::default(), None, false);
                let count = initial.len();
                assert_eq!(panel.matching_note_count(), count);
                let repeated = panel.visible_notes("", Default::default(), None, false);
                assert!(Arc::ptr_eq(&initial, &repeated));
                panel.collapsed.insert("other".into());
                let collapsed = panel.visible_notes("", Default::default(), None, false);
                assert_eq!(collapsed.len(), count - 1);
                assert_eq!(
                    panel.matching_note_count(),
                    count,
                    "Badge counts matches in collapsed branches too"
                );
                let search = panel.visible_notes("b", Default::default(), None, false);
                assert!(!Arc::ptr_eq(&collapsed, &search));
                assert!(
                    search
                        .iter()
                        .any(
                            |&position| panel.snapshot.as_ref().unwrap().notes[position].id == "b"
                        ),
                    "Search reveals collapsed matches"
                );
                let sorted =
                    panel.visible_notes("b", studyapp_core::LibrarySort::Newest, None, false);
                assert!(!Arc::ptr_eq(&search, &sorted));
                let mut changed = (*selection_snapshot()).clone();
                changed
                    .notes
                    .iter_mut()
                    .find(|note| note.id == "b")
                    .unwrap()
                    .detached = true;
                panel.accept(Arc::new(changed), window, cx);
                let attached =
                    panel.visible_notes("b", studyapp_core::LibrarySort::Newest, None, false);
                assert!(
                    !attached
                        .iter()
                        .any(
                            |&position| panel.snapshot.as_ref().unwrap().notes[position].id == "b"
                        )
                );
                let detached =
                    panel.visible_notes("b", studyapp_core::LibrarySort::Newest, None, true);
                assert!(
                    detached
                        .iter()
                        .any(
                            |&position| panel.snapshot.as_ref().unwrap().notes[position].id == "b"
                        )
                );
            })
        });
    }

    #[gpui_kit::test]
    fn selected_note_reveals_ancestors_once_and_allows_explicit_collapse(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (host, cx) = cx.add_window_view(|window, cx| TestHost::new("New", window, cx));
        let panel = cx.read(|cx| host.read(cx).panel.clone());
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.cancel_action(cx);
                panel.accept(selection_snapshot(), window, cx);
                panel.collapsed.insert("other".into());
                let visible = panel.visible_notes("", Default::default(), None, false);
                assert_eq!(
                    visible.len(),
                    2,
                    "Collapsed note must be filtered before element construction"
                );
                _ = panel.library_tree(Default::default(), "", Default::default(), Some("b"), cx);
                assert!(!panel.collapsed.contains("other"));
                panel.collapsed.insert("other".into());
                for _ in 0..3 {
                    _ = panel.library_tree(
                        Default::default(),
                        "",
                        Default::default(),
                        Some("b"),
                        cx,
                    );
                    assert!(
                        panel.collapsed.contains("other"),
                        "Hover rerender cannot undo explicit collapse"
                    );
                }
            })
        });
    }

    #[cfg(feature = "ui-test")]
    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
    }

    #[gpui_kit::test]
    fn selecting_a_directory_preserves_row_bounds_and_paints_one_surface(cx: &mut TestAppContext) {
        use gpui_kit::{Styled, px};
        struct CompactHost {
            panel: Entity<FolderPanel>,
        }
        impl Render for CompactHost {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                gpui_kit::div()
                    .w(px(144.))
                    .h(px(600.))
                    .child(self.panel.clone())
            }
        }
        cx.update(gpui_kit::init);
        let (host, cx) = cx.add_window_view(|window, cx| {
            let panel = cx.new(|cx| {
                let mut panel = FolderPanel::new(window, cx);
                panel.accept(panel_snapshot(), window, cx);
                panel
            });
            cx.observe(&panel, |_, _, cx| cx.notify()).detach();
            CompactHost { panel }
        });
        let panel = cx.read(|cx| host.read(cx).panel.clone());
        draw(cx);
        let before = cx
            .debug_bounds("folder-drop-existing")
            .expect("Directory row is mounted");
        panel.update(cx, |panel, cx| {
            panel.set_filter(FolderFilter::Folder("existing".into()), cx)
        });
        draw(cx);
        let after = cx.debug_bounds("folder-drop-existing").unwrap();
        assert_eq!(
            before, after,
            "Selecting must not change directory dimensions"
        );
        let (bounds, surfaces) = cx.update(|window, _| {
            // Painted quads use pixel-snapped bounds. Bounds::contains is
            // half-open, so it deliberately excludes an identical bottom/right
            // edge; use closed interval comparisons for rectangle containment.
            let bounds = window.pixel_snap_bounds(after).scale(window.scale_factor());
            let surfaces = window
                .painted_quads()
                .into_iter()
                .filter(|quad| {
                    !quad.background.is_transparent()
                        && quad.bounds.left() >= bounds.left()
                        && quad.bounds.top() >= bounds.top()
                        && quad.bounds.right() <= bounds.right()
                        && quad.bounds.bottom() <= bounds.bottom()
                })
                .collect::<Vec<_>>();
            (bounds, surfaces)
        });
        assert_eq!(
            surfaces.len(),
            1,
            "Only the outer directory row paints the selected background"
        );
        assert_eq!(
            surfaces[0].bounds, bounds,
            "The selected surface covers the whole row"
        );
        assert_eq!(
            surfaces[0].background,
            crate::theme::selection_color().into()
        );
    }

    #[gpui_kit::test]
    fn directory_hover_is_read_only_and_release_emits_one_move(cx: &mut TestAppContext) {
        use std::{cell::RefCell, rc::Rc};
        cx.update(gpui_kit::init);
        let (host, cx) = cx.add_window_view(|window, cx| TestHost::new("New", window, cx));
        let panel = cx.read(|cx| host.read(cx).panel.clone());
        let commands = Rc::new(RefCell::new(Vec::new()));
        let observed = commands.clone();
        cx.update(|window, cx| {
            cx.subscribe(&panel, move |_, event: &FolderEvent, _| {
                if let FolderEvent::Command(command) = event {
                    observed.borrow_mut().push(command.as_ref().clone());
                }
            })
            .detach();
            panel.update(cx, |panel, cx| {
                panel.cancel_action(cx);
                let mut snapshot = (*panel_snapshot()).clone();
                snapshot.folders.push(NoteFolder {
                    id: "destination".into(),
                    name: "Destination".into(),
                    parent_id: None,
                });
                panel.accept(Arc::new(snapshot), window, cx);
                let drag = FolderDrag {
                    folder_id: "existing".into(),
                    library_path: "C:/test-library".into(),
                    title: "Existing".into(),
                };
                assert!(panel.can_drop_folder(&drag, Some("destination")));
                assert!(commands.borrow().is_empty(), "Hover cannot emit a move");
                panel.drop_folder(&drag, Some("destination".into()), cx);
            });
        });
        cx.run_until_parked();
        assert_eq!(commands.borrow().len(), 1);
        assert!(
            matches!(&commands.borrow()[0], Command::MoveFolder { folder_id, parent_id } if folder_id == "existing" && parent_id.as_deref() == Some("destination"))
        );
    }

    #[gpui_kit::test]
    fn menu_actions_use_the_clicked_directory_and_keep_a_new_modal_visible(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (host, cx) = cx.add_window_view(|window, cx| TestHost::new("New", window, cx));
        let panel = cx.read(|cx| host.read(cx).panel.clone());
        cx.update(|window, cx| panel.update(cx, |panel, cx| {
            panel.cancel_action(cx);
            panel.set_filter(FolderFilter::All, cx);
            panel.active_menu = Some(7);
            panel.begin_menu_action("C:/wrong-library", "existing", FolderMenuAction::Rename, window, cx);
            assert!(!panel.modal_open());
            panel.begin_menu_action("C:/test-library", "existing", FolderMenuAction::Rename, window, cx);
            assert!(matches!(&panel.action, Some(FolderAction::Rename { folder_id }) if folder_id == "existing"));
            panel.close_menu(7, cx);
            assert!(panel.overlay_open());
            assert!(panel.modal_open());
        }));
    }

    #[cfg(feature = "ui-test")]
    #[gpui_kit::test]
    fn enter_creates_and_renames_each_folder_exactly_once(cx: &mut TestAppContext) {
        use std::{cell::RefCell, rc::Rc};
        cx.update(gpui_kit::init);
        let (host, cx) = cx.add_window_view(|window, cx| TestHost::new("New", window, cx));
        let panel = cx.read(|cx| host.read(cx).panel.clone());
        focus_form(&panel, cx);
        let commands = Rc::new(RefCell::new(Vec::new()));
        let observed = commands.clone();
        cx.update(|_, cx| {
            cx.subscribe(&panel, move |_, event: &FolderEvent, _| {
                if let FolderEvent::Command(command) = event {
                    observed.borrow_mut().push(command.as_ref().clone());
                }
            })
            .detach();
        });
        draw(cx);
        cx.simulate_keystrokes("enter enter");
        assert_eq!(
            commands.borrow().len(),
            1,
            "Enter did not submit exactly once"
        );
        assert!(
            matches!(&commands.borrow()[0], Command::CreateFolder { name, parent_id: None } if name == "New")
        );
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.edit_name(
                    FolderAction::Rename {
                        folder_id: "existing".into(),
                    },
                    "Renamed".into(),
                    window,
                    cx,
                );
            })
        });
        focus_form(&panel, cx);
        draw(cx);
        cx.simulate_keystrokes("enter enter");
        assert_eq!(
            commands.borrow().len(),
            2,
            "Rename Enter did not submit exactly once"
        );
        assert!(
            matches!(&commands.borrow()[1], Command::RenameFolder { folder_id, name } if folder_id == "existing" && name == "Renamed")
        );
    }

    #[cfg(feature = "ui-test")]
    #[gpui_kit::test]
    fn enter_keeps_invalid_or_busy_name_form_open(cx: &mut TestAppContext) {
        use std::{cell::RefCell, rc::Rc};
        cx.update(gpui_kit::init);
        let (host, cx) = cx.add_window_view(|window, cx| TestHost::new("Existing", window, cx));
        let panel = cx.read(|cx| host.read(cx).panel.clone());
        focus_form(&panel, cx);
        let commands = Rc::new(RefCell::new(Vec::new()));
        let observed = commands.clone();
        cx.update(|_, cx| {
            cx.subscribe(&panel, move |_, event: &FolderEvent, _| {
                if let FolderEvent::Command(command) = event {
                    observed.borrow_mut().push(command.as_ref().clone());
                }
            })
            .detach();
        });
        draw(cx);
        cx.simulate_keystrokes("enter");
        assert!(commands.borrow().is_empty());
        cx.read(|cx| {
            assert!(
                panel.read(cx).error.is_some(),
                "Invalid name was not validated"
            );
            assert!(panel.read(cx).action.is_some());
        });
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel
                    .name
                    .update(cx, |input, cx| input.set_value("Valid", window, cx));
                panel.set_busy(true, cx);
            })
        });
        draw(cx);
        cx.simulate_keystrokes("enter");
        assert!(commands.borrow().is_empty());
        cx.read(|cx| assert!(panel.read(cx).action.is_some()));
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.set_busy(false, cx);
                panel.name.focus_handle(cx).focus(window, cx);
            })
        });
        draw(cx);
        cx.simulate_keystrokes("enter");
        assert_eq!(commands.borrow().len(), 1);
    }

    #[gpui_kit::test]
    fn modal_cancel_and_library_switch_release_the_native_overlay(cx: &mut TestAppContext) {
        use std::{cell::RefCell, rc::Rc};
        cx.update(gpui_kit::init);
        let (host, cx) = cx.add_window_view(|window, cx| TestHost::new("New", window, cx));
        let panel = cx.read(|cx| host.read(cx).panel.clone());
        let overlays = Rc::new(RefCell::new(Vec::new()));
        let observed = overlays.clone();
        cx.update(|_, cx| {
            cx.subscribe(&panel, move |_, event: &FolderEvent, _| {
                if let FolderEvent::OverlayChanged(open) = event {
                    observed.borrow_mut().push(*open);
                }
            })
            .detach();
        });
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.cancel_action(cx);
                assert!(!panel.modal_open());
                panel.edit_name(
                    FolderAction::Rename {
                        folder_id: "existing".into(),
                    },
                    "Renamed".into(),
                    window,
                    cx,
                );
                assert!(panel.modal_open());
                let mut changed = (*panel_snapshot()).clone();
                changed.data_path = "C:/other-library".into();
                panel.accept(Arc::new(changed), window, cx);
                assert!(!panel.modal_open());
                assert!(panel.modal_content(cx).is_none());
            })
        });
        cx.run_until_parked();
        assert_eq!(*overlays.borrow(), vec![false, true, false]);
    }
    #[gpui_kit::test]
    fn note_rename_prefills_captures_identity_submits_once_and_cancels(cx: &mut TestAppContext) {
        use crate::note_actions::{NoteAction, NoteActionKind};
        use std::{cell::RefCell, rc::Rc};
        cx.update(gpui_kit::init);
        let (host, cx) = cx.add_window_view(|window, cx| TestHost::new("New", window, cx));
        let panel = cx.read(|cx| host.read(cx).panel.clone());
        let commands = Rc::new(RefCell::new(Vec::new()));
        let observed = commands.clone();
        cx.update(|_, cx| {
            cx.subscribe(&panel, move |_, event: &FolderEvent, _| {
                if let FolderEvent::RenameNote {
                    library_path,
                    command,
                } = event
                {
                    observed
                        .borrow_mut()
                        .push((library_path.clone(), command.as_ref().clone()));
                }
            })
            .detach();
        });
        let action = NoteAction {
            note_id: "b".into(),
            library_path: "C:/test-library".into(),
            kind: NoteActionKind::Rename {
                expected_revision: 1,
            },
        };
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.cancel_action(cx);
                panel.accept(selection_snapshot(), window, cx);
                panel.begin_rename_note(action.clone(), window, cx);
                assert_eq!(panel.name.read(cx).value().as_ref(), "b");
                assert!(panel.modal_initial_focus(cx).is_some());
                panel.name.update(cx, |input, cx| {
                    input.set_value("  新显示名称  ", window, cx)
                });
            })
        });
        focus_form(&panel, cx);
        draw(cx);
        cx.simulate_keystrokes("enter enter");
        assert_eq!(commands.borrow().len(), 1);
        assert!(
            matches!(&commands.borrow()[0], (library, Command::RenameNote { note_id, title, expected_revision: 1 }) if library == "C:/test-library" && note_id == "b" && title == "新显示名称")
        );
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.begin_rename_note(action.clone(), window, cx);
                panel.cancel_action(cx);
                panel.submit_action(window, cx);
                panel.begin_rename_note(action.clone(), window, cx);
                panel.set_busy(true, cx);
                panel.submit_action(window, cx);
                assert!(panel.modal_open());
                panel.set_busy(false, cx);
                let mut changed = (*selection_snapshot()).clone();
                changed.notes[1].revision += 1;
                panel.accept(Arc::new(changed), window, cx);
                assert!(!panel.modal_open());
                panel.begin_rename_note(action, window, cx);
                assert!(!panel.modal_open());
            })
        });
        cx.run_until_parked();
        assert_eq!(commands.borrow().len(), 1);
    }
}
