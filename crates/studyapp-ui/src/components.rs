use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

/// Shared bounded scroll surface. The original ID remains the actual viewport,
/// so wheel containment, reveal and native clipping use the same rectangle.
pub fn scroll_body(id: impl Into<ElementId>, handle: &ScrollHandle) -> Stateful<Div> {
    column()
        .id(id)
        .relative()
        .min_h_0()
        .min_w_0()
        .overflow_y_scroll()
        .track_scroll(handle)
        .vertical_scrollbar(handle)
}

pub const INK: u32 = 0x233631;
pub const MUTED: u32 = 0x74827C;
pub const GREEN: u32 = 0x187B5B;
pub const SUCCESS: u32 = 0x247448;
pub const LIGHT: u32 = 0xEAF4ED;
pub const BORDER: u32 = 0xE3E9E3;
pub const BG: u32 = 0xF5F7F3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Success,
    Warning,
    Danger,
    Accent,
}

impl Tone {
    /// Foreground and background, including for intentionally low-contrast bases.
    pub fn colors(self, palette: crate::theme::ResolvedTheme) -> (u32, u32) {
        let background = match self {
            Self::Neutral => palette.inset,
            Self::Success => palette.success_soft,
            Self::Warning => palette.warning_soft,
            Self::Danger => palette.danger_soft,
            Self::Accent => palette.accent_soft,
        };
        (pill_foreground(palette, background), background)
    }
}

fn pill_foreground(palette: crate::theme::ResolvedTheme, background: u32) -> u32 {
    if studyapp_core::theme_contrast(palette.text, background) >= 4.5 {
        palette.text
    } else if studyapp_core::theme_contrast(palette.accent_text, background) >= 4.5 {
        palette.accent_text
    } else {
        studyapp_core::readable_theme_color(palette.text, &[background])
    }
}

pub fn typography(role: crate::design::TypeRole) -> Div {
    let size = crate::design::type_size(crate::theme::palette().font_size, role);
    div()
        .text_size(size)
        .line_height(size * crate::design::line_height(role))
}

/// Shared information bubble, measured and centered on the raw pointer.
pub fn sidebar_tooltip(text: String, window: &mut Window, cx: &mut App) -> AnyView {
    pointer_tooltip(text, window, cx)
}

pub fn pointer_tooltip(text: String, window: &mut Window, cx: &mut App) -> AnyView {
    let position = window.mouse_position();
    let registry = crate::bubble::window_bubble_registry(window, cx);
    cx.new(|_| crate::bubble::PointerBubble {
        text,
        position,
        registry,
    })
    .into()
}

pub const LIBRARY_ROW_HEIGHT: f32 = 32.;

pub fn library_entry() -> Div {
    row()
        .min_h(px(LIBRARY_ROW_HEIGHT))
        .flex_shrink_0()
        .min_w_0()
        .gap_2()
        .px_2()
        .rounded(px(crate::design::ROW_RADIUS))
        .text_size(crate::design::type_size(
            crate::theme::palette().font_size,
            crate::design::TypeRole::Body,
        ))
}

pub fn library_interaction(row: Stateful<Div>, selected: bool, disabled: bool) -> Stateful<Div> {
    row.role(accesskit::Role::ListBoxOption)
        .aria_selected(selected)
        .tab_index(0)
        .tab_stop(!disabled)
        .border_1()
        .border_color(if selected {
            rgb(crate::theme::palette().accent).opacity(0.35)
        } else {
            rgba(0)
        })
        .when(selected, |row| row.bg(crate::theme::selection_color()))
        .when(disabled, |row| row.opacity(0.5).cursor_default())
        .when(!disabled, |row| {
            row.cursor_pointer()
                .hover(|style| {
                    style
                        .bg(crate::theme::hover_color())
                        .border_color(rgb(crate::theme::palette().border))
                })
                .active(|style| {
                    style
                        .bg(crate::theme::pressed_color())
                        .border_color(rgb(crate::theme::palette().muted))
                })
                .focus_visible(|style| style.border_color(rgb(crate::theme::palette().accent)))
        })
}

/// Secondary library metadata stays visually separate from study-change badges.
pub fn library_tag(label: &'static str, warning: bool) -> Div {
    div()
        .flex_shrink_0()
        .px_1()
        .rounded_sm()
        .text_xs()
        .bg(crate::theme::color(BG))
        .text_color(crate::theme::color(if warning { 0x9B5719 } else { MUTED }))
        .child(label)
}

