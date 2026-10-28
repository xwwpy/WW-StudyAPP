//! OS drag feedback changes only on session transitions; path I/O stays in workers.
use crate::{app::StudyApp, external_drop::DropBlock, worker::Work};
use gpui_kit::*;
use std::path::PathBuf;

impl StudyApp {
    pub(crate) fn external_drop_block(&self, cx: &App) -> Option<DropBlock> {
        if self.has_modal(cx) || self.active_note_menu.is_some() {
            Some(DropBlock::Modal)
        } else if self.storage_busy
            || self.preferences_ui.busy
            || self.preferences_ui.replacing
            || self.note_saving
            || self.media_busy()
        {
            Some(DropBlock::Busy)
        } else {
            None
        }
    }

    pub fn hover_external_files(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        // Keep only the OS-supplied payload. No enumeration or disk access on
        // pointer moves, and no allocation when the payload is unchanged.
        if self.external_drop_paths != paths {
            self.external_drop_paths = paths.to_vec();
        }
        self.hover_external_file_count(paths.len(), cx);
    }

    pub fn hover_external_file_count(&mut self, count: usize, cx: &mut Context<Self>) {
        let blocked = self.external_drop_block(cx);
        if self.external_drop.hover(count, blocked) {
            self.note_content
                .update(cx, |editor, cx| editor.set_visible(false, cx));
            cx.notify();
        }
    }

    pub fn cancel_external_drag(&mut self, cx: &mut Context<Self>) {
        self.external_drop_paths.clear();
        if self.external_drop.cancel() {
            cx.notify();
        }
    }

    pub fn handle_external_drag(
        &mut self,
        phase: String,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match phase.as_str() {
            "enter" | "over" => self.hover_external_files(&paths, cx),
            // Hiding the WebView on enter can itself emit Leave. The parent OS
            // drag lifecycle below owns cancellation after this handoff.
            "leave" => {}
            "drop" => self.drop_external_files(paths, window, cx),
            _ => {}
        }
    }

    pub fn drop_external_files(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let blocked = self.external_drop_block(cx);
        let accepted = self.external_drop.finish(blocked);
        self.external_drop_paths.clear();
        cx.notify();
        if !accepted || paths.is_empty() {
            return;
        }
        if self.import_dropped_images(paths.clone(), window, cx) {
            return;
        }
        let work = Work::Scoped {
            library: self.last_library.clone(),
            work: Box::new(Work::ImportPaths(paths)),
        };
        if self.worker.sender.send(work).is_err() {
            self.push_notice(Err("后台服务已停止".into()), cx);
        } else {
            self.push_notice(Ok("正在后台导入拖入的 Markdown 文件…".into()), cx);
        }
    }

    pub fn external_drop_event_layer(&self, cx: &Context<Self>) -> impl IntoElement {
        let owner = cx.weak_entity();
        canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let release_owner = owner.clone();
                // GPUI translates OS Submit into MouseUp. A newly shown
                // overlay need not have a hovered hitbox until another move,
                // so complete this session in capture, independent of hitboxes.
                window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                    if !phase.capture() || event.button != MouseButton::Left {
                        return;
                    }
                    let _ = release_owner.update(cx, |this, cx| {
                        if this.external_drop.active() {
                            let paths = std::mem::take(&mut this.external_drop_paths);
                            cx.stop_active_drag(window);
                            this.drop_external_files(paths, window, cx);
                            cx.stop_propagation();
                        }
                    });
                });
                let owner = owner.clone();
                window.on_mouse_event(move |event: &FileDropEvent, phase, _, cx| {
                    if !phase.capture() {
                        return;
                    }
                    let _ = owner.update(cx, |this, cx| match event {
                        FileDropEvent::Entered { paths, .. } => {
                            this.hover_external_files(paths.paths(), cx)
                        }
                        FileDropEvent::Exited | FileDropEvent::Ended => {
                            this.cancel_external_drag(cx)
                        }
                        _ => {}
                    });
                });
            },
        )
        .absolute()
        .size_full()
    }
}
