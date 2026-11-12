//! Virtual-window regressions; no native windows, focus, or clipboard access.
use crate::{compact_markdown::CompactMarkdown, components::*, update_dialog::*};
use gpui_kit::{
    AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    ScrollDelta, ScrollHandle, StatefulInteractiveElement, Styled, TestAppContext, TestSupportExt,
    Window, component::input::TextareaState, div, point, px, size, test::TestWindowExt,
};

struct NestedPanes {
    page: ScrollHandle,
    notes: ScrollHandle,
    summary: Entity<CompactMarkdown>,
}
impl Render for NestedPanes {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        scroll_body("outer-page", &self.page)
            .test_support()
            .w(px(800.))
            .h(px(620.))
            .child(
                column()
                    .flex_shrink_0()
                    .child(self.summary.clone())
                    .child(
                        scroll_body("inner-notes", &self.notes)
                            .test_support()
                            .h(px(150.))
                            .child(div().h(px(800.)).flex_shrink_0().child("long notes")),
                    )
                    .child(
                        div()
                            .id("outside-panes")
                            .test_support()
                            .h(px(160.))
                            .flex_shrink_0(),
                    )
                    .child(div().h(px(900.)).flex_shrink_0()),
            )
    }
}

#[gpui_kit::test]
fn nested_summary_and_notes_wheels_do_not_move_the_page(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::apply(&Default::default(), false, cx);
    });
    let page = ScrollHandle::new();
    let notes = ScrollHandle::new();
    let (view, test_cx) = cx.add_window_view(|window, cx| {
        let input = cx.new(|cx| TextareaState::new(window, cx));
        input.update(cx, |input, cx| {
            input.set_value("long summary line\n\n".repeat(100), window, cx)
        });
        let summary = cx.new(|cx| CompactMarkdown::new(input, cx));
        NestedPanes {
            page: page.clone(),
            notes: notes.clone(),
            summary,
        }
    });
    test_cx.update(|window, cx| {
        window.render_frame(cx);
        assert!(page.max_offset().y > px(0.));
        assert!(notes.max_offset().y > px(0.));
        let preview = view.read(cx).summary.read(cx).preview_scroll_handle();
        assert!(preview.max_offset().y > px(150.));
        for target in [
            "summary-markdown-preview",
            "summary-markdown-source",
            "inner-notes",
        ] {
            for _ in 0..3 {
                window.scroll(target, ScrollDelta::Pixels(point(px(0.), px(-500.))), cx);
                window.render_frame(cx);
                assert_eq!(page.offset().y, px(0.), "wheel escaped from {target}");
            }
            window.scroll(target, ScrollDelta::Pixels(point(px(120.), px(0.))), cx);
            window.render_frame(cx);
            assert_eq!(
                page.offset().y,
                px(0.),
                "horizontal wheel remapped at {target}"
            );
        }
        assert!(notes.offset().y < px(0.));
        assert!(preview.offset().y < px(0.));
        // A wheel outside the independent panes still moves the main page.
        window.scroll(
            "outside-panes",
            ScrollDelta::Pixels(point(px(0.), px(-120.))),
            cx,
        );
        window.render_frame(cx);
        assert!(page.offset().y < px(0.));
        assert!(view.read(cx).summary.read(cx).is_preview());
    });
}

const ROW_BACKGROUND: u32 = 0x1259A3;

struct CoveredScrollbar {
    scroll: ScrollHandle,
    inset: f32,
    width: f32,
    height: f32,
    count: usize,
    presses: std::rc::Rc<std::cell::Cell<usize>>,
    clicks: std::rc::Rc<std::cell::Cell<usize>>,
}
impl Render for CoveredScrollbar {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().p(px(24.)).child(
            scroll_body("covered-scroll", &self.scroll)
                .test_support()
                .w(px(self.width))
                .h(px(self.height))
                .px(px(self.inset))
                .child(
                    column()
                        .gap_0()
                        .w_full()
                        .children((0..self.count).map(|index| {
                            let presses = self.presses.clone();
                            let clicks = self.clicks.clone();
                            div()
                                .id(("covered-row", index))
                                .test_support()
                                .w_full()
                                .h(px(32.))
                                .flex_shrink_0()
                                .bg(gpui_kit::rgb(ROW_BACKGROUND))
                                .on_mouse_down(gpui_kit::MouseButton::Left, move |_, _, _| {
                                    presses.set(presses.get() + 1);
                                })
                                .on_click(move |_, _, _| clicks.set(clicks.get() + 1))
                        })),
                ),
        )
    }
}

