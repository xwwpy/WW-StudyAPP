//! Preview real components with explicitly labelled examples and inert actions.
use crate::{
    components::*,
    controls::Button,
    modal_frame::{ModalFrame, ModalTone, ModalWidth},
};
use gpui_kit::{
    component::{
        Disableable, Sizable,
        menu::{PopupMenu, PopupMenuItem},
        popover::Popover,
    },
    *,
};
use studyapp_core::{NoticeLayout, NoticeStyle, NotificationAppearance};
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
        NoticeStyle::ModalCard => ModalFrame::new(model.title, muted(model.summary))
            .tone(ModalTone::Warning)
            .width(ModalWidth::Compact)
            .preview(appearance)
            .footer(
                row()
                    .flex_wrap()
                    .gap_2()
                    .child(button("preview-alert-cancel", "取消"))
                    .child(button("preview-alert-confirm", "确认示例操作")),
            )
            .into_any_element(),
        NoticeStyle::ActionSheet => ModalFrame::new(
            "新建或导入",
            crate::notification_surfaces::action_sheet(
                vec![
                    button("preview-create", "新建笔记"),
                    button("preview-import", "导入文件…"),
                ],
                button("preview-cancel", "取消"),
                layout,
            ),
        )
        .component(style)
        .preview(appearance)
        .into_any_element(),
        NoticeStyle::Share => ModalFrame::new(
            "分享与导出",
            crate::notification_surfaces::share_panel(
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
            ),
        )
        .width(ModalWidth::Wide)
        .component(style)
        .preview(appearance)
        .footer(button("preview-share-close", "关闭"))
        .into_any_element(),
        NoticeStyle::ActivityView => crate::activity_view::panel(
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
            appearance,
            button("preview-activity-close", "收起"),
        )
        .h(px(300.))
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
        .preview(appearance)
        .footer(
            row()
                .gap_2()
                .child(button("preview-form-cancel", "取消"))
                .child(button("preview-form-save", "保存")),
        )
        .into_any_element(),
        NoticeStyle::Actionbar => {
            let surface = appearance.surface(style);
            let tools = crate::interactive_surface::frame(style, appearance)
                .flex()
                .flex_wrap()
                .gap_2()
                .p_2()
                .max_w_full()
                .max_h(px(f32::from(surface.max_height)))
                .child(button("preview-bold", "加粗"))
                .child(button("preview-italic", "斜体"))
                .child(button("preview-copy", "复制"));
            if layout == NoticeLayout::Alternative {
                column()
                    .gap_2()
                    .child(tools.w_full())
                    .child(muted("正文示例 · 工具栏在内容上方占位，不覆盖选区。"))
            } else {
                column()
                    .gap_2()
                    .child(muted("选区示例"))
                    .child(tools.w(px(f32::from(surface.width))))
                    .child(muted("悬浮于选区附近；真实编辑器中靠近边缘时自动避让。"))
            }
            .into_any_element()
        }
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
