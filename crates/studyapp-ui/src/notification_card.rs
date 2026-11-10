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
    let surface = appearance.surface(model.style);
    let fields = &appearance.content;
    let required = model.severity >= NoticeSeverity::Error;
    let raw_background = surface
        .background
        .as_deref()
        .and_then(studyapp_core::parse_rgba_color)
        .unwrap_or(studyapp_core::RgbaColor::opaque(palette.surface));
    let alpha = (u16::from(raw_background.alpha()) * u16::from(surface.opacity) / 100) as u8;
    let painted_background = studyapp_core::RgbaColor::new(raw_background.rgb(), alpha);
    let background = painted_background.over(palette.surface);
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
        .map_or(semantic, |v| v.over(background));
    let foreground = studyapp_core::readable_theme_color(palette.text, &[background]);
    let secondary = studyapp_core::readable_theme_color(palette.muted, &[background]);
    let symbol = match model.severity {
        NoticeSeverity::Information => IconName::Info,
        NoticeSeverity::Success => IconName::CircleCheck,
        NoticeSeverity::Warning => IconName::TriangleAlert,
        NoticeSeverity::Error | NoticeSeverity::Critical => IconName::CircleX,
    };
    let glyph = px((palette.font_size as f32 + 2.).clamp(16., 22.));
    let compact = matches!(model.style, NoticeStyle::Bubble | NoticeStyle::Snackbar);
    let mut header = row().min_w_0().gap_2().items_center();
    if fields.icon {
        header = header.child(
            div()
                .flex_shrink_0()
                .child(Icon::new(symbol).with_size(glyph).text_color(rgb(accent))),
        );
    }
    if fields.title || required {
        header = header.child(
            div()
                .min_w_0()
                .flex_1()
                .font_weight(FontWeight::SEMIBOLD)
                .child(model.title),
        );
    } else {
        header = header.child(div().flex_1());
    }
    if fields.repetitions && model.repetitions > 1 {
        header = header.child(
            div()
                .text_sm()
                .text_color(rgb(secondary))
                .child(format!("×{}", model.repetitions)),
        );
    }
    header = header.child(close);
    let mut card = column()
        .min_w_0()
        .w(px(f32::from(surface.width)))
        .max_w_full()
        .gap(px(if compact { 2. } else { 8. }))
        .p(px(if surface.density == NoticeDensity::Compact {
            8.
        } else {
            12.
        }))
        .rounded(px(f32::from(surface.radius)))
        .border_1()
        .border_color(
            surface
                .border_color
                .as_deref()
                .and_then(studyapp_core::parse_rgba_color)
                .map_or(rgb(palette.border), |v| rgba(v.packed_rgba())),
        )
        .bg(rgba(painted_background.packed_rgba()))
        .text_color(rgb(foreground))
        .when(appearance.shadow, |c| c.shadow_sm())
        .child(header);
    card = card.border(px(f32::from(surface.border_width)));
    if matches!(
        model.style,
        NoticeStyle::ActivityView | NoticeStyle::ActionSheet | NoticeStyle::ModalCard
    ) {
        card = card.child(div().h(px(2.)).w_full().bg(rgb(accent).opacity(0.35)));
    }
    if fields.summary || required {
        let summary = if fields.detailed || required {
            model.summary.to_string()
        } else {
            model.summary.chars().take(120).collect::<String>()
                + if model.summary.chars().count() > 120 {
                    "…"
                } else {
                    ""
                }
        };
        card = card.child(
            scroll_body(
                SharedString::from(format!("notification-body-{}", model.key)),
                scroll,
            )
            .test_support()
            .min_w_0()
            .max_h(px(f32::from(surface.max_height)))
            .text_color(rgb(foreground))
            .text_size(crate::design::type_size(
                palette.font_size,
                crate::design::TypeRole::Body,
            ))
            .line_height(px(palette.font_size as f32
                * crate::design::line_height(crate::design::TypeRole::Body)))
            .child(summary)
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation()),
        );
    }
    if fields.event_id
        && let Some(id) = model.event_id
    {
        card = card.child(div().text_xs().text_color(rgb(secondary)).child(id));
    }
    if (fields.action || required)
        && let Some(action) = action
    {
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
