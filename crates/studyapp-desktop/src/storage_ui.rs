use crate::controls::Button;
use crate::settings_components as form;
use crate::storage_backend::viewmodel::{CopyOrder, ManagedCopyView, StoragePresentation};
use crate::{app::StudyApp, components::*, editor_lifecycle::NoteIntent, worker::Work};
use gpui_kit::{
    component::{Disableable, Sizable, button::ButtonVariants},
    prelude::FluentBuilder,
    *,
};
use std::{cell::RefCell, rc::Rc, sync::Arc};

#[derive(Clone, Copy, PartialEq, Eq)]
struct StorageLayout {
    show_paths: bool,
    draft_revision: u64,
    font_size: u16,
    spacing: u16,
    details_revision: u64,
}

struct CopySource {
    presentation: Arc<StoragePresentation>,
    layout: StorageLayout,
    query: SharedString,
    order: CopyOrder,
    rows: Rc<Vec<usize>>,
}

pub struct StorageUi {
    pub scroll: ListState,
    source: RefCell<Option<CopySource>>,
    pub order: CopyOrder,
    expanded_copy: Option<String>,
    details_revision: u64,
    #[cfg(feature = "ui-test")]
    pub eager_baseline: bool,
    #[cfg(feature = "ui-test")]
    pub frame_rows: std::cell::Cell<u32>,
    #[cfg(feature = "ui-test")]
    pub total_builds: std::cell::Cell<u64>,
    #[cfg(feature = "ui-test")]
    eager_scroll: ScrollHandle,
}

impl Default for StorageUi {
    fn default() -> Self {
        Self {
            scroll: ListState::new(0, ListAlignment::Top, px(48.)),
            source: RefCell::new(None),
            order: CopyOrder::default(),
            expanded_copy: None,
            details_revision: 0,
            #[cfg(feature = "ui-test")]
            eager_baseline: false,
            #[cfg(feature = "ui-test")]
            frame_rows: std::cell::Cell::new(0),
            #[cfg(feature = "ui-test")]
            total_builds: std::cell::Cell::new(0),
            #[cfg(feature = "ui-test")]
            eager_scroll: ScrollHandle::new(),
        }
    }
}

impl StorageUi {
    fn sync(
        &self,
        presentation: &Arc<StoragePresentation>,
        layout: StorageLayout,
        query: SharedString,
    ) -> Rc<Vec<usize>> {
        let mut source = self.source.borrow_mut();
        if let Some(previous) = source.as_ref()
            && Arc::ptr_eq(&previous.presentation, presentation)
            && previous.query == query
            && previous.order == self.order
        {
            if previous.layout != layout {
                self.scroll.remeasure();
            }
            let rows = previous.rows.clone();
            if let Some(previous) = source.as_mut() {
                previous.layout = layout;
            }
            return rows;
        }
        let rows = Rc::new(presentation.copy_rows(&query, self.order));
        let top = self.scroll.logical_scroll_top();
        self.scroll.reset_with_uniform_height(rows.len(), px(96.));
        if let Some(previous) = source.as_ref()
            && previous.query == query
            && previous.order == self.order
            && let Some((next, exact)) = presentation.relocate_copy(
                &previous.presentation,
                &previous.rows,
                &rows,
                top.item_ix,
            )
        {
            self.scroll.scroll_to(ListOffset {
                item_ix: next,
                offset_in_item: if exact { top.offset_in_item } else { px(0.) },
            });
        }
        *source = Some(CopySource {
            presentation: presentation.clone(),
            layout,
            query,
            order: self.order,
            rows: rows.clone(),
        });
        rows
    }
}

fn storage_copy_list(
    scroll: &ListState,
    render: impl FnMut(usize, &mut Window, &mut App) -> AnyElement + 'static,
) -> impl IntoElement + Styled {
    div()
        .id("storage-copy-list")
        .test_support()
        .relative()
        .flex_shrink_0()
        .min_w_0()
        .h(px(340.))
        .overflow_hidden()
        .child(list(scroll.clone(), render).size_full())
        .child(studyapp_ui::virtual_list_scrollbar::virtual_list_scrollbar(
            "storage-copy-scrollbar",
            scroll,
            px(0.),
        ))
        // The list handles scrolling first. Contain the wheel at both ends too.
        // Consume every wheel event here, even after the offset reaches an end.
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
}

impl StudyApp {
    fn storage_action_blocked(&self, read_only_scan: bool, cx: &App) -> bool {
        self.storage_busy
            || crate::storage_policy::media_blocks_storage(read_only_scan, self.media_jobs_active())
            || self.note_saving
            || self.draft_discard.is_some()
            || self.draft_persist.is_some()
            || self.preferences_ui.busy
            || self.preferences_ui.replacing
            || (!read_only_scan && self.sync_panel.read(cx).status.busy)
    }

