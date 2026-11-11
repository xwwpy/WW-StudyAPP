use crate::components::*;
use crate::controls::Button;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};
use studyapp_desktop::editor_protocol::{EditPatch, EditorDocument};

pub struct MarkdownChanged;
#[derive(Clone)]
pub enum MarkdownEvent {
    OutlinePreference(studyapp_core::OutlinePreference),
    TableView {
        epoch: u64,
        library: String,
        document: String,
        table: String,
        value: Option<studyapp_core::TableView>,
    },
    Save,
    RequestEdit(usize),
    OpenLink(String),
    RequestInsertImage,
    RequestNativeImagePaste {
        generation: u64,
    },
    PasteImage {
        generation: u64,
        name: String,
        mime: String,
        base64: String,
    },
    ExternalDrag {
        phase: String,
        paths: Vec<PathBuf>,
    },
    CancelResize,
    Shortcut(String),
    Flushed(u64),
    Error(String),
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum Message {
    OutlinePreference {
        generation: u64,
        value: studyapp_core::OutlinePreference,
    },
    TableView {
        generation: u64,
        table: String,
        value: Option<studyapp_core::TableView>,
    },
    Ready,
    Change {
        generation: u64,
        #[serde(rename = "baseRevision")]
        base: u64,
        revision: u64,
        changes: Vec<EditPatch>,
    },
    Flush {
        generation: u64,
        request: u64,
        revision: u64,
        #[serde(rename = "normalizedText")]
        normalized_text: String,
    },
    FlushRejected {
        generation: u64,
        request: u64,
        message: String,
    },
    Save {
        generation: u64,
    },
    RequestEdit {
        generation: u64,
        position: usize,
    },
    OpenLink {
        generation: u64,
        url: String,
    },
    RenderDocument {
        generation: u64,
        request: u64,
        page: usize,
        width: u32,
        password: String,
    },
    RequestInsertImage {
        generation: u64,
    },
    RequestNativeImagePaste {
        generation: u64,
    },
    PasteImage {
        generation: u64,
        name: String,
        mime: String,
        base64: String,
    },
    ExternalDrag {
        phase: String,
        paths: Vec<PathBuf>,
    },
    CancelResize {
        generation: u64,
    },
    Shortcut {
        generation: u64,
        action: String,
    },
    Error {
        generation: Option<u64>,
        message: String,
    },
    #[cfg(feature = "ui-test")]
    Test {
        request: serde_json::Value,
        result: serde_json::Value,
    },
    #[serde(other)]
    Other,
}

/// One continuous document lives in CodeMirror. Native persistence receives only
/// revisioned text transactions; no Markdown/HTML parsing runs while typing.
pub struct MarkdownEditor {
    document: EditorDocument,
    tables: TableBinding,
    table_epoch: u64,
    sender: mpsc::Sender<String>,
    receiver: mpsc::Receiver<String>,
    host: Option<crate::web_editor_host::HostedEditor>,
    #[cfg(target_os = "linux")]
    native_closed: bool,
    creating: bool,
    suspended: bool,
    host_epoch: u64,
    ready: bool,
    visible: bool,
    modal_overlay: Option<f32>,
    readonly: bool,
    preview: Option<std::sync::Arc<studyapp_core::documents::DocumentPreview>>,
    pdf_busy: bool,
    preview_checked: bool,
    preview_validating: bool,
    preview_error: Option<String>,
    resume_readonly: bool,
    can_request_edit: bool,
    outline_enabled: bool,
    outline_preference: Option<studyapp_core::OutlinePreference>,
    theme: serde_json::Value,
    error: Option<String>,
    pending: Option<(u64, Instant)>,
    recovering: bool,
    request: u64,
    shortcuts: std::collections::BTreeMap<String, String>,
    images: BTreeMap<String, String>,
    occlusions: crate::overlay_regions::OverlayRegions<studyapp_ui::bubble::OverlayShape>,
    #[cfg(feature = "ui-test")]
    pub(crate) test_results: std::collections::HashMap<String, serde_json::Value>,
    _poll: Task<()>,
}
fn valid_open_link(generation: u64, current: u64, interactive: bool, url: &str) -> bool {
    generation == current
        && interactive
        && !url.is_empty()
        && url.len() <= 8192
        && !url.chars().any(|c| c.is_control() || c.is_whitespace())
        && reqwest::Url::parse(url).is_ok_and(|value| {
            matches!(value.scheme(), "https" | "http" | "mailto")
                && value.username().is_empty()
                && value.password().is_none()
        })
}

fn valid_outline_message(generation: u64, current: u64, interactive: bool) -> bool {
    generation == current && interactive
}

impl EventEmitter<MarkdownChanged> for MarkdownEditor {}
impl EventEmitter<MarkdownEvent> for MarkdownEditor {}

fn schedule_editor_profile(
    executor: &BackgroundExecutor,
    resolve: impl FnOnce() -> anyhow::Result<studyapp_desktop::app_paths::AppPaths> + Send + 'static,
) -> Task<anyhow::Result<PathBuf>> {
    executor.spawn(async move { resolve().map(|paths| paths.editor_profile()) })
}

#[derive(Default)]
struct TableBinding {
    current: Option<(String, String)>,
    views: studyapp_core::DocumentTableViews,
    pending: Option<((String, String), studyapp_core::DocumentTableViews)>,
}
impl TableBinding {
    fn stage(
        &mut self,
        library: String,
        document: String,
        views: studyapp_core::DocumentTableViews,
    ) {
        let context = (library, document);
        self.pending = (self.current.as_ref() != Some(&context)).then_some((context, views));
    }
    fn commit(&mut self) {
        if let Some((context, views)) = self.pending.take() {
            self.current = Some(context);
            self.views = views;
        }
    }
}

fn valid_table_message(
    generation: u64,
    expected: u64,
    document_bound: bool,
    table: &str,
    value: &Option<studyapp_core::TableView>,
) -> bool {
    generation == expected
        && document_bound
        && studyapp_core::valid_table_key(table)
        && value.as_ref().is_none_or(|v| v.validate().is_ok())
}

impl MarkdownEditor {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        crate::audit_log::stage(
            studyapp_core::LogLevel::Debug,
            crate::audit_log::Phase::EditorCreated,
            crate::audit_log::Outcome::Success,
        );
        #[cfg(target_os = "linux")]
        cx.on_app_quit(|this, _| {
            // This must run synchronously: GPUI clears its windows before it
            // polls futures returned by quit observers.
            this.close_native_host();
            async {}
        })
        .detach();
        let (sender, receiver) = mpsc::channel();
        let poll = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(25))
                    .await;
                #[cfg(target_os = "linux")]
                crate::web_editor_host::HostedEditor::pump_events();
                if !crate::owned_poll::update(&this, cx, |this, _, cx| this.poll(cx)) {
                    break;
                }
            }
        });
        Self {
            document: EditorDocument::new(String::new()),
            tables: Default::default(),
            table_epoch: 0,
            sender,
            receiver,
            host: None,
            #[cfg(target_os = "linux")]
            native_closed: false,
            creating: false,
            suspended: false,
            host_epoch: 0,
            ready: false,
            visible: false,
            modal_overlay: None,
            readonly: false,
            preview: None,
            pdf_busy: false,
            preview_checked: false,
            preview_validating: false,
            preview_error: None,
            resume_readonly: false,
            can_request_edit: false,
            outline_enabled: true,
            outline_preference: None,
            theme: serde_json::Value::Null,
            error: None,
            pending: None,
            recovering: false,
            request: 0,
            shortcuts: crate::shortcuts::default_bindings(),
            images: BTreeMap::new(),
            occlusions: Default::default(),
            #[cfg(feature = "ui-test")]
            test_results: Default::default(),
            _poll: poll,
        }
    }

    /// Run after durable persistence, while the GPUI X11 parent still exists.
    #[cfg(target_os = "linux")]
    pub(crate) fn close_native_host(&mut self) {
        self.native_closed = true;
        self.visible = false;
        self.ready = false;
        self.creating = false;
        self.reset_host_messages();
        drop(self.host.take());
    }

    pub fn value(&self) -> &str {
        self.document.text()
    }

    pub(crate) fn set_outline_preference(
        &mut self,
        value: Option<studyapp_core::OutlinePreference>,
        cx: &mut Context<Self>,
    ) {
        if self.outline_preference == value {
            return;
        }
        self.outline_preference = value;
        if self.ready {
            let value = value.unwrap_or_default();
            if let Err(error) = self.evaluate(&format!(
                "window.studyappEditor.setOutlinePreference({});",
                serde_json::to_string(&value).unwrap()
            )) {
                self.fail(format!("无法恢复目录布局：{error:#}"), cx);
            }
        }
    }

    pub fn without_outline(mut self) -> Self {
        self.outline_enabled = false;
        self
    }

    #[cfg(all(test, feature = "ui-test"))]
    pub(crate) fn outline_enabled(&self) -> bool {
        self.outline_enabled
    }
    pub fn generation(&self) -> u64 {
        self.document.generation()
    }
    pub fn revision(&self) -> u64 {
        self.document.revision()
    }
    pub fn editable(&self) -> bool {
        self.ready()
            && !self.readonly
            && self.modal_overlay.is_none()
            && self.pending.is_none()
            && !self.recovering
    }

    /// Image data comes only from the bounded native image resolver.
    pub fn set_images(
        &mut self,
        generation: u64,
        images: BTreeMap<String, String>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.document.generation() {
            return;
        }
        if self.images == images {
            return;
        }
        self.images = images;
        if self.ready && self.error.is_none() {
            self.send_images(cx);
        }
    }

    fn send_images(&mut self, cx: &mut Context<Self>) {
        let payload =
            serde_json::json!({ "generation": self.document.generation(), "images": self.images });
        if let Err(error) = self.evaluate(&format!("window.studyappEditor.setImages({payload});")) {
            self.fail(format!("无法更新图片预览：{error:#}"), cx);
        }
    }

    pub fn insert_markdown(
        &mut self,
        generation: u64,
        text: &str,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            generation == self.document.generation() && self.editable(),
            "当前笔记已切换或无法编辑；图片文件已保留"
        );
        let text = serde_json::to_string(text)?;
        self.evaluate(&format!("window.studyappEditor.insertText({text});"))?;
        cx.notify();
        Ok(())
    }

    pub fn set_occlusions(&mut self, bounds: Vec<Bounds<Pixels>>, cx: &mut Context<Self>) {
        self.set_overlay_regions("notices", bounds, cx);
    }

    pub fn set_overlay_regions(
        &mut self,
        source: &'static str,
        bounds: Vec<Bounds<Pixels>>,
        cx: &mut Context<Self>,
    ) {
        self.set_overlay_shapes(
            source,
            bounds
                .into_iter()
                .map(studyapp_ui::bubble::OverlayShape::card)
                .collect(),
            cx,
        );
    }

    pub fn set_overlay_shapes(
        &mut self,
        source: &'static str,
        shapes: Vec<studyapp_ui::bubble::OverlayShape>,
        cx: &mut Context<Self>,
    ) {
        if !self.occlusions.set(source, shapes) {
            return;
        }
        if let Some(host) = &self.host
            && let Err(error) = host.set_occlusions(self.occlusions.all())
        {
            self.fail(format!("无法更新窗口遮挡区域：{error:#}"), cx);
        }
    }
    pub fn ready(&self) -> bool {
        self.ready && self.error.is_none()
    }
    pub fn creating(&self) -> bool {
        self.creating
    }
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    pub(crate) fn readonly(&self) -> bool {
        self.readonly
    }

    pub(crate) fn table_epoch(&self) -> u64 {
        self.table_epoch
    }

    pub(crate) fn clear_table_context(&mut self) {
        // A restore can replace a database without changing its path or note
        // UUIDs. Old optimistic display values and queued events must not win.
        self.tables = TableBinding::default();
        self.table_epoch = self.table_epoch.wrapping_add(1);
    }

    pub(crate) fn set_table_context(
        &mut self,
        library: String,
        document: String,
        views: studyapp_core::DocumentTableViews,
    ) {
        self.tables.stage(library, document, views);
    }

    pub fn set_value(&mut self, value: String, window: &mut Window, cx: &mut Context<Self>) {
        self.set_value_with_readonly(value, false, window, cx);
    }

    /// Load the document and its input state in the same browser message. Saving
    /// can replace image references without briefly reopening the flushed editor.
    pub(crate) fn set_value_with_readonly(
        &mut self,
        value: String,
        readonly: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.poll(cx);
        self.tables.commit();
        self.preview = studyapp_core::documents::DocumentPreview::from_content(&value)
            .map(std::sync::Arc::new);
        self.preview_checked = false;
        self.preview_validating = false;
        self.preview_error = None;
        let readonly = readonly || self.preview.is_some();
        let value = if self.preview.is_some() {
            "原文档阅读；页面内容和版式保存在资料库版本中。".to_owned()
        } else {
            value
        };
        self.document.reset(value);
        self.images.clear();
        self.readonly = readonly;
        self.resume_readonly = readonly;
        self.can_request_edit = false;
        self.pending = None;
        self.recovering = false;
        // A protocol failure can be recovered only by explicitly loading a
        // document again. A runtime creation failure remains visible.
        if self.ready {
            self.error = None;
            self.load(cx);
        }
        cx.notify();
    }

    pub(crate) fn rebase_saved_value(
        &mut self,
        value: String,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.readonly() && self.pending.is_none(),
            "请等待当前输入确认后再保存"
        );
        if value == self.document.text() {
            return Ok(());
        }
        let previous_generation = self.document.generation();
        let previous_revision = self.document.revision();
        let changes = studyapp_desktop::editor_protocol::rebase_edits(self.document.text(), &value);
        self.document.reset(value);
        if self.ready {
            let payload = serde_json::json!({
                "previousGeneration": previous_generation, "previousRevision": previous_revision,
                "generation": self.document.generation(), "text": self.document.text(),
                "readonly": true, "changes": changes
            });
            if let Err(error) = self.evaluate(&format!("window.studyappEditor.rebase({payload});"))
            {
                self.fail(
                    format!("无法更新保存中的正文，当前草稿仍保留：{error:#}"),
                    cx,
                );
                return Err(error);
            }
        }
        cx.notify();
        Ok(())
    }

    fn evaluate(&self, script: &str) -> anyhow::Result<()> {
        if let Some(host) = &self.host {
            return host.evaluate(script);
        }
        anyhow::bail!("编辑器尚未准备好")
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        if !self.preview_checked
            && let Some(document) = self
                .preview
                .as_ref()
                .filter(|document| document.kind == "docx")
                .cloned()
        {
            let generation = self.document.generation();
            let payload = serde_json::json!({"generation":generation,"text":"正在检查原文档，请稍候…","readonly":true,"outlineEnabled":false});
            let _ = self.evaluate(&format!("window.studyappEditor.load({payload});"));
            if !self.preview_validating {
                self.preview_validating = true;
                cx.spawn(async move |this, cx| {
                    let result = cx.background_executor().spawn(async move { document.validate().map_err(|_| "原文档格式、编码或大小检查未通过。请重新导入可正常打开的 DOCX 文件；原文件未被改变。".to_owned()) }).await;
                    let _ = this.update(cx, |this, cx| {
                        if this.document.generation() != generation { return; }
                        this.preview_validating = false;
                        this.preview_checked = true;
                        this.preview_error = result.err();
                        if this.ready { this.load(cx); }
                    });
                }).detach();
            }
            return;
        }
        if let Some(error) = &self.preview_error {
            let payload = serde_json::json!({"generation":self.document.generation(),"text":error,"readonly":true,"outlineEnabled":false});
            let _ = self.evaluate(&format!("window.studyappEditor.load({payload});"));
            return;
        }
        let payload = serde_json::json!({"generation": self.document.generation(), "text": self.document.text(), "readonly": self.readonly, "canRequestEdit": self.can_request_edit, "outlineEnabled": self.outline_enabled, "outlinePreference": self.outline_preference, "tableViews": self.tables.views, "tableConfigurable": self.tables.current.is_some(), "positionKey": self.tables.current.as_ref().map(|key| serde_json::to_string(key).expect("document identity")), "document": self.preview.as_deref().map(|document| if document.kind == "pdf" { serde_json::json!({"kind":"pdf","data":""}) } else { serde_json::json!(document) })});
        let shortcuts = serde_json::to_string(&self.shortcuts).expect("string map serializes");
        let theme = &self.theme;
        if let Err(error) = self.evaluate(&format!("window.studyappEditor.setShortcuts({shortcuts}); window.studyappEditor.setTheme({theme}); window.studyappEditor.load({payload}); {}", self.modal_overlay_script())) {
            self.fail(format!("无法载入编辑器：{error:#}"), cx);
        }
        if self.error.is_none() {
            self.send_images(cx);
        }
    }

    pub fn set_theme(&mut self, theme: serde_json::Value, cx: &mut Context<Self>) {
        if self.theme == theme {
            return;
        }
        self.theme = theme;
        if self.ready
            && self.error.is_none()
            && let Err(error) =
                self.evaluate(&format!("window.studyappEditor.setTheme({});", self.theme))
        {
            self.fail(format!("无法更新编辑器外观：{error:#}"), cx);
        }
    }

    pub fn set_modal_overlay(&mut self, opacity: Option<f32>, cx: &mut Context<Self>) {
        if self.modal_overlay == opacity {
            return;
        }
        self.modal_overlay = opacity;
        if opacity.is_none() {
            self.set_overlay_shapes("blocking-modal", vec![], cx);
        }
        if let Some(host) = &self.host
            && let Err(error) = host.set_input_blocked(opacity.is_some())
        {
            self.fail(format!("无法更新编辑器输入状态：{error:#}"), cx);
        }
        if self.ready
            && self.error.is_none()
            && let Err(error) = self.evaluate(&self.modal_overlay_script())
        {
            self.fail(format!("无法更新编辑器遮罩：{error:#}"), cx);
        }
    }

    fn modal_overlay_script(&self) -> String {
        format!(
            "window.studyappEditor.setModalOverlay({}, {});",
            self.modal_overlay.is_some(),
            self.modal_overlay.unwrap_or(0.)
        )
    }

    pub fn set_can_request_edit(&mut self, enabled: bool, cx: &mut Context<Self>) {
        let enabled = enabled && self.preview.is_none();
        if self.can_request_edit == enabled {
            return;
        }
        self.can_request_edit = enabled;
        if self.ready
            && self.error.is_none()
            && let Err(error) = self.evaluate(&format!(
                "window.studyappEditor.setCanRequestEdit({enabled});"
            ))
        {
            self.fail(format!("无法更新编辑入口：{error:#}"), cx);
        }
    }

    pub fn restore_selection(&mut self, position: usize, cx: &mut Context<Self>) {
        if self.ready
            && !self.readonly
            && let Err(error) = self.evaluate(&format!(
                "window.studyappEditor.restoreSelection({position});"
            ))
        {
            self.fail(format!("无法恢复编辑位置：{error:#}"), cx);
        }
    }

    pub fn set_shortcuts(
        &mut self,
        shortcuts: std::collections::BTreeMap<String, String>,
        cx: &mut Context<Self>,
    ) {
        if self.shortcuts == shortcuts {
            return;
        }
        self.shortcuts = shortcuts;
        if self.ready && self.error.is_none() {
            let value = serde_json::to_string(&self.shortcuts).expect("string map serializes");
            if let Err(error) =
                self.evaluate(&format!("window.studyappEditor.setShortcuts({value});"))
            {
                self.fail(format!("无法更新快捷键：{error:#}"), cx);
            }
        }
    }

    pub fn set_readonly(&mut self, readonly: bool, cx: &mut Context<Self>) {
        let readonly = readonly || self.preview.is_some();
        self.readonly = readonly;
        self.resume_readonly = readonly;
        if self.ready
            && self.error.is_none()
            && let Err(error) =
                self.evaluate(&format!("window.studyappEditor.setReadonly({readonly});"))
        {
            self.fail(format!("无法更新编辑状态：{error:#}"), cx);
        }
        cx.notify();
    }

    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        crate::audit_log::record(crate::audit_log::Record {
            count: Some(u64::from(visible)),
            ..crate::audit_log::Record::new(
                studyapp_core::LogLevel::Trace,
                crate::audit_log::Phase::EditorVisibility,
                crate::audit_log::Outcome::Success,
            )
        });
        self.visible = visible;
        if let Some(host) = &self.host
            && let Err(error) = host.set_visible(visible && self.ready)
        {
            self.fail(format!("无法显示编辑区：{error:#}"), cx);
        }
        cx.notify();
    }

    #[cfg(all(test, feature = "ui-test"))]
    pub(crate) fn modal_blocked_for_test(&self) -> bool {
        self.modal_overlay.is_some()
    }

    #[cfg(all(test, feature = "ui-test"))]
    pub(crate) fn requested_visible_for_test(&self) -> bool {
        self.visible
    }

    /// The caller records its intent before this acknowledgement is dispatched.
    /// Saving, navigation and close must wait for MarkdownEvent::Flushed.
    pub fn request_flush(&mut self, cx: &mut Context<Self>) -> anyhow::Result<u64> {
        anyhow::ensure!(self.pending.is_none(), "正在确认最后的输入，请稍候");
        anyhow::ensure!(
            self.error.is_none() || !self.ready,
            "编辑状态异常，当前内容已保留，请先恢复编辑器"
        );
        self.request = self.request.wrapping_add(1).max(1);
        let request = self.request;
        if !self.ready {
            // The HWND stays hidden until ready. No browser input can exist yet.
            self.readonly = true;
            cx.emit(MarkdownEvent::Flushed(request));
        } else {
            self.pending = Some((request, Instant::now()));
            if let Err(error) = self.evaluate(&format!(
                "window.studyappEditor.flush({{request:{request}}});"
            )) {
                self.pending = None;
                return Err(error);
            }
        }
        Ok(request)
    }

    fn fail(&mut self, message: String, cx: &mut Context<Self>) {
        self.pending = None;
        self.recovering = false;
        self.error = Some(message.clone());
        self.readonly = true;
        if self.ready {
            let _ = self.evaluate("window.studyappEditor.setReadonly(true);");
        }
        cx.emit(MarkdownEvent::Error(message));
        cx.notify();
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        // Native construction may pump the foreground event loop. In that
        // interval the page can already post Ready, but evaluate() has no host
        // to target yet. Leave the queue intact until this exact host attaches.
        if self.creating || self.host.is_none() || self.suspended {
            return;
        }
        let mut changed = false;
        for _ in 0..256 {
            let Ok(json) = self.receiver.try_recv() else {
                break;
            };
            let result = (|| -> anyhow::Result<()> {
                match serde_json::from_str::<Message>(&json)? {
                    Message::Ready => {
                        if self.ready {
                            return Ok(());
                        }
                        crate::audit_log::stage(
                            studyapp_core::LogLevel::Debug,
                            crate::audit_log::Phase::EditorReady,
                            crate::audit_log::Outcome::Success,
                        );
                        self.ready = true;
                        self.load(cx);
                        cx.notify();
                    }
                    Message::Change {
                        generation,
                        base,
                        revision,
                        changes,
                    } => {
                        if self.error.is_some() || self.recovering {
                            return Ok(());
                        }
                        changed |= self.document.apply(generation, base, revision, &changes)?;
                    }
                    Message::Flush {
                        generation,
                        request,
                        revision,
                        normalized_text,
                    } => {
                        if !self.document.matches_flush_response(
                            generation,
                            request,
                            self.pending.as_ref().map(|(id, _)| *id),
                        ) {
                            return Ok(());
                        }
                        if self.recovering {
                            self.document.recover(&normalized_text)?;
                            self.pending = None;
                            self.recovering = false;
                            self.readonly = self.resume_readonly;
                            self.error = None;
                            self.load(cx);
                            changed = true;
                            cx.notify();
                            return Ok(());
                        }
                        self.document
                            .verify_flush(generation, revision, &normalized_text)?;
                        self.pending = None;
                        self.readonly = true;
                        cx.emit(MarkdownEvent::Flushed(request));
                    }
                    Message::FlushRejected {
                        generation,
                        request,
                        message,
                    } => {
                        if self.document.matches_flush_response(
                            generation,
                            request,
                            self.pending.as_ref().map(|(id, _)| *id),
                        ) {
                            self.pending = None;
                            self.recovering = false;
                            self.readonly = self.resume_readonly;
                            self.evaluate(&format!(
                                "window.studyappEditor.setReadonly({});",
                                self.readonly
                            ))?;
                            // A safe refusal cancels the caller's save intent,
                            // while preserving an editable, synchronized draft.
                            cx.emit(MarkdownEvent::Error(message));
                            cx.notify();
                        }
                    }
                    Message::Save { generation } => {
                        if generation == self.document.generation() && !self.readonly {
                            cx.emit(MarkdownEvent::Save);
                        }
                    }
                    Message::RequestEdit {
                        generation,
                        position,
                    } => {
                        if generation == self.document.generation()
                            && self.visible
                            && self.readonly
                            && self.can_request_edit
                            && self.pending.is_none()
                            && !self.recovering
                        {
                            cx.emit(MarkdownEvent::RequestEdit(position));
                        }
                    }
                    Message::RenderDocument {
                        generation,
                        request,
                        page,
                        width,
                        password,
                    } => {
                        if generation == self.document.generation()
                            && self.visible
                            && self.ready
                            && self.pending.is_none()
                            && !self.recovering
                            && password.len() <= 1024
                            && (96..=3200).contains(&width)
                            && let Some(document) = self
                                .preview
                                .as_ref()
                                .filter(|document| document.kind == "pdf")
                                .cloned()
                        {
                            if self.pdf_busy {
                                let value = serde_json::json!({"generation":generation,"request":request,"retry":true});
                                let _ = self.evaluate(&format!(
                                    "window.studyappEditor.documentResult({value});"
                                ));
                            } else {
                                self.pdf_busy = true;
                                let epoch = self.host_epoch;
                                cx.spawn(async move |this, cx| {
                                    let mut value = cx
                                        .background_executor()
                                        .spawn(async move {
                                            crate::document_renderer::render(
                                                &document,
                                                page,
                                                width,
                                                zeroize::Zeroizing::new(password),
                                            )
                                        })
                                        .await;
                                    value["generation"] = generation.into();
                                    value["request"] = request.into();
                                    let _ = this.update(cx, |this, _| {
                                        this.pdf_busy = false;
                                        if generation == this.document.generation()
                                            && epoch == this.host_epoch
                                            && this.ready
                                        {
                                            let _ = this.evaluate(&format!(
                                                "window.studyappEditor.documentResult({value});"
                                            ));
                                        }
                                    });
                                })
                                .detach();
                            }
                        }
                    }
                    Message::OpenLink { generation, url } => {
                        if valid_open_link(
                            generation,
                            self.document.generation(),
                            self.visible
                                && self.modal_overlay.is_none()
                                && self.pending.is_none()
                                && !self.recovering,
                            &url,
                        ) {
                            cx.emit(MarkdownEvent::OpenLink(url));
                        }
                    }
                    Message::RequestInsertImage { generation } => {
                        if generation == self.document.generation()
                            && self.visible
                            && self.editable()
                        {
                            cx.emit(MarkdownEvent::RequestInsertImage);
                        }
                    }
                    Message::RequestNativeImagePaste { generation } => {
                        if generation == self.document.generation()
                            && self.visible
                            && self.editable()
                        {
                            cx.emit(MarkdownEvent::RequestNativeImagePaste { generation });
                        }
                    }
                    Message::PasteImage {
                        generation,
                        name,
                        mime,
                        base64,
                    } => {
                        if generation == self.document.generation()
                            && self.visible
                            && self.editable()
                        {
                            cx.emit(MarkdownEvent::PasteImage {
                                generation,
                                name,
                                mime,
                                base64,
                            });
                        }
                    }
                    // HostedEditor filters page-authored externalDrag messages;
                    // only the operating system drop callback supplies paths.
                    Message::ExternalDrag { phase, paths } => {
                        cx.emit(MarkdownEvent::ExternalDrag { phase, paths });
                    }
                    Message::OutlinePreference { generation, value } => {
                        if valid_outline_message(
                            generation,
                            self.document.generation(),
                            self.ready
                                && self.outline_enabled
                                && self.visible
                                && self.pending.is_none(),
                        ) && self.outline_preference != Some(value)
                        {
                            self.outline_preference = Some(value);
                            cx.emit(MarkdownEvent::OutlinePreference(value));
                        }
                    }
                    Message::TableView {
                        generation,
                        table,
                        value,
                    } => {
                        if valid_table_message(
                            generation,
                            self.document.generation(),
                            self.tables.current.is_some(),
                            &table,
                            &value,
                        ) && let Some((library, document)) = self.tables.current.clone()
                        {
                            if value.is_some()
                                && !self.tables.views.contains_key(&table)
                                && self.tables.views.len() >= 128
                            {
                                cx.emit(MarkdownEvent::Error(
                                    "该笔记的表格显示配置已达上限，请先恢复不再使用的表格默认显示"
                                        .into(),
                                ));
                                return Ok(());
                            }
                            if let Some(value) = &value {
                                self.tables.views.insert(table.clone(), value.clone());
                            } else {
                                self.tables.views.remove(&table);
                            }
                            cx.emit(MarkdownEvent::TableView {
                                epoch: self.table_epoch,
                                library,
                                document,
                                table,
                                value,
                            });
                        }
                    }
                    Message::CancelResize { generation } => {
                        if generation == self.document.generation() && self.visible {
                            cx.emit(MarkdownEvent::CancelResize);
                        }
                    }
                    Message::Shortcut { generation, action } => {
                        if generation == self.document.generation()
                            && self.visible
                            && self.pending.is_none()
                            && action.starts_with("app.")
                            && crate::shortcuts::SHORTCUT_ACTIONS
                                .iter()
                                .any(|entry| entry.id == action)
                        {
                            cx.emit(MarkdownEvent::Shortcut(action));
                        }
                    }
                    Message::Error {
                        generation,
                        message,
                    } => {
                        if generation.is_none_or(|g| g == self.document.generation()) {
                            anyhow::bail!("{message}");
                        }
                    }
                    #[cfg(feature = "ui-test")]
                    Message::Test { request, result } => {
                        self.test_results.insert(
                            request
                                .as_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| request.to_string()),
                            result,
                        );
                    }
                    Message::Other => {}
                }
                Ok(())
            })();
            if let Err(error) = result {
                self.fail(
                    format!("编辑器同步失败：{error:#}。已保存的版本未被覆盖。"),
                    cx,
                );
            }
        }
        if changed {
            cx.emit(MarkdownChanged);
        }
        if self
            .pending
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() > Duration::from_secs(12))
        {
            self.fail(
                "等待输入法完成超时，内容仍保留；请结束组合输入后重新打开草稿。".into(),
                cx,
            );
        }
    }

    /// The channel belongs to one native host, not to the document. Dropping
    /// its receiver revokes queued and late callbacks from a retired WebView,
    /// including Ready/Error messages that lack a document generation.
    fn reset_host_messages(&mut self) {
        (self.sender, self.receiver) = mpsc::channel();
        self.host_epoch = self.host_epoch.wrapping_add(1);
    }

    pub(crate) fn suspend_for_migration(&mut self, suspended: bool, cx: &mut Context<Self>) {
        #[cfg(feature = "ui-test")]
        eprintln!(
            "migration editor suspended={suspended} host={} creating={} epoch={}",
            self.host.is_some(),
            self.creating,
            self.host_epoch
        );
        self.suspended = suspended;
        self.reset_host_messages();
        self.host = None;
        self.creating = false;
        self.ready = false;
        self.error = None;
        cx.notify();
    }

    /// Start the built-in WebKit process during application startup, without
    /// selecting a document or making the child visible/focused. Cold process
    /// creation can then overlap the initial native storage load.
    #[cfg(all(target_os = "macos", not(test)))]
    pub(crate) fn prewarm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_host(window, true, cx);
    }

    fn ensure_host(&mut self, window: &mut Window, eager: bool, cx: &mut Context<Self>) {
        #[cfg(target_os = "linux")]
        if self.native_closed {
            return;
        }
        if self.suspended
            || crate::app_migration::startup_pending()
            || self.host.is_some()
            || self.creating
            || self.error.is_some()
            || (!self.visible && !eager)
        {
            return;
        }
        let parent = match crate::web_editor_host::ParentWindow::capture(window) {
            Ok(parent) => parent,
            Err(error) => {
                self.fail(format!("无法创建编辑窗口：{error:#}"), cx);
                return;
            }
        };
        self.reset_host_messages();
        let sender = self.sender.clone();
        self.creating = true;
        let epoch = self.host_epoch;
        cx.spawn(async move |this, cx| {
            // Wry pumps Windows messages. Absolutely no GPUI App/entity borrow
            // may be held during the constructor (including a render callback).
            cx.background_executor()
                .timer(Duration::from_millis(1))
                .await;
            #[cfg(target_os = "linux")]
            if this
                .update(cx, |this, _| this.native_closed)
                .unwrap_or(true)
            {
                return;
            }
            // Resolve the locator on a worker. Render only schedules this work.
            let profile =
                schedule_editor_profile(cx.background_executor(), crate::app_usage::runtime_paths)
                    .await;
            if this
                .update(cx, |this, _| this.suspended || this.host_epoch != epoch)
                .unwrap_or(true)
            {
                return;
            }
            let profile = match profile {
                Ok(profile) => profile,
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        this.creating = false;
                        this.fail(format!("无法定位编辑器缓存：{error:#}"), cx);
                    });
                    return;
                }
            };
            #[cfg(feature = "ui-test")]
            eprintln!(
                "native editor: creating WebView profile={}",
                profile.display()
            );
            #[cfg(all(windows, feature = "update-acceptance"))]
            crate::update_acceptance::editor_profile(&profile);
            #[cfg(all(windows, feature = "update-acceptance"))]
            let diagnostic_profile = profile.clone();
            let result = crate::web_editor_host::HostedEditor::new(parent, sender, profile);
            #[cfg(all(windows, feature = "update-acceptance"))]
            if let Err(error) = &result {
                crate::update_acceptance::editor_creation_failed(
                    &diagnostic_profile,
                    &format!("{error:#}"),
                );
            }
            #[cfg(feature = "ui-test")]
            eprintln!("native editor: constructor finished");
            let _ = this.update(cx, |this, cx| {
                if this.suspended || this.host_epoch != epoch {
                    drop(result);
                    return;
                }
                this.creating = false;
                #[cfg(target_os = "linux")]
                if this.native_closed {
                    drop(result);
                    return;
                }
                match result {
                    Ok(host) => {
                        if let Err(error) = host.set_input_blocked(this.modal_overlay.is_some()) {
                            this.fail(format!("无法更新编辑器输入状态：{error:#}"), cx);
                        }
                        if let Err(error) = host.set_occlusions(this.occlusions.all()) {
                            this.fail(format!("无法更新窗口遮挡区域：{error:#}"), cx);
                        }
                        this.host = Some(host);
                        cx.notify();
                    }
                    Err(error) => this.fail(
                        format!("连续编辑器启动失败：{error:#}。请确认系统 WebView 运行时可用。"),
                        cx,
                    ),
                }
            });
        })
        .detach();
    }

    #[cfg(feature = "ui-test")]
    pub(crate) fn test_profile_marker(&self, write: bool) -> anyhow::Result<()> {
        let write = if write {
            "localStorage.setItem('studyapp-relocation-smoke','persisted');"
        } else {
            ""
        };
        self.evaluate(&format!("{write}window.ipc.postMessage(JSON.stringify({{type:'test',generation:0,request:'migration-profile-marker',result:localStorage.getItem('studyapp-relocation-smoke')}}));"))
    }

    #[cfg(feature = "ui-test")]
    pub(crate) fn test(&self, action: serde_json::Value) -> anyhow::Result<()> {
        self.evaluate(&format!("window.studyappEditor.test({action});"))
    }
    #[cfg(feature = "ui-test")]
    pub(crate) fn inject_unfinished_character(&self) -> anyhow::Result<()> {
        // Omitting request keeps the intermediate unpaired UTF-16 snapshot in
        // JavaScript; serializing it through test-result IPC would be invalid.
        self.evaluate(
            "void (async () => {
                const { text } = await window.studyappEditor.test({ type: 'snapshot' });
                await window.studyappEditor.test({
                    type: 'replace', from: text.length, to: text.length,
                    text: String.fromCharCode(0xD83E)
                });
            })();",
        )
    }
    #[cfg(feature = "ui-test")]
    pub(crate) fn inject_test_error(&self, stale: bool) {
        let generation = self.document.generation() + u64::from(stale);
        self.sender
            .send(
                serde_json::json!({
                    "type":"error", "generation":generation, "message":"模拟可恢复的输入确认失败"
                })
                .to_string(),
            )
            .unwrap();
    }

    #[cfg(all(feature = "ui-test", any(target_os = "macos", target_os = "linux")))]
    pub(crate) fn startup_diagnostic(&self) -> serde_json::Value {
        serde_json::json!({"ready":self.ready,"creating":self.creating,"hostEpoch":self.host_epoch,
            "hostAttached":self.host.is_some(),
            "visible":self.visible,"readonly":self.readonly,"themeInitialized":!self.theme.is_null(),
            "error":self.error,"native":self.native_state().ok()})
    }

    #[cfg(feature = "ui-test")]
    pub(crate) fn native_state(&self) -> anyhow::Result<serde_json::Value> {
        self.host
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("WebView 尚未创建"))?
            .native_state()
    }

    #[cfg(all(windows, feature = "ui-test"))]
    pub(crate) fn insert_text(
        &self,
        text: &str,
    ) -> anyhow::Result<mpsc::Receiver<Result<(), String>>> {
        self.host
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("WebView 尚未创建"))?
            .insert_text(text)
    }

    #[cfg(all(windows, feature = "ui-test"))]
    pub(crate) fn dispatch_key(
        &self,
        key: &str,
        code: &str,
        modifiers: u32,
        virtual_key: i32,
    ) -> anyhow::Result<mpsc::Receiver<Result<(), String>>> {
        self.host
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("WebView 尚未创建"))?
            .dispatch_key(key, code, modifiers, virtual_key)
    }

    #[cfg(all(windows, feature = "ui-test"))]
    pub(crate) fn capture_png(
        &self,
        path: std::path::PathBuf,
    ) -> anyhow::Result<mpsc::Receiver<Result<(), String>>> {
        self.host
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("WebView 尚未创建"))?
            .capture_png(path)
    }
}

