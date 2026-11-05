//! Bounded native HTML layout with one selection participant per document.
//! Every leaf contributes geometry; only the root projects the complete run set.
use crate::native_markdown_model::{
    BoxStyle, Dimension, Document, Font as RunFont, Kind, Node, Run,
};
use gpui_kit::{
    base::{TextSelectionHandle, TextSelectionRegistration, TextSelectionRun, TouchHandleLayout},
    *,
};
use std::{cell::RefCell, ops::Range, rc::Rc};
use studyapp_core::RgbaColor;

#[path = "native_html_border.rs"]
mod dotted_border;
#[path = "native_html_flow.rs"]
mod inline_flow;

type Ranges = Vec<Option<Range<usize>>>;
type SharedFrame = Rc<RefCell<Frame>>;
struct Frame {
    document: Rc<Document>,
    handle: TextSelectionHandle,
    geometry: Vec<Option<TextSelectionRun>>,
    ranges: Ranges,
    layout_ranges: Ranges,
    remaining_dots: usize,
}
impl Frame {
    fn projection(&self, cx: &mut App) -> Ranges {
        let runs: Vec<_> = self.geometry.iter().flatten().cloned().collect();
        let projected = self.handle.update_runs(&runs, cx);
        if self.handle.has_local_selection(cx) {
            return self
                .document
                .runs
                .iter()
                .map(|run| Some(0..run.text.len()))
                .collect();
        }
        let mut ranges = vec![None; self.document.runs.len()];
        for (run, range) in runs.iter().zip(projected.ranges()) {
            ranges[run.document_order() as usize] = range.clone();
        }
        ranges
    }
}

