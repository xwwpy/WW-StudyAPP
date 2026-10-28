//! App-drawn window chrome with native Windows non-client hit regions.
//!
//! Keep action groups outside the Drag region. Window management stays in the
//! platform: in particular Close dispatches WM_CLOSE and the app's existing
//! `on_window_should_close` persistence/unsaved-settings barrier.
use crate::components::{BORDER, INK, MUTED};
use gpui_kit::{
    component::{Icon, IconName, Sizable, tooltip::Tooltip},
    *,
};
use std::rc::Rc;

pub const TITLE_BAR_HEIGHT: f32 = crate::design::TITLE_BAR_HEIGHT;
const WINDOW_CONTROL_WIDTH: f32 = 44.;
type CloseRequest = Rc<dyn Fn(&mut Window, &mut App)>;

fn window_control(
    id: &'static str,
    label: &'static str,
    icon: IconName,
    area: WindowControlArea,
    active: bool,
    request_close: CloseRequest,
) -> impl IntoElement {
    let palette = crate::theme::palette();
    let close = matches!(area, WindowControlArea::Close);
    let hover_fill = if close {
        rgb(palette.danger).into()
    } else {
        crate::theme::selection_color()
    };
    let hover_text = if close {
        // The danger hover still needs readable icon contrast in custom themes.
        rgb(
            if studyapp_core::theme_contrast(palette.danger, 0xFFFFFF)
                >= studyapp_core::theme_contrast(palette.danger, 0x000000)
            {
                0xFFFFFF
            } else {
                0x000000
            },
        )
    } else {
        crate::theme::color(INK)
    };
    div()
        .id(id)
        .test_support()
        .role(accesskit::Role::Button)
        .aria_label(label)
        // Accessibility Invoke is a client action, unlike the native pointer
        // hit regions. Handle it explicitly without duplicating pointer clicks.
        .on_a11y_action(accesskit::Action::Click, move |_, window, cx| match area {
            WindowControlArea::Min => window.minimize_window(),
            WindowControlArea::Max => window.zoom_window(),
            WindowControlArea::Close => request_close(window, cx),
            WindowControlArea::Drag => {}
        })
        .tooltip(move |window, cx| Tooltip::new(label).build(window, cx))
        .window_control_area(area)
        .flex()
        .items_center()
        .justify_center()
        .w(px(WINDOW_CONTROL_WIDTH))
        .h(px(TITLE_BAR_HEIGHT))
        .flex_shrink_0()
        .cursor_pointer()
        .text_color(crate::theme::color(if active { INK } else { MUTED }))
        .hover(move |style| style.bg(hover_fill).text_color(hover_text))
        .active(move |style| style.bg(hover_fill).text_color(hover_text))
        .child(Icon::new(icon).small())
}

/// Compose application navigation/content actions with native window controls.
/// The caller supplies its existing buttons so keyboard actions and test IDs
/// remain unchanged. The title and remaining center space are the drag region.
pub fn title_bar(
    title: impl Into<SharedString>,
    leading: Vec<AnyElement>,
    actions: Vec<AnyElement>,
    window: &Window,
    request_close: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let request_close: CloseRequest = Rc::new(request_close);
    let active = window.is_window_active();
    let supported = window.window_controls();
    let mut controls = div()
        .id("window-controls")
        .test_support()
        .flex()
        .h_full()
        .flex_shrink_0();
    if supported.minimize {
        controls = controls.child(window_control(
            "window-minimize",
            "最小化",
            IconName::WindowMinimize,
            WindowControlArea::Min,
            active,
            request_close.clone(),
        ));
    }
    if supported.maximize {
        controls = controls.child(window_control(
            "window-maximize",
            if window.is_maximized() {
                "还原"
            } else {
                "最大化"
            },
            if window.is_maximized() {
                IconName::WindowRestore
            } else {
                IconName::WindowMaximize
            },
            WindowControlArea::Max,
            active,
            request_close.clone(),
        ));
    }
    controls = controls.child(window_control(
        "window-close",
        "关闭",
        IconName::WindowClose,
        WindowControlArea::Close,
        active,
        request_close,
    ));
    div()
        .id("window-titlebar")
        .test_support()
        .flex()
        .items_center()
        .h(px(TITLE_BAR_HEIGHT))
        .min_h(px(TITLE_BAR_HEIGHT))
        .w_full()
        .min_w_0()
        .flex_shrink_0()
        .border_b_1()
        .border_color(crate::theme::color(BORDER))
        .bg(crate::theme::color(0xFFFFFF))
        .child(
            div()
                .id("window-title-leading")
                .test_support()
                .flex()
                .items_center()
                .h_full()
                .flex_shrink_0()
                .pl_2()
                .gap_1()
                .children(leading),
        )
        .child(
            div()
                .id("window-title-drag")
                .test_support()
                .window_control_area(WindowControlArea::Drag)
                .flex()
                .items_center()
                .flex_1()
                .h_full()
                .min_w(px(96.))
                .px_3()
                .cursor_default()
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(crate::theme::color(if active { INK } else { MUTED }))
                        .child(title.into()),
                ),
        )
        .child(
            div()
                .id("window-title-actions")
                .test_support()
                .flex()
                .items_center()
                .h_full()
                .flex_shrink_0()
                .px_2()
                .gap_1()
                .children(actions),
        )
        .child(controls)
}
