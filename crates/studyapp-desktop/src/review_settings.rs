//! A paginated ordered number list; pagination never truncates saved stages.
use crate::{app::StudyApp, components::*, controls::Button, settings_components as form};
use gpui_kit::{
    component::{
        Disableable, Sizable,
        button::ButtonVariants,
        input::{InputEvent, InputState},
    },
    *,
};
const PAGE_SIZE: usize = 5;
impl StudyApp {
    fn add_review_input(&mut self, value: u32, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).default_value(value.to_string()));
        cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::Change) {
                let values = this
                    .preferences_ui
                    .review_stages
                    .iter()
                    .map(|input| input.read(cx).value().to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                this.review_days
                    .update(cx, |input, cx| input.set_value(values, window, cx));
                cx.notify();
            }
        })
        .detach();
        self.preferences_ui.review_stages.push(input);
    }
    pub(crate) fn fill_review_stages(
        &mut self,
        stages: &[u32],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preferences_ui.review_stages.clear();
        self.preferences_ui.review_page = 0;
        for stage in stages {
            self.add_review_input(*stage, window, cx);
        }
    }
    fn sync_review_stage_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self
            .preferences_ui
            .review_stages
            .iter()
            .map(|input| input.read(cx).value().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        self.review_days
            .update(cx, |input, cx| input.set_value(value, window, cx));
        cx.notify();
    }
    pub(crate) fn review_stages(&self, cx: &Context<Self>) -> Div {
        let stages = &self.preferences_ui.review_stages;
        let busy = self.preferences_ui.busy || self.preferences_ui.replacing || self.storage_busy;
        let pages = stages.len().div_ceil(PAGE_SIZE).max(1);
        let page = self.preferences_ui.review_page.min(pages - 1);
        let defaults = studyapp_core::Settings::default().review_days;
        let mut view = form::section("复习间隔").child(muted("每个阶段直接填写天数。1–20 个阶段，每阶段 1–3650 天；跨页保留全部草稿，只影响新建复习计划。"));
        for (index, input) in stages
            .iter()
            .enumerate()
            .skip(page * PAGE_SIZE)
            .take(PAGE_SIZE)
        {
            view = view.child(form::settings_row(
                format!("阶段 {}", index + 1),
                row()
                    .flex_wrap()
                    .child(form::NumericSetting::new(
                        format!("review-stage-{index}"),
                        input,
                        "天",
                        defaults.get(index).copied().unwrap_or(1).into(),
                        1,
                        3650,
                        busy,
                    ))
                    .child(
                        Button::new(SharedString::from(format!("review-remove-{index}")))
                            .small()
                            .ghost()
                            .label("删除阶段")
                            .disabled(busy || stages.len() <= 1)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if this.preferences_ui.review_stages.len() > 1
                                    && index < this.preferences_ui.review_stages.len()
                                {
                                    this.preferences_ui.review_stages.remove(index);
                                    this.sync_review_stage_list(window, cx);
                                }
                            })),
                    ),
            ));
        }
        view = view.child(
            row()
                .flex_wrap()
                .child(
                    Button::new("review-add-stage")
                        .small()
                        .label("添加阶段")
                        .disabled(busy || stages.len() >= 20)
                        .on_click(cx.listener(|this, _, window, cx| {
                            if this.preferences_ui.review_stages.len() < 20 {
                                this.add_review_input(1, window, cx);
                                this.preferences_ui.review_page =
                                    (this.preferences_ui.review_stages.len() - 1) / PAGE_SIZE;
                                this.sync_review_stage_list(window, cx);
                            }
                        })),
                )
                .child(
                    Button::new("review-reset-stages")
                        .small()
                        .ghost()
                        .label("重置复习间隔")
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.fill_review_stages(
                                &studyapp_core::Settings::default().review_days,
                                window,
                                cx,
                            );
                            this.sync_review_stage_list(window, cx);
                        })),
                ),
        );
        if pages > 1 {
            view = view.child(
                row()
                    .child(
                        Button::new("review-previous-page")
                            .small()
                            .ghost()
                            .label("上一页")
                            .disabled(busy || page == 0)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.preferences_ui.review_page = page.saturating_sub(1);
                                cx.notify();
                            })),
                    )
                    .child(muted(format!(
                        "第 {} / {} 页 · 共 {} 阶段",
                        page + 1,
                        pages,
                        stages.len()
                    )))
                    .child(
                        Button::new("review-next-page")
                            .small()
                            .ghost()
                            .label("下一页")
                            .disabled(busy || page + 1 >= pages)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.preferences_ui.review_page = (page + 1).min(pages - 1);
                                cx.notify();
                            })),
                    ),
            );
        }
        view
    }
}
