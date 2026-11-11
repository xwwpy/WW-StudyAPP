//! Project-owned styling for real interactive surfaces and passive task HUDs.
use gpui_kit::{component::popover::Popover, *};
use studyapp_core::{NoticeLayout, NoticeStyle, NotificationAppearance, RgbaColor};

fn colors(style: NoticeStyle, appearance: &NotificationAppearance) -> (RgbaColor, RgbaColor, u32) {
    let surface = appearance.surface(style);
    let palette = crate::theme::palette();
    let raw = surface
        .background
        .as_deref()
        .and_then(studyapp_core::parse_rgba_color)
        .unwrap_or(RgbaColor::opaque(palette.surface_raised));
    let background = RgbaColor::new(
        raw.rgb(),
        (u16::from(raw.alpha()) * u16::from(surface.opacity) / 100) as u8,
    );
    let border = surface
        .border_color
        .as_deref()
        .and_then(studyapp_core::parse_rgba_color)
        .unwrap_or(RgbaColor::opaque(palette.border));
    let foreground =
        studyapp_core::readable_theme_color(palette.text, &[background.over(palette.surface)]);
    (background, border, foreground)
}

pub(crate) fn menu_border(appearance: &NotificationAppearance) -> Hsla {
    rgba(colors(NoticeStyle::PopupMenu, appearance).1.packed_rgba()).into()
}

pub(crate) fn menu_foreground(appearance: &NotificationAppearance) -> Hsla {
    rgb(colors(NoticeStyle::PopupMenu, appearance).2).into()
}

pub fn frame(style: NoticeStyle, appearance: &NotificationAppearance) -> Div {
    let surface = appearance.surface(style);
    let (background, border, foreground) = colors(style, appearance);
    div()
        .bg(rgba(background.packed_rgba()))
        .border(px(f32::from(surface.border_width)))
        .border_color(rgba(border.packed_rgba()))
        .rounded(px(f32::from(surface.radius)))
        .text_color(rgb(foreground))
}

pub fn popover(mut popover: Popover) -> Popover {
    let appearance = crate::theme::notification_appearance();
    let surface = appearance.surface(NoticeStyle::Anchored);
    let mut styled = frame(NoticeStyle::Anchored, &appearance);
    popover.style().refine(styled.style());
    popover
        .arrow(appearance.layout(NoticeStyle::Anchored) == NoticeLayout::Primary)
        .max_w(px(f32::from(surface.width)))
        .max_h(px(f32::from(surface.max_height)))
}
