//! Individual boolean setting, with a real switch and draft-only reset.
use gpui_kit::{
    component::{Disableable, Sizable, button::ButtonVariants, switch::Switch},
    *,
};
use std::rc::Rc;
type Change = Rc<dyn Fn(&bool, &mut Window, &mut App)>;
#[derive(IntoElement)]
pub struct SettingSwitch {
    id: ElementId,
    label: SharedString,
    value: bool,
    default: bool,
    disabled: bool,
    on_change: Option<Change>,
}
impl SettingSwitch {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            label: "".into(),
            value: false,
            default: false,
            disabled: false,
            on_change: None,
        }
    }
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = label.into();
        self
    }
    pub fn checked(mut self, value: bool) -> Self {
        self.value = value;
        self
    }
    pub fn default_value(mut self, value: bool) -> Self {
        self.default = value;
        self
    }
    pub fn disabled(mut self, value: bool) -> Self {
        self.disabled = value;
        self
    }
    pub fn on_click(mut self, callback: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(callback));
        self
    }
}
impl RenderOnce for SettingSwitch {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let change = self.on_change.clone();
        let reset = self.on_change;
        let default = self.default;
        crate::settings_components::settings_row(
            self.label.clone(),
            crate::components::row()
                .flex_wrap()
                .gap_2()
                .child(
                    Switch::new(self.id.clone())
                        .accessibility_label(self.label)
                        .checked(self.value)
                        .disabled(self.disabled)
                        .color(gpui_kit::rgb(crate::theme::palette().accent))
                        .on_click(move |value, window, cx| {
                            if let Some(callback) = &change {
                                callback(value, window, cx);
                            }
                        }),
                )
                .child(
                    crate::controls::Button::new((self.id, "reset"))
                        .small()
                        .ghost()
                        .label("恢复默认")
                        .disabled(self.disabled || self.value == default)
                        .on_click(move |_, window, cx| {
                            if let Some(callback) = &reset {
                                callback(&default, window, cx);
                            }
                        }),
                ),
        )
    }
}