    pub fn storage_work(&mut self, work: Work, cx: &mut Context<Self>) {
        let migrating = matches!(&work, Work::MigrateLibrary(_));
        if self.storage_action_blocked(matches!(&work, Work::RefreshStorage), cx) {
            self.push_notice(Err("正在完成另一项资料操作，请稍后重试。".into()), cx);
            return;
        }
        self.storage_busy = true;
        self.folders
            .update(cx, |panel, cx| panel.set_busy(true, cx));
        if migrating {
            self.preferences_ui.replacing = true;
            self.sync_panel
                .update(cx, |panel, cx| panel.set_editing_locked(true, cx));
        }
        if self
            .worker
            .sender
            .send(Work::Scoped {
                library: self.last_library.clone(),
                work: Box::new(work),
            })
            .is_err()
        {
            self.storage_busy = false;
            self.folders
                .update(cx, |panel, cx| panel.set_busy(false, cx));
            if migrating {
                self.preferences_ui.replacing = false;
                self.sync_panel
                    .update(cx, |panel, cx| panel.set_editing_locked(false, cx));
            }
            self.push_notice(Err("后台服务已停止，请重新启动应用".into()), cx);
        }
        cx.notify();
    }

    pub fn choose_storage(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.media_jobs_active() {
            self.push_notice(Err("正在处理图片，请稍候再迁移资料库。".into()), cx);
            return;
        }
        if self.guard_settings(crate::settings_ui::SettingsContinuation::Migrate, cx) {
            return;
        }
        if self.storage_busy
            || self.note_saving
            || self.draft_loading
            || self.draft_discard.is_some()
            || self.sync_panel.read(cx).status.busy
        {
            return;
        }
        if self.draft_failed {
            self.push_notice(Err("请先重试草稿保存，再迁移资料库。".into()), cx);
            cx.notify();
        } else if self.note_editor.is_some() {
            self.request_note_action(NoteIntent::PrepareMigration, window, cx);
        } else {
            self.begin_draft_drain(NoteIntent::PrepareMigration, cx);
        }
    }

