//! A passive view of real controllers. Closing it changes no worker state.
use crate::{components::*, controls::Button, sync_ui::SyncPanel, update_ui::UpdatePanel};
use gpui_kit::{
    component::{Disableable, button::ButtonVariants},
    prelude::FluentBuilder,
    *,
};
use studyapp_core::{NoticeLayout, NoticeStyle};
use studyapp_ui::activity_view::{ActivityTask, TaskState};
pub(crate) struct ActivityOcclusion(pub Vec<Bounds<Pixels>>);
pub(crate) struct ActivityPanel {
    bounds: Option<Bounds<Pixels>>,
    pub open: bool,
    sync: Entity<SyncPanel>,
    updates: Entity<UpdatePanel>,
    migration: Option<ActivityTask>,
    scroll: ScrollHandle,
}
impl EventEmitter<ActivityOcclusion> for ActivityPanel {}
impl ActivityPanel {
    pub fn new(
        sync: Entity<SyncPanel>,
        updates: Entity<UpdatePanel>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&sync, |_, _, cx| cx.notify()).detach();
        cx.observe(&updates, |_, _, cx| cx.notify()).detach();
        Self {
            bounds: None,
            open: false,
            sync,
            updates,
            migration: None,
            scroll: ScrollHandle::new(),
        }
    }
    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        self.open = !self.open;
        cx.notify();
    }
    pub fn migration_progress(
        &mut self,
        progress: &studyapp_ui::transfer_progress::TransferProgress,
        cx: &mut Context<Self>,
    ) {
        self.migration = Some(ActivityTask {
            id: "migration-task",
            title: "资料迁移".into(),
            detail: progress.label.clone(),
            state: TaskState::Running,
            completed: progress.completed(),
        });
        cx.notify();
    }
    pub fn migration_finished(&mut self, result: Result<(), String>, cx: &mut Context<Self>) {
        if let Some(task) = self
            .migration
            .as_mut()
            .filter(|task| task.state == TaskState::Running)
        {
            task.state = if result.is_ok() {
                TaskState::Completed
            } else {
                TaskState::Failed
            };
            task.detail = result
                .err()
                .unwrap_or_else(|| "迁移已完成，资料已切换到目标位置。".into());
            task.completed = None;
            cx.notify();
        }
    }
}
impl Render for ActivityPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.open {
            if self.bounds.take().is_some() {
                cx.emit(ActivityOcclusion(vec![]));
            }
            return div();
        }
        let appearance = crate::theme::notification_appearance();
        let layout = appearance.layout(NoticeStyle::ActivityView);
        let surface = appearance.surface(NoticeStyle::ActivityView);
        let sync = self.sync.read(cx);
        let status = &sync.status;
        let mut tasks = vec![];
        if status.busy || status.failed || status.last_success.is_some() {
            let state = if status.busy {
                TaskState::Running
            } else if status.failed {
                TaskState::Failed
            } else {
                TaskState::Completed
            };
            let target = self.sync.clone();
            let retry = (state == TaskState::Failed).then(|| {
                Button::new("activity-retry-sync")
                    .label("重试同步")
                    .disabled(sync.editing_locked || sync.has_unsaved_changes(cx))
                    .tooltip(if sync.has_unsaved_changes(cx) {
                        "请先保存或放弃同步设置草稿"
                    } else {
                        "使用已保存设置重试同步"
                    })
                    .on_click(move |_, _, cx| target.update(cx, |panel, cx| panel.start(false, cx)))
            });
            tasks.push((
                ActivityTask {
                    id: "sync-task",
                    title: "资料同步".into(),
                    detail: status.message.clone(),
                    state,
                    completed: None,
                },
                retry,
            ));
        }
        if let Some(task) = self.updates.read(cx).activity_task() {
            let target = self.updates.clone();
            // A fresh check is supported; do not pretend to resume a download.
            let action = (task.state == TaskState::Failed).then(|| {
                Button::new("activity-check-update")
                    .label("重新检查更新")
                    .on_click(move |_, _, cx| {
                        target.update(cx, |panel, cx| panel.retry_from_notice(cx))
                    })
            });
            tasks.push((task, action));
        }
        if let Some(task) = self.migration.clone() {
            tasks.push((task, None));
        }
        let viewport = window.viewport_size();
        let primary = layout == NoticeLayout::Primary;
        let width = if primary {
            f32::from(surface.width).min(f32::from(viewport.width) - 24.)
        } else {
            let configured = appearance
                .surface_patches
                .get(&NoticeStyle::ActivityView)
                .and_then(|patch| patch.width)
                .or_else(|| {
                    appearance
                        .surfaces
                        .get(&NoticeStyle::ActivityView)
                        .map(|surface| surface.width)
                });
            (f32::from(viewport.width) * 0.8).min(configured.map_or(800., f32::from))
        };
        let height = (f32::from(viewport.height) - crate::design::TITLE_BAR_HEIGHT - 24.)
            .max(0.)
            .min(f32::from(surface.max_height) + 104.);
        let owner = cx.weak_entity();
        let panel = studyapp_ui::interactive_surface::frame(NoticeStyle::ActivityView, &appearance)
            .flex()
            .flex_col()
            .relative()
            .id("task-activity-panel")
            .test_support()
            .occlude()
            .gap_3()
            .p_4()
            .w(px(width.max(0.)))
            .h(px(height))
            .shadow_lg()
            .child(
                row()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .font_weight(FontWeight::BOLD)
                            .child("任务活动"),
                    )
                    .child(
                        Button::new("close-activity")
                            .ghost()
                            .label("收起")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.open = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(muted(
                "收起面板不会取消任务。这里只显示真实任务状态与支持的操作。",
            ))
            .child(studyapp_ui::activity_view::task_list(tasks, &self.scroll, layout).flex_1());
        let panel = panel.child(
            canvas(
                move |bounds, _, cx| {
                    let _ = owner.update(cx, |this, cx| {
                        if this.bounds != Some(bounds) {
                            this.bounds = Some(bounds);
                            cx.emit(ActivityOcclusion(vec![bounds]));
                        }
                    });
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0(),
        );
        div()
            .absolute()
            .top(px(crate::design::TITLE_BAR_HEIGHT + 12.))
            .when(primary, |v| v.right(px(12.)))
            .when(!primary, |v| {
                v.left(px((f32::from(viewport.width) - width) * 0.5))
            })
            .child(panel)
    }
}
