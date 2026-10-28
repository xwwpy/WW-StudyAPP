//! Native image jobs are scoped to a library, document generation and request.
//! Resolution is debounced with two bounded slots; imports run in a bounded queue.
use crate::{
    app::{Page, StudyApp},
    drafts::NoteDraft,
    media::{MediaInput, MediaOutcome, MediaRequest, MediaResponse, ResolveControl, asset_group},
    worker::Work,
};
use gpui_kit::*;
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};
use studyapp_core::ImageSettings;

#[cfg(windows)]
#[path = "native_clipboard.rs"]
mod native_clipboard;

const MAX_FILES: usize = 20;
// Bound retained base64 strings as well as item count. Match the browser's
// 20 MiB batch budget, allowing each separately encoded file's padding.
const MAX_CLIPBOARD_QUEUE_BYTES: usize = (20 * 1024 * 1024_usize).div_ceil(3) * 4 + MAX_FILES * 4;

#[derive(Clone, PartialEq, Eq)]
struct DocumentKey {
    library: String,
    generation: u64,
    revision: u64,
    source: Option<PathBuf>,
    settings: ImageSettings,
}

struct Insertion {
    library: String,
    generation: u64,
    group: String,
    title: String,
    settings: ImageSettings,
    inputs: VecDeque<MediaInput>,
    request: Option<u64>,
    accepted_count: usize,
    clipboard_bytes: usize,
}

impl Insertion {
    fn append(
        &mut self,
        library: &str,
        generation: u64,
        group: &str,
        inputs: VecDeque<MediaInput>,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.library == library && self.generation == generation && self.group == group,
            "另一篇笔记的图片仍在处理中，请等待处理完成"
        );
        let bytes = inputs.iter().try_fold(0_usize, |sum, input| {
            sum.checked_add(match input {
                MediaInput::Encoded(value) => value.len(),
                MediaInput::Bytes(value) => value.len().div_ceil(3) * 4,
                MediaInput::Path(_) => 0,
            })
        });
        anyhow::ensure!(
            self.accepted_count.saturating_add(inputs.len()) <= MAX_FILES
                && bytes
                    .is_some_and(|bytes| self.clipboard_bytes.saturating_add(bytes)
                        <= MAX_CLIPBOARD_QUEUE_BYTES),
            "每次最多插入 {MAX_FILES} 张图片，剪贴板图片合计最多 20 MiB，请等待当前图片处理完成"
        );
        // These totals cover the entire insertion session, including the input
        // already owned by the worker; draining a slot never bypasses the cap.
        self.accepted_count += inputs.len();
        self.clipboard_bytes += bytes.unwrap();
        self.inputs.extend(inputs);
        Ok(())
    }
}

struct Adoption {
    library: String,
    generation: u64,
    request: u64,
    draft_id: Option<String>,
    selected_note: Option<String>,
    selected_version: Option<String>,
    draft: NoteDraft,
    commit_after_adoption: bool,
}

struct DraftRetention {
    library: String,
    generation: u64,
    request: u64,
    id: String,
    revision: u64,
    intent: crate::editor_lifecycle::NoteIntent,
}

struct PickerResult {
    token: u64,
    library: String,
    generation: u64,
    paths: Vec<PathBuf>,
}

pub struct MediaUi {
    sequence: u64,
    observed: Option<DocumentKey>,
    dirty_since: Option<Instant>,
    resolving: Vec<(u64, DocumentKey, ResolveControl)>,
    current_resolve: Option<u64>,
    insertion: Option<Insertion>,
    adoption: Option<Adoption>,
    retention: Option<DraftRetention>,
    picker: Option<u64>,
    picker_sender: mpsc::Sender<PickerResult>,
    picker_results: mpsc::Receiver<PickerResult>,
    last_missing: Vec<String>,
}

impl MediaUi {
    pub fn new() -> Self {
        let (picker_sender, picker_results) = mpsc::channel();
        Self {
            sequence: 0,
            observed: None,
            dirty_since: None,
            resolving: Vec::new(),
            current_resolve: None,
            insertion: None,
            adoption: None,
            retention: None,
            picker: None,
            picker_sender,
            picker_results,
            last_missing: Vec::new(),
        }
    }
    fn next(&mut self) -> u64 {
        self.sequence = self.sequence.wrapping_add(1).max(1);
        self.sequence
    }
    pub fn busy(&self) -> bool {
        self.insertion.is_some()
            || self.adoption.is_some()
            || self.retention.is_some()
            || self.picker.is_some()
    }
    fn observe(&mut self, key: Option<DocumentKey>) {
        if self.observed != key {
            for (_, _, control) in &self.resolving {
                control.cancel();
            }
            self.current_resolve = None;
            self.observed = key;
            self.dirty_since = Some(Instant::now());
        }
    }
    fn resolution_available(&self) -> bool {
        self.resolving.len() < 2 && self.current_resolve.is_none()
    }
}

fn clipboard_input(encoded: String) -> anyhow::Result<MediaInput> {
    anyhow::ensure!(
        encoded.len() <= crate::media::MAX_IMAGE_BYTES.div_ceil(3) * 4,
        "图片超过 10 MiB"
    );
    anyhow::ensure!(!encoded.is_empty(), "剪贴板图片为空");
    // Decoding and format/dimension validation run on the media worker.
    Ok(MediaInput::Encoded(encoded))
}

fn image_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|extension| {
            ["png", "jpg", "jpeg", "gif", "webp", "bmp"]
                .iter()
                .any(|supported| extension.eq_ignore_ascii_case(supported))
        })
}

impl StudyApp {
    pub fn media_busy(&self) -> bool {
        self.media_ui.busy()
    }