    pub fn pick_migration(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.storage_busy
            || self.preferences_ui.busy
            || self.preferences_ui.replacing
            || self.sync_panel.read(cx).status.busy
        {
            return;
        }
        let library = self.last_library.clone();
        self.storage_busy = true;
        self.storage_confirm = None;
        self.folders
            .update(cx, |panel, cx| panel.set_busy(true, cx));
        self.sync_panel
            .update(cx, |panel, cx| panel.set_editing_locked(true, cx));
        cx.spawn_in(window, async move |this, cx| {
            let initial_directory = std::path::PathBuf::from(&library);
            let path = cx
                .background_executor()
                .spawn(async move {
                    crate::folder_picker::pick_folder(
                        "选择新的空文件夹（迁移后保留原资料）",
                        Some(initial_directory),
                    )
                })
                .await;
            let _ = this.update_in(cx, |this, _, cx| {
                // A reply from another library must not release the new
                // library's operation locks or migrate its data.
                if library != this.last_library {
                    return;
                }
                this.storage_busy = false;
                this.folders
                    .update(cx, |panel, cx| panel.set_busy(false, cx));
                this.sync_panel
                    .update(cx, |panel, cx| panel.set_editing_locked(false, cx));
                if let Some(path) = path {
                    this.accept_picker_result(library, Work::MigrateLibrary(path), cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub fn storage_view(&self, cx: &mut Context<Self>) -> Div {
        use crate::settings_ui::StoragePage;
        let show_paths = self
            .preferences_ui
            .draft
            .as_ref()
            .is_some_and(|draft| draft.show_file_paths);
        let copies_page = self.preferences_ui.storage_page == StoragePage::Copies;
        let mut view = form::section(if copies_page {
            "托管副本"
        } else {
            "空间概览"
        })
        .child(
            row()
                .child(muted(if copies_page {
                    "仅管理资料库自有副本，原始文件和历史记录受到保护。"
                } else {
                    "了解当前资料库的占用构成。"
                }))
                .child(div().flex_1())
                .child(
                    Button::new("refresh-storage")
                        .small()
                        .disabled(self.storage_action_blocked(true, cx))
                        .label(if self.storage_busy {
                            "正在统计…"
                        } else {
                            "刷新统计"
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.storage_work(Work::RefreshStorage, cx)
                        })),
                ),
        );
        let Some(report) = self
            .storage_report
            .as_ref()
            .filter(|report| report.data_path == self.last_library)
        else {
            return view.child(muted("尚未统计。点击刷新后查看占用与可管理的副本。"));
        };
        let presentation = report.presentation.clone();
        if !copies_page {
            let total = presentation
                .categories
                .iter()
                .map(|category| category.byte_count)
                .sum::<u64>();
            view = view
                .child(heading(presentation.logical_label.clone()).text_xl())
                .child(muted(presentation.scanned_label.clone()));
            for (index, category) in presentation.categories.iter().enumerate() {
                let fraction = if total == 0 {
                    0.
                } else {
                    category.byte_count as f32 / total as f32
                };
                view = view.child(
                    div()
                        .id(("storage-category", index))
                        .test_support()
                        .py_2()
                        .border_b_1()
                        .border_color(crate::theme::color(BORDER))
                        .child(form::settings_row(
                            category.title,
                            row().child(div().flex_1()).child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(category.bytes.clone()),
                            ),
                        ))
                        .child(
                            div()
                                .h(px(4.))
                                .rounded_sm()
                                .bg(crate::theme::color(BG))
                                .child(
                                    div()
                                        .h_full()
                                        .w(relative(fraction))
                                        .rounded_sm()
                                        .bg(crate::theme::color(GREEN)),
                                ),
                        )
                        .child(muted(category.description).text_xs()),
                );
            }
            view = view.child(
                row()
                    .child(
                        Button::new("storage-manage-copies")
                            .small()
                            .label("管理托管副本")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.select_storage_page(StoragePage::Copies, cx)
                            })),
                    )
                    .child(
                        Button::new("storage-manage-images")
                            .small()
                            .ghost()
                            .label("图片存储设置")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.select_storage_page(StoragePage::Images, cx)
                            })),
                    ),
            );
            view = view.child(muted("统计为文件逻辑大小，云端空间未计入。硬链接可能共享空间，因此不等于实际磁盘占用或可释放量。"));
            if !presentation.image_locations.is_empty() {
                view = view.child(muted(presentation.image_summary.clone()));
                if show_paths {
                    view = view.children(
                        presentation
                            .image_locations
                            .iter()
                            .cloned()
                            .map(|path| muted(path).text_xs()),
                    );
                }
            }
            for issue in &presentation.issues {
                view = view
                    .child(muted(issue.message.clone()).text_color(crate::theme::color(0xA34829)));
                if show_paths {
                    view = view.child(muted(issue.path.clone()).text_xs());
                }
            }
            return view;
        }
        let query = self
            .preferences_ui
            .storage_search
            .as_ref()
            .map(|input| input.read(cx).value())
            .unwrap_or_default();
        let theme = crate::theme::palette();
        let rows = self.storage_ui.sync(
            &presentation,
            StorageLayout {
                show_paths,
                draft_revision: self.storage_draft_revision,
                font_size: theme.font_size,
                spacing: theme.spacing,
                details_revision: self.storage_ui.details_revision,
            },
            query,
        );
        #[cfg(feature = "ui-test")]
        self.storage_ui.frame_rows.set(0);
        let mut tools = row().gap_2();
        if let Some(search) = &self.preferences_ui.storage_search {
            tools = tools.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(form::input(search, self.storage_busy)),
            );
        }
        for (order, id, label) in [
            (CopyOrder::Largest, "storage-sort-largest", "大小"),
            (CopyOrder::Name, "storage-sort-name", "名称"),
        ] {
            let mut button = Button::new(id).small().label(label).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.storage_ui.order = order;
                    cx.notify();
                },
            ));
            if self.storage_ui.order == order {
                button = button.primary();
            }
            tools = tools.child(button);
        }
        view = view.child(tools).child(muted(format!(
            "{} / {} 份副本 · 逐项确认清理",
            rows.len(),
            presentation.copies.len()
        )));
        if rows.is_empty() {
            return view.child(muted(if presentation.copies.is_empty() {
                "没有托管副本。导入的外部原文件不在此清理。"
            } else {
                "没有匹配的副本，请修改搜索条件。"
            }));
        }
        #[cfg(feature = "ui-test")]
        if self.storage_ui.eager_baseline {
            return view.child(
                scroll_body("storage-copy-list", &self.storage_ui.eager_scroll)
                    .test_support()
                    .flex_shrink_0()
                    .h(px(340.))
                    .gap_0()
                    .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                    .children(rows.iter().map(|&ix| {
                        self.storage_copy_row(&presentation.copies[ix], show_paths, cx)
                    })),
            );
        }
        let owner = cx.entity();
        view.child(storage_copy_list(
            &self.storage_ui.scroll,
            move |row, _, cx| {
                owner
                    .update(cx, |owner, cx| {
                        owner.storage_copy_row(&presentation.copies[rows[row]], show_paths, cx)
                    })
                    .into_any_element()
            },
        ))
    }

    fn storage_copy_row(
        &self,
        copy: &ManagedCopyView,
        show_paths: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        #[cfg(feature = "ui-test")]
        {
            self.storage_ui
                .frame_rows
                .set(self.storage_ui.frame_rows.get() + 1);
            self.storage_ui
                .total_builds
                .set(self.storage_ui.total_builds.get() + 1);
        }
        let blocked = self.storage_action_blocked(false, cx);
        let id = copy.note_id.clone();
        let details_id = id.clone();
        let history_id = id.clone();
        let reveal_path = copy.path.clone();
        let path_id = SharedString::from(format!("storage-copy-path-{id}"));
        // Mutable draft protection belongs to the current UI read model, never the scan.
        let in_draft = self.storage_active_draft_notes.contains(&id);
        let expanded = self.storage_ui.expanded_copy.as_ref() == Some(&id);
        detail_list_row()
            .child(
                row()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(copy.title.clone()),
                    )
                    .child(muted(copy.bytes.clone()).flex_shrink_0()),
            )
            .child(
                row()
                    .flex_wrap()
                    .child(muted(if in_draft {
                        "草稿使用中"
                    } else {
                        copy.state.label()
                    }))
                    .child(div().flex_1())
                    .child(
                        Button::new(SharedString::from(format!("copy-details-{id}")))
                            .small()
                            .ghost()
                            .label(if expanded { "收起" } else { "详情" })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.storage_ui.expanded_copy = if this
                                    .storage_ui
                                    .expanded_copy
                                    .as_ref()
                                    == Some(&details_id)
                                {
                                    None
                                } else {
                                    Some(details_id.clone())
                                };
                                this.storage_ui.details_revision += 1;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("copy-reveal-{id}")))
                            .small()
                            .ghost()
                            .label("定位文件")
                            .disabled(!copy.deletable)
                            .on_click(move |_, _, cx| {
                                cx.reveal_path(std::path::Path::new(&reveal_path))
                            }),
                    )
                    .child(
                        Button::new(SharedString::from(format!("copy-history-{id}")))
                            .small()
                            .ghost()
                            .label("历史与恢复")
                            .disabled(blocked)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if this.guard_settings(
                                    crate::settings_ui::SettingsContinuation::ViewNote(
                                        history_id.clone(),
                                    ),
                                    cx,
                                ) {
                                    return;
                                }
                                let version = this.snapshot.as_ref().and_then(|s| {
                                    s.versions
                                        .iter()
                                        .rev()
                                        .find(|v| v.note_id == history_id)
                                        .map(|v| v.id.clone())
                                });
                                this.select_note_document(
                                    history_id.clone(),
                                    version,
                                    true,
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("delete-copy-{id}")))
                            .small()
                            .ghost()
                            .label("清理副本")
                            .text_color(crate::theme::color(0xBE3434))
                            .disabled(blocked || !copy.deletable || in_draft)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.guard_settings(
                                    crate::settings_ui::SettingsContinuation::ConfirmDeleteCopy(
                                        id.clone(),
                                    ),
                                    cx,
                                ) {
                                    return;
                                }
                                this.storage_confirm = Some(id.clone());
                                cx.notify();
                            })),
                    ),
            )
            .when(show_paths || expanded, |v| {
                v.child(
                    muted(copy.path.clone())
                        .id(path_id)
                        .test_support()
                        .text_xs()
                        .truncate(),
                )
            })
            .when(in_draft && expanded, |v| {
                v.child(muted("请先保存或删除该笔记的草稿，再清理副本。"))
            })
            .when_some(copy.reason.as_ref().filter(|_| expanded), |v, reason| {
                v.child(muted(reason.clone()))
            })
    }

    #[cfg(feature = "ui-test")]
    pub fn storage_render_work(&self) -> (u64, u32) {
        (
            self.storage_ui.total_builds.get(),
            self.storage_ui.frame_rows.get(),
        )
    }

    fn copy_confirmation_allowed(&self, note_id: &str, cx: &App) -> bool {
        !self.storage_busy
            && !self.note_saving
            && !self.sync_panel.read(cx).status.busy
            && self.draft_discard.is_none()
            && self.draft_persist.is_none()
            && !self.draft_catalog.values().any(|r| {
                r.active()
                    && (r.draft.note_id.as_deref() == Some(note_id)
                        || r.save_target.as_deref() == Some(note_id))
            })
            && self
                .storage_report
                .as_ref()
                .filter(|report| report.data_path == self.last_library)
                .is_some_and(|report| {
                    report
                        .presentation
                        .copy_by_id(note_id)
                        .is_some_and(|copy| copy.deletable)
                })
    }

    pub fn confirm_storage_copy(&mut self, cx: &mut Context<Self>) {
        let Some(note_id) = self.storage_confirm.clone() else {
            return;
        };
        if !self.copy_confirmation_allowed(&note_id, cx) {
            self.push_notice(
                Err("这份副本当前无法删除，请等待任务结束后重新统计空间。".into()),
                cx,
            );
            cx.notify();
            return;
        }
        self.storage_confirm = None;
        self.storage_work(Work::DeleteManagedCopy { note_id }, cx);
    }

    /// A confirmation for exactly one managed copy, mounted in the root dialog.
    pub fn storage_confirmation_view(&self, cx: &mut Context<Self>) -> Option<Div> {
        let note_id = self.storage_confirm.as_ref()?;
        let copy = self
            .storage_report
            .as_ref()
            .filter(|report| report.data_path == self.last_library)
            .and_then(|report| report.presentation.copy_by_id(note_id));
        let allowed = self.copy_confirmation_allowed(note_id, cx);
        let mut content = column().child(muted(
            "文件删除不可撤销；历史快照和学习记录保留，可通过“历史与恢复”导出或复制。",
        ));
        if let Some(copy) = copy {
            content = content.child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(copy.title.clone()),
            );
            if self
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.settings.show_file_paths)
            {
                content = content.child(muted(copy.path.clone()));
            }
        }
        if !allowed {
            content = content.child(muted(
                "目标或资料库状态已改变，当前无法删除。请取消后重新统计空间。",
            ));
        }
        Some(
            div().child(
                studyapp_ui::modal_frame::ModalFrame::new("删除这一份托管副本？", content)
                    .tone(studyapp_ui::modal_frame::ModalTone::Danger)
                    .width(studyapp_ui::modal_frame::ModalWidth::Compact)
                    .scroll_handle(self.modal_focus.scroll())
                    .footer(
                        row()
                            .child(
                                Button::new("cancel-delete-copy")
                                    .small()
                                    .ghost()
                                    .label("取消")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.storage_confirm = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("confirm-delete-copy")
                                    .small()
                                    .danger()
                                    .disabled(!allowed)
                                    .label("删除副本")
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.confirm_storage_copy(cx)),
                                    ),
                            ),
                    ),
            ),
        )
    }
}

