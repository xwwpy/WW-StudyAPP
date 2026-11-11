//! Structurally distinct feedback surfaces. Controllers retain every action;
//! these views never invent progress, confirmation effects or share targets.
use crate::{
    components::{column, row, scroll_body},
    controls::Button,
};
use gpui_kit::{
    component::{Icon, IconName, Sizable},
    prelude::FluentBuilder,
    *,
};
use studyapp_core::{NoticeLayout, NoticeSurface, RgbaColor};

pub(crate) struct SurfacePaint {
    pub background: RgbaColor,
    pub border: u32,
    pub foreground: u32,
    pub accent: u32,
    pub symbol: IconName,
    pub shadow: bool,
}
impl SurfacePaint {
    fn frame(&self, surface: &NoticeSurface) -> Div {
        column()
            .min_w_0()
            .w(px(f32::from(surface.width)))
            .max_w_full()
            .rounded(px(f32::from(surface.radius)))
            .bg(rgba(self.background.packed_rgba()))
            .border(px(f32::from(surface.border_width)))
            .border_color(rgb(self.border))
            .text_color(rgb(self.foreground))
            .when(self.shadow, |view| view.shadow_md())
    }
    fn icon(&self, size: f32) -> Icon {
        Icon::new(self.symbol.clone())
            .with_size(px(size))
            .text_color(rgb(self.accent))
    }
}

pub(crate) fn toast(
    message: SharedString,
    surface: &NoticeSurface,
    paint: SurfacePaint,
    icon: bool,
) -> Div {
    paint.frame(surface).rounded_full().px_4().py_2().child(
        row()
            .min_w_0()
            .gap_2()
            .children(icon.then(|| paint.icon(18.)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(message),
            ),
    )
}
pub(crate) fn snackbar(
    message: SharedString,
    surface: &NoticeSurface,
    paint: SurfacePaint,
    action: Option<Button>,
    close: Button,
    layout: NoticeLayout,
) -> Div {
    paint
        .frame(surface)
        .when(layout == NoticeLayout::Alternative, |view| {
            view.w_full().rounded_none()
        })
        .px_3()
        .py_2()
        .child(
            row()
                .min_w_0()
                .gap_2()
                .flex_wrap()
                .child(div().flex_1().min_w_0().child(message))
                .children(action)
                .child(close),
        )
}
pub(crate) fn banner(
    message: SharedString,
    surface: &NoticeSurface,
    paint: SurfacePaint,
    icon: bool,
    action: Option<Button>,
    close: Button,
) -> Div {
    paint
        .frame(surface)
        .w_full()
        .rounded_none()
        .px_4()
        .py_3()
        .child(
            row()
                .min_w_0()
                .gap_3()
                .flex_wrap()
                .children(icon.then(|| paint.icon(20.)))
                .child(div().flex_1().min_w_0().child(message))
                .children(action)
                .child(close),
        )
}
pub(crate) struct AlertContent {
    pub title: SharedString,
    pub risk: SharedString,
    pub confirm: Option<Button>,
    pub cancel: Button,
}
pub(crate) fn alert(
    content: AlertContent,
    surface: &NoticeSurface,
    paint: SurfacePaint,
    scroll: &ScrollHandle,
    layout: NoticeLayout,
) -> Div {
    let AlertContent {
        title,
        risk,
        confirm,
        cancel,
    } = content;
    let actions = row()
        .w_full()
        .gap_3()
        .justify_center()
        .child(cancel)
        .children(confirm);
    let details = column()
        .min_w_0()
        .gap_3()
        .child(div().text_lg().font_weight(FontWeight::BOLD).child(title))
        .child(
            scroll_body("alert-risk-scroll", scroll)
                .max_h(px(f32::from(surface.max_height)))
                .child(div().text_sm().child(risk)),
        );
    if layout == NoticeLayout::Alternative {
        paint
            .frame(surface)
            .p_5()
            .gap_4()
            .child(
                row()
                    .items_start()
                    .gap_4()
                    .child(paint.icon(40.))
                    .child(details.flex_1()),
            )
            .child(actions)
    } else {
        paint
            .frame(surface)
            .p_5()
            .gap_4()
            .items_center()
            .child(paint.icon(44.))
            .child(details.w_full().text_center())
            .child(actions)
    }
}

/// Scrollable operation list; the owning ModalFrame keeps cancellation in its
/// fixed footer. Destructive actions still open their separate confirmation.
pub fn action_sheet(actions: Vec<Button>, layout: NoticeLayout) -> Div {
    let list = if layout == NoticeLayout::Alternative {
        row().flex_wrap().gap_3()
    } else {
        column().gap_1()
    };
    column()
        .w_full()
        .gap_3()
        .p_4()
        .child(list.children(actions.into_iter().map(|button| {
            if layout == NoticeLayout::Alternative {
                button.min_w(px(120.))
            } else {
                button.w_full().justify_start()
            }
        })))
}

/// Actual export/share targets are supplied by the controller, never inferred
/// from a notification message. The preview is a separate visual region.
pub fn share_panel(
    preview: impl IntoElement,
    targets: impl IntoElement,
    layout: NoticeLayout,
    available: Size<Pixels>,
) -> Div {
    let wide = layout == NoticeLayout::Primary && available.width >= px(640.);
    let body = if wide {
        row().items_stretch()
    } else {
        column()
    };
    body.min_w_0()
        .w_full()
        .gap_4()
        .child(div().flex_1().min_w_0().child(preview))
        .child(div().flex_1().min_w_0().child(targets))
}

/// Passive progress only. A percentage exists only for a known nonzero total.
pub fn hud(
    label: impl Into<SharedString>,
    completed: Option<(u64, u64)>,
    layout: NoticeLayout,
) -> Div {
    hud_with_id("passive-hud", label, completed, layout)
}
pub fn hud_with_id(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    completed: Option<(u64, u64)>,
    layout: NoticeLayout,
) -> Div {
    use gpui_kit::component::{progress::Progress, spinner::Spinner};
    let id: ElementId = id.into();
    let scroll_id = (id.clone(), "hud-content");
    let label = label.into();
    let progress = completed.filter(|(done, total)| *total > 0 && done <= total);
    let indicator = if let Some((done, total)) = progress {
        column()
            .gap_1()
            .child(Progress::new(id).value(done as f32 / total as f32 * 100.))
            .child(crate::components::muted(format!(
                "{:.0}%",
                done as f64 / total as f64 * 100.
            )))
            .into_any_element()
    } else {
        Spinner::new().into_any_element()
    };
    let appearance = crate::theme::notification_appearance();
    let surface = appearance.surface(studyapp_core::NoticeStyle::Hud);
    let content = if layout == NoticeLayout::Alternative {
        column()
            .gap_3()
            .items_center()
            .p_4()
            .child(indicator)
            .child(div().text_center().child(label))
    } else {
        row()
            .gap_3()
            .p_3()
            .min_w_0()
            .child(div().flex_1().min_w_0().child(label))
            .child(div().min_w(px(80.)).child(indicator))
    };
    crate::interactive_surface::frame(studyapp_core::NoticeStyle::Hud, &appearance)
        .w(px(f32::from(surface.width)))
        .max_w_full()
        .max_h(px(f32::from(surface.max_height)))
        .child(
            div()
                .id(scroll_id)
                .min_h_0()
                .max_h(px(f32::from(surface.max_height)))
                .overflow_y_scroll()
                .child(content),
        )
}
