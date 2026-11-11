//! Minute-precision 24-hour input. Invalid intermediate text remains editable.
use crate::{components::*, controls::Button};
use gpui_kit::{
    component::{Disableable, Sizable, button::ButtonVariants, input::InputState},
    *,
};
pub fn parse_time(text: &str) -> Option<u16> {
    let bytes = text.trim().as_bytes();
    if bytes.len() != 5
        || bytes[2] != b':'
        || [0, 1, 3, 4].into_iter().any(|i| !bytes[i].is_ascii_digit())
    {
        return None;
    }
    let hour = u16::from(bytes[0] - b'0') * 10 + u16::from(bytes[1] - b'0');
    let minute = u16::from(bytes[3] - b'0') * 10 + u16::from(bytes[4] - b'0');
    (hour < 24 && minute < 60).then_some(hour * 60 + minute)
}
pub fn format_time(minute: u16) -> String {
    format!("{:02}:{:02}", minute / 60, minute % 60)
}
pub fn time_input(
    id: &'static str,
    source: &Entity<InputState>,
    default: u16,
    disabled: bool,
    cx: &App,
) -> Div {
    let input = source.clone();
    let invalid = parse_time(source.read(cx).value().as_str()).is_none();
    let mut controls = row()
        .flex_wrap()
        .gap_2()
        .child(crate::settings_components::input(source, disabled).w(gpui_kit::px(130.)))
        .child(muted("24 小时 · HH:MM"));
    for (suffix, label, delta) in [
        ("hour-down", "−时", -60),
        ("hour-up", "+时", 60),
        ("minute-down", "−分", -1),
        ("minute-up", "+分", 1),
    ] {
        let input = source.clone();
        controls = controls.child(
            Button::new(SharedString::from(format!("{id}-{suffix}")))
                .small()
                .ghost()
                .label(label)
                .disabled(disabled || invalid)
                .on_click(move |_, window, cx| {
                    if let Some(minute) = parse_time(input.read(cx).value().as_str()) {
                        input.update(cx, |input, cx| {
                            input.set_value(format_time(shift_minute(minute, delta)), window, cx);
                            cx.emit(gpui_kit::component::input::InputEvent::Change);
                        });
                    }
                }),
        );
    }
    column()
        .gap_1()
        .child(
            controls.child(
                Button::new(SharedString::from(format!("{id}-reset")))
                    .small()
                    .ghost()
                    .label("重置")
                    .disabled(disabled)
                    .on_click(move |_, window, cx| {
                        input.update(cx, |input, cx| {
                            input.set_value(format_time(default), window, cx);
                            cx.emit(gpui_kit::component::input::InputEvent::Change);
                        })
                    }),
            ),
        )
        .children(invalid.then(|| muted("请输入 00:00–23:59，精确到分钟")))
}
fn shift_minute(minute: u16, delta: i32) -> u16 {
    (i32::from(minute) + delta).rem_euclid(1440) as u16
}
#[cfg(test)]
mod tests {
    use super::{format_time, parse_time, shift_minute};
    #[test]
    fn hour_and_minute_adjustments_keep_minute_precision_across_midnight() {
        assert_eq!(shift_minute(0, -1), 1439);
        assert_eq!(shift_minute(1439, 1), 0);
        assert_eq!(shift_minute(23 * 60 + 17, 60), 17);
        assert_eq!(shift_minute(17, -60), 23 * 60 + 17);
    }
    #[test]
    fn validates_exact_minutes_without_wrapping_or_accepting_invalid_intermediate_values() {
        assert_eq!(parse_time("00:00"), Some(0));
        assert_eq!(parse_time("23:59"), Some(1439));
        for raw in ["24:00", "12:60", "3:00", "03:", "", "12:00:01", "１２:00"] {
            assert_eq!(parse_time(raw), None, "{raw}");
        }
        for minute in 0..1440 {
            assert_eq!(parse_time(&format_time(minute)), Some(minute));
        }
    }
}