    /// Storage migration must wait for all jobs that can read/write media.
    /// Preview resolution remains nonblocking for ordinary editor controls.
    pub fn media_jobs_active(&self) -> bool {
        self.media_ui.busy() || !self.media_ui.resolving.is_empty()
    }

    fn media_key(&self, cx: &App) -> Option<DocumentKey> {
        if self.page != Page::Notes || self.last_library.is_empty() {
            return None;
        }
        let snapshot = self.snapshot.as_ref()?;
        if self.note_editor.is_none() && self.reader_key.is_empty() {
            return None;
        }
        let source_id = self
            .note_editor
            .as_ref()
            .and_then(|draft| draft.note_id.as_ref())
            .or(self.selected_note.as_ref());
        // An unsaved new draft has no source directory. Retained references use
        // studyapp-media, so resolving relative paths against another note is wrong.
        let source = if self
            .note_editor
            .as_ref()
            .is_some_and(|draft| draft.note_id.is_none())
        {
            None
        } else {
            snapshot
                .notes
                .iter()
                .find(|note| Some(&note.id) == source_id)
                .map(|note| PathBuf::from(&note.path))
        };
        let source = source.or_else(|| {
            self.active_draft_id
                .as_ref()
                .and_then(|id| self.draft_catalog.get(id))
                .and_then(|r| r.source_path.clone())
        });
        let editor = self.note_content.read(cx);
        Some(DocumentKey {
            library: self.last_library.clone(),
            generation: editor.generation(),
            revision: editor.revision(),
            source,
            settings: snapshot.settings.images.clone(),
        })
    }

    fn send_media(
        &mut self,
        library: String,
        request: MediaRequest,
        cx: &mut Context<Self>,
    ) -> bool {
        if self
            .worker
            .sender
            .send(Work::Scoped {
                library,
                work: Box::new(Work::Media(request)),
            })
            .is_err()
        {
            self.push_notice(Err("图片处理服务已停止，当前笔记仍保留。".into()), cx);
            return false;
        }
        true
    }

    pub fn poll_media(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        while let Ok(result) = self.media_ui.picker_results.try_recv() {
            if self.media_ui.picker != Some(result.token) {
                continue;
            }
            self.media_ui.picker = None;
            if result.library == self.last_library
                && result.generation == self.note_content.read(cx).generation()
                && !result.paths.is_empty()
            {
                self.import_dropped_images(result.paths, window, cx);
            }
            cx.notify();
        }
        let key = self.media_key(cx);
        self.media_ui.observe(key);
        if self.media_ui.resolution_available()
            && !self.storage_busy
            && !self.preferences_ui.replacing
            && self
                .media_ui
                .dirty_since
                .is_some_and(|at| at.elapsed() >= Duration::from_millis(200))
            && let Some(key) = self.media_ui.observed.clone()
        {
            let request = self.media_ui.next();
            let control = ResolveControl::default();
            let job = MediaRequest::ResolveRetained {
                control: control.clone(),
                generation: key.generation,
                request,
                content: self.note_content.read(cx).value().to_owned(),
                note_path: key.source.clone(),
                settings: key.settings.clone(),
                bindings: self
                    .active_draft_id
                    .as_ref()
                    .and_then(|id| self.draft_catalog.get(id))
                    .map(|r| r.media_bindings.clone())
                    .unwrap_or_default(),
            };
            self.media_ui.dirty_since = None;
            if self.send_media(key.library.clone(), job, cx) {
                self.media_ui.resolving.push((request, key, control));
                self.media_ui.current_resolve = Some(request);
            }
        }
        self.send_next_image(cx);
    }

    fn can_insert_image(&self, cx: &App) -> bool {
        self.page == Page::Notes
            && self.note_editor.is_some()
            && !self.note_saving
            && !self.storage_busy
            && self.confirmation.is_none()
            && !self.has_modal(cx)
            && self.note_content.read(cx).editable()
    }

    pub fn request_insert_image(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_insert_image(cx) || self.media_busy() {
            self.push_notice(
                Err("请先打开可编辑的草稿，并等待当前图片操作完成。".into()),
                cx,
            );
            return;
        }
        let token = self.media_ui.next();
        self.media_ui.picker = Some(token);
        let sender = self.media_ui.picker_sender.clone();
        let library = self.last_library.clone();
        let generation = self.note_content.read(cx).generation();
        std::thread::spawn(move || {
            let paths = rfd::FileDialog::new()
                .set_title("插入图片")
                .add_filter("图片", &["png", "jpg", "jpeg", "gif", "webp", "bmp"])
                .pick_files()
                .unwrap_or_default();
            let _ = sender.send(PickerResult {
                token,
                library,
                generation,
                paths,
            });
        });
        cx.notify();
    }

    pub fn paste_editor_image(
        &mut self,
        generation: u64,
        _name: String,
        _mime: String,
        encoded: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if generation != self.note_content.read(cx).generation() {
            return;
        }
        if !self.can_insert_image(cx) || (self.media_busy() && self.media_ui.insertion.is_none()) {
            self.push_notice(Err("请等待当前图片操作完成后再粘贴。".into()), cx);
            return;
        }
        match clipboard_input(encoded) {
            Ok(input) => self.queue_image_inputs(VecDeque::from([input]), cx),
            Err(error) => self.push_notice(Err(format!("无法粘贴图片：{error}")), cx),
        }
    }

