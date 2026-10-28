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
    Save,
    RequestEdit(usize),
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
    sender: mpsc::Sender<String>,
    receiver: mpsc::Receiver<String>,
    host: Option<crate::web_editor_host::HostedEditor>,
    creating: bool,
    ready: bool,
    visible: bool,
    readonly: bool,
    resume_readonly: bool,
    can_request_edit: bool,
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
impl EventEmitter<MarkdownChanged> for MarkdownEditor {}
impl EventEmitter<MarkdownEvent> for MarkdownEditor {}

impl MarkdownEditor {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (sender, receiver) = mpsc::channel();
        let poll = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(25))
                    .await;
                #[cfg(target_os = "linux")]
                crate::web_editor_host::HostedEditor::pump_events();
                if this.update_in(cx, |this, _, cx| this.poll(cx)).is_err() {
                    break;
                }
            }
        });
        Self {
            document: EditorDocument::new(String::new()),
            sender,
            receiver,
            host: None,
            creating: false,
            ready: false,
            visible: false,
            readonly: false,
            resume_readonly: false,
            can_request_edit: false,
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

    pub fn value(&self) -> &str {
        self.document.text()
    }
    pub fn generation(&self) -> u64 {
        self.document.generation()
    }
    pub fn revision(&self) -> u64 {
        self.document.revision()
    }
    pub fn editable(&self) -> bool {
        self.ready() && !self.readonly && self.pending.is_none() && !self.recovering
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
        let payload = serde_json::json!({"generation": self.document.generation(), "text": self.document.text(), "readonly": self.readonly, "canRequestEdit": self.can_request_edit});
        let shortcuts = serde_json::to_string(&self.shortcuts).expect("string map serializes");
        let theme = &self.theme;
        if let Err(error) = self.evaluate(&format!("window.studyappEditor.setShortcuts({shortcuts}); window.studyappEditor.setTheme({theme}); window.studyappEditor.load({payload});")) {
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

    pub fn set_can_request_edit(&mut self, enabled: bool, cx: &mut Context<Self>) {
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
        self.visible = visible;
        if let Some(host) = &self.host
            && let Err(error) = host.set_visible(visible && self.ready)
        {
            self.fail(format!("无法显示编辑区：{error:#}"), cx);
        }
        cx.notify();
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

    fn ensure_host(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.host.is_some() || self.creating || self.error.is_some() || !self.visible {
            return;
        }
        let parent = match crate::web_editor_host::ParentWindow::capture(window) {
            Ok(parent) => parent,
            Err(error) => {
                self.fail(format!("无法创建编辑窗口：{error:#}"), cx);
                return;
            }
        };
        let profile = if cfg!(feature = "ui-test") {
            std::env::temp_dir().join(format!("studyapp-editor-test-{}", std::process::id()))
        } else {
            directories::ProjectDirs::from("io", "StudyApp", "StudyApp")
                .map(|p| p.cache_dir().join("editor"))
                .unwrap_or_else(|| std::env::temp_dir().join("studyapp-editor"))
        };
        let sender = self.sender.clone();
        self.creating = true;
        cx.spawn(async move |this, cx| {
            // Wry pumps Windows messages. Absolutely no GPUI App/entity borrow
            // may be held during the constructor (including a render callback).
            cx.background_executor()
                .timer(Duration::from_millis(1))
                .await;
            #[cfg(feature = "ui-test")]
            eprintln!("native editor: creating WebView");
            let result = crate::web_editor_host::HostedEditor::new(parent, sender, profile);
            #[cfg(feature = "ui-test")]
            eprintln!("native editor: constructor finished");
            let _ = this.update(cx, |this, cx| {
                this.creating = false;
                match result {
                    Ok(host) => {
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

    #[cfg(all(windows, feature = "ui-test"))]
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
        self.ensure_host(window, cx);
        let weak = cx.weak_entity();
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
                                .label("重试同步并恢复输入")
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
                    |_, _, _, _| {},
                )
                .size_full()
                .min_h(px(180.)),
            )
    }
}