pub(crate) fn content(id: impl Into<ElementId>, source: &str) -> Option<AnyElement> {
    let palette = crate::theme::palette();
    if !palette.render_html {
        return None;
    }
    let document = cached_document(source, palette.font_size, palette.markdown_rgba[0])?;
    Some(
        NativeHtml {
            id: id.into(),
            document,
            inner: None,
            #[cfg(all(test, feature = "ui-test"))]
            capture: None,
        }
        .into_any_element(),
    )
}
// At most four source/model pairs (including negative results). Each source
// is <=64 KiB and each cached model <=4096 runs; the cache holds no UI handles.
fn cached_document(source: &str, size: u16, color: RgbaColor) -> Option<Rc<Document>> {
    use std::collections::VecDeque;
    struct Entry {
        source: String,
        size: u16,
        color: RgbaColor,
        document: Option<Rc<Document>>,
    }
    thread_local! {
        static CACHE: RefCell<VecDeque<Entry>> = const { RefCell::new(VecDeque::new()) };
    }
    if source.len() > 65_536 || !source.contains('<') {
        return None;
    }
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(i) = cache
            .iter()
            .position(|e| e.source == source && e.size == size && e.color == color)
        {
            let entry = cache.remove(i).expect("cached native document index");
            let result = entry.document.clone();
            cache.push_back(entry);
            return result;
        }
        let document = Document::from_markdown(source, f32::from(size), color).map(Rc::new);
        if document.as_ref().is_none_or(|d| d.runs.len() <= 4096) {
            if cache.len() >= 4 {
                cache.pop_front();
            }
            cache.push_back(Entry {
                source: source.into(),
                size,
                color,
                document: document.clone(),
            });
        }
        document
    })
}
struct NativeHtml {
    id: ElementId,
    document: Rc<Document>,
    inner: Option<AnyElement>,
    #[cfg(all(test, feature = "ui-test"))]
    capture: Option<Rc<RefCell<Option<SharedFrame>>>>,
}
impl IntoElement for NativeHtml {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for NativeHtml {
    type RequestLayoutState = SharedFrame;
    type PrepaintState = TouchHandleLayout;
    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, SharedFrame) {
        let document = self.document.clone();
        let frame = window.with_element_state(
            id.expect("native HTML has a stable id"),
            |old: Option<SharedFrame>, _| {
                let frame = old.unwrap_or_else(|| {
                    Rc::new(RefCell::new(Frame {
                        document: document.clone(),
                        handle: TextSelectionHandle::new(document.plain.clone(), cx),
                        geometry: vec![None; document.runs.len()],
                        ranges: Vec::new(),
                        layout_ranges: Vec::new(),
                        remaining_dots: 4096,
                    }))
                });
                {
                    let mut state = frame.borrow_mut();
                    if !Rc::ptr_eq(&state.document, &document) && *state.document != *document {
                        state.geometry = vec![None; document.runs.len()];
                    }
                    state.document = document.clone();
                    state
                        .handle
                        .set_fallback_copy_text(document.plain.clone(), cx);
                    state.layout_ranges = state.projection(cx);
                    state.ranges = state.layout_ranges.clone();
                }
                (frame.clone(), frame)
            },
        );
        #[cfg(all(test, feature = "ui-test"))]
        if let Some(capture) = &self.capture {
            *capture.borrow_mut() = Some(frame.clone());
        }
        let weak = Rc::downgrade(&frame);
        // Re-project on demand too: a right click may occur before the next
        // paint after a selection change. No strong handle/callback cycle.
        frame.borrow().handle.copy_with(
            move |cx| {
                weak.upgrade()
                    .map(|frame| {
                        let frame = frame.borrow();
                        frame.document.selected(&frame.projection(cx))
                    })
                    .unwrap_or_default()
            },
            cx,
        );
        let mut inner = div()
            .id("native-html-surface")
            .test_support()
            .aria_label(document.plain.clone())
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .child(node(&document.root, &frame))
            .into_any_element();
        let layout = inner.request_layout(window, cx);
        self.inner = Some(inner);
        (layout, frame)
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        frame: &mut SharedFrame,
        window: &mut Window,
        cx: &mut App,
    ) -> TouchHandleLayout {
        {
            let mut state = frame.borrow_mut();
            state.geometry.fill(None);
            state.remaining_dots = 4096;
        }
        self.inner.as_mut().unwrap().prepaint(window, cx);
        let state = frame.borrow();
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let text_bounds = state
            .geometry
            .iter()
            .flatten()
            .map(TextSelectionRun::bounds)
            .collect();
        state.handle.register(
            TextSelectionRegistration::new(hitbox, bounds)
                .with_text_bounds(text_bounds)
                .with_rendered_element(&state.handle, window, cx),
            window,
            cx,
        );
        state.handle.prepaint_touch_handles(window, cx)
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        frame: &mut SharedFrame,
        touch: &mut TouchHandleLayout,
        window: &mut Window,
        cx: &mut App,
    ) {
        {
            let mut state = frame.borrow_mut();
            state.ranges = state.projection(cx);
            // Glyph color highlights are measured with unchanged font metrics.
            // One more frame resolves a newly changed projection, then stops.
            if state.ranges != state.layout_ranges {
                window.refresh();
            }
        }
        self.inner.as_mut().unwrap().paint(window, cx);
        frame.borrow().handle.paint_touch_handles(
            touch,
            color(crate::theme::palette().markdown_rgba[24]),
            window,
            cx,
        );
    }
}
fn color(value: RgbaColor) -> Hsla {
    rgba(value.packed_rgba()).into()
}
struct Word {
    index: usize,
    run: Run,
    frame: SharedFrame,
    text: Option<StyledText>,
}
impl IntoElement for Word {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for Word {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let font = &self.run.font;
        let style = run_style(font, window);
        let selection = self
            .frame
            .borrow()
            .layout_ranges
            .get(self.index)
            .cloned()
            .flatten();
        let highlights = selection.into_iter().map(|range| {
            (
                range,
                HighlightStyle {
                    color: Some(color(crate::theme::palette().markdown_rgba[25])),
                    ..Default::default()
                },
            )
        });
        let mut text =
            StyledText::new(self.run.text.clone()).with_default_highlights(&style, highlights);
        let (layout, ()) = window.with_text_style(
            Some(TextStyleRefinement {
                color: Some(style.color),
                font_size: Some(style.font_size),
                line_height: Some(style.line_height),
                white_space: Some(WhiteSpace::Normal),
                ..Default::default()
            }),
            |window| text.request_layout(None, None, window, cx),
        );
        self.text = Some(text);
        (layout, ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let text = self.text.as_mut().unwrap();
        text.prepaint(None, None, bounds, &mut (), window, cx);
        if bounds.size.width > px(0.) && bounds.size.height > px(0.) {
            self.frame.borrow_mut().geometry[self.index] = Some(
                TextSelectionRun::new(self.run.text.clone(), text.layout().clone(), bounds)
                    .with_document_order(self.index as u64),
            );
        }
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        for background in &self.run.font.backgrounds {
            let mut background_bounds = bounds;
            background_bounds.size.width += px(self.run.font.tracking);
            window.paint_quad(fill(background_bounds, color(*background)));
        }
        let text = self.text.as_mut().unwrap();
        if let Some(range) = self.frame.borrow().ranges[self.index].clone() {
            paint_selection(
                text.layout(),
                range,
                color(crate::theme::palette().markdown_rgba[24]),
                window,
            );
        }
        text.paint(None, None, bounds, &mut (), &mut (), window, cx);
    }
}
fn run_style(font: &RunFont, window: &Window) -> TextStyle {
    let mut style = window.text_style().clone();
    style.color = color(font.color);
    style.background_color = None;
    style.font_size = px(font.size).into();
    style.line_height = px(font.line_height).into();
    style.font_weight = FontWeight(font.weight);
    style.font_style = if font.oblique {
        FontStyle::Oblique
    } else if font.italic {
        FontStyle::Italic
    } else {
        FontStyle::Normal
    };
    style.white_space = WhiteSpace::Normal;
    style.line_clamp = None;
    style.text_overflow = None;
    if let Some(family) = &font.family {
        style.font_family = match family.as_str() {
            "sans-serif" => ".SystemUIFont",
            "serif" if cfg!(target_os = "linux") => "serif",
            "serif" => "Times New Roman",
            "monospace" if cfg!(target_os = "macos") => "Menlo",
            "monospace" if cfg!(target_os = "linux") => "monospace",
            _ => crate::design::MONO_FONT,
        }
        .into();
    }
    style.underline = font.underline.then_some(UnderlineStyle {
        thickness: px(1.),
        color: None,
        wavy: false,
    });
    style.strikethrough = font.strike.then_some(StrikethroughStyle {
        thickness: px(1.),
        color: None,
    });
    style
}
fn node(value: &Node, frame: &SharedFrame) -> AnyElement {
    match value {
        Node::Run(index) => {
            let run = frame.borrow().document.runs[*index].clone();
            div()
                .id(("native-html-run", *index))
                .test_support()
                .min_w_0()
                .max_w_full()
                .flex_shrink_0()
                .pr(px(run.font.tracking))
                .child(Word {
                    index: *index,
                    run,
                    frame: frame.clone(),
                    text: None,
                })
                .into_any_element()
        }
        Node::Break(height) => div().w_full().h(px(*height)).into_any_element(),
        Node::Box {
            kind,
            style,
            children,
        } => {
            let mut container = div().flex().min_w_0().max_w_full().flex_shrink_0();
            if *kind == Kind::Row {
                container = container
                    .w_full()
                    .items_stretch()
                    .children(children.iter().map(|c| node(c, frame)));
            } else if *kind == Kind::Column {
                container = container
                    .w_full()
                    .flex_col()
                    .children(children.iter().map(|c| node(c, frame)));
            } else {
                container = container.flex_col();
                if *kind != Kind::Inline {
                    container = container.w_full();
                }
                if *kind == Kind::Cell {
                    container = container.flex_1();
                }
                let mut line = Vec::new();
                for child in children {
                    if let Node::Break(height) = child {
                        container = container.child(flow(
                            &line,
                            &style.align,
                            *height,
                            *kind != Kind::Inline || style.width.is_some(),
                            frame,
                        ));
                        line.clear();
                    } else {
                        line.push(child);
                    }
                }
                container = container.child(flow(
                    &line,
                    &style.align,
                    0.,
                    *kind != Kind::Inline || style.width.is_some(),
                    frame,
                ));
            }
            box_style(container, style, frame).into_any_element()
        }
    }
}
fn flow(
    children: &[&Node],
    align: &str,
    empty_height: f32,
    fill_width: bool,
    frame: &SharedFrame,
) -> AnyElement {
    if !children.is_empty() && children.iter().all(|child| matches!(child, Node::Run(_))) {
        let indices = children
            .iter()
            .filter_map(|child| match child {
                Node::Run(index) => Some(*index),
                _ => None,
            })
            .collect();
        return inline_flow::render(indices, align, fill_width, frame);
    }
    let mut row = div()
        .flex()
        .flex_wrap()
        .items_baseline()
        .min_w_0()
        .max_w_full()
        .flex_shrink_0();
    if fill_width {
        row = row.w_full();
    }
    row = match align {
        "center" => row.justify_center(),
        "right" => row.justify_end(),
        _ => row.justify_start(),
    };
    if children.is_empty() {
        row = row.h(px(empty_height));
    }
    row.children(children.iter().map(|n| node(n, frame)))
        .into_any_element()
}
fn length(value: Dimension) -> Length {
    match value {
        Dimension::Pixels(n) => px(n).into(),
        Dimension::Percent(n) => relative(n).into(),
    }
}
fn box_style(mut container: Div, bx: &BoxStyle, frame: &SharedFrame) -> Div {
    container = container
        .mt(px(bx.margin[0]))
        .mr(px(bx.margin[1]))
        .mb(px(bx.margin[2]))
        .ml(px(bx.margin[3]))
        .pt(px(bx.padding[0]))
        .pr(px(bx.padding[1]))
        .pb(px(bx.padding[2]))
        .pl(px(bx.padding[3]))
        .rounded(px(bx.radius));
    if let Some(value) = bx.background {
        container = container.bg(color(value));
    }
    if let Some(value) = bx.width {
        container = container.w(length(value));
    }
    if let Some(value) = bx.max_width {
        container = container.max_w(length(value));
    }
    if let Some(value) = bx.height {
        container = container.h(px(value)).overflow_hidden();
    }
    if bx.border > 0. {
        let style = container.style();
        style.border_widths.top = Some(px(bx.border).into());
        style.border_widths.right = Some(px(bx.border).into());
        style.border_widths.bottom = Some(px(bx.border).into());
        style.border_widths.left = Some(px(bx.border).into());
        let border_color = color(
            bx.border_color
                .unwrap_or_else(|| RgbaColor::opaque(crate::theme::palette().text)),
        );
        style.border_color = Some(if bx.border_style == "dotted" {
            rgba(0).into()
        } else {
            border_color
        });
        if bx.border_style == "dashed" {
            container = container.border_dashed();
        }
        if bx.border_style == "dotted" {
            let width = bx.border;
            let radius = bx.radius;
            let frame = frame.clone();
            container = container.relative().child(
                canvas(
                    |_, _, _| (),
                    move |bounds, (), window, _| {
                        let allowance = frame.borrow().remaining_dots;
                        let painted = dotted_border::paint(
                            bounds,
                            px(width),
                            px(radius),
                            border_color,
                            allowance,
                            window,
                        );
                        frame.borrow_mut().remaining_dots -= painted;
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            );
        }
    }
    container
}
fn paint_selection(
    layout: &TextLayout,
    range: Range<usize>,
    background: Hsla,
    window: &mut Window,
) {
    let (Some(start), Some(end)) = (
        layout.position_for_index(range.start),
        layout.position_for_index(range.end),
    ) else {
        return;
    };
    let height = layout.line_height();
    let bounds = layout.bounds();
    if start.y == end.y {
        window.paint_quad(fill(
            Bounds::from_corners(start, point(end.x, end.y + height)),
            background,
        ));
    } else {
        window.paint_quad(fill(
            Bounds::from_corners(start, point(bounds.right(), start.y + height)),
            background,
        ));
        if end.y > start.y + height {
            window.paint_quad(fill(
                Bounds::from_corners(
                    point(bounds.left(), start.y + height),
                    point(bounds.right(), end.y),
                ),
                background,
            ));
        }
        window.paint_quad(fill(
            Bounds::from_corners(point(bounds.left(), end.y), point(end.x, end.y + height)),
            background,
        ));
    }
}

#[cfg(all(test, feature = "ui-test"))]
#[path = "native_html_tests.rs"]
mod tests;
