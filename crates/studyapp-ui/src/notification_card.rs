//! One content surface for foreground styles, desktop cards and appearance previews.
use crate::{
    components::{column, row, scroll_body},
    controls::Button,
};
use gpui_kit::{
    component::{Icon, IconName, Sizable},
    prelude::FluentBuilder,
    *,
};
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
        NoticeSeverity::Information => IconName::Info,
        NoticeSeverity::Success => IconName::CircleCheck,
        NoticeSeverity::Warning => IconName::TriangleAlert,
        NoticeSeverity::Error | NoticeSeverity::Critical => IconName::CircleX,
    };
    let glyph = px((palette.font_size as f32 + 2.).clamp(16., 22.));
    let badge = glyph + px(12.);
    let tint = studyapp_core::RgbaColor::new(accent, 22).over(background);
    let compact = model.style == NoticeStyle::Bubble;
    let header = row()
        .min_w_0()
        .gap_2()
        .items_center()
        .child(
            div()
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .size(badge)
                .rounded(px(crate::design::CONTROL_RADIUS))
                .bg(rgb(tint))
                .child(
                    Icon::new(symbol)
                        .with_size(glyph)
                        .text_color(rgb(studyapp_core::readable_theme_color(accent, &[tint]))),
                ),
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
                    .flex_shrink_0()
                    .text_sm()
                    .text_color(rgb(secondary))
                    .child(format!("×{}", model.repetitions)),
            )
        })
        .child(close);
    let body = scroll_body(
        SharedString::from(format!("notification-body-{}", model.key)),
        scroll,
    )
    .test_support()
    .min_w_0()
    .max_h(px(164.))
    .text_color(rgb(foreground))
    .text_size(crate::design::type_size(
        palette.font_size,
        crate::design::TypeRole::Body,
    ))
    .line_height(px(
        palette.font_size as f32 * crate::design::line_height(crate::design::TypeRole::Body)
    ))
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
        card = card.child(
            div()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .font_family(crate::design::MONO_FONT)
                .text_xs()
                .text_color(rgb(secondary))
                .child(id),
        );
    }
    if let Some(action) = action {
        card = card.child(row().justify_end().child(action));
    }
    if model.style == NoticeStyle::Banner {
        card = card.w_full();
    }
    card
}

#[cfg(all(test, feature = "ui-test"))]
mod tests {
    use super::{NotificationCardModel, notification_card};
    use crate::controls::Button;
    use gpui_kit::{
        Context, InteractiveElement, IntoElement, Render, ScrollHandle, TestAppContext,
        TestSupportExt, Window,
        component::{Sizable, button::ButtonVariants},
        px, rgb,
        test::TestWindowExt,
    };
    use studyapp_core::{NoticeSeverity, NoticeStyle, NotificationAppearance};
    struct CardFixture {
        scroll: ScrollHandle,
    }
    impl Render for CardFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let appearance = NotificationAppearance {
                width: 240,
                ..Default::default()
            };
            let model = NotificationCardModel {
                key: "geometry".into(),
                title: "小组件提示".into(),
                summary: "桌面小组件已显示。长消息在卡片内滚动，关闭提示不会改变计时。\n"
                    .repeat(20)
                    .into(),
                severity: NoticeSeverity::Error,
                event_id: Some("EV-12345678-1234-1234-1234-123456789012".into()),
                repetitions: 2,
                style: NoticeStyle::CornerCard,
            };
            notification_card(
                model,
                &appearance,
                &self.scroll,
                Button::new("card-close").small().ghost().action("关闭通知"),
                Some(Button::new("card-action").small().label("查看详情")),
            )
            .id("card-fixture")
            .test_support()
        }
    }
    #[gpui_kit::test]
    fn narrow_cards_keep_neutral_borders_scroll_and_actions_at_large_type(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (_, visual) = cx.add_window_view(|_, _| CardFixture {
            scroll: ScrollHandle::new(),
        });
        for dark in [false, true] {
            for font_size in [12, 14, 22] {
                visual.update(|window, cx| {
                    crate::theme::apply(
                        &studyapp_core::AppearanceSettings {
                            mode: if dark {
                                studyapp_core::ThemeMode::Dark
                            } else {
                                studyapp_core::ThemeMode::Light
                            },
                            font_size,
                            ..Default::default()
                        },
                        dark,
                        cx,
                    );
                    window.render_frame(cx);
                    let card = window.find("card-fixture").bounds();
                    assert_eq!(card.size.width, px(240.));
                    let body = window.find("notification-body-geometry").bounds();
                    assert!(body.size.height <= px(164.) && body.size.height > px(0.));
                    for id in ["card-close", "card-action"] {
                        let control = window.find(id).bounds();
                        assert!(control.left() >= card.left() && control.right() <= card.right());
                        assert!(control.top() >= card.top() && control.bottom() <= card.bottom());
                    }
                    let border = window
                        .painted_quads()
                        .into_iter()
                        .find(|quad| {
                            (quad.bounds.size.width.0
                                - f32::from(card.size.width) * window.scale_factor())
                            .abs()
                                < 0.1
                                && quad.border_widths.top > gpui_kit::ScaledPixels(0.)
                        })
                        .expect("card border was not painted");
                    assert_eq!(
                        border.border_color,
                        rgb(crate::theme::palette().border).into()
                    );
                    assert_eq!(border.border_widths.left, border.border_widths.right);
                });
            }
        }
    }
}