/// A compact drag representation sharing the selectable row's height.
pub fn library_drag_card(
    id: &'static str,
    title: String,
    icon: impl IntoElement,
) -> impl IntoElement {
    row()
        .id(id)
        .test_support()
        .w(px(220.))
        .h(px(LIBRARY_ROW_HEIGHT))
        .min_w_0()
        .px_2()
        .gap_2()
        .rounded_md()
        .border_1()
        .border_color(crate::theme::color(BORDER))
        .bg(crate::theme::color(0xFFFFFF))
        .text_color(crate::theme::color(INK))
        .text_sm()
        .shadow_sm()
        .child(icon)
        .child(div().flex_1().min_w_0().truncate().child(title))
}

pub fn column() -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(crate::theme::palette().spacing as f32))
}
pub fn row() -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(crate::theme::palette().spacing as f32))
}
pub fn card() -> Div {
    column()
        .flex_shrink_0()
        .p(crate::design::space(crate::theme::palette().spacing, 2.))
        .rounded(px(crate::design::CARD_RADIUS))
        .bg(crate::theme::color(0xFFFFFF))
        .border_1()
        .border_color(crate::theme::color(BORDER))
}

/// Full-width metadata/action row for virtualized detail lists.
pub fn detail_list_row() -> Div {
    column()
        .w_full()
        .min_w_0()
        .flex_shrink_0()
        .gap_2()
        .px_2()
        .pr_4()
        .py_3()
        .border_b_1()
        .border_color(crate::theme::color(BORDER))
}

/// A document is a continuous reading surface, not a stack of form cards.
pub fn document_surface() -> Div {
    column()
        .flex_1()
        .min_h_0()
        .h_full()
        .min_w_0()
        .gap_0()
        .bg(crate::theme::color(0xFFFFFF))
}

pub fn document_toolbar() -> Div {
    row()
        .flex_shrink_0()
        .min_w_0()
        .min_h(px(48.))
        .px_3()
        .py_2()
        .gap_2()
        .border_b_1()
        .border_color(crate::theme::color(BORDER))
}
pub fn muted(text: impl Into<SharedString>) -> Div {
    typography(crate::design::TypeRole::Caption)
        .text_color(crate::theme::color(MUTED))
        .child(text.into())
}
pub fn heading(text: impl Into<SharedString>) -> Div {
    typography(crate::design::TypeRole::Section)
        .font_weight(FontWeight::SEMIBOLD)
        .child(text.into())
}

/// A compact selection marker shared by navigation surfaces; the supplied
/// control retains ownership of focus, hit testing, and action identifiers.
pub fn navigation_item(control: impl IntoElement, selected: bool) -> Div {
    div()
        .relative()
        .w_full()
        .min_w_0()
        .child(control)
        .children(selected.then(|| {
            div()
                .absolute()
                .left_0()
                .top(px(9.))
                .bottom(px(9.))
                .w(px(2.))
                .rounded_full()
                .bg(rgb(crate::theme::palette().accent))
        }))
}
pub fn pill(text: impl Into<SharedString>, color: u32) -> Div {
    let background = crate::theme::resolve_color(color);
    typography(crate::design::TypeRole::Caption)
        .px_3()
        .py_1()
        .rounded_full()
        .bg(rgb(background))
        .text_color(rgb(pill_foreground(crate::theme::palette(), background)))
        .child(text.into())
}

pub fn pill_tone(text: impl Into<SharedString>, tone: Tone) -> Div {
    let (foreground, background) = tone.colors(crate::theme::palette());
    typography(crate::design::TypeRole::Caption)
        .px_3()
        .py_1()
        .rounded_full()
        .bg(rgb(background))
        .text_color(rgb(foreground))
        .child(text.into())
}

/// A shared title hierarchy; callers keep ownership of page actions and IDs.
pub fn page_header(
    eyebrow: impl Into<SharedString>,
    title: impl Into<SharedString>,
    subtitle: Option<SharedString>,
    actions: impl IntoElement,
) -> Div {
    let spacing = crate::theme::palette().spacing;
    row()
        .w_full()
        .min_w_0()
        .items_start()
        .gap(crate::design::space(spacing, 3.))
        .mb(crate::design::space(spacing, 3.))
        .child(
            column()
                .flex_1()
                .min_w_0()
                .gap(crate::design::space(spacing, 0.5))
                .child(muted(eyebrow))
                .child(
                    typography(crate::design::TypeRole::Title)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title.into()),
                )
                .when_some(subtitle, |this, subtitle| {
                    this.child(
                        typography(crate::design::TypeRole::Body)
                            .text_color(rgb(crate::theme::palette().muted))
                            .child(subtitle),
                    )
                }),
        )
        .child(row().flex_shrink_0().child(actions))
}

