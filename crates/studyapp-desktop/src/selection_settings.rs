use crate::{
    app::StudyApp,
    components::*,
    controls::{Button, Checkbox},
    settings_components as form,
};
use gpui_kit::{
    component::{Disableable, Selectable, input::InputState},
    *,
};
use studyapp_core::SelectionAppearance;

type ColorField = (
    &'static str,
    &'static str,
    fn(&mut SelectionAppearance) -> &mut Option<String>,
);
const FIELDS: [ColorField; 5] = [
    ("selection-background", "选中背景", |s| {
        &mut s.selected_background
    }),
    ("selection-text", "选中文字", |s| &mut s.selected_text),
    ("selection-border", "选中边框与标记", |s| {
        &mut s.selected_border
    }),
    ("selection-idle-background", "未选中背景", |s| {
        &mut s.idle_background
    }),
    ("selection-idle-text", "未选中文字", |s| {
        &mut s.idle_text
    }),
];
impl StudyApp {
    pub(crate) fn fill_selection_fields(
        &mut self,
        value: &SelectionAppearance,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for (key, _, field) in FIELDS {
            let text = field(&mut value.clone()).clone().unwrap_or_default();
            if let Some(input) = self.preferences_ui.appearance_fields.get(key) {
                input.update(cx, |input, cx| input.set_value(text, window, cx));
            } else {
                let input = cx.new(|cx| InputState::new(window, cx).default_value(text));
                self.preferences_ui
                    .appearance_fields
                    .insert(key, input.clone());
                let color = cx.new(|cx| {
                    crate::settings_color::ColorSetting::new(key, input, true, window, cx)
                });
                cx.subscribe(
                    &color,
                    move |this, color, _: &crate::settings_color::ColorSettingChanged, cx| {
                        let raw = color.read(cx).preview_value(cx);
                        if raw.is_empty() || studyapp_core::parse_rgba_color(&raw).is_some() {
                            if let Some(draft) = &mut this.preferences_ui.draft {
                                *field(&mut draft.appearance.selection) =
                                    (!raw.is_empty()).then(|| raw.to_string());
                            }
                            cx.notify();
                        }
                    },
                )
                .detach();
                self.preferences_ui.appearance_colors.insert(key, color);
            }
        }
    }
    pub(crate) fn selection_settings(&self, cx: &mut Context<Self>) -> Div {
        let style = self
            .preferences_ui
            .draft
            .as_ref()
            .map(|d| d.appearance.selection.clone())
            .unwrap_or_default();
        let busy = self.preferences_ui.busy || self.preferences_ui.replacing || self.storage_busy;
        let mut view = form::section("选中项与未选中项")
            .child(muted("页面导航、设置分类、配置选项和笔记列表共用同一套交互样式。留空跟随当前明暗主题；支持 RGB / RGBA 自定义。"))
            .child(row().child(Button::new("selection-preview-on").label("选中项").selected(true))
                .child(Button::new("selection-preview-off").label("未选中项").selected(false)))
            .child(Checkbox::new("selection-indicator").label("导航显示选中标记").checked(style.indicator).disabled(busy)
                .on_click(cx.listener(|this,value,window,cx| {
                    if let Some(draft)=&mut this.preferences_ui.draft { draft.appearance.selection.indicator= *value; }
                    this.refresh_appearance(window,cx); cx.notify();
                })))
            .child(form::settings_row("选中项圆角",form::choice("selection-radius",style.radius,
                [0,4,8,12,16,24].into_iter().chain(std::iter::once(style.radius)).collect::<std::collections::BTreeSet<_>>().into_iter().map(|v|(v,format!("{v} px"))).collect(),busy,cx.weak_entity(),|this,value,window,cx| {
                    if let Some(draft)=&mut this.preferences_ui.draft { draft.appearance.selection.radius=value; }
                    this.refresh_appearance(window,cx); cx.notify();
                })));
        for (key, label, _) in FIELDS {
            if let Some(color) = self.preferences_ui.appearance_colors.get(key) {
                color.update(cx, |color, cx| {
                    color.configure(crate::theme::palette().accent, busy, cx)
                });
                view = view.child(form::field(label, color.clone()));
            }
        }
        view
    }
}
