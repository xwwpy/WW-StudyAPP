//! A user-opened sharing flow, not an automatically queued notification.
use crate::{app::StudyApp, components::*, controls::Button};
use gpui_kit::{
    component::{Disableable, Sizable, button::ButtonVariants},
    *,
};
use studyapp_core::{Command, NoticeStyle};
use studyapp_ui::modal_frame::{ModalFrame, ModalWidth};

pub(crate) struct ExportImageJob {
    pub request: u64,
    pub generation: u64,
    pub control: crate::media::ResolveControl,
}
impl Drop for ExportImageJob {
    fn drop(&mut self) {
        self.control.cancel();
    }
}
pub(crate) struct ExportPanel {
    library: String,
    note_id: String,
    version_id: Option<String>,
    expected_hash: String,
    title: String,
    pub(crate) preview: Entity<crate::markdown_editor::MarkdownEditor>,
    pub(crate) images: Option<ExportImageJob>,
    pub(crate) image_warning: Option<String>,
    extension: String,
}
impl StudyApp {
    pub(crate) fn open_note_export(
        &mut self,
        note_id: String,
        version_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.library_controls_busy(cx) {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(note) = snapshot.notes.iter().find(|note| note.id == note_id) else {
            return;
        };
        let body = if let Some(version) = &version_id {
            snapshot
                .versions
                .iter()
                .find(|item| item.note_id == note_id && item.id == *version)
                .map(|item| (item.content.as_str(), item.hash.as_str()))
        } else {
            snapshot.note_body(&note_id)
        };
        let Some((content, hash)) = body else {
            return;
        };
        let document = studyapp_core::documents::DocumentPreview::from_content(content);
        let extension = document
            .as_ref()
            .map(|doc| doc.kind.clone())
            .unwrap_or_else(|| "md".into());
        let image_source = std::path::PathBuf::from(&note.path);
        let image_settings = snapshot.settings.images.clone();
        let preview = cx.new(|cx| {
            let mut editor =
                crate::markdown_editor::MarkdownEditor::new(window, cx).without_outline();
            editor.set_value_with_readonly(content.to_owned(), true, window, cx);
            editor.set_can_request_edit(false, cx);
            editor.set_theme(crate::theme::palette().editor_tokens(), cx);
            editor.set_visible(true, cx);
            editor.set_shortcuts(
                crate::shortcuts::SHORTCUT_ACTIONS
                    .iter()
                    .map(|action| (action.id.to_owned(), String::new()))
                    .collect(),
                cx,
            );
            editor
        });
        self.export_panel = Some(ExportPanel {
            library: self.last_library.clone(),
            note_id,
            version_id,
            expected_hash: hash.into(),
            title: note.title.clone(),
            preview,
            extension,
            images: None,
            image_warning: None,
        });
        self.start_export_preview_images(image_source, image_settings, cx);
        cx.notify();
    }
    pub(crate) fn export_panel_modal(&self, window: &Window, cx: &Context<Self>) -> AnyElement {
        let Some(state) = &self.export_panel else {
            return div().into_any_element();
        };
        let layout = self
            .effective_appearance()
            .notifications
            .layout(NoticeStyle::Share);
        let preview = column()
            .gap_3()
            .child(heading(state.title.clone()))
            .child(muted(if state.version_id.is_some() {
                "选中的历史版本"
            } else {
                "当前已保存正文"
            }))
            .children(state.image_warning.clone().map(muted))
            .child(
                div()
                    .id("share-document-preview")
                    .test_support()
                    .h(px(260.))
                    .min_h_0()
                    .min_w_0()
                    .overflow_hidden()
                    .child(state.preview.clone()),
            );
        let targets = column().gap_3().child(heading("导出目标"))
            .child(muted(format!("原格式 · {}", state.extension.to_uppercase())))
            .child(Button::new("share-export-file").label("选择新文件并导出…")
                .disabled(self.storage_busy || self.preferences_ui.busy)
                .on_click(cx.listener(|this, _, _, cx| this.export_previewed_note(cx))))
            .child(muted("仅提供应用已支持的原文导出。选择新文件名后才写入，不覆盖已有文件；关闭面板不执行操作。"));
        ModalFrame::new(
            "分享与导出",
            studyapp_ui::notification_surfaces::share_panel(
                preview,
                targets,
                layout,
                window.viewport_size(),
            ),
        )
        .width(ModalWidth::Wide)
        .component(NoticeStyle::Share)
        .footer(
            Button::new("share-close")
                .small()
                .ghost()
                .label("关闭")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.export_panel = None;
                    cx.notify();
                })),
        )
        .into_any_element()
    }
    fn export_previewed_note(&mut self, cx: &mut Context<Self>) {
        if self.storage_busy || self.preferences_ui.busy || self.preferences_ui.replacing {
            return;
        }
        let Some(state) = self.export_panel.take() else {
            return;
        };
        if state.library != self.last_library {
            cx.notify();
            return;
        }
        let sender = self.worker.sender.clone();
        std::thread::spawn(move || {
            if let Some(path) = rfd::FileDialog::new()
                .set_title("导出原文（请选择新文件名）")
                .add_filter(state.extension.to_uppercase(), &[state.extension.as_str()])
                .set_file_name(format!("笔记原文.{}", state.extension))
                .save_file()
            {
                let _ = sender.send(crate::worker::Work::Scoped {
                    library: state.library,
                    work: Box::new(crate::worker::Work::Apply(Command::ExportPreviewedNote {
                        note_id: state.note_id,
                        version_id: state.version_id,
                        expected_hash: state.expected_hash,
                        path,
                    })),
                });
            }
        });
        cx.notify();
    }
}