#[derive(IntoElement)]
pub struct PageHeader {
    eyebrow: SharedString,
    title: SharedString,
    subtitle: Option<SharedString>,
    actions: Option<AnyElement>,
}
impl PageHeader {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            eyebrow: "".into(),
            title: title.into(),
            subtitle: None,
            actions: None,
        }
    }
    pub fn eyebrow(mut self, text: impl Into<SharedString>) -> Self {
        self.eyebrow = text.into();
        self
    }
    pub fn subtitle(mut self, text: impl Into<SharedString>) -> Self {
        self.subtitle = Some(text.into());
        self
    }
    pub fn actions(mut self, actions: impl IntoElement) -> Self {
        self.actions = Some(actions.into_any_element());
        self
    }
}
impl RenderOnce for PageHeader {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        page_header(
            self.eyebrow,
            self.title,
            self.subtitle,
            div().children(self.actions),
        )
    }
}

/// Static by design, so reduced motion never produces a looping shimmer.
pub fn skeleton(label: impl Into<SharedString>) -> Div {
    let palette = crate::theme::palette();
    column()
        .min_w_0()
        .child(muted(label))
        .children((0..3).map(|index| {
            card()
                .w_full()
                .gap_2()
                .child(
                    div()
                        .w(relative(0.45 + index as f32 * 0.1))
                        .h(px(14.))
                        .rounded_sm()
                        .bg(rgb(palette.border)),
                )
                .child(
                    div()
                        .w(relative(0.8))
                        .h(px(10.))
                        .rounded_sm()
                        .bg(rgb(palette.inset)),
                )
        }))
}
pub fn empty(title: &str, detail: &str) -> Div {
    card()
        .items_center()
        .justify_center()
        .py_12()
        .child(
            div()
                .size(px(36.))
                .rounded_full()
                .bg(rgb(crate::theme::palette().inset))
                .flex()
                .items_center()
                .justify_center()
                .child("○"),
        )
        .child(heading(title.to_owned()))
        .child(muted(detail.to_owned()))
}
pub fn metric(label: &str, value: String, detail: &str) -> Div {
    card()
        .flex_1()
        .child(muted(label.to_owned()))
        .child(
            typography(crate::design::TypeRole::Display)
                .font_family(crate::design::MONO_FONT)
                .font_features(FontFeatures(std::sync::Arc::new(vec![("tnum".into(), 1)])))
                .font_weight(FontWeight::SEMIBOLD)
                .child(value),
        )
        .child(muted(detail.to_owned()))
}

#[cfg(all(test, feature = "ui-test"))]
mod tests {
    use super::{card, column, scroll_body};
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{
        AppContext, Context, InteractiveElement, IntoElement, MouseMoveEvent, ParentElement,
        Render, ScrollHandle, StatefulInteractiveElement, Styled, TestAppContext, TestSupportExt,
        Window, div, point, px,
    };

    struct ScrollGeometryFixture {
        handle: ScrollHandle,
        horizontal: bool,
        content: f32,
    }