    pub fn paste_native_image(
        &mut self,
        generation: u64,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Check the document again after event delivery, before reading any
        // clipboard format. The worker owns validation/decoding and durable IO.
        if generation != self.note_content.read(cx).generation()
            || !self.can_insert_image(cx)
            || (self.media_busy() && self.media_ui.insertion.is_none())
        {
            return;
        }
        #[cfg(windows)]
        match native_clipboard::png(_window) {
            Ok(Some(bytes)) => {
                self.queue_image_inputs(VecDeque::from([MediaInput::Bytes(bytes)]), cx)
            }
            Ok(None) => {}
            Err(error) => self.push_notice(Err(format!("无法粘贴图片：{error}")), cx),
        }
    }

    /// Returning false leaves non-image payloads to normal Markdown import.
    pub fn import_dropped_images(
        &mut self,
        paths: Vec<PathBuf>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if paths.is_empty() || !paths.iter().all(|path| image_path(path)) {
            return false;
        }
        if !self.can_insert_image(cx) || self.media_busy() {
            self.push_notice(
                Err("请先打开可编辑的草稿，并等待当前图片操作完成后插入图片。".into()),
                cx,
            );
            return true;
        }
        if paths.len() > MAX_FILES {
            self.push_notice(Err(format!("每次最多插入 {MAX_FILES} 张图片。")), cx);
            return true;
        }
        self.queue_image_inputs(paths.into_iter().map(MediaInput::Path).collect(), cx);
        true
    }

    fn queue_image_inputs(&mut self, inputs: VecDeque<MediaInput>, cx: &mut Context<Self>) {
        if inputs.is_empty()
            || !self.can_insert_image(cx)
            || self.media_ui.adoption.is_some()
            || self.media_ui.retention.is_some()
            || self.media_ui.picker.is_some()
        {
            return;
        }
        let Some(draft) = self.note_editor.as_mut() else {
            return;
        };
        if draft.asset_group.is_empty() {
            draft.asset_group = asset_group(draft.note_id.as_deref(), &draft.content);
        }
        let group = draft.asset_group.clone();
        let generation = self.note_content.read(cx).generation();
        if let Some(pending) = self.media_ui.insertion.as_mut() {
            if let Err(error) = pending.append(&self.last_library, generation, &group, inputs) {
                self.push_notice(Err(format!("无法粘贴图片：{error}")), cx);
            } else {
                self.send_next_image(cx);
                cx.notify();
            }
            return;
        }
        let settings = self
            .snapshot
            .as_ref()
            .map(|s| s.settings.images.clone())
            .unwrap_or_default();
        let mut pending = Insertion {
            library: self.last_library.clone(),
            generation,
            group: group.clone(),
            title: self.note_title.read(cx).value().to_string(),
            settings,
            inputs: VecDeque::new(),
            request: None,
            accepted_count: 0,
            clipboard_bytes: 0,
        };
        if let Err(error) = pending.append(&self.last_library, generation, &group, inputs) {
            self.push_notice(Err(format!("无法插入图片：{error}")), cx);
            return;
        }
        self.media_ui.insertion = Some(pending);
        self.capture_note_draft(cx);
        self.send_next_image(cx);
        cx.notify();
    }

    fn send_next_image(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.media_ui.insertion.as_ref() else {
            return;
        };
        if pending.library != self.last_library
            || pending.generation != self.note_content.read(cx).generation()
            || self.note_editor.is_none()
        {
            self.media_ui.insertion = None;
            return;
        }
        if pending.request.is_some() {
            return;
        }
        let request = self.media_ui.next();
        let pending = self.media_ui.insertion.as_mut().unwrap();
        let Some(input) = pending.inputs.pop_front() else {
            self.media_ui.insertion = None;
            cx.notify();
            return;
        };
        pending.request = Some(request);
        let library = pending.library.clone();
        let job = MediaRequest::Import {
            generation: pending.generation,
            request,
            group: pending.group.clone(),
            title: pending.title.clone(),
            input,
            settings: pending.settings.clone(),
        };
        if !self.send_media(library, job, cx) {
            self.media_ui.insertion = None;
        }
    }

    pub(crate) fn cancel_draft_retention(&mut self) {
        self.media_ui.retention = None;
    }

    pub(crate) fn begin_draft_retention(
        &mut self,
        intent: crate::editor_lifecycle::NoteIntent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(record) = self
            .active_draft_id
            .as_ref()
            .and_then(|id| self.draft_catalog.get(id))
            .cloned()
        else {
            return;
        };
        let request = self.media_ui.next();
        let generation = self.note_content.read(cx).generation();
        let content =
            match crate::media::rewrite_images(&record.draft.content, &record.media_bindings) {
                Ok(content) => content,
                Err(error) => {
                    self.note_saving = false;
                    self.close_requested = false;
                    self.pending_update = None;
                    self.note_content
                        .update(cx, |editor, cx| editor.set_readonly(false, cx));
                    self.push_notice(Err(error.to_string()), cx);
                    return;
                }
            };
        let settings = self
            .snapshot
            .as_ref()
            .map(|s| s.settings.images.clone())
            .unwrap_or_default();
        let job = MediaRequest::Retain {
            generation,
            request,
            group: record.draft.asset_group.clone(),
            title: record.draft.title.clone(),
            content,
            source_path: record.source_path.clone(),
            settings,
        };
        if self.send_media(self.last_library.clone(), job, cx) {
            self.media_ui.retention = Some(DraftRetention {
                library: self.last_library.clone(),
                generation,
                request,
                id: record.id,
                revision: record.revision,
                intent,
            });
        } else {
            self.note_saving = false;
            self.close_requested = false;
            self.pending_update = None;
            self.note_content
                .update(cx, |editor, cx| editor.set_readonly(false, cx));
        }
    }

