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

/// Boolean overrides use the same switch as ordinary values without storing an
/// inherited value until the user explicitly chooses to customize it.
pub fn inherited_switch<T: 'static>(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    current: Option<bool>,
    effective: (bool, String),
    disabled: bool,
    owner: WeakEntity<T>,
    change: impl Fn(&mut T, Option<bool>, &mut Window, &mut Context<T>) + Clone + 'static,
) -> impl IntoElement {
    let id = id.into();
    let label = label.into();
    let mut controls = crate::components::row().flex_wrap().gap_2();
    if let Some(value) = current {
        let owner = owner.clone();
        let change = change.clone();
        controls = controls.child(
            Switch::new(id.clone())
                .accessibility_label(label.clone())
                .checked(value)
                .disabled(disabled)
                .color(gpui_kit::rgb(crate::theme::palette().accent))
                .on_click(move |value, window, cx| {
                    let _ = owner.update(cx, |this, cx| change(this, Some(*value), window, cx));
                }),
        );
    } else {
        controls = controls.child(crate::components::muted(effective.1));
    }
    let next = if current.is_some() {
        None
    } else {
        Some(effective.0)
    };
    controls = controls.child(
        crate::controls::Button::new((
            id,
            if current.is_some() {
                "inherit"
            } else {
                "customize"
            },
        ))
        .small()
        .ghost()
        .label(if current.is_some() {
            "恢复继承"
        } else {
            "自定义"
        })
        .disabled(disabled)
        .on_click(move |_, window, cx| {
            let _ = owner.update(cx, |this, cx| change(this, next, window, cx));
        }),
    );
    crate::settings_components::settings_row(label, controls)
}

#[cfg(all(test, feature = "ui-test"))]
mod tests {
    use super::inherited_switch;
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{
        Context, ElementId, IntoElement, ParentElement, Render, Styled, TestAppContext,
        VisualTestContext, Window, div, px,
    };

    struct InheritanceFixture {
        value: Option<bool>,
        parent: bool,
        disabled: bool,
        changes: usize,
    }
    impl Render for InheritanceFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().w(px(600.)).child(inherited_switch(
                "inherited-switch",
                "选中标记",
                self.value,
                (self.parent, format!("继承全局 · {}", self.parent)),
                self.disabled,
                cx.weak_entity(),
                |this, value, _, cx| {
                    this.value = value;
                    this.changes += 1;
                    cx.notify();
                },
            ))
        }
    }
    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
    #[gpui_kit::test]
    fn inherited_switch_materializes_only_on_input_and_restores_live_inheritance(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (view, visual) = cx.add_window_view(|_, _| InheritanceFixture {
            value: None,
            parent: false,
            disabled: false,
            changes: 0,
        });
        draw(visual);
        view.update(visual, |this, cx| {
            this.parent = true;
            cx.notify();
        });
        draw(visual);
        visual.update(|window, cx| {
            assert_eq!(view.read(cx).value, None);
            assert_eq!(view.read(cx).changes, 0);
            window.click((ElementId::from("inherited-switch"), "customize"), cx);
        });
        draw(visual);
        visual.update(|window, cx| {
            assert_eq!(view.read(cx).value, Some(true));
            window.click("inherited-switch", cx);
        });
        draw(visual);
        view.update(visual, |this, cx| {
            assert_eq!(this.value, Some(false));
            assert_eq!(this.changes, 2);
            this.disabled = true;
            cx.notify();
        });
        draw(visual);
        visual.update(|window, cx| {
            window.click((ElementId::from("inherited-switch"), "inherit"), cx);
            assert_eq!(view.read(cx).value, Some(false));
            assert_eq!(view.read(cx).changes, 2);
        });
        view.update(visual, |this, cx| {
            this.disabled = false;
            cx.notify();
        });
        draw(visual);
        visual.update(|window, cx| {
            window.click((ElementId::from("inherited-switch"), "inherit"), cx);
        });
        draw(visual);
        view.update(visual, |this, cx| {
            assert_eq!(this.value, None);
            assert_eq!(this.changes, 3);
            this.parent = false;
            cx.notify();
        });
        draw(visual);
        visual.update(|window, cx| {
            assert_eq!(view.read(cx).value, None);
            assert_eq!(view.read(cx).changes, 3);
            assert!(window.try_find("inherited-switch").is_none());
        });
    }
}