    impl Render for ScrollGeometryFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            use gpui_kit::component::scroll::ScrollableElement;
            let viewport = if self.horizontal {
                super::row()
                    .id("geometry-viewport")
                    .relative()
                    .track_scroll(&self.handle)
                    .horizontal_scrollbar(&self.handle)
                    .overflow_x_scroll()
                    .items_end()
                    .w(px(360.))
                    .h(px(160.))
                    .pb(px(12.))
                    .child(div().w(px(self.content)).h(px(80.)).flex_shrink_0())
            } else {
                scroll_body("geometry-viewport", &self.handle)
                    .w(px(360.))
                    .h(px(160.))
                    .p(px(16.))
                    .child(div().w(px(80.)).h(px(self.content)).flex_shrink_0())
            };
            div().w(px(500.)).h(px(300.)).p(px(32.)).child(viewport)
        }
    }

    #[gpui_kit::test]
    fn fitting_scroll_surfaces_do_not_paint_scrollbars(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::apply(&Default::default(), false, cx);
        });
        for (horizontal, content) in [
            (false, 0.),
            (false, 80.),
            (false, 128.),
            (true, 0.),
            (true, 80.),
            (true, 360.),
        ] {
            let handle = ScrollHandle::new();
            let (_, test_cx) = cx.add_window_view(|_, _| ScrollGeometryFixture {
                handle: handle.clone(),
                horizontal,
                content,
            });
            test_cx.update(|window, cx| window.render_frame(cx));
            let painted = test_cx.update(|window, _| {
                window
                    .painted_quads()
                    .into_iter()
                    .filter(|quad| !quad.background.is_transparent())
                    .map(|quad| quad.bounds)
                    .collect::<Vec<_>>()
            });
            assert_eq!(handle.max_offset(), point(px(0.), px(0.)));
            assert!(
                painted.is_empty(),
                "fitting content paints scrollbar geometry: {painted:?}"
            );
        }
    }

    #[gpui_kit::test]
    fn overflowing_scrollbars_stay_at_the_viewport_and_disappear_when_content_fits(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::apply(&Default::default(), false, cx);
        });
        for horizontal in [false, true] {
            let handle = ScrollHandle::new();
            let (view, test_cx) = cx.add_window_view(|_, _| ScrollGeometryFixture {
                handle: handle.clone(),
                horizontal,
                content: 900.,
            });
            test_cx.update(|window, cx| window.render_frame(cx));
            let expected = if horizontal {
                point(px(540.), px(0.))
            } else {
                point(px(0.), px(772.))
            };
            assert_eq!(
                handle.max_offset(),
                expected,
                "scroll decoration changes the content extent"
            );
            let mut positions = Vec::new();
            for offset in [point(px(0.), px(0.)), -expected] {
                handle.set_offset(offset);
                test_cx.update(|window, cx| window.render_frame(cx));
                test_cx.update(|window, _| {
                    let viewport = window
                        .pixel_snap_bounds(handle.bounds())
                        .scale(window.scale_factor());
                    let thumbs: Vec<_> = window
                        .painted_quads()
                        .into_iter()
                        .filter(|quad| !quad.background.is_transparent())
                        .collect();
                    assert_eq!(
                        thumbs.len(),
                        1,
                        "exactly one scrollbar thumb should be visible"
                    );
                    let thumb = thumbs[0].bounds;
                    assert!(
                        thumb.left() >= viewport.left()
                            && thumb.right() <= viewport.right()
                            && thumb.top() >= viewport.top()
                            && thumb.bottom() <= viewport.bottom(),
                        "scrollbar moved with its contents: thumb={thumb:?}, viewport={viewport:?}"
                    );
                    positions.push(if horizontal {
                        thumb.center().x
                    } else {
                        thumb.center().y
                    });
                });
            }
            assert!(
                positions[1] > positions[0],
                "thumb does not move toward the end of the track"
            );
            handle.set_offset(point(px(0.), px(0.)));
            test_cx.update(|window, cx| {
                window.render_frame(cx);
                let bounds = handle.bounds();
                let (start, end) = if horizontal {
                    (
                        point(bounds.left() + px(8.), bounds.bottom() - px(8.)),
                        point(bounds.right() - px(4.), bounds.bottom() - px(8.)),
                    )
                } else {
                    (
                        point(bounds.right() - px(8.), bounds.top() + px(8.)),
                        point(bounds.right() - px(8.), bounds.bottom() - px(4.)),
                    )
                };
                window.drag(start, end, cx);
            });
            assert_eq!(
                handle.offset(),
                -expected,
                "painted thumb and drag hit testing disagree"
            );
            view.update(test_cx, |view, cx| {
                view.content = 80.;
                cx.notify();
            });
            test_cx.update(|window, cx| window.render_frame(cx));
            assert_eq!(handle.max_offset(), point(px(0.), px(0.)));
            assert_eq!(handle.offset(), point(px(0.), px(0.)));
            test_cx.update(|window, _| {
                assert!(
                    window
                        .painted_quads()
                        .iter()
                        .all(|quad| quad.background.is_transparent())
                )
            });
        }
    }

    #[gpui_kit::test]
    fn detail_rows_constrain_long_titles_and_align_trailing_actions(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        struct DetailList;
        impl Render for DetailList {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let state = gpui_kit::ListState::new(2, gpui_kit::ListAlignment::Top, px(0.));
                div().w(px(360.)).h(px(240.)).child(
                    gpui_kit::list(state, |ix, _, _| {
                        super::detail_list_row()
                            .id(("detail-row", ix))
                            .test_support()
                            .child(
                                super::row()
                                    .child(div().flex_1().min_w_0().truncate().child(if ix == 0 {
                                        "长标题".repeat(100)
                                    } else {
                                        "短标题".into()
                                    }))
                                    .child(
                                        div()
                                            .id(("detail-action", ix))
                                            .test_support()
                                            .w(px(72.))
                                            .flex_shrink_0()
                                            .child("操作"),
                                    ),
                            )
                            .into_any_element()
                    })
                    .size_full(),
                )
            }
        }
        let (_, cx) = cx.add_window_view(|_, _| DetailList);
        cx.update(|window, cx| window.render_frame(cx));
        cx.update(|window, _| {
            for ix in 0_usize..2 {
                let row = window.find(("detail-row", ix)).bounds();
                let action = window.find(("detail-action", ix)).bounds();
                assert_eq!(
                    row.size.width,
                    px(360.),
                    "row width depends on title length"
                );
                assert!(
                    action.right() <= row.right() - px(16.),
                    "action overlaps the list scrollbar gutter"
                );
            }
        });
    }

    struct ScrollCards {
        scroll: ScrollHandle,
    }

    impl Render for ScrollCards {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            column()
                .w(px(400.))
                .h(px(280.))
                .gap_0()
                .child(
                    div()
                        .id("fixed-header")
                        .test_support()
                        .h(px(32.))
                        .flex_shrink_0(),
                )
                .child(
                    scroll_body("cards-viewport", &self.scroll)
                        .test_support()
                        .flex_1()
                        .gap_2()
                        .children((0_usize..5).map(|i| {
                            card()
                                .id(("whole-card", i))
                                .test_support()
                                .child(div().h(px(48.)).flex_shrink_0())
                                .child(div().h(px(48.)).flex_shrink_0())
                        })),
                )
                .child(
                    div()
                        .id("fixed-footer")
                        .test_support()
                        .h(px(32.))
                        .flex_shrink_0(),
                )
        }
    }

    #[gpui_kit::test]
    fn shared_scroll_keeps_cards_whole_and_footer_outside_viewport(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let scroll = ScrollHandle::new();
        let (_, cx) = cx.add_window_view(|_, _| ScrollCards {
            scroll: scroll.clone(),
        });
        cx.update(|window, cx| window.render_frame(cx));
        let (header, footer, card_height) = cx.update(|window, _| {
            let header = window.find("fixed-header").bounds();
            let footer = window.find("fixed-footer").bounds();
            let viewport = window.find("cards-viewport").bounds();
            let first = window.find(("whole-card", 0_usize)).bounds();
            assert!(
                first.size.height >= px(128.),
                "card contents were flex-compressed"
            );
            assert_eq!(viewport.top(), header.bottom());
            assert_eq!(viewport.bottom(), footer.top());
            (header, footer, first.size.height)
        });
        assert!(scroll.max_offset().y > px(400.));
        scroll.set_offset(point(px(0.), -scroll.max_offset().y));
        cx.update(|window, cx| window.render_frame(cx));
        cx.update(|window, _| {
            assert_eq!(window.find("fixed-header").bounds(), header);
            assert_eq!(window.find("fixed-footer").bounds(), footer);
            let last = window.find(("whole-card", 4_usize)).bounds();
            assert_eq!(last.size.height, card_height);
            assert!(
                last.bottom() <= footer.top() + px(1.),
                "last card cannot be fully revealed"
            );
        });
    }

    #[gpui_kit::test]
    fn pointer_tooltip_follows_pointer_after_it_is_open(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        struct TooltipSurface(gpui_kit::Entity<crate::bubble::PointerBubble>);
        impl Render for TooltipSurface {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().size_full().relative().child(self.0.clone())
            }
        }
        let (surface, cx) = cx.add_window_view(|window, cx| {
            let registry = crate::bubble::window_bubble_registry(window, cx);
            TooltipSurface(cx.new(|_| crate::bubble::PointerBubble {
                registry,
                text: "最近学习 / 上次复习 / 下次复习".into(),
                position: point(px(320.), px(120.)),
            }))
        });
        let tooltip = cx.update(|_, cx| surface.read(cx).0.clone());
        cx.update(|window, cx| window.render_frame(cx));
        for position in [point(px(420.), px(160.)), point(px(450.), px(190.))] {
            cx.simulate_event(MouseMoveEvent {
                position,
                ..Default::default()
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                window.render_frame(cx);
                assert_eq!(tooltip.read(cx).position, position);
                let bounds = window.find("sidebar-tooltip-content").bounds();
                assert_eq!(
                    bounds.center().x,
                    position.x,
                    "bounds {bounds:?} shapes {:?}",
                    tooltip.read(cx).registry.read(cx).shapes()
                );
                assert_eq!(bounds.top(), position.y + px(14.));
            });
        }
    }
}
