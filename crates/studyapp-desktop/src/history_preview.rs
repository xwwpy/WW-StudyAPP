use crate::components::*;
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::*;
use std::{cell::RefCell, rc::Rc, sync::Arc};
use studyapp_core::NoteVersion;

type DiffRows = Arc<Vec<(String, char)>>;

#[derive(Default)]
struct MeasuredRows {
    source: String,
    width: Option<Pixels>,
    rows: DiffRows,
    starts: Vec<usize>,
}

#[derive(Default)]
pub struct HistoryPreview {
    version: String,
    measured: Rc<RefCell<MeasuredRows>>,
    pub scroll: UniformListScrollHandle,
}

impl HistoryPreview {
    pub fn prepare(&mut self, version: Option<&NoteVersion>) {
        let key = version.map(|v| v.id.as_str()).unwrap_or_default();
        if self.version == key {
            return;
        }
        self.version = key.to_owned();
        *self.measured.borrow_mut() = MeasuredRows {
            source: version
                .map(|v| v.diff.replace('\t', "    "))
                .unwrap_or_default(),
            ..Default::default()
        };
        self.scroll = UniformListScrollHandle::default();
    }

    pub fn rows(&self) -> DiffRows {
        self.measured.borrow().rows.clone()
    }

    pub fn render(&self) -> Div {
        let measured = self.measured.clone();
        let scroll = self.scroll.clone();
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .bg(crate::theme::color(BG))
            .rounded_lg()
            .child(
                canvas(
                    move |bounds, window, cx| {
                        // Prepare after parent layout: even the first paint uses the real pane.
                        let rows = measured
                            .borrow_mut()
                            .reflow(bounds.size.width, &scroll, window);
                        let mut list =
                            uniform_list("history-diff-scroll", rows.len(), move |range, _, _| {
                                range
                                    .map(|i| {
                                        let (text, marker) = &rows[i];
                                        let color = match marker {
                                            '+' => GREEN,
                                            '-' => 0xB45142,
                                            _ => MUTED,
                                        };
                                        div()
                                            .id(("diff-line", i))
                                            .test_support()
                                            .h(px(22.))
                                            .px(px(8.))
                                            .font_family("Consolas")
                                            .text_size(px(12.))
                                            .line_height(px(22.))
                                            .text_color(crate::theme::color(color))
                                            .whitespace_nowrap()
                                            .child(text.clone())
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .with_sizing_behavior(ListSizingBehavior::Auto)
                            .track_scroll(&scroll)
                            .size_full()
                            .test_support()
                            .into_any_element();
                        list.layout_as_root(bounds.size.map(AvailableSpace::Definite), window, cx);
                        list.prepaint_at(bounds.origin, window, cx);
                        list
                    },
                    |_, mut list, window, cx| list.paint(window, cx),
                )
                .absolute()
                .inset_0(),
            )
            .vertical_scrollbar(&self.scroll)
    }
}

impl MeasuredRows {
    fn reflow(
        &mut self,
        pane_width: Pixels,
        scroll: &UniformListScrollHandle,
        window: &Window,
    ) -> DiffRows {
        // Match explicit row padding (16px) and the overlay scrollbar (8px).
        let width = (pane_width - px(24.)).floor().max(px(1.));
        if self.width == Some(width) {
            return self.rows.clone();
        }
        let offset = scroll.0.borrow().base_handle.offset().y;
        let old_row = (f32::from(-offset).max(0.) / 22.) as usize;
        let anchor = self.starts.get(old_row).copied().unwrap_or(0);
        let mut rows = Vec::new();
        let mut starts = Vec::new();
        let mut source_offset = 0;
        for line in self.source.lines() {
            let marker = line.chars().next().unwrap_or(' ');
            let shaped = window.text_system().shape_line(
                line.to_owned().into(),
                px(12.),
                &[TextRun {
                    len: line.len(),
                    font: font("Consolas"),
                    color: black(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }],
                None,
            );
            let mut boundaries: Vec<_> = shaped
                .runs
                .iter()
                .flat_map(|run| run.glyphs.iter())
                .map(|glyph| (glyph.index, glyph.position.x))
                .collect();
            boundaries.push((line.len(), shaped.width));
            boundaries.sort_by_key(|entry| entry.0);
            boundaries.dedup_by_key(|entry| entry.0);
            let (mut start, mut origin) = (0, px(0.));
            let mut previous = (0, px(0.));
            for boundary in boundaries {
                if boundary.1 - origin > width && previous.0 > start {
                    rows.push((line[start..previous.0].to_owned(), marker));
                    starts.push(source_offset + start);
                    start = previous.0;
                    origin = previous.1;
                }
                previous = boundary;
            }
            rows.push((line[start..].to_owned(), marker));
            starts.push(source_offset + start);
            source_offset += line.len() + 1;
        }
        if self.width.is_some() && !starts.is_empty() {
            let row = starts
                .partition_point(|&start| start <= anchor)
                .saturating_sub(1);
            let fraction = f32::from(-offset).max(0.) % 22.;
            scroll
                .0
                .borrow()
                .base_handle
                .set_offset(point(px(0.), px(-(row as f32 * 22. + fraction))));
        }
        self.width = Some(width);
        self.rows = Arc::new(rows);
        self.starts = starts;
        self.rows.clone()
    }
}

#[cfg(all(test, feature = "ui-test"))]
mod tests {
    use super::HistoryPreview;
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{
        Context, IntoElement, ParentElement, Render, Styled, TestAppContext,
        UniformListScrollHandle, Window, div, point, px,
    };
    use std::sync::Arc;

    #[gpui_kit::test]
    fn history_preview_wraps_against_measured_pane(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::apply(&Default::default(), false, cx);
        });
        let version = studyapp_core::NoteVersion {
            id: "width-fixture".into(),
            note_id: "note".into(),
            content: String::new(),
            hash: String::new(),
            captured_at: 0,
            offline: false,
            summary: String::new(),
            diff: format!(
                "+studyapp-media:{}\n+{}\t\n{}",
                "abcdef0123456789".repeat(80),
                "中文🧠a".repeat(150),
                "+tail\n".repeat(1600)
            ),
        };
        let (view, test_cx) = cx.add_window_view(|_, _| {
            let mut preview = HistoryPreview::default();
            preview.prepare(Some(&version));
            PreviewFixture {
                preview,
                width: 770.,
                height: 160.,
            }
        });
        for width in [770., 360., 915., 625., 770.] {
            test_cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.width = width;
                    cx.notify();
                });
                window.render_frame(cx);
                let preview = &view.read(cx).preview;
                let rows = preview.rows();
                assert!(rows.len() > 1500);
                assert_eq!(
                    rows.iter().map(|r| r.0.as_str()).collect::<String>(),
                    version.diff.replace('\t', "    ").replace('\n', "")
                );
                for (text, _) in rows.iter() {
                    let shaped = window.text_system().shape_line(
                        text.clone().into(),
                        px(12.),
                        &[gpui_kit::TextRun {
                            len: text.len(),
                            font: gpui_kit::font("Consolas"),
                            color: gpui_kit::black(),
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        }],
                        None,
                    );
                    assert!(
                        shaped.width <= px(width - 24. + 1.),
                        "row exceeds pane: {:?}, pane {width}",
                        shaped.width
                    );
                }
                let first = &rows[0].0;
                let shaped = window.text_system().shape_line(
                    first.clone().into(),
                    px(12.),
                    &[gpui_kit::TextRun {
                        len: first.len(),
                        font: gpui_kit::font("Consolas"),
                        color: gpui_kit::black(),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    }],
                    None,
                );
                assert!(
                    shaped.width > px(width - 40.),
                    "pane wastes horizontal space: {:?}, pane {width}",
                    shaped.width
                );
                window.render_frame(cx);
                assert!(
                    Arc::ptr_eq(&rows, &view.read(cx).preview.rows()),
                    "unchanged layout must reuse wrapped rows"
                );
                let mounted = (0..rows.len())
                    .filter(|&ix| window.try_find(("diff-line", ix)).is_some())
                    .count();
                assert!(mounted <= 10, "diff must remain virtualized");
            });
        }
        test_cx.update(|window, cx| {
            let scroll = view.read(cx).preview.scroll.clone();
            scroll
                .0
                .borrow()
                .base_handle
                .set_offset(point(px(0.), px(-220.)));
            window.render_frame(cx);
            let anchor = view.read(cx).preview.measured.borrow().starts[10];
            view.update(cx, |view, cx| {
                view.width = 360.;
                cx.notify();
            });
            window.render_frame(cx);
            let offset = scroll.0.borrow().base_handle.offset().y;
            let current = (f32::from(-offset) / 22.) as usize;
            let measured = view.read(cx).preview.measured.borrow();
            assert!(measured.starts[current] <= anchor);
            assert!(
                measured
                    .starts
                    .get(current + 1)
                    .is_none_or(|&next| next > anchor)
            );
            drop(measured);
            let last = view.read(cx).preview.rows().len() - 1;
            scroll.scroll_to_item(last, gpui_kit::ScrollStrategy::Bottom);
            window.render_frame(cx);
            assert!(
                window.try_find(("diff-line", last)).is_some(),
                "last row remains reachable"
            );
            let mut next = version.clone();
            next.id = "another-version".into();
            view.update(cx, |view, cx| {
                view.preview.prepare(Some(&next));
                cx.notify();
            });
            window.render_frame(cx);
            assert_eq!(
                view.read(cx)
                    .preview
                    .scroll
                    .0
                    .borrow()
                    .base_handle
                    .offset()
                    .y,
                px(0.)
            );
        });
    }

    struct PreviewFixture {
        preview: HistoryPreview,
        width: f32,
        height: f32,
    }

    impl Render for PreviewFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().p(px(24.)).child(
                self.preview
                    .render()
                    .flex_none()
                    .w(px(self.width))
                    .h(px(self.height)),
            )
        }
    }

    #[gpui_kit::test]
    fn history_preview_scrollbar_uses_first_frame_and_resized_uniform_viewport(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::apply(&Default::default(), false, cx);
        });
        let scroll = UniformListScrollHandle::default();
        let (view, test_cx) = cx.add_window_view(|_, _| PreviewFixture {
            preview: HistoryPreview::default(),
            width: 360.,
            height: 160.,
        });
        assert_eq!(scroll.0.borrow().base_handle.bounds().size.height, px(0.));
        for (frame, (width, height)) in [(360., 160.), (280., 112.), (280., 400.)]
            .into_iter()
            .enumerate()
        {
            // Keep installation, drawing and inspection in one transaction:
            // separate context updates can flush an extra frame and hide stale bounds.
            test_cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.width = width;
                    view.height = height;
                    view.preview.scroll = scroll.clone();
                    view.preview.measured.borrow_mut().rows = Arc::new(
                        (0..1000).map(|ix| (format!("diff line {ix}"), '+')).collect(),
                    );
                    view.preview.measured.borrow_mut().width = Some(px(width - 24.).floor());
                    if frame > 0 {
                        scroll.0.borrow().base_handle.set_offset(point(
                            px(0.), px(-(22000. - height)),
                        ));
                    }
                    cx.notify();
                });
                window.render_frame(cx);
                let bounds = scroll.0.borrow().base_handle.bounds();
                assert_eq!(bounds.size, gpui_kit::size(px(width), px(height)));
                let viewport = window.pixel_snap_bounds(bounds).scale(window.scale_factor());
                let quads = window.painted_quads();
                let thumbs: Vec<_> = quads.iter().filter(|quad| !quad.background.is_transparent()
                    && quad.background != crate::theme::color(crate::components::BG).into()).collect();
                assert_eq!(thumbs.len(), 1, "uniform list scrollbar must paint on its first frame and each resize");
                let thumb = thumbs[0].bounds;
                assert!(thumb.top() >= viewport.top() && thumb.bottom() <= viewport.bottom(),
                    "history thumb uses stale vertical bounds: thumb={thumb:?}, viewport={viewport:?}");
                let inset = px(4.).scale(window.scale_factor());
                assert!(thumb.right() >= viewport.right() - inset - px(1.).scale(window.scale_factor())
                    && thumb.right() <= viewport.right(), "history thumb uses stale width");
                if frame > 0 {
                    assert!(thumb.bottom() >= viewport.bottom() - inset - px(1.).scale(window.scale_factor()),
                        "thumb at the end must use this frame's height");
                    assert!(window.try_find(("diff-line", 999_usize)).is_some(), "last diff row must remain reachable");
                }
                let mounted = (0_usize..1000).filter(|&ix| window.try_find(("diff-line", ix)).is_some()).count();
                assert!(mounted <= (height / 22.).ceil() as usize + 2,
                    "history diff stopped virtualizing: {mounted} mounted rows");
            });
        }
    }
}
