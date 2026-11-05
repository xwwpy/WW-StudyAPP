//! One content surface for foreground styles, desktop cards and appearance previews.
use crate::{
    components::{column, row, scroll_body},
    controls::Button,
};
use gpui_kit::{prelude::FluentBuilder, *};
use studyapp_core::{
    NoticeDensity, NoticeEvent, NoticeSeverity, NoticeStyle, NotificationAppearance,
};

#[derive(Clone)]
pub struct NotificationCardModel {
    pub key: SharedString,
    pub title: SharedString,
    pub summary: SharedString,
    pub severity: NoticeSeverity,
    pub event_id: Option<SharedString>,
    pub repetitions: u32,
    pub style: NoticeStyle,
}
impl NotificationCardModel {
    pub fn from_event(event: &NoticeEvent, repetitions: u32, style: NoticeStyle) -> Self {
        Self {
            key: event.id.to_string().into(),
            title: event.category.label().into(),
            summary: event.summary.clone().into(),
            severity: event.severity,
            event_id: Some(event.id.to_string().into()),
            repetitions,
            style,
        }
    }
}

/// Actions are owned by the calling controller; this component performs no IO.
pub fn notification_card(
    model: NotificationCardModel,
    appearance: &NotificationAppearance,
    scroll: &ScrollHandle,
    close: Button,
    action: Option<Button>,
) -> Div {
    let palette = crate::theme::palette();
    let background = appearance
        .background
        .as_deref()
        .and_then(studyapp_core::parse_rgba_color)
        .map_or(palette.surface, |value| value.over(palette.surface));
    let semantic = match model.severity {
        NoticeSeverity::Information => palette.accent,
        NoticeSeverity::Success => palette.success,
        NoticeSeverity::Warning => palette.warning,
        NoticeSeverity::Error | NoticeSeverity::Critical => palette.danger,
    };
    let accent = appearance
        .accent
        .as_deref()
        .and_then(studyapp_core::parse_rgba_color)
        .map_or(semantic, |value| value.over(background));
    let foreground = studyapp_core::readable_theme_color(palette.text, &[background]);
    let secondary = studyapp_core::readable_theme_color(palette.muted, &[background]);
    let symbol = match model.severity {
        NoticeSeverity::Information => "i",
        NoticeSeverity::Success => "✓",
        _ => "!",
    };
    let compact = model.style == NoticeStyle::Bubble;
    let header = row()
        .min_w_0()
        .gap_2()
        .items_center()
        .child(
            div()
                .flex_shrink_0()
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(studyapp_core::readable_theme_color(
                    accent,
                    &[background],
                )))
                .child(symbol),
        )
        .child(
            div()
                .min_w_0()
                .flex_1()
                .font_weight(FontWeight::SEMIBOLD)
                .child(model.title),
        )
        .when(model.repetitions > 1, |row| {
            row.child(
                div()
                    .text_color(rgb(secondary))
                    .child(format!("×{}", model.repetitions)),
            )
        })
        .child(close);
    let body = scroll_body(
        SharedString::from(format!("notification-body-{}", model.key)),
        scroll,
    )
    .min_w_0()
    .max_h(px(164.))
    .text_color(rgb(foreground))
    .line_height(px(22.))
    .child(model.summary)
    .on_scroll_wheel(|_, _, cx| cx.stop_propagation());
    let mut card = column()
        .min_w_0()
        .w(px(f32::from(appearance.width)))
        .max_w_full()
        .gap(px(if compact { 2. } else { 8. }))
        .p(px(if appearance.density == NoticeDensity::Compact {
            8.
        } else {
            12.
        }))
        .rounded(px(f32::from(appearance.radius)))
        .border_1()
        .border_color(rgb(palette.border))
        .border_l(px(3.))
        .border_color(rgb(accent))
        .bg(rgb(background))
        .text_color(rgb(foreground))
        .when(appearance.shadow, |card| card.shadow_sm())
        .child(header)
        .child(body);
    if !compact
        && let Some(id) = model
            .event_id
            .filter(|_| model.severity >= NoticeSeverity::Error)
    {
        card = card.child(div().text_xs().text_color(rgb(secondary)).child(id));
    }
    if let Some(action) = action {
        card = card.child(row().justify_end().child(action));
    }
    if model.style == NoticeStyle::Banner {
        card = card.w_full();
    }
    card
}