    pub(crate) fn begin_media_adoption(
        &mut self,
        draft: NoteDraft,
        source_path: Option<PathBuf>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.start_media_adoption(
            draft,
            source_path,
            false,
            crate::media::MediaAdoptionPolicy::CopyToGroup,
            cx,
        );
    }

    pub(crate) fn begin_save_images(
        &mut self,
        copy: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mut draft) = self.note_editor.clone() else {
            return;
        };
        let source_path = self.snapshot.as_ref().and_then(|snapshot| {
            snapshot
                .notes
                .iter()
                .find(|note| Some(&note.id) == draft.note_id.as_ref())
                .map(|note| PathBuf::from(&note.path))
        });
        let source_path = source_path.or_else(|| {
            self.active_draft_id
                .as_ref()
                .and_then(|id| self.draft_catalog.get(id))
                .and_then(|r| r.source_path.clone())
        });
        if let Some(record) = self
            .active_draft_id
            .as_ref()
            .and_then(|id| self.draft_catalog.get(id))
            && let Ok(content) =
                crate::media::rewrite_images(&draft.content, &record.media_bindings)
        {
            draft.content = content;
        }
        if copy {
            draft.note_id = None;
            draft.expected_hash = None;
            draft.asset_group = crate::media::new_group();
        } else if draft.asset_group.is_empty() {
            draft.asset_group = asset_group(draft.note_id.as_deref(), &draft.content);
        }
        self.start_media_adoption(
            draft,
            source_path,
            true,
            if copy {
                crate::media::MediaAdoptionPolicy::CopyToGroup
            } else {
                crate::media::MediaAdoptionPolicy::PreserveManaged
            },
            cx,
        );
    }

    fn start_media_adoption(
        &mut self,
        draft: NoteDraft,
        source_path: Option<PathBuf>,
        commit_after_adoption: bool,
        policy: crate::media::MediaAdoptionPolicy,
        cx: &mut Context<Self>,
    ) {
        if self.media_busy() {
            return;
        }
        let request = self.media_ui.next();
        let generation = self.note_content.read(cx).generation();
        let settings = self
            .snapshot
            .as_ref()
            .map(|s| s.settings.images.clone())
            .unwrap_or_default();
        let job = MediaRequest::Adopt {
            policy,
            generation,
            request,
            group: draft.asset_group.clone(),
            title: draft.title.clone(),
            content: draft.content.clone(),
            source_path,
            settings,
        };
        let library = self.last_library.clone();
        if self.send_media(library.clone(), job, cx) {
            self.media_ui.adoption = Some(Adoption {
                library,
                generation,
                request,
                draft_id: self.active_draft_id.clone(),
                selected_note: self.selected_note.clone(),
                selected_version: self.selected_version.clone(),
                draft,
                commit_after_adoption,
            });
            cx.notify();
        } else if commit_after_adoption {
            self.note_saving = false;
            self.note_content
                .update(cx, |editor, cx| editor.set_readonly(false, cx));
        }
    }

    fn warn_missing_images(&mut self, missing: Vec<String>, copying: bool, cx: &mut Context<Self>) {
        if !missing.is_empty() && (copying || missing != self.media_ui.last_missing) {
            let prefix = if copying {
                "部分图片未能保存到资料库，原引用已保留"
            } else {
                "部分图片无法显示"
            };
            self.push_notice(
                Err(format!(
                    "{prefix}（{} 项）：{}",
                    missing.len(),
                    missing
                        .iter()
                        .take(3)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("；")
                )),
                cx,
            );
        }
        self.media_ui.last_missing = missing;
    }

    pub fn accept_media_result(
        &mut self,
        response: MediaResponse,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.media_ui.retention.as_ref().is_some_and(|p| {
            p.library == response.library
                && p.generation == response.generation
                && p.request == response.request
        }) {
            let pending = self.media_ui.retention.take().unwrap();
            if pending.library != self.last_library
                || self.active_draft_id.as_ref() != Some(&pending.id)
                || self.note_content.read(cx).generation() != pending.generation
                || self
                    .draft_catalog
                    .get(&pending.id)
                    .is_none_or(|r| r.revision != pending.revision)
            {
                return;
            }
            match response.result {
                Ok(MediaOutcome::Retained { bindings, missing }) => {
                    let record = self.draft_catalog.get_mut(&pending.id).unwrap();
                    let mut merged = record.media_bindings.clone();
                    for (original, owned) in &record.media_bindings {
                        if let Some(new) = bindings.get(owned) {
                            merged.insert(original.clone(), new.clone());
                        }
                    }
                    for (original, owned) in bindings {
                        if !record.media_bindings.values().any(|old| old == &original) {
                            merged.insert(original, owned);
                        }
                    }
                    if record.media_bindings != merged {
                        record.media_bindings = merged;
                        record.touch(chrono::Utc::now().timestamp_millis());
                    }
                    let record = record.clone();
                    self.warn_missing_images(missing, true, cx);
                    self.persist_record_intent(record, pending.intent, cx);
                }
                Err(error) => {
                    self.note_saving = false;
                    self.close_requested = false;
                    self.pending_update = None;
                    self.note_content
                        .update(cx, |editor, cx| editor.set_readonly(false, cx));
                    self.push_notice(
                        Err(format!("草稿图片尚未保存，当前正文已保留，请重试：{error}")),
                        cx,
                    );
                }
                _ => {}
            }
            cx.notify();
            return;
        }
        if let Some(index) = self
            .media_ui
            .resolving
            .iter()
            .position(|(request, key, _)| {
                *request == response.request
                    && key.library == response.library
                    && key.generation == response.generation
            })
        {
            let (_, key, control) = self.media_ui.resolving.remove(index);
            if self.media_ui.current_resolve != Some(response.request) {
                return;
            }
            self.media_ui.current_resolve = None;
            if control.is_cancelled() || self.media_key(cx).as_ref() != Some(&key) {
                return;
            }
            match response.result {
                Ok(MediaOutcome::Resolved { images, missing }) => {
                    self.note_content.update(cx, |editor, cx| {
                        editor.set_images(response.generation, images, cx)
                    });
                    self.warn_missing_images(missing, false, cx);
                }
                Err(error) => self.push_notice(Err(format!("图片预览失败：{error}")), cx),
                _ => {}
            }
            return;
        }
        if self.media_ui.adoption.as_ref().is_some_and(|pending| {
            pending.request == response.request
                && pending.library == response.library
                && pending.generation == response.generation
        }) {
            let mut pending = self.media_ui.adoption.take().unwrap();
            // A snapshot may update the list selection while the same draft is
            // saving (for example after its folder is removed). Saving belongs
            // to that draft; reader-to-copy adoption belongs to the selection.
            let target_changed = if pending.commit_after_adoption {
                pending.draft_id != self.active_draft_id
            } else {
                pending.selected_note != self.selected_note
                    || pending.selected_version != self.selected_version
            };
            if pending.library != self.last_library
                || pending.generation != self.note_content.read(cx).generation()
                || self.note_editor.is_some() != pending.commit_after_adoption
                || target_changed
            {
                cx.notify();
                return;
            }
            match response.result {
                Ok(MediaOutcome::Adopted { content, missing }) => {
                    pending.draft.content = content;
                    if !self.start_note_draft(pending.draft, window, cx) {
                        return;
                    }
                    if missing.is_empty() && !pending.commit_after_adoption {
                        self.push_notice(Ok("已创建可编辑副本".into()), cx);
                    }
                    self.warn_missing_images(missing, true, cx);
                    if pending.commit_after_adoption {
                        self.request_note_action(
                            crate::editor_lifecycle::NoteIntent::Commit,
                            window,
                            cx,
                        );
                    }
                }
                Err(error) => {
                    if pending.commit_after_adoption {
                        self.note_saving = false;
                        self.note_content
                            .update(cx, |editor, cx| editor.set_readonly(false, cx));
                    }
                    self.push_notice(
                        Err(format!("无法保存笔记图片：{error}；原笔记和草稿已保留。")),
                        cx,
                    );
                }
                _ => {}
            }
            cx.notify();
            return;
        }
        if self.media_ui.insertion.as_ref().is_some_and(|pending| {
            pending.request == Some(response.request)
                && pending.library == response.library
                && pending.generation == response.generation
        }) {
            let pending = self.media_ui.insertion.as_mut().unwrap();
            pending.request = None;
            if response.library != self.last_library
                || response.generation != self.note_content.read(cx).generation()
                || self.note_editor.is_none()
            {
                self.media_ui.insertion = None;
                cx.notify();
                return;
            }
            match response.result {
                Ok(MediaOutcome::Inserted { markdown }) => {
                    let result = self.note_content.update(cx, |editor, cx| {
                        editor.insert_markdown(response.generation, &markdown, cx)
                    });
                    if let Err(error) = result {
                        self.push_notice(Err(error.to_string()), cx);
                    }
                }
                Err(error) => self.push_notice(Err(format!("图片插入失败：{error}")), cx),
                _ => {}
            }
            self.send_next_image(cx);
            cx.notify();
        }
    }
}