#[cfg(all(test, feature = "ui-test"))]
mod tests {
    use super::storage_copy_list;
    use gpui_kit::{
        Context, InteractiveElement, IntoElement, ListAlignment, ListState, ParentElement, Render,
        ScrollDelta, ScrollHandle, ScrollWheelEvent, SharedString, StatefulInteractiveElement,
        Styled, TestAppContext, TestSupportExt, VisualTestContext, Window, div, point, px,
    };
    use std::{cell::RefCell, collections::BTreeSet, rc::Rc};

    #[test]
    fn storage_copy_filter_and_refresh_keep_identity_without_a_window() {
        use super::{StorageLayout, StorageUi};
        use crate::storage_backend::{CopyState, ManagedCopy, StorageReport};
        let mut report = StorageReport {
            copies: (0..3)
                .map(|index| ManagedCopy {
                    note_id: format!("copy-{index}"),
                    title: format!("Copy {index}"),
                    path: format!("C:/notes/{index}.md"),
                    bytes: 10,
                    state: CopyState::Present,
                    deletable: true,
                    reason: None,
                })
                .collect(),
            ..Default::default()
        };
        report.prepare_presentation();
        let presentation = &report.presentation;
        let state = StorageUi::default();
        let layout = StorageLayout {
            show_paths: true,
            draft_revision: 0,
            font_size: 14,
            spacing: 8,
            details_revision: 0,
        };
        let rows = state.sync(presentation, layout, "".into());
        assert_eq!(rows.as_slice(), &[0, 1, 2]);
        state.scroll.scroll_to(gpui_kit::ListOffset {
            item_ix: 1,
            offset_in_item: px(20.),
        });
        state.sync(
            presentation,
            StorageLayout {
                show_paths: false,
                ..layout
            },
            "".into(),
        );
        let top = state.scroll.logical_scroll_top();
        assert_eq!(top.item_ix, 1);
        assert_eq!(
            top.offset_in_item,
            px(20.),
            "hiding paths must preserve the same copy's anchor"
        );
        report.copies.remove(0);
        report.prepare_presentation();
        let rows = state.sync(&report.presentation, layout, "".into());
        let top = state.scroll.logical_scroll_top();
        assert_eq!(
            report.presentation.copies[rows[top.item_ix]].note_id,
            "copy-1"
        );
        assert_eq!(top.offset_in_item, px(20.));
        let filtered = state.sync(&report.presentation, layout, "copy 2".into());
        assert_eq!(filtered.as_slice(), &[1]);
        assert_eq!(
            state.scroll.logical_scroll_top().item_ix,
            0,
            "a new search starts at its first result"
        );
    }