impl Render for MarkdownEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_host(window, false, cx);
        let weak = cx.weak_entity();
        #[cfg(target_os = "linux")]
        let focus_weak = weak.clone();
        column()
            .id("continuous-markdown-editor")
            .test_support()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .when(self.error.is_some(), |c| {
                c.child(muted(self.error.clone().unwrap_or_default()))
                    .when(self.ready, |c| {
                        c.child(
                            Button::new("recover-editor-bridge")
                                .primary()
                                .action("重试同步并恢复输入")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.error = None;
                                    this.recovering = true;
                                    if let Err(error) = this.request_flush(cx) {
                                        this.fail(error.to_string(), cx);
                                    }
                                    cx.notify();
                                })),
                        )
                    })
            })
            .when(!self.ready && self.error.is_none(), |c| {
                c.child(muted("正在准备连续编辑器…"))
            })
            .child(
                document_viewport()
                    .id("markdown-native-viewport")
                    .test_support()
                    .child(
                        canvas(
                            move |bounds, window, cx| {
                                let bounds = bounds.intersect(&window.content_mask().bounds);
                                let _ = weak.update(cx, |this, cx| {
                                    if let Some(host) = &this.host {
                                        let result = host.set_bounds(bounds).and_then(|()| {
                                            host.set_visible(
                                                this.visible
                                                    && this.ready
                                                    && bounds.size.width > px(0.)
                                                    && bounds.size.height > px(0.),
                                            )
                                        });
                                        if let Err(error) = result {
                                            this.fail(format!("编辑器布局失败：{error:#}"), cx);
                                        }
                                    }
                                });
                            },
                            move |_, _, _window, _| {
                                #[cfg(target_os = "linux")]
                                {
                                    let weak = focus_weak.clone();
                                    // Mouse listeners belong to the paint phase. Return
                                    // X11 focus before GPUI handles its own input fields.
                                    _window.on_mouse_event(
                                        move |event: &MouseDownEvent, phase, _, cx| {
                                            if phase.capture() {
                                                let _ = weak.update(cx, |editor, _| {
                                                    if let Some(host) = &editor.host {
                                                        host.restore_focus_for_parent_click(
                                                            event.position,
                                                        );
                                                    }
                                                });
                                            }
                                        },
                                    );
                                }
                            },
                        )
                        .size_full(),
                    ),
            )
    }
}