#[cfg(all(test, feature = "ui-test"))]
mod save_barrier_tests {
    use super::{Adoption, MediaOutcome, MediaResponse, StudyApp};
    use crate::{editor_lifecycle::NoteIntent, worker::Worker};
    use gpui_kit::{AppContext, Context, Entity, IntoElement, Render, TestAppContext, Window};

    // Exercise the save state machine without mounting MarkdownEditor or
    // creating a native WebView, even if the test dispatcher renders a frame.
    struct SaveTestHost {
        app: Entity<StudyApp>,
    }

    impl Render for SaveTestHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            gpui_kit::div()
        }
    }

    #[gpui_kit::test]
    fn ordinary_reopen_uses_note_identity_and_keeps_recovered_group(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let directory = tempfile::tempdir().unwrap().keep();
        let database = directory.join("studyapp.sqlite3");
        let mut engine = studyapp_core::Engine::open(&database, 1).unwrap();
        engine
            .apply(
                studyapp_core::Command::CreateNote {
                    title: "Existing".into(),
                    content: "Existing content".into(),
                },
                2,
            )
            .unwrap();
        let snapshot = std::sync::Arc::new(engine.snapshot());
        let id = snapshot.notes[0].id.clone();
        drop(engine);
        let worker = Worker::start(database, None);
        let (host, cx) = cx.add_window_view(|window, cx| SaveTestHost {
            app: cx.new(|cx| StudyApp::new(worker, window, cx)),
        });
        let view = cx.read(|cx| host.read(cx).app.clone());
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.accept_snapshot(snapshot, false, window, cx);
                app.draft_loading = false;
                app.selected_note = Some(id.clone());
                app.open_note_editor(false, window, cx);
                let mut draft = app.note_editor.clone().unwrap();
                assert_eq!(draft.asset_group, id);
                draft.asset_group = crate::media::new_group();
                let recovered = draft.asset_group.clone();
                app.start_note_draft(draft.clone(), window, cx);
                assert_eq!(app.note_editor.as_ref().unwrap().asset_group, recovered);
                draft.asset_group.clear();
                app.start_note_draft(draft, window, cx);
                assert_eq!(app.note_editor.as_ref().unwrap().asset_group, id);
            })
        });
    }

    #[gpui_kit::test]
    fn save_after_folder_deletion_survives_snapshot_selection_change(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let directory = tempfile::tempdir().unwrap().keep();
        let database = directory.join("studyapp.sqlite3");
        let mut engine = studyapp_core::Engine::open(&database, 1).unwrap();
        engine
            .apply(
                studyapp_core::Command::CreateNote {
                    title: "Existing root note".into(),
                    content: "Existing content".into(),
                },
                2,
            )
            .unwrap();
        engine
            .apply(
                studyapp_core::Command::CreateFolder {
                    name: "Draft folder".into(),
                    parent_id: None,
                },
                3,
            )
            .unwrap();
        let initial = std::sync::Arc::new(engine.snapshot());
        let folder = initial.folders[0].id.clone();
        engine
            .apply(
                studyapp_core::Command::DeleteFolder {
                    folder_id: folder.clone(),
                },
                4,
            )
            .unwrap();
        let deleted = std::sync::Arc::new(engine.snapshot());
        drop(engine);
        let worker = Worker::start(database, None);
        let (host, cx) = cx.add_window_view(|window, cx| SaveTestHost {
            app: cx.new(|cx| StudyApp::new(worker, window, cx)),
        });
        let view = cx.read(|cx| host.read(cx).app.clone());
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.accept_snapshot(initial, false, window, cx);
                app.draft_loading = false;
                app.folders.update(cx, |panel, cx| {
                    panel.set_filter(crate::folders_ui::FolderFilter::Folder(folder.clone()), cx)
                });
                app.select_folder_document(
                    crate::folders_ui::FolderFilter::Folder(folder.clone()),
                    window,
                    cx,
                );
                app.start_new_draft(Some(folder), window, cx);
                app.note_content.update(cx, |editor, cx| {
                    editor.set_value("Draft survives folder deletion".into(), window, cx)
                });
                let draft_id = app.active_draft_id.clone().unwrap();
                app.accept_snapshot(deleted.clone(), false, window, cx);
                assert!(app.note_editor.as_ref().unwrap().folder_id.is_none());
                assert!(app.selected_note.is_none());

                app.save_note(window, cx);
                let flush = app.note_intent.as_ref().unwrap().0;
                app.editor_event(
                    crate::markdown_editor::MarkdownEvent::Flushed(flush),
                    window,
                    cx,
                );
                let pending = app.media_ui.adoption.as_ref().unwrap();
                let response = MediaResponse {
                    library: pending.library.clone(),
                    generation: pending.generation,
                    request: pending.request,
                    result: Ok(MediaOutcome::Adopted {
                        content: pending.draft.content.clone(),
                        missing: vec![],
                    }),
                };
                // The deleted folder resets the filter on the first snapshot;
                // the next worker tick chooses an unrelated root note.
                app.accept_snapshot(deleted.clone(), false, window, cx);
                assert_eq!(app.selected_note.as_ref(), Some(&deleted.notes[0].id));
                assert_eq!(app.active_draft_id.as_ref(), Some(&draft_id));
                app.accept_media_result(response, window, cx);
                let flush = app
                    .note_intent
                    .as_ref()
                    .expect("saving the same draft must continue after a list selection refresh")
                    .0;
                app.editor_event(
                    crate::markdown_editor::MarkdownEvent::Flushed(flush),
                    window,
                    cx,
                );
                let commit = app
                    .draft_persist
                    .as_ref()
                    .expect("save must reach durable commit");
                assert_eq!(commit.id, draft_id);
                assert!(matches!(commit.intent, NoteIntent::Commit));
                let record = &app.draft_catalog[&draft_id];
                assert_eq!(record.draft.content, "Draft survives folder deletion");
                assert!(record.draft.folder_id.is_none());
                assert!(record.draft.note_id.is_none());
                assert!(app.note_content.read(cx).readonly());
            });
        });
    }

    #[gpui_kit::test]
    fn save_adoption_cannot_replace_another_active_draft(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let directory = tempfile::tempdir().unwrap().keep();
        let database = directory.join("studyapp.sqlite3");
        let library = database.to_string_lossy().into_owned();
        let worker = Worker::start(database, None);
        let (host, cx) = cx.add_window_view(|window, cx| SaveTestHost {
            app: cx.new(|cx| StudyApp::new(worker, window, cx)),
        });
        let view = cx.read(|cx| host.read(cx).app.clone());
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.last_library = library;
                app.draft_loading = false;
                app.start_new_draft(None, window, cx);
                app.note_saving = true;
                app.note_content
                    .update(cx, |editor, cx| editor.set_readonly(true, cx));
                app.begin_save_images(false, window, cx);
                let pending = app.media_ui.adoption.as_ref().unwrap();
                let response = MediaResponse {
                    library: pending.library.clone(),
                    generation: pending.generation,
                    request: pending.request,
                    result: Ok(MediaOutcome::Adopted {
                        content: pending.draft.content.clone(),
                        missing: vec![],
                    }),
                };
                let mut replacement = crate::drafts::DraftRecord::new(
                    app.note_editor.clone().unwrap(),
                    chrono::Utc::now().timestamp_millis(),
                );
                replacement.draft.title = "Replacement draft".into();
                app.active_draft_id = Some(replacement.id.clone());
                app.note_editor = Some(replacement.draft.clone());
                app.draft_catalog
                    .insert(replacement.id.clone(), replacement);
                app.accept_media_result(response, window, cx);
                assert!(
                    app.note_intent.is_none(),
                    "an old draft's media result must not start saving its replacement"
                );
                assert_eq!(app.note_editor.as_ref().unwrap().title, "Replacement draft");
            });
        });
    }

    #[gpui_kit::test]
    fn copy_adoption_still_rejects_a_changed_reader_selection(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let directory = tempfile::tempdir().unwrap().keep();
        let database = directory.join("studyapp.sqlite3");
        let worker = Worker::start(database.clone(), None);
        let (host, cx) = cx.add_window_view(|window, cx| SaveTestHost {
            app: cx.new(|cx| StudyApp::new(worker, window, cx)),
        });
        let view = cx.read(|cx| host.read(cx).app.clone());
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.last_library = database.to_string_lossy().into_owned();
                app.selected_note = Some("original-note".into());
                app.begin_media_adoption(
                    crate::drafts::NoteDraft {
                        asset_group: crate::media::new_group(),
                        note_id: None,
                        expected_hash: None,
                        folder_id: None,
                        title: "Original note copy".into(),
                        content: "Original note content".into(),
                    },
                    None,
                    window,
                    cx,
                );
                let pending = app.media_ui.adoption.as_ref().unwrap();
                let response = MediaResponse {
                    library: pending.library.clone(),
                    generation: pending.generation,
                    request: pending.request,
                    result: Ok(MediaOutcome::Adopted {
                        content: pending.draft.content.clone(),
                        missing: vec![],
                    }),
                };
                app.selected_note = Some("different-note".into());
                app.accept_media_result(response, window, cx);
                assert!(app.note_editor.is_none());
                assert!(app.active_draft_id.is_none());
                assert_eq!(app.selected_note.as_deref(), Some("different-note"));
                assert!(app.note_content.read(cx).value().is_empty());
            });
        });
    }

    #[gpui_kit::test]
    fn save_keeps_editor_frozen_across_adoption_commit_and_retirement(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let directory = tempfile::tempdir().unwrap().keep();
        let database = directory.join("studyapp.sqlite3");
        let library = database.to_string_lossy().into_owned();
        let worker = Worker::start(database, None);
        let (host, cx) = cx.add_window_view(|window, cx| SaveTestHost {
            app: cx.new(|cx| StudyApp::new(worker, window, cx)),
        });
        let view = cx.read(|cx| host.read(cx).app.clone());
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.last_library = library.clone();
                app.draft_loading = false;
                app.start_new_draft(None, window, cx);
                let mut draft = app.note_editor.clone().unwrap();
                draft.content = "正文及已导入图片".into();
                let note_id = draft.asset_group.clone();
                let generation = app.note_content.read(cx).generation();
                app.note_saving = true;
                app.note_content
                    .update(cx, |editor, cx| editor.set_readonly(true, cx));
                app.media_ui.adoption = Some(Adoption {
                    library: library.clone(),
                    generation,
                    request: 901,
                    draft_id: app.active_draft_id.clone(),
                    selected_note: None,
                    selected_version: None,
                    draft,
                    commit_after_adoption: true,
                });
                app.accept_media_result(
                    MediaResponse {
                        library: library.clone(),
                        generation,
                        request: 901,
                        result: Ok(MediaOutcome::Adopted {
                            content: "正文及已导入图片".into(),
                            missing: vec![],
                        }),
                    },
                    window,
                    cx,
                );
                let request = app.note_intent.as_ref().unwrap().0;
                app.editor_event(
                    crate::markdown_editor::MarkdownEvent::Flushed(request),
                    window,
                    cx,
                );
                assert!(matches!(
                    app.draft_persist.as_ref().unwrap().intent,
                    NoteIntent::Commit
                ));
                assert!(
                    app.note_content.read(cx).readonly(),
                    "image adoption reopened input before the commit receipt was durable"
                );
                let commit = app.draft_persist.take().unwrap();
                app.finish_draft_persist(commit.intent, window, cx);
                assert!(app.pending_note_save.is_some());
                let mut snapshot =
                    studyapp_core::Engine::open(&directory.join("metadata-fixture.sqlite3"), 1)
                        .unwrap()
                        .snapshot();
                snapshot.data_path = library.clone();
                let active = app.active_draft_id.clone().unwrap();
                app.draft_catalog.get_mut(&active).unwrap().draft.folder_id =
                    Some("removed-folder".into());
                let revision = app.draft_catalog[&active].revision;
                app.reconcile_draft_folders(&snapshot);
                assert_eq!(
                    app.draft_catalog[&active].revision, revision,
                    "folder metadata invalidated a pending save receipt"
                );
                app.accept_note_save_result(Ok(note_id), window, cx);
                assert!(matches!(
                    app.draft_persist.as_ref().unwrap().intent,
                    NoteIntent::Saved(_)
                ));
                assert!(
                    app.note_content.read(cx).readonly(),
                    "source publication reopened input before the retirement receipt was durable"
                );
                assert_eq!(app.note_content.read(cx).value(), "正文及已导入图片");
                let saved = app.draft_persist.take().unwrap();
                app.finish_draft_persist(saved.intent, window, cx);
                assert!(app.note_editor.is_none());
                assert!(!app.note_saving);

                app.start_new_draft(None, window, cx);
                let id = app.active_draft_id.clone();
                app.note_saving = true;
                app.note_content
                    .update(cx, |editor, cx| editor.set_readonly(true, cx));
                app.accept_note_save_result(Err("fixture publication failure".into()), window, cx);
                assert!(
                    !app.note_content.read(cx).readonly(),
                    "failed save must allow a retry in the same draft"
                );
                assert_eq!(app.active_draft_id, id);
                assert!(app.note_editor.is_some());
                let active = app.active_draft_id.clone().unwrap();
                app.draft_catalog.get_mut(&active).unwrap().draft.folder_id =
                    Some("removed-folder".into());
                app.reconcile_draft_folders(&snapshot);
                assert_eq!(
                    app.draft_catalog[&active].draft.folder_id, None,
                    "deferred folder metadata was never reconciled after failure"
                );
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{asset_group, clipboard_input};
    fn image_insertion() -> super::Insertion {
        super::Insertion {
            library: "library".into(),
            generation: 7,
            group: "group".into(),
            title: "title".into(),
            settings: Default::default(),
            inputs: Default::default(),
            request: Some(99),
            accepted_count: 1,
            clipboard_bytes: 4,
        }
    }
    #[test]
    fn clipboard_queue_appends_in_order_without_replacing_the_inflight_request() {
        use super::MediaInput;
        let mut pending = image_insertion();
        pending
            .append(
                "library",
                7,
                "group",
                [
                    MediaInput::Encoded("AgIC".into()),
                    MediaInput::Encoded("AwMD".into()),
                ]
                .into(),
            )
            .unwrap();
        assert_eq!(pending.request, Some(99));
        assert_eq!(pending.accepted_count, 3);
        assert_eq!(pending.clipboard_bytes, 12);
        assert!(
            matches!(pending.inputs.pop_front(), Some(MediaInput::Encoded(value)) if value == "AgIC")
        );
        assert!(
            matches!(pending.inputs.pop_front(), Some(MediaInput::Encoded(value)) if value == "AwMD")
        );
    }
    #[test]
    fn clipboard_queue_rejects_old_documents_and_other_asset_groups_atomically() {
        for (library, generation, group) in [
            ("other", 7, "group"),
            ("library", 8, "group"),
            ("library", 7, "other"),
        ] {
            let mut pending = image_insertion();
            assert!(
                pending
                    .append(
                        library,
                        generation,
                        group,
                        [super::MediaInput::Encoded("AgIC".into())].into()
                    )
                    .is_err()
            );
            assert_eq!(pending.request, Some(99));
            assert!(pending.inputs.is_empty());
            assert_eq!(pending.accepted_count, 1);
        }
    }
    #[test]
    fn clipboard_queue_limits_include_the_inflight_image_and_completed_session_items() {
        let mut pending = image_insertion();
        pending.accepted_count = super::MAX_FILES;
        assert!(
            pending
                .append(
                    "library",
                    7,
                    "group",
                    [super::MediaInput::Encoded("AgIC".into())].into()
                )
                .is_err()
        );
        pending.accepted_count = 1;
        pending.clipboard_bytes = super::MAX_CLIPBOARD_QUEUE_BYTES;
        assert!(
            pending
                .append(
                    "library",
                    7,
                    "group",
                    [super::MediaInput::Encoded("AgIC".into())].into()
                )
                .is_err()
        );
        assert_eq!(pending.request, Some(99));
        assert!(pending.inputs.is_empty());
        assert_eq!(pending.clipboard_bytes, super::MAX_CLIPBOARD_QUEUE_BYTES);
    }
    #[test]
    fn document_switch_cancels_old_preview_but_keeps_two_tracked_slots() {
        use super::{DocumentKey, MediaUi, ResolveControl};
        let key = |generation| DocumentKey {
            library: "library".into(),
            generation,
            revision: 0,
            source: None,
            settings: Default::default(),
        };
        let mut ui = MediaUi::new();
        ui.observe(Some(key(1)));
        let first = ResolveControl::default();
        ui.resolving.push((1, key(1), first.clone()));
        ui.current_resolve = Some(1);
        ui.observe(Some(key(2)));
        assert!(first.is_cancelled());
        assert!(
            ui.resolution_available(),
            "new local preview can start while old network drains"
        );
        let second = ResolveControl::default();
        ui.resolving.push((2, key(2), second.clone()));
        ui.current_resolve = Some(2);
        ui.observe(Some(key(3)));
        ui.observe(Some(key(4)));
        assert!(second.is_cancelled());
        assert!(
            !ui.resolution_available(),
            "rapid switches must not create unbounded workers"
        );
        assert_eq!(
            ui.resolving.len(),
            2,
            "cancelled storage jobs remain tracked until completion"
        );
        ui.resolving.remove(0);
        assert!(ui.resolution_available());
        assert_eq!(
            ui.observed.unwrap().generation,
            4,
            "only newest pending document is retained"
        );
    }
    #[test]
    fn clipboard_rejects_oversized_payload_before_queueing_for_decode() {
        assert!(
            clipboard_input("A".repeat(crate::media::MAX_IMAGE_BYTES.div_ceil(3) * 4 + 1)).is_err()
        );
        assert!(clipboard_input(String::new()).is_err());
        assert!(
            matches!(clipboard_input("AQID".into()).unwrap(), crate::media::MediaInput::Encoded(value) if value == "AQID")
        );
    }
    #[test]
    fn note_identity_owns_assets_regardless_of_markdown_content() {
        let id = "36b79f08-5128-43ab-a1cb-13f318c455f7";
        assert_eq!(asset_group(Some(id), ""), id);
        assert_ne!(
            asset_group(None, &format!("![图](studyapp-media:{id}/file.png)")),
            id
        );
        let own = "8529f643-e153-4a0b-983b-c32bb6d28da7";
        assert_eq!(
            asset_group(Some(own), &format!("![图](studyapp-media:{id}/file.png)")),
            own
        );
        assert_eq!(
            asset_group(
                Some(own),
                &format!("```\nstudyapp-media:{id}/file.png\n```")
            ),
            own
        );
        assert_ne!(asset_group(None, ""), asset_group(None, ""));
    }
}
