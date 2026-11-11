//! Task records come from controllers, independently of dismissible notices.
use crate::{components::*, controls::Button};
use gpui_kit::*;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskState {
    Running,
    Completed,
    Failed,
}
#[derive(Clone, Debug)]
pub struct ActivityTask {
    pub id: &'static str,
    pub title: String,
    pub detail: String,
    pub state: TaskState,
    pub completed: Option<(u64, u64)>,
}
pub fn task_list(
    tasks: Vec<(ActivityTask, Option<Button>)>,
    scroll: &ScrollHandle,
    layout: studyapp_core::NoticeLayout,
) -> Div {
    let table = layout == studyapp_core::NoticeLayout::Alternative;
    let hud_layout =
        crate::theme::notification_appearance().layout(studyapp_core::NoticeStyle::Hud);
    let (running_tasks, finished_tasks): (Vec<_>, Vec<_>) = tasks
        .into_iter()
        .partition(|(task, _)| task.state == TaskState::Running);
    let mut groups = [Some(running_tasks), Some(finished_tasks)];
    let mut body = column().gap_4();
    for (index, (running, label)) in [(true, "进行中"), (false, "已结束")]
        .into_iter()
        .enumerate()
    {
        let mut group = column()
            .gap_2()
            .child(div().font_weight(FontWeight::BOLD).child(label));
        if table {
            group = group.child(
                row()
                    .gap_3()
                    .border_b_1()
                    .border_color(crate::theme::color(BORDER))
                    .pb_2()
                    .child(muted("任务").w(px(100.)))
                    .child(muted("状态").w(px(60.)))
                    .child(muted("进度与说明").flex_1())
                    .child(muted("可用操作").w(px(100.))),
            );
        }
        let mut count = 0;
        for (task, action) in groups[index].take().unwrap_or_default() {
            count += 1;
            let status = match task.state {
                TaskState::Running => "进行中",
                TaskState::Completed => "已完成",
                TaskState::Failed => "失败",
            };
            let item = if table {
                let detail = if running {
                    crate::notification_surfaces::hud_with_id(
                        task.id,
                        task.detail.clone(),
                        task.completed,
                        hud_layout,
                    )
                } else {
                    column().child(muted(task.detail.clone()))
                };
                row()
                    .id(task.id)
                    .w_full()
                    .items_start()
                    .flex_wrap()
                    .gap_3()
                    .py_3()
                    .border_b_1()
                    .border_color(crate::theme::color(BORDER))
                    .child(
                        div()
                            .w(px(100.))
                            .min_w_0()
                            .font_weight(FontWeight::MEDIUM)
                            .child(task.title),
                    )
                    .child(muted(status).w(px(60.)))
                    .child(div().flex_1().min_w(px(120.)).child(detail))
                    .child(column().w(px(100.)).children(action))
            } else {
                let mut item = column()
                    .id(task.id)
                    .w_full()
                    .gap_2()
                    .py_3()
                    .border_b_1()
                    .border_color(crate::theme::color(BORDER))
                    .child(
                        row()
                            .flex_wrap()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(task.title),
                            )
                            .child(muted(status)),
                    );
                if running {
                    item = item.child(crate::notification_surfaces::hud_with_id(
                        task.id,
                        task.detail,
                        task.completed,
                        hud_layout,
                    ));
                } else {
                    item = item.child(muted(task.detail));
                }
                item.children(action)
            };
            group = group.child(item);
        }
        if count == 0 {
            group = group.child(muted(if running {
                "当前没有运行中的任务"
            } else {
                "本次会话尚无已结束任务"
            }));
        }
        body = body.child(group);
    }
    column().w_full().min_h_0().gap_3().child(
        scroll_body("activity-task-scroll", scroll)
            .flex_1()
            .min_h_0()
            .child(body),
    )
}
