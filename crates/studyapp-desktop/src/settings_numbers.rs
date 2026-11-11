//! Numeric drafts retain invalid intermediate text until global Save validates it.
use crate::{app::StudyApp, components::*, settings_components as form};
use gpui_kit::{
    component::input::{InputEvent, InputState},
    *,
};
use std::{cell::Cell, rc::Rc};
type NumberSetter = Rc<dyn Fn(&mut StudyApp, u64)>;
pub(crate) struct NumberField {
    pub input: Entity<InputState>,
    applied: Rc<Cell<u64>>,
    min: u64,
    max: u64,
}
#[derive(IntoElement)]
struct NumberControl {
    owner: WeakEntity<StudyApp>,
    key: String,
    label: String,
    unit: String,
    value: u64,
    default: u64,
    min: u64,
    max: u64,
    disabled: bool,
    setter: NumberSetter,
}
impl RenderOnce for NumberControl {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        self.owner
            .update(cx, |this, cx| {
                let existing = this
                    .preferences_ui
                    .number_fields
                    .borrow()
                    .get(&self.key)
                    .map(|field| (field.input.clone(), field.applied.clone()));
                let (input, applied) = if let Some(existing) = existing {
                    existing
                } else {
                    let input = cx.new(|cx| {
                        InputState::new(window, cx).default_value(self.value.to_string())
                    });
                    let applied = Rc::new(Cell::new(self.value));
                    let saved = applied.clone();
                    let callback = self.setter.clone();
                    let (min, max) = (self.min, self.max);
                    cx.subscribe(&input, move |this, input, event: &InputEvent, cx| {
                        if matches!(event, InputEvent::Change) {
                            if let Ok(value) = input.read(cx).value().trim().parse::<u64>()
                                && (min..=max).contains(&value)
                            {
                                saved.set(value);
                                callback(this, value);
                            }
                            cx.notify();
                        }
                    })
                    .detach();
                    this.preferences_ui.number_fields.borrow_mut().insert(
                        self.key.clone(),
                        NumberField {
                            input: input.clone(),
                            applied: applied.clone(),
                            min,
                            max,
                        },
                    );
                    (input, applied)
                };
                if applied.get() != self.value {
                    applied.set(self.value);
                    input.update(cx, |input, cx| {
                        input.set_value(self.value.to_string(), window, cx)
                    });
                }
                form::settings_row(
                    self.label,
                    form::NumericSetting::new(
                        self.key,
                        &input,
                        self.unit,
                        self.default,
                        self.min,
                        self.max,
                        self.disabled,
                    ),
                )
            })
            .unwrap_or_else(|_| column())
    }
}
impl StudyApp {
    pub(crate) fn setting_number(
        &self,
        key: String,
        label: &str,
        unit: &str,
        options: (u64, u64, u64, u64, bool),
        set: impl Fn(&mut Self, u64) + 'static,
        cx: &mut Context<Self>,
    ) -> Div {
        let (value, default, min, max, disabled) = options;
        // A RenderOnce boundary supplies Window without an auto-height container
        // query, whose zero-height layout would conceal or overlap form rows.
        column().w_full().child(NumberControl {
            owner: cx.weak_entity(),
            key,
            label: label.into(),
            unit: unit.into(),
            value,
            default,
            min,
            max,
            disabled,
            setter: Rc::new(set),
        })
    }
    pub(crate) fn setting_numbers_invalid(&self, cx: &App) -> bool {
        self.preferences_ui
            .number_fields
            .borrow()
            .values()
            .any(|field| {
                field
                    .input
                    .read(cx)
                    .value()
                    .trim()
                    .parse::<u64>()
                    .map_or(true, |value| !(field.min..=field.max).contains(&value))
            })
    }
}
