//! Shared dialog geometry; Base Dialog still owns blocking and keyboard scope.
use crate::{
    components::*,
    design::{TypeRole, space},
    theme,
};
use gpui_kit::{
    component::{Sizable, button::ButtonVariants},
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
        self.tone = tone;
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
        let palette = theme::palette();
        let scroll = self.scroll.unwrap_or_else(|| {
            window
                .use_keyed_state("modal-frame-scroll", cx, |_, _| ScrollHandle::new())
                .read(cx)
                .clone()
        });
        let (symbol, tone) = match self.tone {
            ModalTone::Neutral => ("i", Tone::Neutral),
            ModalTone::Warning => ("!", Tone::Warning),
            ModalTone::Danger => ("!", Tone::Danger),
        };
        let (foreground, soft) = tone.colors(palette);
        let padding = space(palette.spacing, 3.).min(px(32.));
        let body_height = (window.viewport_size().height * 0.60).min(px(520.));
        column()
            .id("modal-frame")
            .test_support()
            .min_w_0()
            .gap_0()
            .w(px(self.width.pixels()).min(window.viewport_size().width * 0.9))
            .max_h((window.viewport_size().height - px(48.)).max(px(120.)))
            .rounded(px(16.))
            .overflow_hidden()
            .bg(rgb(palette.surface_raised))
            .child(
                row()
                    .id("modal-header")
                    .test_support()
                    .items_start()
                    .flex_shrink_0()
                    .p(padding)
                    .gap_3()
                    .child(
                        div()
                            .size(px(36.))
                            .flex_shrink_0()
                            .rounded_full()
                            .bg(rgb(soft))
                            .text_color(rgb(foreground))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(symbol),
                    )
                    .child(
                        column()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(
                                typography(TypeRole::Title)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(self.title),
                            )
                            .children(self.description.map(muted)),
                    )
                    .child(div().size(px(28.)).flex_shrink_0().child(
                        gpui_kit::component::dialog::DialogClose::new().trigger(|button| {
                            button
                                .small()
                                .ghost()
                                .label("×")
                                .accessibility_label("关闭弹窗")
                                .tab_index(-1)
                        }),
                    )),
            )
            .child(
                scroll_body("modal-content-scroll", &scroll)
                    .test_support()
                    .min_h_0()
                    .max_h(body_height)
                    .p(padding)
                    .pt_0()
                    .gap(space(palette.spacing, 2.))
                    .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                    .child(self.body),
            )
            .children(self.footer.map(|footer| {
                row()
                    .id("modal-footer")
                    .test_support()
                    .flex_shrink_0()
                    .min_w_0()
                    .justify_end()
                    .px(padding)
                    .py(space(palette.spacing, 2.).min(px(24.)))
                    .border_t_1()
                    .border_color(rgb(palette.border))
                    .child(footer)
            }))
    }
}
