use super::{Document, NativeHtml, SharedFrame, color, run_style};
use gpui_kit::{
    AppContext, Context, InteractiveElement, IntoElement, Modifiers, MouseButton, ParentElement,
    Render, ScrollHandle, StatefulInteractiveElement, Styled, TestAppContext, Window,
    base::{Root, TextSelection},
    div, point, px,
    test::TestWindowExt,
};
use std::{cell::RefCell, rc::Rc};
use studyapp_core::RgbaColor;
type Capture = Rc<RefCell<Option<SharedFrame>>>;
struct Fixture {
    source: String,
    width: f32,
    capture: Capture,
    scroll: Option<ScrollHandle>,
}
impl Render for Fixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let palette = crate::theme::palette();
        let document = Rc::new(
            Document::from_markdown_with_style(
                &self.source,
                f32::from(palette.font_size),
                palette.markdown_rgba[0],
                palette.render_html,
                palette.list_markers,
            )
            .unwrap(),
        );
        let mut body = div().id("native-fixture-body").w(px(self.width)).p_2();
        if let Some(scroll) = &self.scroll {
            body = body.h(px(120.)).overflow_y_scroll().track_scroll(scroll);
        }
        crate::markdown_view::copyable(
            "native-copy",
            body.child(NativeHtml {
                id: "native-test".into(),
                document,
                inner: None,
                capture: Some(self.capture.clone()),
            }),
        )
    }
}
fn init(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::apply(&Default::default(), false, cx);
    });
}
#[test]
fn native_html_cache_is_bounded_and_invalidated_by_font_and_color() {
    let source = "<b>cache</b>";
    let color = RgbaColor::opaque(0x123456);
    let first = super::cached_document(source, 16, color).unwrap();
    assert!(Rc::ptr_eq(
        &first,
        &super::cached_document(source, 16, color).unwrap()
    ));
    assert!(!Rc::ptr_eq(
        &first,
        &super::cached_document(source, 20, color).unwrap()
    ));
    assert!(!Rc::ptr_eq(
        &first,
        &super::cached_document(source, 16, RgbaColor::opaque(0xffffff)).unwrap()
    ));
    for i in 0..8 {
        super::cached_document(&format!("<b>{i}</b>"), 16, color);
    }
    assert!(!Rc::ptr_eq(
        &first,
        &super::cached_document(source, 16, color).unwrap()
    ));
    assert!(super::cached_document("plain Markdown", 16, color).is_none());
    assert!(
        super::cached_document(
            "```html
<b>code</b>
```",
            16,
            color
        )
        .is_none()
    );
    assert!(super::cached_document(&format!("<b>{}</b>", "x".repeat(65_536)), 16, color).is_none());
}
#[gpui_kit::test]
fn native_html_styles_change_actual_layout_and_paint_background_once(cx: &mut TestAppContext) {
    init(cx);
    let capture: Capture = Rc::default();
    let (_, visual) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| Fixture {
                source: "<div style='background-color:#EE110080;padding:8px;border-width:2px;border-style:solid;border-color:#008800'>small <span style='font-size:30px;line-height:2;color:#2200EE80;font-weight:700;text-decoration:underline'>BIG</span></div>".into(),
                width: 340., capture: capture.clone(), scroll: None,
            });
            Root::new(view, window, cx)
        });
    visual.update(|window, cx| {
        window.render_frame(cx);
        let state = capture.borrow().clone().unwrap();
        let state = state.borrow();
        let index = state
            .document
            .runs
            .iter()
            .position(|r| r.text == "BIG")
            .unwrap();
        let layout = state.geometry[index].as_ref().unwrap().layout();
        assert_eq!(layout.line_height(), px(60.));
        assert_eq!(
            layout
                .line_layout_for_index(0)
                .unwrap()
                .unwrapped_layout
                .font_size,
            px(30.)
        );
        assert_eq!(
            run_style(&state.document.runs[index].font, window).color,
            color(RgbaColor::new(0x2200ee, 128))
        );
        let small = state
            .document
            .runs
            .iter()
            .position(|r| r.text == "small")
            .unwrap();
        let baseline = |i: usize| {
            let geometry = state.geometry[i].as_ref().unwrap();
            let line = geometry.layout().line_layout_for_index(0).unwrap();
            geometry.bounds().top()
                + (geometry.layout().line_height() - line.ascent() - line.descent()) / 2.
                + line.ascent()
        };
        // Child origins snap to device pixels; subpixel font metrics do not.
        assert!(
            f32::from(baseline(small) - baseline(index)).abs() < 1. / window.scale_factor(),
            "mixed sizes must share the painted glyph baseline: small={:?}, big={:?}",
            baseline(small),
            baseline(index)
        );
        assert!(!window.painted_underlines().is_empty());
        let alpha = color(RgbaColor::new(0xee1100, 128));
        assert_eq!(
            window
                .painted_quads()
                .iter()
                .filter(|q| q.background.as_solid() == Some(alpha))
                .count(),
            1,
            "block alpha background must not also be painted under each word"
        );
    });
}
#[gpui_kit::test]
fn native_html_aggregates_all_runs_and_copies_visible_text_in_both_directions(
    cx: &mut TestAppContext,
) {
    init(cx);
    let capture: Capture = Rc::default();
    let (_, visual) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| Fixture { source: "First <span style='font-size:24px;color:red'>重点🙂</span> final\n\n<p>Second <i>end</i></p>".into(), width: 340., capture: capture.clone(), scroll: None });
            Root::new(view, window, cx)
        });
    let (start, end, plain) = visual.update(|window, cx| {
        window.render_frame(cx);
        let frame = capture.borrow().clone().unwrap();
        let state = frame.borrow();
        let first = state.geometry.iter().flatten().next().unwrap().bounds();
        let last = state.geometry.iter().flatten().last().unwrap().bounds();
        (
            point(first.left() + px(0.1), first.top() + px(1.)),
            point(last.right() - px(0.1), last.bottom() - px(1.)),
            state.document.plain.clone(),
        )
    });
    for (anchor, cursor) in [(start, end), (end, start)] {
        visual.update(TextSelection::clear);
        visual.simulate_mouse_down(anchor, MouseButton::Left, Modifiers::default());
        visual.update(|window, cx| window.render_frame(cx));
        visual.simulate_mouse_move(cursor, Some(MouseButton::Left), Modifiers::default());
        visual.simulate_mouse_up(cursor, MouseButton::Left, Modifiers::default());
        // Query immediately as well as after paint: right-click copy must
        // never return a previous frame or just the final styled leaf.
        assert_eq!(visual.update(TextSelection::selected_text), plain);
        visual.update(|window, cx| window.render_frame(cx));
        assert_eq!(visual.update(TextSelection::selected_text), plain);
    }
    visual.simulate_mouse_down(end, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(end, MouseButton::Right, Modifiers::default());
    visual.run_until_parked();
    visual.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.find("popup-menu").bounds().size.height > px(0.));
        window.press("down", cx);
        window.press("enter", cx);
    });
    visual.run_until_parked();
    visual.update(|_, cx| {
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some(plain.clone())
        )
    });
    visual.update(|window, cx| {
        window.render_frame(cx);
        let frame = capture.borrow().clone().unwrap();
        let state = frame.borrow();
        assert!(state.ranges.iter().flatten().count() > 3);
        assert!(!plain.contains("span") && !plain.contains("font-size"));
    });
}

#[gpui_kit::test]
fn native_html_scroll_retains_offscreen_selection_and_reprojects_changed_source(
    cx: &mut TestAppContext,
) {
    init(cx);
    let capture: Capture = Rc::default();
    let scroll = ScrollHandle::new();
    let mut fixture = None;
    let (_, visual) = cx.add_window_view(|window, cx| {
        let view = cx.new(|_| Fixture {
            source: (0..35)
                .map(|i| format!("<p>Row{i} <b>中文🙂</b></p>"))
                .collect(),
            width: 180.,
            capture: capture.clone(),
            scroll: Some(scroll.clone()),
        });
        fixture = Some(view.clone());
        Root::new(view, window, cx)
    });
    let fixture = fixture.unwrap();
    visual.update(|window, cx| {
        window.render_frame(cx);
        capture
            .borrow()
            .as_ref()
            .unwrap()
            .borrow()
            .handle
            .set_local_selection(true, cx);
        let plain = capture
            .borrow()
            .as_ref()
            .unwrap()
            .borrow()
            .document
            .plain
            .clone();
        assert!(TextSelection::selected_text(window, cx).ends_with("中文🙂"));
        assert_eq!(TextSelection::selected_text(window, cx), plain);
    });
    scroll.set_offset(point(px(0.), px(-250.)));
    visual.update(|window, cx| {
        for _ in 0..4 {
            window.render_frame(cx);
        }
        let frame = capture.borrow().clone().unwrap();
        let state = frame.borrow();
        assert!(
            state
                .geometry
                .iter()
                .flatten()
                .next()
                .unwrap()
                .bounds()
                .top()
                < px(0.)
        );
        assert_eq!(
            TextSelection::selected_text(window, cx),
            state.document.plain
        );
        assert_eq!(
            state.ranges, state.layout_ranges,
            "selection redraw must converge"
        );
    });
    for source in [
        "<span style='font-size:28px'>é🙂</span>",
        "<b>a</b>",
        "<span style='letter-spacing:2px'>é👩‍💻</span>",
    ] {
        fixture.update(visual, |view, cx| {
            view.source = source.into();
            cx.notify();
        });
        scroll.set_offset(point(px(0.), px(0.)));
        visual.update(|window, cx| {
            window.render_frame(cx);
            capture
                .borrow()
                .as_ref()
                .unwrap()
                .borrow()
                .handle
                .set_local_selection(true, cx);
            window.render_frame(cx);
            let frame = capture.borrow().clone().unwrap();
            let state = frame.borrow();
            assert_eq!(
                TextSelection::selected_text(window, cx),
                state.document.plain
            );
            assert_eq!(state.geometry.len(), state.document.runs.len());
        });
    }
}

#[gpui_kit::test]
fn native_html_dotted_border_paints_round_separated_dots(cx: &mut TestAppContext) {
    init(cx);
    let (_, visual) = cx.add_window_view(|window, cx| {
        let view = cx.new(|_| Fixture {
            source: "<div style='border-width:4px;border-style:dotted;border-color:#13579B;border-radius:12px;padding:8px'>dots</div>".into(),
            width: 180., capture: Rc::default(), scroll: None,
        });
        Root::new(view, window, cx)
    });
    visual.update(|window, cx| {
        window.render_frame(cx);
        let quads = window.painted_quads();
        let dots: Vec<_> = quads
            .iter()
            .filter(|q| q.background.as_solid() == Some(color(RgbaColor::opaque(0x13579b))))
            .collect();
        assert!(dots.len() > 12);
        assert!(
            dots.iter()
                .all(|q| q.bounds.size.width == q.bounds.size.height
                    && q.corner_radii.top_left * 2. == q.bounds.size.width)
        );
    });
}

#[gpui_kit::test]
fn native_html_switch_and_code_fences_keep_the_original_boundaries(cx: &mut TestAppContext) {
    init(cx);
    cx.update(|cx| {
        assert!(super::content("enabled", "<span style='color:red'>visible</span>").is_some());
        let appearance = studyapp_core::AppearanceSettings {
            render_html: false,
            ..Default::default()
        };
        crate::theme::apply(&appearance, false, cx);
        assert!(super::content("disabled", "<span>literal</span>").is_none());
        crate::theme::apply(&Default::default(), false, cx);
        let source = "<b>visible</b>

```html
<img src='file:///private' onerror='bad()'>
```";
        let document = Document::from_markdown(source, 16., RgbaColor::opaque(0x123456)).unwrap();
        assert!(
            document
                .plain
                .contains("<img src='file:///private' onerror='bad()'>")
        );
        assert!(
            document
                .runs
                .iter()
                .find(|r| r.text == "img")
                .unwrap()
                .font
                .family
                .as_deref()
                == Some("monospace")
        );
    });
}

#[gpui_kit::test]
fn native_html_dotted_paint_budget_is_document_wide(cx: &mut TestAppContext) {
    init(cx);
    let (_, visual) = cx.add_window_view(|window, cx| {
        let view = cx.new(|_| Fixture {
            source: "<div style='border-width:1px;border-style:dotted;border-color:#13579B;height:6px;width:100px'>x</div>".repeat(75),
            width: 180., capture: Rc::default(), scroll: None,
        });
        Root::new(view, window, cx)
    });
    visual.update(|window, cx| {
        window.render_frame(cx);
        let dots = window
            .painted_quads()
            .iter()
            .filter(|q| q.background.as_solid() == Some(color(RgbaColor::opaque(0x13579b))))
            .count();
        assert!(dots > 100 && dots <= 4096);
    });
}

#[gpui_kit::test]
fn native_html_framed_inline_span_does_not_expand_to_the_paragraph_width(cx: &mut TestAppContext) {
    init(cx);
    let capture: Capture = Rc::default();
    let (_, visual) = cx.add_window_view(|window, cx| {
        let view = cx.new(|_| Fixture {
            source: "left <span style='padding:4px;border-radius:8px;background-color:#13579B'>chip</span> right".into(),
            width: 280., capture: capture.clone(), scroll: None,
        });
        Root::new(view, window, cx)
    });
    visual.update(|window, cx| {
        window.render_frame(cx);
        let quads = window.painted_quads();
        let surface = quads
            .iter()
            .find(|q| q.background.as_solid() == Some(color(RgbaColor::opaque(0x13579b))))
            .unwrap();
        assert!(
            surface.bounds.size.width < gpui_kit::ScaledPixels(100. * window.scale_factor()),
            "inline background should fit its content, not the paragraph"
        );
        let frame = capture.borrow().clone().unwrap();
        let state = frame.borrow();
        let get = |word: &str| {
            let i = state
                .document
                .runs
                .iter()
                .position(|r| r.text == word)
                .unwrap();
            state.geometry[i].as_ref().unwrap().bounds()
        };
        assert!(get("right").left() > get("chip").right());
        assert!(f32::from(get("left").top() - get("chip").top()).abs() < 10.);
    });
}

#[gpui_kit::test]
fn native_list_marker_geometry_and_copy_survive_style_changes_with_html_disabled(
    cx: &mut TestAppContext,
) {
    init(cx);
    cx.update(|cx| {
        let settings = studyapp_core::AppearanceSettings {
            render_html: false,
            ..Default::default()
        };
        crate::theme::apply(&settings, false, cx);
    });
    let capture: Capture = Rc::default();
    let (_, visual) = cx.add_window_view(|window, cx| {
        let view = cx.new(|_| Fixture {
            source: "- parent 中文🙂\n  - child\n    - third\n      - fourth\n\n- [x] done".into(),
            width: 240.,
            capture: capture.clone(),
            scroll: None,
        });
        Root::new(view, window, cx)
    });
    let (start, end, plain) = visual.update(|window, cx| {
        window.render_frame(cx);
        let frame = capture.borrow().clone().unwrap();
        let state = frame.borrow();
        let xs: Vec<_> = state
            .document
            .runs
            .iter()
            .enumerate()
            .filter_map(|(i, r)| {
                r.marker
                    .map(|_| state.geometry[i].as_ref().unwrap().bounds().left())
            })
            .collect();
        assert_eq!(xs.len(), 4);
        assert!(
            xs.windows(2).all(|p| p[1] > p[0]),
            "nested markers require increasing gutters: {xs:?}"
        );
        for (i, run) in state.document.runs.iter().enumerate() {
            let geometry = state.geometry[i].as_ref().unwrap();
            assert_eq!(geometry.text().as_str(), run.text.as_str());
            assert!(geometry.bounds().size.height > px(0.));
        }
        let first = state.geometry.iter().flatten().next().unwrap().bounds();
        let last = state.geometry.iter().flatten().last().unwrap().bounds();
        (
            point(first.left() + px(0.1), first.top() + px(1.)),
            point(last.right() - px(0.1), last.bottom() - px(1.)),
            state.document.plain.clone(),
        )
    });
    for (anchor, cursor) in [(start, end), (end, start)] {
        visual.update(TextSelection::clear);
        visual.simulate_mouse_down(anchor, MouseButton::Left, Modifiers::default());
        visual.update(|window, cx| window.render_frame(cx));
        visual.simulate_mouse_move(cursor, Some(MouseButton::Left), Modifiers::default());
        visual.simulate_mouse_up(cursor, MouseButton::Left, Modifiers::default());
        assert_eq!(visual.update(TextSelection::selected_text), plain);
    }
    visual.update(|window, cx| {
        let mut settings = studyapp_core::AppearanceSettings {
            render_html: false,
            ..Default::default()
        };
        settings.list_style.levels = [Some(studyapp_core::ListMarker::Plus); 8];
        crate::theme::apply(&settings, false, cx);
        window.refresh();
        window.render_frame(cx);
        let frame = capture.borrow().clone().unwrap();
        let state = frame.borrow();
        assert_eq!(state.document.plain, plain);
        assert!(
            state
                .document
                .runs
                .iter()
                .filter_map(|r| r.marker)
                .all(|m| m == studyapp_core::ListMarker::Plus)
        );
        assert_eq!(TextSelection::selected_text(window, cx), plain);
    });
}

#[gpui_kit::test]
fn native_list_cache_invalidates_marker_styles_and_html_permission(cx: &mut TestAppContext) {
    init(cx);
    let source = "- parent <b>bold</b>\n  - child";
    let color = RgbaColor::opaque(0x202020);
    let first = super::cached_document(source, 16, color).unwrap();
    cx.update(|cx| {
        let mut settings = studyapp_core::AppearanceSettings {
            render_html: false,
            ..Default::default()
        };
        settings.list_style.levels[1] = Some(studyapp_core::ListMarker::Star);
        crate::theme::apply(&settings, false, cx);
    });
    let second = super::cached_document(source, 16, color).unwrap();
    assert!(!Rc::ptr_eq(&first, &second));
    assert!(second.plain.contains("<b>bold</b>"));
    assert_eq!(
        second.runs.iter().filter_map(|r| r.marker).nth(1),
        Some(studyapp_core::ListMarker::Star)
    );
}
