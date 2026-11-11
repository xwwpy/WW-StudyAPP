//! Preview real components with explicitly labelled examples and inert actions.
use crate::{
    components::*,
    controls::Button,
    modal_frame::{ModalFrame, ModalWidth},
};
use gpui_kit::{
    component::{
        Disableable, Sizable,
        menu::{PopupMenu, PopupMenuItem},
        popover::Popover,
    },
    *,
};
use studyapp_core::{NoticeStyle, NotificationAppearance};
#[derive(IntoElement)]
pub struct ComponentPreview {
    pub model: crate::notification_card::NotificationCardModel,
    pub appearance: NotificationAppearance,
    pub scroll: ScrollHandle,
}
impl RenderOnce for ComponentPreview {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let outer = window
            .use_keyed_state("component-preview-outer-scroll", cx, |_, _| {
                ScrollHandle::new()
            })
            .read(cx)
            .clone();
        scroll_body("component-preview-scroll", &outer)
            .max_h(px(360.))
            .min_w_0()
            .child(component_preview(
                self.model,
                &self.appearance,
                &self.scroll,
                window,
                cx,
            ))
    }
}
pub fn component_preview(
    model: crate::notification_card::NotificationCardModel,
    appearance: &NotificationAppearance,
    scroll: &ScrollHandle,
    window: &mut Window,
    cx: &mut App,
) -> Div {
    let style = model.style;
    let layout = appearance.layout(style);
    let button = |id, text| Button::new(id).small().label(text).disabled(true);
    let body = match style {
        NoticeStyle::Bubble
        | NoticeStyle::Snackbar
        | NoticeStyle::Banner
        | NoticeStyle::ModalCard
        | NoticeStyle::CornerCard => crate::notification_card::notification_card(
            model,
            appearance,
            scroll,
            button("preview-dismiss", "关闭"),
            Some(button("preview-action", "查看详情")),
        )
        .into_any_element(),
        NoticeStyle::Hud => {
            crate::notification_surfaces::hud("布局示例 · 未提供任务总量", None, layout)
                .into_any_element()
        }
        NoticeStyle::ActionSheet => crate::notification_surfaces::action_sheet(
            vec![
                button("preview-create", "新建笔记"),
                button("preview-import", "导入文件…"),
            ],
            button("preview-cancel", "取消"),
            layout,
        )
        .into_any_element(),
        NoticeStyle::Share => crate::notification_surfaces::share_panel(
            column()
                .gap_2()
                .child(heading("内容预览示例"))
                .child(muted("这里显示真正选中的内容，预览不导出文件。")),
            column()
                .gap_2()
                .child(heading("可用目标"))
                .child(button("preview-export", "选择文件并导出…")),
            layout,
            window.viewport_size(),
        )
        .into_any_element(),
        NoticeStyle::ActivityView => column()
            .h(px(300.))
            .child(crate::activity_view::task_list(
                vec![(
                    crate::activity_view::ActivityTask {
                        id: "preview-task",
                        title: "示例任务（不运行）".into(),
                        detail: "任务状态由真实控制器提供".into(),
                        state: crate::activity_view::TaskState::Completed,
                        completed: None,
                    },
                    None,
                )],
                scroll,
                layout,
            ))
            .into_any_element(),
        NoticeStyle::Dialog => ModalFrame::new(
            "表单示例",
            column().gap_3().child(muted("表单内容独立滚动")).child(
                crate::settings_components::field(
                    "示例字段",
                    button("preview-form-value", "只读示例"),
                ),
            ),
        )
        .width(ModalWidth::Wide)
        .footer(
            row()
                .gap_2()
                .child(button("preview-form-cancel", "取消"))
                .child(button("preview-form-save", "保存")),
        )
        .into_any_element(),
        NoticeStyle::Actionbar => row()
            .flex_wrap()
            .gap_2()
            .child(button("preview-bold", "加粗"))
            .child(button("preview-italic", "斜体"))
            .child(button("preview-copy", "复制"))
            .into_any_element(),
        NoticeStyle::Anchored => {
            crate::interactive_surface::popover(Popover::new("preview-popover"))
                .trigger(
                    Button::new("preview-popover-trigger")
                        .small()
                        .label("示例：点击查看详情"),
                )
                .child(
                    column()
                        .gap_2()
                        .child(heading("控件详情".to_owned()))
                        .child(muted(
                            "这是可交互 Popover，不是鼠标悬停说明。点击外部关闭。",
                        ))
                        .child(button("preview-popover-inert", "示例控件（不执行操作）")),
                )
                .into_any_element()
        }
        NoticeStyle::PopupMenu => {
            let state = window.use_keyed_state("notice-popup-preview", cx, |window, cx| {
                PopupMenu::build(window, cx, |menu, _, _| {
                    menu.item(PopupMenuItem::new("示例命令（不执行）").disabled(true))
                        .separator()
                        .item(PopupMenuItem::new("另一项命令").disabled(true))
                })
            });
            state.read(cx).clone().into_any_element()
        }
        NoticeStyle::Tooltip => Button::new("preview-tooltip-trigger")
            .small()
            .label("示例：悬停查看说明")
            .tooltip("纯文本说明，不包含按钮，也不会执行操作")
            .into_any_element(),
    };
    column().w_full().min_w_0().gap_2().child(body)
}