    struct NestedStorage {
        outer: ScrollHandle,
        inner: ListState,
        rendered: Rc<RefCell<BTreeSet<usize>>>,
        heights: Option<Rc<Vec<f32>>>,
        top: f32,
    }

    impl Render for NestedStorage {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let rendered = self.rendered.clone();
            let heights = self.heights.clone();
            let page = div()
                .id("page-scroll")
                .test_support()
                .w(px(400.))
                .h(px(500.))
                .overflow_y_scroll()
                .track_scroll(&self.outer)
                .child(
                    storage_copy_list(&self.inner, move |ix, _, _| {
                        rendered.borrow_mut().insert(ix);
                        div()
                            .id(SharedString::from(format!("storage-test-row-{ix}")))
                            .test_support()
                            .h(px(heights.as_ref().map_or(50., |heights| heights[ix])))
                            .flex_shrink_0()
                            .into_any_element()
                    })
                    .w(px(350.)),
                )
                .child(div().h(px(1000.)).flex_shrink_0());
            div().pt(px(self.top)).child(page)
        }
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.refresh();
            _ = window.draw(cx);
        });
    }

    fn wheel(cx: &mut VisualTestContext, x: f32, y: f32, delta: f32) {
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(x), px(y)),
            delta: ScrollDelta::Pixels(point(px(0.), px(delta))),
            ..Default::default()
        });
        draw(cx);
    }

    #[gpui_kit::test]
    fn storage_wheel_is_contained_while_scrolling_and_at_both_boundaries(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let inner =
            ListState::new(20, ListAlignment::Top, px(48.)).with_uniform_item_height(px(50.));
        let outer = ScrollHandle::new();
        let (_, cx) = cx.add_window_view(|_, _| NestedStorage {
            inner: inner.clone(),
            outer: outer.clone(),
            rendered: Rc::default(),
            heights: None,
            top: 0.,
        });
        draw(cx);
        assert!(inner.max_offset_for_scrollbar().y > px(0.));
        assert!(outer.max_offset().y > px(0.));

        wheel(cx, 100., 100., -80.);
        assert_eq!(inner.scroll_px_offset_for_scrollbar().y, px(-80.));
        assert_eq!(outer.offset().y, px(0.), "inner wheel moved Settings");

        wheel(cx, 100., 100., -10000.);
        assert_eq!(
            inner.scroll_px_offset_for_scrollbar().y,
            -inner.max_offset_for_scrollbar().y
        );
        wheel(cx, 100., 100., -80.);
        assert_eq!(outer.offset().y, px(0.), "bottom boundary moved Settings");

        wheel(cx, 100., 100., 10000.);
        assert_eq!(inner.scroll_px_offset_for_scrollbar().y, px(0.));
        // Put Settings in the middle while keeping the top of the list visible.
        outer.set_offset(point(px(0.), px(-40.)));
        draw(cx);
        wheel(cx, 100., 100., 80.);
        assert_eq!(inner.scroll_px_offset_for_scrollbar().y, px(0.));
        assert_eq!(outer.offset().y, px(-40.), "top boundary moved Settings");

        // The narrow right-hand strip is outside the nested list.
        wheel(cx, 380., 100., -80.);
        assert_eq!(inner.scroll_px_offset_for_scrollbar().y, px(0.));
        assert_eq!(
            outer.offset().y,
            px(-120.),
            "Settings did not scroll outside list"
        );
    }

    #[gpui_kit::test]
    fn storage_list_builds_only_visible_rows_at_hundred_and_thousand_copy_scale(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        for count in [100, 1000] {
            let rendered = Rc::new(RefCell::new(BTreeSet::new()));
            let inner = ListState::new(count, ListAlignment::Top, px(48.))
                .with_uniform_item_height(px(50.));
            let (_, cx) = cx.add_window_view(|_, _| NestedStorage {
                inner: inner.clone(),
                outer: ScrollHandle::new(),
                rendered: rendered.clone(),
                heights: None,
                top: 0.,
            });
            draw(cx);
            assert!(
                rendered.borrow().len() < 12,
                "constructed offscreen rows at {count} copies"
            );
            assert!(!rendered.borrow().contains(&(count - 1)));
            rendered.borrow_mut().clear();
            inner.scroll_to_end();
            draw(cx);
            assert!(
                rendered.borrow().contains(&(count - 1)),
                "final copy must be reachable"
            );
            assert!(rendered.borrow().len() < 12);
            assert_eq!(
                inner.scroll_px_offset_for_scrollbar().y,
                -inner.max_offset_for_scrollbar().y
            );
        }
    }

    #[gpui_kit::test]
    fn storage_scrollbar_drag_reaches_end_for_uniform_and_variable_rows(cx: &mut TestAppContext) {
        use gpui_kit::test::TestWindowExt;

        cx.update(gpui_kit::init);
        cx.update(|cx| {
            studyapp_ui::theme::apply(&Default::default(), false, cx);
        });
        let mut results = Vec::new();
        for (case, minimum_height) in [
            ("uniform", None),
            ("shrinking", Some(50.)),
            ("growing", Some(110.)),
        ] {
            let variable = minimum_height.is_some();
            let inner = ListState::new(20, ListAlignment::Top, px(48.));
            let heights = minimum_height.map(|minimum| {
                Rc::new(
                    (0..20)
                        .map(|index| minimum + (index % 3) as f32 * 30.)
                        .collect(),
                )
            });
            if variable {
                // Match StorageUi's initial height estimate while letting the
                // visible rows replace it with their actual measured heights.
                inner.reset_with_uniform_height(20, px(96.));
            }
            let inner = if variable {
                inner
            } else {
                inner.with_uniform_item_height(px(50.))
            };
            let outer = ScrollHandle::new();
            let rendered = Rc::new(RefCell::new(BTreeSet::new()));
            let (_, cx) = cx.add_window_view(|_, _| NestedStorage {
                inner: inner.clone(),
                outer: outer.clone(),
                rendered: rendered.clone(),
                heights,
                top: 0.,
            });
            draw(cx);
            cx.update(|window, cx| {
                // Repeat the native smoke's boundary-wheel setup and drag.
                for delta in [-80., -10000., -80., 10000., 80.] {
                    window.scroll(
                        "storage-copy-list",
                        ScrollDelta::Pixels(point(px(0.), px(delta))),
                        cx,
                    );
                }
                let bounds = window.find("storage-copy-list").bounds();
                let before = inner.scroll_px_offset_for_scrollbar().y;
                let max_before = inner.max_offset_for_scrollbar().y;
                rendered.borrow_mut().clear();
                window.drag(
                    point(bounds.right() - px(8.), bounds.top() + px(8.)),
                    point(bounds.right() - px(8.), bounds.bottom() - px(8.)),
                    cx,
                );
                let after = inner.scroll_px_offset_for_scrollbar().y;
                let max_after = inner.max_offset_for_scrollbar().y;
                let final_visible = window.try_find("storage-test-row-19").is_some_and(|row| {
                    row.visible() && bounds.intersect(&row.bounds()).size.height > px(0.)
                });
                eprintln!(
                    "storage scrollbar case={case}: bounds={bounds:?}; before={before:?}/{max_before:?}; after={after:?}/{max_after:?}; difference={:?}; top={:?}; rendered={:?}",
                    after + max_after,
                    inner.logical_scroll_top(),
                    rendered.borrow(),
                );
                assert_eq!(outer.offset().y, px(0.), "drag moved the outer page");
                results.push((case, after, max_after, final_visible));
            });
        }
        for (case, offset, max, reached_last) in results {
            assert_eq!(offset, -max, "scrollbar drag failed for {case} rows");
            assert!(reached_last, "last row was not visible for {case} rows");
        }
    }

    #[gpui_kit::test]
    fn storage_scrollbar_drag_placement_checks_outer_clip(cx: &mut TestAppContext) {
        use gpui_kit::test::TestWindowExt;

        cx.update(gpui_kit::init);
        cx.update(|cx| {
            studyapp_ui::theme::apply(&Default::default(), false, cx);
        });
        let inner =
            ListState::new(20, ListAlignment::Top, px(48.)).with_uniform_item_height(px(50.));
        let outer = ScrollHandle::new();
        let (_, cx) = cx.add_window_view(|_, _| NestedStorage {
            inner: inner.clone(),
            outer: outer.clone(),
            rendered: Rc::default(),
            heights: None,
            top: 240.,
        });
        draw(cx);
        outer.set_offset(point(px(0.), px(-80.)));
        draw(cx);
        cx.update(|window, cx| {
            let list = window.find("storage-copy-list").bounds();
            let outer_bounds = window.find("page-scroll").bounds();
            let start = point(list.right() - px(8.), list.top() + px(8.));
            // These are the native smoke's old placement checks. They can pass
            // while the list's thumb is clipped by its own scrolling ancestor.
            assert!(list.top() > px(120.));
            assert!(list.bottom() < window.viewport_size().height - px(50.));
            assert!(!outer_bounds.contains(&start));
            eprintln!(
                "storage scrollbar placement: viewport={:?}; list={list:?}; outer={outer_bounds:?}; start={start:?}",
                window.viewport_size(),
            );
            outer.set_offset(point(px(0.), px(0.)));
            window.render_frame(cx);
            let list = window.find("storage-copy-list").bounds();
            let start = point(list.right() - px(8.), list.top() + px(8.));
            assert!(outer_bounds.contains(&start));
            assert_eq!(list.intersect(&outer_bounds), list);
            window.drag(
                start,
                point(list.right() - px(8.), list.bottom() - px(8.)),
                cx,
            );
            assert_eq!(
                inner.scroll_px_offset_for_scrollbar().y,
                -inner.max_offset_for_scrollbar().y,
                "revealing the thumb must make the same drag reach the end"
            );
            let final_row = window.find("storage-test-row-19");
            assert!(final_row.visible());
            assert!(list.intersect(&final_row.bounds()).size.height > px(0.));
        });
    }

    #[gpui_kit::test]
    fn storage_normal_scrollbar_end_does_not_follow_appended_rows(cx: &mut TestAppContext) {
        use gpui_kit::test::TestWindowExt;

        cx.update(gpui_kit::init);
        cx.update(|cx| {
            studyapp_ui::theme::apply(&Default::default(), false, cx);
        });
        let inner =
            ListState::new(20, ListAlignment::Top, px(48.)).with_uniform_item_height(px(50.));
        inner.set_follow_mode(gpui_kit::FollowMode::Normal);
        let (_, cx) = cx.add_window_view(|_, _| NestedStorage {
            inner: inner.clone(),
            outer: ScrollHandle::new(),
            rendered: Rc::default(),
            heights: None,
            top: 0.,
        });
        draw(cx);
        cx.update(|window, cx| {
            let bounds = window.find("storage-copy-list").bounds();
            window.drag(
                point(bounds.right() - px(8.), bounds.top() + px(8.)),
                point(bounds.right() - px(8.), bounds.bottom() - px(8.)),
                cx,
            );
        });
        assert_eq!(
            inner.scroll_px_offset_for_scrollbar().y,
            -inner.max_offset_for_scrollbar().y
        );
        assert!(!inner.is_following_tail());
        let before = inner.logical_scroll_top();
        let offset = inner.scroll_px_offset_for_scrollbar().y;
        inner.splice(20..20, 3);
        draw(cx);
        let after = inner.logical_scroll_top();
        assert_eq!(after.item_ix, before.item_ix);
        assert_eq!(after.offset_in_item, before.offset_in_item);
        assert_eq!(inner.scroll_px_offset_for_scrollbar().y, offset);
        assert!(offset > -inner.max_offset_for_scrollbar().y);
        assert!(!inner.is_following_tail());
    }

    #[gpui_kit::test]
    fn storage_normal_scrollbar_middle_does_not_snap_to_end_when_rows_grow(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        cx.update(|cx| {
            studyapp_ui::theme::apply(&Default::default(), false, cx);
        });
        let inner = ListState::new(20, ListAlignment::Top, px(48.));
        inner.reset_with_uniform_height(20, px(96.));
        inner.set_follow_mode(gpui_kit::FollowMode::Normal);
        let (_, cx) = cx.add_window_view(|_, _| NestedStorage {
            inner: inner.clone(),
            outer: ScrollHandle::new(),
            rendered: Rc::default(),
            heights: Some(Rc::new(
                (0..20)
                    .map(|index| 110. + (index % 3) as f32 * 30.)
                    .collect(),
            )),
            top: 0.,
        });
        draw(cx);
        let frozen_max = inner.max_offset_for_scrollbar().y;
        inner.scrollbar_drag_started();
        inner.set_offset_from_scrollbar(point(px(0.), -frozen_max / 2.));
        draw(cx);
        assert_eq!(inner.max_offset_for_scrollbar().y, frozen_max);
        let before_release = inner.logical_scroll_top();
        inner.scrollbar_drag_ended();
        draw(cx);
        assert!(inner.max_offset_for_scrollbar().y > frozen_max);
        let after_release = inner.logical_scroll_top();
        assert_eq!(after_release.item_ix, before_release.item_ix);
        assert_eq!(after_release.offset_in_item, before_release.offset_in_item);
        let offset = inner.scroll_px_offset_for_scrollbar().y;
        assert!(offset < px(0.));
        assert!(offset > -inner.max_offset_for_scrollbar().y);
        assert!(!inner.is_following_tail());
        assert!(!inner.is_scrollbar_dragging());
    }
}
