use crate::{app::StudyApp, components::*, controls::Button};
use gpui_kit::{
    component::{Disableable, Sizable, button::ButtonVariants},
    *,
};
use std::collections::HashSet;
use studyapp_core::{Command, SnapshotChange, SnapshotSelection};
use studyapp_ui::{
    controls::Checkbox,
    modal_frame::{ModalFrame, ModalWidth},
};

#[derive(Default)]
pub(crate) struct SnapshotPreview {
    pub open: bool,
    pub busy: bool,
    refresh: bool,
    changes: Vec<SnapshotChange>,
    selected: HashSet<String>,
    expanded: HashSet<String>,
    error: Option<String>,
}
impl StudyApp {
    pub(crate) fn open_snapshot_preview(&mut self, cx: &mut Context<Self>) {
        if self.storage_busy || self.preferences_ui.busy || self.preferences_ui.replacing {
            return;
        }
        self.snapshot_preview = SnapshotPreview {
            open: true,
            busy: true,
            refresh: true,
            ..Default::default()
        };
        self.send(Command::PreviewSnapshots, cx);
    }
    pub(crate) fn accept_snapshot_result(&mut self, result: Result<(), String>) {
        let view = &mut self.snapshot_preview;
        view.busy = false;
        match result {
            Err(error) => view.error = Some(error),
            Ok(()) => {
                view.error = None;
                if let Some(snapshot) = &self.snapshot {
                    view.changes = snapshot.snapshot_changes.as_ref().clone();
                    let available: HashSet<_> =
                        view.changes.iter().map(|c| c.note_id.clone()).collect();
                    if view.refresh {
                        view.selected = available;
                    } else {
                        view.selected.retain(|id| available.contains(id));
                    }
                    view.expanded
                        .retain(|id| view.changes.iter().any(|c| &c.note_id == id));
                    let errors: Vec<_> = snapshot
                        .notes
                        .iter()
                        .filter(|note| !note.detached)
                        .filter_map(|note| {
                            note.error
                                .as_ref()
                                .map(|error| format!("{}：{error}", note.title))
                        })
                        .collect();
                    if !errors.is_empty() {
                        view.error = Some(format!(
                            "以下文件未能安全读取，未保存快照：{}",
                            errors.join("；")
                        ));
                    }
                }
            }
        }
        view.refresh = false;
    }
    fn save_selected_snapshots(&mut self, cx: &mut Context<Self>) {
        if self.snapshot_preview.busy || self.storage_busy {
            return;
        }
        let selected: Vec<_> = self
            .snapshot_preview
            .changes
            .iter()
            .filter(|change| self.snapshot_preview.selected.contains(&change.note_id))
            .map(|change| SnapshotSelection {
                note_id: change.note_id.clone(),
                expected_hash: change.expected_hash.clone(),
            })
            .collect();
        if selected.is_empty() {
            return;
        }
        self.snapshot_preview.busy = true;
        self.send(Command::CaptureSelectedSnapshots(selected), cx);
    }
    pub(crate) fn snapshot_preview_modal(&self, cx: &Context<Self>) -> AnyElement {
        let state = &self.snapshot_preview;
        let mut content = column().gap_3()
            .child(muted("比较当前可一致读取的内容与上次快照。只保存勾选项；内容在预览后发生变化时整批拒绝，请重新扫描。"));
        if state.busy {
            content = content.child(muted("正在读取或保存快照，请稍候…"));
        }
        if let Some(error) = &state.error {
            content = content.child(muted(error.clone()));
        }
        if state.changes.is_empty() && !state.busy {
            content = content.child(muted("没有待保存变化。旧快照保持不变。"));
        }
        for change in &state.changes {
            let id = change.note_id.clone();
            let expanded_id = id.clone();
            let expanded = state.expanded.contains(&id);
            let mut entry = column()
                .gap_2()
                .py_2()
                .border_b_1()
                .border_color(crate::theme::color(BORDER))
                .child(
                    row()
                        .flex_wrap()
                        .child(
                            Checkbox::new(SharedString::from(format!("snapshot-check-{id}")))
                                .label(change.title.clone())
                                .checked(state.selected.contains(&id))
                                .disabled(state.busy)
                                .on_click(cx.listener(move |this, checked, _, cx| {
                                    if *checked {
                                        this.snapshot_preview.selected.insert(id.clone());
                                    } else {
                                        this.snapshot_preview.selected.remove(&id);
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new(SharedString::from(format!(
                                "snapshot-diff-{}",
                                change.note_id
                            )))
                            .small()
                            .ghost()
                            .label(if expanded {
                                "收起差异"
                            } else {
                                "查看差异"
                            })
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if !this.snapshot_preview.expanded.remove(&expanded_id) {
                                        this.snapshot_preview.expanded.insert(expanded_id.clone());
                                    }
                                    cx.notify();
                                },
                            )),
                        ),
                );
            if expanded {
                entry = entry.child(studyapp_ui::markdown_view::copyable(
                    SharedString::from(format!("snapshot-diff-text-{}", change.note_id)),
                    div()
                        .w_full()
                        .min_w_0()
                        .text_sm()
                        .font_family(crate::design::MONO_FONT)
                        .child(change.diff.clone()),
                ));
            }
            content = content.child(entry);
        }
        ModalFrame::new("待保存快照", content)
            .width(ModalWidth::Wide)
            .scroll_handle(self.modal_focus.scroll())
            .footer(
                row()
                    .flex_wrap()
                    .child(
                        Button::new("snapshot-preview-close")
                            .small()
                            .ghost()
                            .label("关闭")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.snapshot_preview.open = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("snapshot-preview-refresh")
                            .small()
                            .label("重新扫描")
                            .disabled(state.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.open_snapshot_preview(cx))),
                    )
                    .child(
                        Button::new("snapshot-preview-save")
                            .small()
                            .primary()
                            .label(format!("保存勾选的 {} 份", state.selected.len()))
                            .disabled(state.busy || state.selected.is_empty())
                            .on_click(
                                cx.listener(|this, _, _, cx| this.save_selected_snapshots(cx)),
                            ),
                    ),
            )
            .into_any_element()
    }
}
