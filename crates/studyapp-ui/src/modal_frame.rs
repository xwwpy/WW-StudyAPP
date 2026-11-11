//! Shared dialog geometry; Base Dialog still owns blocking and keyboard scope.
use crate::{
    components::*,
    design::{TypeRole, space},
    theme,
};
use gpui_kit::{
    component::{Sizable, button::ButtonVariants},
    prelude::FluentBuilder,
    *,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ModalTone {
    #[default]
    Neutral,
    Warning,
    Danger,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ModalWidth {
    Compact,
    #[default]
    Regular,
    Wide,
}
impl ModalWidth {
    pub const fn pixels(self) -> f32 {
        match self {
            Self::Compact => 440.,
            Self::Regular => 520.,
            Self::Wide => 640.,
        }
    }
}

#[derive(IntoElement)]
pub struct ModalFrame {
    title: SharedString,
    description: Option<SharedString>,
    body: AnyElement,
    footer: Option<AnyElement>,
    tone: ModalTone,
    width: ModalWidth,
    scroll: Option<ScrollHandle>,
    component: studyapp_core::NoticeStyle,
}

impl ModalFrame {
    pub fn new(title: impl Into<SharedString>, body: impl IntoElement) -> Self {
        Self {
            title: title.into(),
            description: None,
            body: body.into_any_element(),
            footer: None,
            tone: ModalTone::Neutral,
            width: ModalWidth::Regular,
            scroll: None,
            component: studyapp_core::NoticeStyle::Dialog,
        }
    }
    pub fn description(mut self, text: impl Into<SharedString>) -> Self {
        self.description = Some(text.into());
        self
    }
    pub fn footer(mut self, actions: impl IntoElement) -> Self {
        self.footer = Some(actions.into_any_element());
        self
    }
    pub fn tone(mut self, tone: ModalTone) -> Self {
        self.component = if tone == ModalTone::Neutral {
            studyapp_core::NoticeStyle::Dialog
        } else {
            studyapp_core::NoticeStyle::ModalCard
        };
        self.tone = tone;
        self
    }
    pub fn component(mut self, component: studyapp_core::NoticeStyle) -> Self {
        self.component = component;
        self
    }
    pub fn width(mut self, width: ModalWidth) -> Self {
        self.width = width;
        self
    }
    pub fn scroll_handle(mut self, scroll: &ScrollHandle) -> Self {
        self.scroll = Some(scroll.clone());
        self
    }
}

impl RenderOnce for ModalFrame {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        use gpui_kit::component::{Icon, IconName};
        use studyapp_core::{NoticeLayout, NoticeStyle, RgbaColor};
        let palette = theme::palette();
        let appearance = theme::notification_appearance();
        let surface = appearance.surface(self.component);
        let alert = self.component == NoticeStyle::ModalCard;
        let centered_alert = alert && appearance.layout(self.component) == NoticeLayout::Primary;
        let near_full = self.component == NoticeStyle::Dialog
            && appearance.layout(self.component) == NoticeLayout::Alternative;
        let scroll = self.scroll.unwrap_or_else(|| {
            window
                .use_keyed_state("modal-frame-scroll", cx, |_, _| ScrollHandle::new())
                .read(cx)
                .clone()
        });
        let raw = surface
            .background
            .as_deref()
            .and_then(studyapp_core::parse_rgba_color)
            .unwrap_or(RgbaColor::opaque(palette.surface_raised));
        let background = RgbaColor::new(
            raw.rgb(),
            (u16::from(raw.alpha()) * u16::from(surface.opacity) / 100) as u8,
        );
        let edge = surface
            .border_color
            .as_deref()
            .and_then(studyapp_core::parse_rgba_color)
            .unwrap_or(RgbaColor::opaque(palette.border));
        let (_, tone) = match self.tone {
            ModalTone::Neutral => ("i", Tone::Neutral),
            ModalTone::Warning => ("!", Tone::Warning),
            ModalTone::Danger => ("!", Tone::Danger),
        };
        let (accent, _) = tone.colors(palette);
        let title = typography(TypeRole::Title)
            .font_weight(FontWeight::SEMIBOLD)
            .child(self.title);
        let icon = Icon::new(if self.tone == ModalTone::Danger {
            IconName::CircleX
        } else {
            IconName::TriangleAlert
        })
        .with_size(px(if centered_alert { 44. } else { 36. }))
        .text_color(rgb(accent));
        let heading = if centered_alert {
            column().items_center().gap_3().child(icon).child(title)
        } else {
            row()
                .items_center()
                .gap_3()
                .children(alert.then_some(icon))
                .child(title)
        };
        let header = column()
            .id("modal-header")
            .test_support()
            .relative()
            .flex_shrink_0()
            .gap_2()
            .p_5()
            .pr_10()
            .when(centered_alert, |view| view.text_center())
            .child(heading)
            .children(self.description.map(muted))
            .child(div().absolute().top(px(12.)).right(px(12.)).child(
                gpui_kit::component::dialog::DialogClose::new().trigger(|button| {
                    button
                        .small()
                        .ghost()
                        .icon(IconName::Close)
                        .tooltip("关闭弹窗")
                        .accessibility_label("关闭弹窗")
                        .tab_index(-1)
                }),
            ));
        let padding = space(palette.spacing, 3.).min(px(32.));
        let viewport = window.viewport_size();
        let custom_width = appearance
            .surface_patches
            .get(&self.component)
            .and_then(|patch| patch.width)
            .or_else(|| {
                appearance
                    .surfaces
                    .get(&self.component)
                    .map(|surface| surface.width)
            });
        let width = custom_width.map_or(self.width.pixels(), f32::from);
        let body_height = if near_full {
            (viewport.height - px(200.)).max(px(80.))
        } else {
            (viewport.height * 0.60).min(px(f32::from(surface.max_height).max(120.)))
        };
        column()
            .id("modal-frame")
            .test_support()
            .min_w_0()
            .gap_0()
            .w(if near_full {
                viewport.width * 0.94
            } else {
                px(width).min(viewport.width * 0.9)
            })
            .max_h((viewport.height - px(48.)).max(px(120.)))
            .rounded(px(f32::from(surface.radius)))
            .border(px(f32::from(surface.border_width)))
            .border_color(rgba(edge.packed_rgba()))
            .shadow_lg()
            .overflow_hidden()
            .bg(rgba(background.packed_rgba()))
            .text_color(rgb(studyapp_core::readable_theme_color(
                palette.text,
                &[background.over(palette.surface)],
            )))
            .child(header)
            .child(
                scroll_body("modal-content-scroll", &scroll)
                    .test_support()
                    .min_h_0()
                    .max_h(body_height)
                    .px(padding)
                    .pb(padding)
                    .when(centered_alert, |view| view.text_center())
                    .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                    .child(self.body),
            )
            .children(self.footer.map(|footer| {
                row()
                    .id("modal-footer")
                    .test_support()
                    .flex_shrink_0()
                    .min_w_0()
                    .flex_wrap()
                    .justify_end()
                    .when(centered_alert, |view| view.justify_center())
                    .px(padding)
                    .py(space(palette.spacing, 2.).min(px(24.)))
                    .border_t_1()
                    .border_color(rgb(palette.border))
                    .child(footer)
            }))
    }
}
