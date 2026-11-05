//! Shape inline runs without re-entering the layout engine from a measure callback.
//! Child element trees are laid out only during prepaint, using measured positions.
use super::{Node, Run, SharedFrame, node, run_style};
use gpui_kit::*;

pub(super) fn render(
    indices: Vec<usize>,
    align: &str,
    fill_width: bool,
    frame: &SharedFrame,
) -> AnyElement {
    let runs = indices
        .into_iter()
        .map(|i| (i, frame.borrow().document.runs[i].clone()))
        .collect();
    InlineFlow {
        runs,
        align: align.into(),
        fill_width,
        frame: frame.clone(),
        fonts: Vec::new(),
    }
    .into_any_element()
}
struct InlineFlow {
    runs: Vec<(usize, Run)>,
    align: String,
    fill_width: bool,
    frame: SharedFrame,
    fonts: Vec<TextStyle>,
}
#[derive(Clone, Copy)]
struct Metrics {
    size: Size<Pixels>,
    baseline: Pixels,
    space: bool,
    ideographic: bool,
}
struct Placement {
    bounds: Vec<Bounds<Pixels>>,
    size: Size<Pixels>,
}
fn measure(
    runs: &[(usize, Run)],
    fonts: &[TextStyle],
    fill_width: bool,
    width: Option<Pixels>,
    align: &str,
    window: &Window,
) -> Placement {
    let metrics: Vec<_> = runs
        .iter()
        .zip(fonts)
        .map(|((_, run), style)| {
            let font_size = style.font_size.to_pixels(window.rem_size());
            let line_height = window.pixel_snap(
                style
                    .line_height
                    .to_pixels(font_size.into(), window.rem_size()),
            );
            let tracking = px(run.font.tracking);
            let shaped = window
                .text_system()
                .shape_text(
                    run.text.clone().into(),
                    font_size,
                    &[style.to_run(run.text.len())],
                    width.map(|w| (w - tracking).max(px(1.))),
                    None,
                )
                .unwrap_or_default();
            let mut size = size(px(0.), px(0.));
            for line in &shaped {
                let line_size = line.size(line_height);
                size.width = size.width.max(line_size.width).ceil();
                size.height += line_size.height;
            }
            size.width += tracking;
            let baseline = shaped
                .first()
                .map(|line| (line_height - line.ascent() - line.descent()) / 2. + line.ascent())
                .unwrap_or(line_height);
            Metrics {
                size,
                baseline,
                space: run.text.bytes().all(|b| b.is_ascii_whitespace()),
                ideographic: run.text.chars().last().is_some_and(|ch| matches!(ch as u32,
                    0x3040..=0x30ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xac00..=0xd7af | 0x20000..=0x323af)),
            }
        })
        .collect();
    let width = width.map(|w| {
        if fill_width {
            w
        } else {
            w.min(metrics.iter().map(|m| m.size.width).sum::<Pixels>())
        }
    });
    place(&metrics, width, align)
}
fn place(metrics: &[Metrics], width: Option<Pixels>, align: &str) -> Placement {
    let natural = metrics.iter().map(|m| m.size.width).sum::<Pixels>();
    let width = width.unwrap_or(natural).max(px(1.));
    let mut bounds = vec![Bounds::default(); metrics.len()];
    let mut start = 0;
    let mut y = px(0.);
    while start < metrics.len() {
        let mut end = start;
        let mut used = px(0.);
        let mut above = px(0.);
        let mut below = px(0.);
        while end < metrics.len() {
            let m = metrics[end];
            if end > start && used + m.size.width > width {
                break;
            }
            used += m.size.width;
            above = above.max(m.baseline);
            below = below.max(m.size.height - m.baseline);
            end += 1;
        }
        let free = (width - used).max(px(0.));
        let mut x = match align {
            "right" => free,
            "center" => free / 2.,
            _ => px(0.),
        };
        // Do not justify the final line or stretch leading/trailing whitespace.
        let spaces = (start..end)
            .filter(|&i| i > start && i + 1 < end && metrics[i].space)
            .count();
        let gap = |i: usize| {
            i + 1 < end
                && if spaces > 0 {
                    i > start && metrics[i].space
                } else {
                    metrics[i].ideographic && metrics[i + 1].ideographic
                }
        };
        let gaps = (start..end).filter(|&i| gap(i)).count();
        let extra = if align == "justify" && end < metrics.len() && gaps > 0 {
            free / gaps as f32
        } else {
            px(0.)
        };
        for i in start..end {
            let m = metrics[i];
            bounds[i] = Bounds::new(point(x, y + above - m.baseline), m.size);
            x += m.size.width;
            if gap(i) {
                x += extra;
            }
        }
        y += above + below;
        start = end;
    }
    Placement {
        bounds,
        size: size(width, y),
    }
}
impl IntoElement for InlineFlow {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for InlineFlow {
    type RequestLayoutState = ();
    type PrepaintState = Vec<AnyElement>;
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
        _: &mut App,
    ) -> (LayoutId, ()) {
        // The layout engine invokes measurement outside ancestor text-style
        // scopes. Capture the resolved fonts now, not inside that callback.
        self.fonts = self
            .runs
            .iter()
            .map(|(_, run)| run_style(&run.font, window))
            .collect();
        let fonts = self.fonts.clone();
        let runs = self.runs.clone();
        let align = self.align.clone();
        let fill_width = self.fill_width;
        let mut style = Style::default();
        if fill_width {
            style.size.width = relative(1.).into();
        }
        style.flex_shrink = 0.;
        let layout = window.request_measured_layout(style, move |known, available, window, _| {
            let width = known.width.or(match available.width {
                AvailableSpace::Definite(w) => Some(w),
                _ => None,
            });
            measure(&runs, &fonts, fill_width, width, &align, window).size
        });
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
    ) -> Vec<AnyElement> {
        let placement = measure(
            &self.runs,
            &self.fonts,
            self.fill_width,
            Some(bounds.size.width),
            &self.align,
            window,
        );
        self.runs
            .iter()
            .zip(placement.bounds)
            .map(|((index, _), local)| {
                let mut child = node(&Node::Run(*index), &self.frame);
                child.layout_as_root(local.size.map(AvailableSpace::Definite), window, cx);
                child.prepaint_at(bounds.origin + local.origin, window, cx);
                child
            })
            .collect()
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        children: &mut Vec<AnyElement>,
        window: &mut Window,
        cx: &mut App,
    ) {
        for child in children {
            child.paint(window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Metrics, place};
    use gpui_kit::{px, size};
    fn word(w: f32, h: f32, baseline: f32, space: bool) -> Metrics {
        Metrics {
            size: size(px(w), px(h)),
            baseline: px(baseline),
            space,
            ideographic: false,
        }
    }
    #[test]
    fn native_flow_aligns_actual_font_baselines_and_reserves_descenders() {
        let m = [word(10., 20., 15., false), word(30., 60., 40., false)];
        let p = place(&m, Some(px(100.)), "left");
        assert_eq!(
            p.bounds[0].top() + m[0].baseline,
            p.bounds[1].top() + m[1].baseline
        );
        assert_eq!(p.size.height, px(60.));
        assert!(p.bounds.iter().all(|b| b.bottom() <= p.size.height));
    }
    #[test]
    fn native_flow_justifies_wrapped_lines_but_not_the_last_line() {
        let m = [
            word(30., 20., 15., false),
            word(5., 20., 15., true),
            word(30., 20., 15., false),
            word(5., 20., 15., true),
            word(30., 20., 15., false),
        ];
        let p = place(&m, Some(px(85.)), "justify");
        assert_eq!(p.bounds[2].right(), px(80.));
        assert_eq!(p.bounds[4].left(), px(0.));
        assert_eq!(p.bounds[4].top(), px(20.));
        for (align, left) in [("center", 27.5), ("right", 55.)] {
            assert_eq!(
                place(&m[4..], Some(px(85.)), align).bounds[0].left(),
                px(left)
            );
        }
    }
    #[test]
    fn native_flow_justifies_ideographs_without_inserting_copy_text() {
        let mut m = [word(20., 20., 15., false); 3];
        for metrics in &mut m {
            metrics.ideographic = true;
        }
        let p = place(&m, Some(px(50.)), "justify");
        assert_eq!(p.bounds[1].right(), px(50.));
        assert_eq!(p.bounds[2].left(), px(0.));
        assert_eq!(p.bounds[2].top(), px(20.));
    }
}