#[cfg(all(test, feature = "ui-test"))]
mod profile_tests {
    use super::schedule_editor_profile;
    use gpui_kit::TestAppContext;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    #[test]
    fn outline_protocol_is_typed_generation_bound_and_requires_an_interactive_editor() {
        assert!(super::valid_outline_message(7, 7, true));
        assert!(!super::valid_outline_message(6, 7, true));
        assert!(!super::valid_outline_message(7, 7, false));
        for position in ["auto", "left", "right", "top", "floating"] {
            let message: super::Message = serde_json::from_value(serde_json::json!({
                "type":"outlinePreference", "generation":7, "value":{"position":position,"shown":true}
            })).unwrap();
            assert!(matches!(
                message,
                super::Message::OutlinePreference { generation: 7, .. }
            ));
        }
        for value in [
            serde_json::json!({"position":"../foreign","shown":true}),
            serde_json::json!({"position":"right","shown":"true"}),
            serde_json::json!({"position":"right"}),
        ] {
            assert!(
                serde_json::from_value::<super::Message>(serde_json::json!({
                "type":"outlinePreference", "generation":7, "value":value}))
                .is_err()
            );
        }
    }

    #[test]
    fn table_identity_stays_old_until_the_document_generation_is_replaced() {
        let mut binding = super::TableBinding::default();
        binding.stage("library".into(), "old-note".into(), Default::default());
        assert!(binding.current.is_none());
        binding.commit();
        let old = binding.current.clone();
        binding.stage("library".into(), "new-note".into(), Default::default());
        // Queued old-generation events must still capture the OLD identity.
        assert_eq!(binding.current, old);
        binding.commit();
        assert_eq!(binding.current, Some(("library".into(), "new-note".into())));
        assert!(binding.pending.is_none());
        // Same-path restores invalidate even a previously identical context.
        binding = super::TableBinding::default();
        binding.stage("library".into(), "new-note".into(), Default::default());
        assert!(binding.current.is_none());
        binding.commit();
        assert!(binding.views.is_empty());
    }
    #[test]
    fn table_protocol_rejects_stale_unbound_and_oversized_values_without_page_owned_identity() {
        let table = "0".repeat(32);
        assert!(super::valid_table_message(7, 7, true, &table, &None));
        assert!(!super::valid_table_message(6, 7, true, &table, &None));
        assert!(!super::valid_table_message(7, 7, false, &table, &None));
        assert!(!super::valid_table_message(7, 7, true, "../foreign", &None));
        let page = serde_json::json!({"type":"tableView","generation":7,"table":table,"value":null,"document":"foreign","library":"elsewhere"});
        let super::Message::TableView {
            generation,
            table,
            value,
        } = serde_json::from_value(page).unwrap()
        else {
            panic!("table message");
        };
        assert!(super::valid_table_message(
            generation, 7, true, &table, &value
        ));
        assert!(serde_json::from_value::<super::Message>(serde_json::json!({"type":"tableView","generation":7,"table":table,"value":{"anchor":{},"widths":[10000]}})).is_err());
    }
    #[gpui_kit::test]
    fn profile_resolution_is_deferred_off_render(cx: &mut TestAppContext) {
        let root = tempfile::tempdir().unwrap().keep();
        let dirs = studyapp_desktop::app_paths::AppDirectories {
            data: root.join("data"),
            config: root.join("config"),
            local_data: root.join("local"),
            cache: root.join("cache"),
            updates: root.join("updates"),
        };
        std::fs::create_dir_all(&dirs.config).unwrap();
        let locator = dirs.config.join("storage-location.json");
        let custom = root.join("custom");
        std::fs::write(
            &locator,
            serde_json::to_vec(&serde_json::json!({"schema":1,"root":custom})).unwrap(),
        )
        .unwrap();
        let resolved = Arc::new(AtomicBool::new(false));
        let observed = resolved.clone();
        let task = schedule_editor_profile(&cx.executor(), move || {
            observed.store(true, Ordering::SeqCst);
            studyapp_desktop::app_paths::AppPaths::from_defaults(dirs, locator)
        });
        assert!(
            !resolved.load(Ordering::SeqCst),
            "Scheduling a render must not read the locator"
        );
        let (sender, receiver) = std::sync::mpsc::channel();
        cx.executor()
            .spawn(async move {
                sender.send(task.await).unwrap();
            })
            .detach();
        cx.executor().run_until_parked();
        assert_eq!(
            receiver.try_recv().unwrap().unwrap(),
            custom.join("cache/editor")
        );
        assert!(resolved.load(Ordering::SeqCst));
    }
}

#[cfg(all(test, feature = "ui-test"))]
#[path = "markdown_editor_startup_tests.rs"]
mod startup_tests;

#[cfg(test)]
mod navigation_gate_tests {
    use super::valid_open_link;
    #[test]
    fn stale_hidden_modal_and_untrusted_navigation_cannot_open_the_operating_system() {
        for url in [
            "https://example.com/",
            "http://example.com/path?q=1",
            "mailto:person@example.com",
        ] {
            assert!(valid_open_link(41, 41, true, url));
            assert!(!valid_open_link(40, 41, true, url));
            assert!(!valid_open_link(41, 41, false, url));
        }
        for url in [
            "javascript:alert(1)",
            "file:///C:/private.txt",
            "data:text/html,hi",
            "https://user:secret@example.com/",
            "https://example.com/\n",
            " https://example.com/",
            "shell:downloads",
            "//example.com/",
            "https://",
        ] {
            assert!(!valid_open_link(41, 41, true, url), "{url:?}");
        }
    }
}