#[gpui_kit::test]
fn scrollbar_stays_above_full_width_content_and_owns_its_pointer_lane(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::apply(&Default::default(), false, cx);
    });
    for inset in [0., 4., 16.] {
        let scroll = ScrollHandle::new();
        let presses = std::rc::Rc::new(std::cell::Cell::new(0));
        let clicks = std::rc::Rc::new(std::cell::Cell::new(0));
        let (view, visual) = cx.add_window_view(|_, _| CoveredScrollbar {
            scroll: scroll.clone(),
            inset,
            width: 360.,
            height: 160.,
            count: 40,
            presses: presses.clone(),
            clicks: clicks.clone(),
        });
        for (width, height) in [(360., 160.), (280., 112.)] {
            view.update(visual, |view, cx| {
                view.width = width;
                view.height = height;
                cx.notify();
            });
            for bottom in [false, true] {
                scroll.set_offset(point(
                    px(0.),
                    if bottom {
                        -scroll.max_offset().y
                    } else {
                        px(0.)
                    },
                ));
                visual.update(|window, cx| {
                    window.render_frame(cx);
                    let viewport = window.find("covered-scroll").bounds();
                    assert_eq!(viewport, scroll.bounds());
                    assert_eq!(viewport.size, size(px(width), px(height)));
                    let viewport = window
                        .pixel_snap_bounds(viewport)
                        .scale(window.scale_factor());
                    let quads = window.painted_quads();
                    let rows: Vec<_> = quads
                        .iter()
                        .enumerate()
                        .filter(|(_, quad)| quad.background == gpui_kit::rgb(ROW_BACKGROUND).into())
                        .collect();
                    let thumbs: Vec<_> = quads
                        .iter()
                        .enumerate()
                        .filter(|(_, quad)| {
                            !quad.background.is_transparent()
                                && quad.background != gpui_kit::rgb(ROW_BACKGROUND).into()
                        })
                        .collect();
                    assert_eq!(thumbs.len(), 1);
                    let (thumb_index, thumb) = thumbs[0];
                    let visible = thumb.bounds.intersect(&thumb.content_mask.bounds);
                    assert!(
                        visible.size.width > px(0.).scale(window.scale_factor())
                            && visible.size.height > px(0.).scale(window.scale_factor())
                    );
                    assert!(
                        visible.top() >= viewport.top() && visible.bottom() <= viewport.bottom()
                    );
                    assert!(
                        visible.left() >= viewport.right() - px(16.).scale(window.scale_factor())
                            && visible.right() <= viewport.right()
                    );
                    assert!(
                        rows.iter().all(|(index, _)| *index < thumb_index),
                        "opaque content paints after the scrollbar (inset={inset})"
                    );
                });
            }
            scroll.set_offset(point(px(0.), px(0.)));
            visual.update(|window, cx| {
                window.render_frame(cx);
                let viewport = scroll.bounds();
                let x = viewport.right() - px(8.);
                window.drag(
                    point(x, viewport.top() + px(8.)),
                    point(x, viewport.bottom() - px(2.)),
                    cx,
                );
                assert_eq!(scroll.offset().y, -scroll.max_offset().y);
                assert_eq!(
                    presses.get(),
                    0,
                    "track press also reached a content control"
                );
                assert_eq!(clicks.get(), 0, "thumb drag activated a content control");
                scroll.set_offset(point(px(0.), px(0.)));
                window.render_frame(cx);
                window.click_at(
                    "covered-scroll",
                    point(px(width - 8.), px(height * 0.75)),
                    cx,
                );
                assert!(scroll.offset().y < px(0.), "track click did not scroll");
                assert_eq!(presses.get(), 0);
                assert_eq!(clicks.get(), 0);
            });
        }
        view.update(visual, |view, cx| {
            view.count = 2;
            cx.notify();
        });
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(scroll.max_offset().y, px(0.));
            assert!(
                window
                    .painted_quads()
                    .iter()
                    .all(|quad| quad.background.is_transparent()
                        || quad.background == gpui_kit::rgb(ROW_BACKGROUND).into())
            );
        });
    }
}

struct DialogFixture {
    scroll: ScrollHandle,
    notes: String,
}
impl Render for DialogFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().p(px(16.)).child(UpdateDialog::new(
            UpdateDialogContent::new(
                "0.2.1",
                "0.2.2",
                "",
                self.notes.clone(),
                UpdateDialogPhase::Ready,
            ),
            &self.scroll,
        ))
    }
}

#[gpui_kit::test]
fn update_markdown_has_a_bounded_scroll_viewport_and_stationary_actions(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::apply(&Default::default(), false, cx);
    });
    let scroll = ScrollHandle::new();
    let (view, test_cx) = cx.add_window_view(|_, _| DialogFixture {
        scroll: scroll.clone(),
        notes: "## Release\n\n- **Update** with `code`\n\n".repeat(60),
    });
    for height in [800., 620.] {
        test_cx.simulate_resize(size(px(600.), px(height)));
        test_cx.update(|window, cx| {
            window.render_frame(cx);
            let viewport = window.find("update-dialog-notes").bounds();
            let actions = window.find("update-dialog-install").bounds();
            assert!(viewport.size.height <= px(240.));
            assert!(
                scroll.max_offset().y > px(240.),
                "release Markdown must overflow its real viewport"
            );
            assert!(actions.top() >= viewport.bottom());
            assert!(
                actions.bottom() < px(height),
                "update action must stay visible"
            );
            window.scroll(
                "update-dialog-notes",
                ScrollDelta::Pixels(point(px(0.), px(-200.))),
                cx,
            );
            window.render_frame(cx);
            assert!(scroll.offset().y < px(0.));
            assert_eq!(window.find("update-dialog-install").bounds(), actions);
            scroll.set_offset(point(px(0.), -scroll.max_offset().y));
            window.render_frame(cx);
            assert_eq!(window.find("update-dialog-install").bounds(), actions);
        });
    }
    view.update(test_cx, |view, cx| {
        view.notes = "- Short update".into();
        cx.notify();
    });
    test_cx.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(scroll.max_offset().y, px(0.));
        assert_eq!(scroll.offset().y, px(0.));
    });
}
