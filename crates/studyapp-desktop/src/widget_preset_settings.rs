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
    fn add_widget_preset_input(&mut self, value: u64, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).default_value(value.to_string()));
        cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::Change) {
                let values = this
                    .preferences_ui
                    .widget_preset_stages
                    .iter()
                    .map(|input| input.read(cx).value().to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                this.preferences_ui.widget_fields["widget-presets"]
                    .update(cx, |input, cx| input.set_value(values, window, cx));
                cx.notify();
            }
        })
        .detach();
        self.preferences_ui.widget_preset_stages.push(input);
    }
    pub(crate) fn fill_widget_preset_stages(
        &mut self,
        stages: &[u64],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preferences_ui.widget_preset_stages.clear();
        self.preferences_ui.widget_preset_page = 0;
        for stage in stages {
            self.add_widget_preset_input(*stage, window, cx);
        }
    }
    fn sync_widget_preset_stage_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self
            .preferences_ui
            .widget_preset_stages
            .iter()
            .map(|input| input.read(cx).value().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        self.preferences_ui.widget_fields["widget-presets"]
            .update(cx, |input, cx| input.set_value(value, window, cx));
        cx.notify();
    }
    pub(crate) fn widget_preset_stages(&self, cx: &Context<Self>) -> Div {
        let stages = &self.preferences_ui.widget_preset_stages;
        let busy = self.preferences_ui.busy || self.preferences_ui.replacing || self.storage_busy;
        let pages = stages.len().div_ceil(PAGE_SIZE).max(1);
        let page = self.preferences_ui.widget_preset_page.min(pages - 1);
        let defaults = studyapp_core::TimerWidgetSettings::default().countdown_presets;
        let mut view = form::section("倒计时预设").child(muted("每个预设直接填写分钟数。1–8 个不重复预设，每项为正整数分钟；跨页保留全部草稿，保存后用于倒计时选择。"));
        for (index, input) in stages
            .iter()
            .enumerate()
            .skip(page * PAGE_SIZE)
            .take(PAGE_SIZE)
        {
            view = view.child(form::settings_row(
                format!("预设 {}", index + 1),
                row()
                    .flex_wrap()
                    .child(form::NumericSetting::new(
                        format!("widget_preset-stage-{index}"),
                        input,
                        "分钟",
                        defaults.get(index).copied().unwrap_or(1),
                        1,
                        u64::MAX / 60,
                        busy,
                    ))
                    .child(
                        Button::new(SharedString::from(format!("widget_preset-remove-{index}")))
                            .small()
                            .ghost()
                            .label("删除预设")
                            .disabled(busy || stages.len() <= 1)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if this.preferences_ui.widget_preset_stages.len() > 1
                                    && index < this.preferences_ui.widget_preset_stages.len()
                                {
                                    this.preferences_ui.widget_preset_stages.remove(index);
                                    this.sync_widget_preset_stage_list(window, cx);
                                }
                            })),
                    ),
            ));
        }
        view = view.child(
            row()
                .flex_wrap()
                .child(
                    Button::new("widget_preset-add-stage")
                        .small()
                        .label("添加预设")
                        .disabled(busy || stages.len() >= 8)
                        .on_click(cx.listener(|this, _, window, cx| {
                            if this.preferences_ui.widget_preset_stages.len() < 8 {
                                let next = (1..=9)
                                    .find(|candidate| {
                                        !this.preferences_ui.widget_preset_stages.iter().any(
                                            |input| {
                                                input.read(cx).value().parse::<u64>().ok()
                                                    == Some(*candidate)
                                            },
                                        )
                                    })
                                    .unwrap_or(1);
                                this.add_widget_preset_input(next, window, cx);
                                this.preferences_ui.widget_preset_page =
                                    (this.preferences_ui.widget_preset_stages.len() - 1)
                                        / PAGE_SIZE;
                                this.sync_widget_preset_stage_list(window, cx);
                            }
                        })),
                )
                .child(
                    Button::new("widget_preset-reset-stages")
                        .small()
                        .ghost()
                        .label("重置倒计时预设")
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.fill_widget_preset_stages(
                                &studyapp_core::TimerWidgetSettings::default().countdown_presets,
                                window,
                                cx,
                            );
                            this.sync_widget_preset_stage_list(window, cx);
                        })),
                ),
        );
        if pages > 1 {
            view = view.child(
                row()
                    .child(
                        Button::new("widget_preset-previous-page")
                            .small()
                            .ghost()
                            .label("上一页")
                            .disabled(busy || page == 0)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.preferences_ui.widget_preset_page = page.saturating_sub(1);
                                cx.notify();
                            })),
                    )
                    .child(muted(format!(
                        "第 {} / {} 页 · 共 {} 预设",
                        page + 1,
                        pages,
                        stages.len()
                    )))
                    .child(
                        Button::new("widget_preset-next-page")
                            .small()
                            .ghost()
                            .label("下一页")
                            .disabled(busy || page + 1 >= pages)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.preferences_ui.widget_preset_page = (page + 1).min(pages - 1);
                                cx.notify();
                            })),
                    ),
            );
        }
        view
    }
}
