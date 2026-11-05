use super::*;
use crate::storage_backend::purge::remote::Scope;

impl StudyApp {
    pub(crate) fn open_purge(&mut self, id: String, cx: &mut Context<Self>) {
        self.storage_confirm = Some(id);
        self.storage_ui.purge = Some((Scope::default(), None));
        self.storage_ui.purge_page = 0;
        cx.notify();
    }
    fn purge_busy(&self, cx: &App) -> bool {
        self.storage_action_blocked(false, cx)
            || self.note_editor.is_some()
            || self.draft_loading
            || self.draft_failed
            || self.draft_drain_intent.is_some()
            || self.draft_dirty_since.is_some()
    }
    pub(super) fn submit_purge(&mut self, cx: &mut Context<Self>) {
        if self.purge_busy(cx) {
            return;
        }
        let Some(id) = self.storage_confirm.clone() else {
            return;
        };
        let Some((scope, preview)) = &self.storage_ui.purge else {
            return;
        };
        let work = if let Some(preview) = preview {
            Work::ConfirmPurge(preview.clone())
        } else {
            Work::PreviewPurge(id, *scope)
        };
        self.storage_work(work, cx);
    }
    pub(super) fn purge_view(&self, id: &str, cx: &mut Context<Self>) -> Option<Div> {
        let (scope, preview) = self.storage_ui.purge.as_ref()?;
        let title = self
            .snapshot
            .as_ref()?
            .notes
            .iter()
            .find(|n| n.id == id)?
            .title
            .clone();
        let mut content = column().gap_2().child(heading(title));
        if let Some(preview) = preview {
            let local = &preview.local;
            content=content.child(muted(format!("本机将删除 {} 个文件（{} 字节）、{} 个历史版本和 {} 份草稿，并清理 {} 份自动恢复备份中的对应记录。",local.files.len(),local.bytes(),local.versions,local.drafts,local.backup_count())))
                .child(muted(format!("共享图片保留 {} 张。远端将移除 {} 份笔记和 {} 张图片。",local.shared+preview.remote_shared,usize::from(preview.remote_note.is_some()),preview.remote_images.len())));
            if !local.unresolved.is_empty() {
                content = content.child(muted(format!(
                    "另有 {} 个文件无法确认归属，将保留并列在清单末尾。",
                    local.unresolved.len()
                )));
            }
            if scope.sync || scope.cloud {
                content = content.child(muted(
                    "远端仅处理确认时的对象版本。同步服务或存储桶的历史保留情况以执行结果为准。",
                ));
            }
            let count = local.files.len()
                + local.backup_count()
                + local.unresolved.len()
                + preview
                    .objects()
                    .map(|p| 1 + p.confirmed_retained.len())
                    .sum::<usize>();
            let names = local
                .paths()
                .map(|p| p.display().to_string())
                .chain(preview.objects().flat_map(|p| {
                    std::iter::once(format!("远端 · {}", p.target)).chain(
                        p.confirmed_retained
                            .iter()
                            .map(|r| format!("同步暂存 · {}（{} 字节）", r.name, r.bytes)),
                    )
                }))
                .chain(
                    local
                        .unresolved
                        .iter()
                        .map(|p| format!("保留 · {}", p.display())),
                );
            let page = self.storage_ui.purge_page.min(count.saturating_sub(1) / 8);
            for path in names.skip(page * 8).take(8) {
                content = content.child(muted(path).text_xs());
            }
            if count > 8 {
                content = content.child(
                    row()
                        .child(
                            Button::new("purge-prev")
                                .small()
                                .action("上一页")
                                .disabled(page == 0)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.storage_ui.purge_page = page.saturating_sub(1);
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("purge-next")
                                .small()
                                .action("下一页")
                                .disabled((page + 1) * 8 >= count)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.storage_ui.purge_page = page + 1;
                                    cx.notify();
                                })),
                        ),
                );
            }
        } else {
            content=content.child(muted("本机托管文件、历史快照、关联草稿和专用图片将一并清除。外部原文件及其他笔记仍使用的图片会保留。"));
            let sync_configured = !matches!(
                self.sync_panel.read(cx).status.settings.target,
                crate::sync_runtime::SyncTarget::Disabled
            );
            let cloud_configured = match &self.snapshot.as_ref()?.settings.images.cloud {
                studyapp_core::media_settings::ImageCloudTarget::NoteSync => sync_configured,
                studyapp_core::media_settings::ImageCloudTarget::ObjectStore(target) => {
                    target.validate().is_ok()
                }
            };
            for (index, (selected, configured, label)) in [
                (scope.sync, sync_configured, "同时删除同步目标中的笔记"),
                (scope.cloud, cloud_configured, "同时删除云端图片"),
            ]
            .into_iter()
            .enumerate()
            {
                content = content.child(
                    Button::new(("purge-scope", index))
                        .small()
                        .ghost()
                        .label(format!(
                            "{} {label}{}",
                            if selected { "☑" } else { "☐" },
                            if configured { "" } else { "（未配置）" }
                        ))
                        .disabled(self.storage_busy || !configured)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some((scope, p)) = &mut this.storage_ui.purge {
                                if index == 0 {
                                    scope.sync = !scope.sync
                                } else {
                                    scope.cloud = !scope.cloud
                                }
                                *p = None;
                            }
                            cx.notify();
                        })),
                );
            }
        }
        if self.purge_busy(cx) {
            content = content.child(muted(if self.storage_busy {
                "正在核对或删除文件…"
            } else {
                "请先保存并关闭笔记编辑器，等待草稿和同步完成。"
            }));
        }
        let final_step = preview.is_some();
        Some(
            div().child(
                studyapp_ui::modal_frame::ModalFrame::new(
                    if final_step {
                        "确认永久删除？"
                    } else {
                        "彻底删除托管笔记"
                    },
                    content,
                )
                .tone(studyapp_ui::modal_frame::ModalTone::Danger)
                .width(studyapp_ui::modal_frame::ModalWidth::Compact)
                .scroll_handle(self.modal_focus.scroll())
                .footer(
                    row()
                        .child(
                            Button::new("purge-cancel")
                                .small()
                                .ghost()
                                .label("取消")
                                .disabled(self.storage_busy)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.storage_confirm = None;
                                    this.storage_ui.purge = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("purge-submit")
                                .small()
                                .danger()
                                .label(if final_step {
                                    "确认彻底删除"
                                } else {
                                    "预览删除清单"
                                })
                                .disabled(self.purge_busy(cx))
                                .on_click(cx.listener(|this, _, _, cx| this.submit_purge(cx))),
                        ),
                ),
            ),
        )
    }
}
