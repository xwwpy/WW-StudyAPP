//! Pure geometry and OS-drag gates shared by the native host and its tests.
#[cfg(test)]
pub use super::native_drag::DragGate;
#[cfg(test)]
use std::path::PathBuf;

pub type LogicalRect = (f64, f64, f64, f64);
pub type PhysicalRect = (i32, i32, i32, i32);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PhysicalShape {
    RoundedRect { rect: PhysicalRect, diameter: i32 },
    ArrowPolygon(Vec<(i32, i32)>),
}

/// The hull of the enclosing pixel cells contains the exact triangle. Every
/// candidate is within one pixel on each axis of its original vertex, so the
/// entire hull stays within triangle + [-1, 1]^2. Unlike rounding vertices
/// independently, this cannot rotate an edge inward on a skewed/clamped arrow.
fn arrow_pixel_outline(points: [(f64, f64); 3]) -> Vec<(i32, i32)> {
    let mut corners = Vec::with_capacity(12);
    for (x, y) in points {
        for x in [x.floor() as i32, x.ceil() as i32] {
            for y in [y.floor() as i32, y.ceil() as i32] {
                corners.push((x, y));
            }
        }
    }
    corners.sort_unstable();
    corners.dedup();
    let cross = |a: (i32, i32), b: (i32, i32), c: (i32, i32)| {
        let ab = (
            i64::from(b.0) - i64::from(a.0),
            i64::from(b.1) - i64::from(a.1),
        );
        let ac = (
            i64::from(c.0) - i64::from(a.0),
            i64::from(c.1) - i64::from(a.1),
        );
        i128::from(ab.0) * i128::from(ac.1) - i128::from(ab.1) * i128::from(ac.0)
    };
    let chain = |points: Vec<(i32, i32)>| {
        let mut hull = Vec::with_capacity(points.len());
        for point in points {
            while hull.len() >= 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], point) <= 0 {
                hull.pop();
            }
            hull.push(point);
        }
        hull.pop();
        hull
    };
    let lower = chain(corners.clone());
    corners.reverse();
    let mut result = lower;
    result.extend(chain(corners));
    result
}

fn triangle_intersects_editor(points: [(f64, f64); 3], editor: LogicalRect) -> bool {
    let (x, y, width, height) = editor;
    let rectangle = [
        (x, y),
        (x + width, y),
        (x + width, y + height),
        (x, y + height),
    ];
    let cross = (points[1].0 - points[0].0) * (points[2].1 - points[0].1)
        - (points[1].1 - points[0].1) * (points[2].0 - points[0].0);
    if width <= 0. || height <= 0. || cross == 0. {
        return false;
    }
    let axes = [(1., 0.), (0., 1.)].into_iter().chain((0..3).map(|index| {
        let a = points[index];
        let b = points[(index + 1) % 3];
        (a.1 - b.1, b.0 - a.0)
    }));
    for (dx, dy) in axes {
        if dx == 0. && dy == 0. {
            continue;
        }
        let project = |point: &(f64, f64)| point.0 * dx + point.1 * dy;
        let triangle_min = points.iter().map(project).fold(f64::INFINITY, f64::min);
        let triangle_max = points.iter().map(project).fold(f64::NEG_INFINITY, f64::max);
        let rectangle_min = rectangle.iter().map(project).fold(f64::INFINITY, f64::min);
        let rectangle_max = rectangle
            .iter()
            .map(project)
            .fold(f64::NEG_INFINITY, f64::max);
        if triangle_max <= rectangle_min || rectangle_max <= triangle_min {
            return false;
        }
    }
    true
}

pub fn physical_overlay_shapes(
    editor: LogicalRect,
    dpi: u32,
    shapes: &[studyapp_ui::bubble::OverlayShape],
) -> Vec<PhysicalShape> {
    use studyapp_ui::bubble::OverlayShape;
    let scale = f64::from(dpi) / 96.;
    let number = |p: gpui_kit::Pixels| f64::from(f32::from(p));
    let origin = (
        (editor.0 * scale).round() as i32,
        (editor.1 * scale).round() as i32,
    );
    let mut result = Vec::new();
    for shape in shapes {
        match shape {
            OverlayShape::RoundedRect { bounds, radius } => {
                let card = (
                    number(bounds.left()),
                    number(bounds.top()),
                    number(bounds.size.width),
                    number(bounds.size.height),
                );
                if let Some(rect) = rounded_occlusion_rects(editor, dpi, &[card]).first() {
                    result.push(PhysicalShape::RoundedRect {
                        rect: *rect,
                        diameter: (2. * number(*radius).max(0.) * scale).round() as i32,
                    });
                }
            }
            OverlayShape::Triangle(points) => {
                let points = points.map(|point| (number(point.x), number(point.y)));
                if !points.iter().all(|(x, y)| x.is_finite() && y.is_finite()) {
                    continue;
                }
                if !triangle_intersects_editor(points, editor) {
                    continue;
                }
                // Enclose absolute physical vertices first, then translate by
                // the actual rounded child origin, preserving fractional DPI.
                let outline = arrow_pixel_outline(points.map(|(x, y)| (x * scale, y * scale)));
                result.push(PhysicalShape::ArrowPolygon(
                    outline
                        .into_iter()
                        .map(|(x, y)| (x - origin.0, y - origin.1))
                        .collect(),
                ));
            }
        }
    }
    result.sort_unstable();
    result.dedup();
    result
}

/// Keep the original rounded card bounds even when part lies outside the child.
/// Clipping the rectangle first would invent rounded corners at the editor edge.
pub fn rounded_occlusion_rects(
    editor: LogicalRect,
    dpi: u32,
    cards: &[LogicalRect],
) -> Vec<PhysicalRect> {
    let (x, y, width, height) = editor;
    let scale = f64::from(dpi) / 96.;
    let origin_x = (x * scale).round();
    let origin_y = (y * scale).round();
    let mut result: Vec<_> = cards
        .iter()
        .filter_map(|&(left, top, w, h)| {
            if ![left, top, w, h].iter().all(|v| v.is_finite())
                || w <= 0.
                || h <= 0.
                || left + w <= x
                || top + h <= y
                || left >= x + width
                || top >= y + height
            {
                return None;
            }
            Some((
                (left * scale).floor() as i32 - origin_x as i32,
                (top * scale).floor() as i32 - origin_y as i32,
                ((left + w) * scale).ceil() as i32 - origin_x as i32,
                ((top + h) * scale).ceil() as i32 - origin_y as i32,
            ))
        })
        .collect();
    result.sort_unstable();
    result.dedup();
    result
}

pub fn clipped_occlusions(
    editor: LogicalRect,
    dpi: u32,
    cards: &[LogicalRect],
) -> Vec<PhysicalRect> {
    let (x, y, width, height) = editor;
    let scale = f64::from(dpi) / 96.;
    let physical_x = (x * scale).round();
    let physical_y = (y * scale).round();
    let mut result = cards
        .iter()
        .filter_map(|&(left, top, w, h)| {
            if ![left, top, w, h].iter().all(|v| v.is_finite()) || w <= 0. || h <= 0. {
                return None;
            }
            let right = (left + w - x).min(width);
            let bottom = (top + h - y).min(height);
            let left = (left - x).max(0.);
            let top = (top - y).max(0.);
            (right > left && bottom > top).then_some((
                (((left + x) * scale).floor() - physical_x).max(0.) as i32,
                (((top + y) * scale).floor() - physical_y).max(0.) as i32,
                (((right + x) * scale).ceil() - physical_x).min((width * scale).round()) as i32,
                (((bottom + y) * scale).ceil() - physical_y).min((height * scale).round()) as i32,
            ))
        })
        .collect::<Vec<PhysicalRect>>();
    result.sort_unstable();
    result.dedup();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn polygon_contains(points: &[(f64, f64)], sample: (f64, f64)) -> bool {
        let sides = (0..points.len())
            .map(|index| {
                let a = points[index];
                let b = points[(index + 1) % points.len()];
                (b.0 - a.0) * (sample.1 - a.1) - (b.1 - a.1) * (sample.0 - a.0)
            })
            .collect::<Vec<_>>();
        sides.iter().all(|side| *side >= -1e-8) || sides.iter().all(|side| *side <= 1e-8)
    }

    fn triangle_distance(points: &[(f64, f64); 3], sample: (f64, f64)) -> f64 {
        if polygon_contains(points, sample) {
            return 0.;
        }
        (0..3)
            .map(|index| {
                let a = points[index];
                let b = points[(index + 1) % 3];
                let edge = (b.0 - a.0, b.1 - a.1);
                let t = (((sample.0 - a.0) * edge.0 + (sample.1 - a.1) * edge.1)
                    / (edge.0 * edge.0 + edge.1 * edge.1))
                    .clamp(0., 1.);
                (sample.0 - a.0 - t * edge.0).hypot(sample.1 - a.1 - t * edge.1)
            })
            .fold(f64::INFINITY, f64::min)
    }

    fn converted_arrow(
        editor: LogicalRect,
        dpi: u32,
        arrow: [gpui_kit::Point<gpui_kit::Pixels>; 3],
    ) -> Vec<(f64, f64)> {
        let converted = physical_overlay_shapes(
            editor,
            dpi,
            &[studyapp_ui::bubble::OverlayShape::Triangle(arrow)],
        );
        let PhysicalShape::ArrowPolygon(points) = &converted[0] else {
            panic!("missing polygon")
        };
        points
            .iter()
            .map(|&(x, y)| (f64::from(x), f64::from(y)))
            .collect()
    }

    #[test]
    fn skewed_arrow_raster_contains_painted_edges_at_fractional_dpi() {
        use gpui_kit::{point, px, size};
        use studyapp_ui::bubble::{BubbleMetrics, bubble_layout};
        let arrow = bubble_layout(
            point(px(1000.), px(100.)),
            size(px(320.), px(40.)),
            size(px(1000.), px(400.)),
            BubbleMetrics::default(),
        )
        .arrow;
        let polygon = converted_arrow((0., 0., 1000., 400.), 120, arrow);
        let exact = [(1217.5, 142.5), (1250., 135.), (1232.5, 142.5)];
        assert!(polygon_contains(&exact, (1220., 142.)));
        assert!(
            polygon_contains(&polygon, (1220., 142.)),
            "125% DPI native outline excluded a painted arrow edge: {polygon:?}"
        );
        for dpi in [120, 144, 192] {
            for pointer_x in [0., 0.3, 999.7, 1000.] {
                let arrow = bubble_layout(
                    point(px(pointer_x), px(100.3)),
                    size(px(320.), px(40.)),
                    size(px(1000.), px(400.)),
                    BubbleMetrics::default(),
                )
                .arrow;
                let editor = (0.4, 0.4, 1000., 400.);
                let scale = f64::from(dpi) / 96.;
                let origin = ((editor.0 * scale).round(), (editor.1 * scale).round());
                let exact = arrow.map(|p| {
                    (
                        f64::from(f32::from(p.x)) * scale - origin.0,
                        f64::from(f32::from(p.y)) * scale - origin.1,
                    )
                });
                let polygon = converted_arrow(editor, dpi, arrow);
                for a in 0..=40 {
                    for b in 0..=40 - a {
                        let weights = [
                            f64::from(a) / 40.,
                            f64::from(b) / 40.,
                            f64::from(40 - a - b) / 40.,
                        ];
                        let sample = (
                            exact.iter().zip(weights).map(|(p, w)| p.0 * w).sum(),
                            exact.iter().zip(weights).map(|(p, w)| p.1 * w).sum(),
                        );
                        assert!(
                            polygon_contains(&polygon, sample),
                            "dpi={dpi}, pointer={pointer_x}, excluded painted sample={sample:?}, polygon={polygon:?}"
                        );
                    }
                }
                let left = polygon
                    .iter()
                    .map(|p| p.0)
                    .fold(f64::INFINITY, f64::min)
                    .floor() as i32;
                let right = polygon
                    .iter()
                    .map(|p| p.0)
                    .fold(f64::NEG_INFINITY, f64::max)
                    .ceil() as i32;
                let top = polygon
                    .iter()
                    .map(|p| p.1)
                    .fold(f64::INFINITY, f64::min)
                    .floor() as i32;
                let bottom = polygon
                    .iter()
                    .map(|p| p.1)
                    .fold(f64::NEG_INFINITY, f64::max)
                    .ceil() as i32;
                for x in left * 4..=right * 4 {
                    for y in top * 4..=bottom * 4 {
                        let sample = (f64::from(x) / 4., f64::from(y) / 4.);
                        if polygon_contains(&polygon, sample) {
                            assert!(
                                triangle_distance(&exact, sample) <= 2_f64.sqrt() + 1e-7,
                                "outline exceeded one pixel-cell expansion: dpi={dpi}, sample={sample:?}"
                            );
                        }
                    }
                }
                let outside_corners = [
                    (left + 1, top + 1),
                    (right - 1, top + 1),
                    (left + 1, bottom - 1),
                    (right - 1, bottom - 1),
                ]
                .map(|(x, y)| (f64::from(x), f64::from(y)))
                .into_iter()
                .filter(|sample| triangle_distance(&exact, *sample) > 2_f64.sqrt())
                .collect::<Vec<_>>();
                assert!(!outside_corners.is_empty());
                assert!(
                    outside_corners
                        .into_iter()
                        .all(|sample| !polygon_contains(&polygon, sample)),
                    "outline became a triangle bounding-box hole"
                );
            }
        }
    }

    #[cfg(windows)]
    #[test]
    fn skewed_arrow_gdi_region_contains_review_edge_without_a_window() {
        use gpui_kit::{point, px, size};
        use studyapp_ui::bubble::{BubbleMetrics, bubble_layout};
        use windows::Win32::Graphics::Gdi::{
            CreatePolygonRgn, DeleteObject, HRGN, PtInRegion, WINDING,
        };
        struct Region(HRGN);
        impl Drop for Region {
            fn drop(&mut self) {
                // A region primitive only: no HWND, window, pointer or focus.
                unsafe {
                    let _ = DeleteObject(self.0.into());
                }
            }
        }
        let arrow = bubble_layout(
            point(px(1000.), px(100.)),
            size(px(320.), px(40.)),
            size(px(1000.), px(400.)),
            BubbleMetrics::default(),
        )
        .arrow;
        let polygon = converted_arrow((0., 0., 1000., 400.), 120, arrow);
        let points = polygon
            .into_iter()
            .map(|(x, y)| windows::Win32::Foundation::POINT {
                x: x as i32,
                y: y as i32,
            })
            .collect::<Vec<_>>();
        let region = Region(unsafe { CreatePolygonRgn(&points, WINDING) });
        assert!(!region.0.0.is_null());
        assert!(
            unsafe { PtInRegion(region.0, 1220, 142).as_bool() },
            "GDI excluded the review's painted edge pixel"
        );
        assert!(
            !unsafe { PtInRegion(region.0, 1218, 136).as_bool() },
            "GDI cut an unpainted triangle bounding-box corner"
        );
    }

    #[test]
    fn arrow_outline_cache_is_canonical_for_vertex_order_and_duplicates() {
        use gpui_kit::{point, px};
        use studyapp_ui::bubble::OverlayShape;
        let points = [
            point(px(974.), px(114.)),
            point(px(1000.), px(108.)),
            point(px(986.), px(114.)),
        ];
        let first = physical_overlay_shapes(
            (0., 0., 1000., 400.),
            120,
            &[OverlayShape::Triangle(points)],
        );
        let reordered = [points[2], points[1], points[0]];
        assert_eq!(
            first,
            physical_overlay_shapes(
                (0., 0., 1000., 400.),
                120,
                &[
                    OverlayShape::Triangle(reordered),
                    OverlayShape::Triangle(points)
                ]
            )
        );
        let PhysicalShape::ArrowPolygon(outline) = &first[0] else {
            panic!()
        };
        assert!((3..=12).contains(&outline.len()));
    }
    #[test]
    fn bubble_shapes_preserve_radius_and_triangle_at_every_dpi() {
        use gpui_kit::{Bounds, point, px, size};
        use studyapp_ui::bubble::OverlayShape;
        for (dpi, factor) in [(96, 1), (144, 3), (192, 2)] {
            let scale = f64::from(dpi) / 96.;
            let shapes = [
                OverlayShape::RoundedRect {
                    bounds: Bounds::new(point(px(90.), px(50.)), size(px(40.), px(40.))),
                    radius: px(6.),
                },
                OverlayShape::Triangle([
                    point(px(110.), px(66.)),
                    point(px(116.), px(60.)),
                    point(px(122.), px(66.)),
                ]),
            ];
            let actual = physical_overlay_shapes((100., 60., 400., 300.), dpi, &shapes);
            assert_eq!(actual.len(), 2, "dpi {dpi}, factor {factor}");
            assert_eq!(
                actual[0],
                PhysicalShape::RoundedRect {
                    rect: (
                        (-10. * scale) as i32,
                        (-10. * scale) as i32,
                        (30. * scale) as i32,
                        (30. * scale) as i32
                    ),
                    diameter: (12. * scale) as i32
                }
            );
            assert_eq!(
                actual[1],
                PhysicalShape::ArrowPolygon(vec![
                    ((10. * scale) as i32, (6. * scale) as i32),
                    ((16. * scale) as i32, 0),
                    ((22. * scale) as i32, (6. * scale) as i32)
                ])
            );
            assert!(physical_overlay_shapes((600., 600., 40., 40.), dpi, &shapes).is_empty());
        }
    }
    #[test]
    fn triangle_conversion_tracks_fractional_child_origin_and_arrow_only_changes() {
        use gpui_kit::{point, px};
        use studyapp_ui::bubble::OverlayShape;
        let triangle = OverlayShape::Triangle([
            point(px(101.2), px(66.2)),
            point(px(107.2), px(60.2)),
            point(px(113.2), px(66.2)),
        ]);
        let editor = (100.4, 60.4, 400., 300.);
        let first = physical_overlay_shapes(editor, 144, &[triangle, triangle]);
        let PhysicalShape::ArrowPolygon(outline) = &first[0] else {
            panic!("missing arrow outline")
        };
        assert!(
            outline.len() > 3,
            "fractional outline should preserve enclosing pixel cells"
        );
        assert_eq!(outline.iter().map(|p| p.0).min(), Some(0));
        assert_eq!(outline.iter().map(|p| p.1).min(), Some(-1));
        assert_eq!(outline.iter().map(|p| p.1).max(), Some(9));
        let moved = OverlayShape::Triangle([
            point(px(102.2), px(66.2)),
            point(px(108.2), px(60.2)),
            point(px(114.2), px(66.2)),
        ]);
        assert_ne!(first, physical_overlay_shapes(editor, 144, &[moved]));
        assert!(physical_overlay_shapes(editor, 144, &[]).is_empty());
    }
    #[test]
    fn triangle_bounds_alone_do_not_create_an_editor_occlusion() {
        use gpui_kit::{point, px};
        use studyapp_ui::bubble::OverlayShape;
        let triangle = OverlayShape::Triangle([
            point(px(0.), px(0.)),
            point(px(10.), px(0.)),
            point(px(0.), px(10.)),
        ]);
        assert!(physical_overlay_shapes((8., 8., 10., 10.), 144, &[triangle]).is_empty());
        assert_eq!(
            physical_overlay_shapes((4., 4., 10., 10.), 144, &[triangle]),
            vec![PhysicalShape::ArrowPolygon(vec![
                (-6, -6),
                (9, -6),
                (-6, 9)
            ])]
        );
    }
    #[test]
    fn rounded_popup_preserves_original_corners_when_clipped_by_editor() {
        assert_eq!(
            rounded_occlusion_rects(
                (100., 60., 400., 300.),
                144,
                &[(90., 50., 40.5, 40.5), (600., 60., 10., 10.)]
            ),
            vec![(-15, -15, 46, 46)]
        );
    }
    #[test]
    fn occlusions_translate_clip_and_round_outward_at_fractional_dpi() {
        assert_eq!(
            clipped_occlusions(
                (100., 60., 400., 300.),
                144,
                &[(90., 50., 40.5, 40.5), (600., 60., 10., 10.)]
            ),
            vec![(0, 0, 46, 46)]
        );
    }
    #[test]
    fn no_intersection_restores_the_whole_native_window() {
        assert!(
            clipped_occlusions(
                (100., 60., 400., 300.),
                96,
                &[(0., 0., 20., 20.), (100., 20., 400., 40.)]
            )
            .is_empty()
        );
    }
    #[test]
    fn fractional_child_origin_matches_win32_rounded_position() {
        assert_eq!(
            clipped_occlusions((100.4, 60.4, 400., 300.), 144, &[(101.2, 61.2, 20., 20.)]),
            vec![(0, 0, 31, 31)]
        );
    }
    #[test]
    fn occlusion_order_and_duplicates_do_not_trigger_native_changes() {
        let a = (100., 60., 20., 20.);
        let b = (150., 100., 20., 20.);
        assert_eq!(
            clipped_occlusions((100., 60., 400., 300.), 96, &[a, b, a]),
            clipped_occlusions((100., 60., 400., 300.), 96, &[b, a])
        );
    }
    #[test]
    fn drag_coalesces_hover_but_allows_distinct_repeated_drops() {
        let mut gate = DragGate::default();
        let paths = vec![PathBuf::from("C:/notes/a.md")];
        assert!(!gate.enter(Vec::new()));
        assert!(gate.enter(paths.clone()));
        assert!(!gate.enter(paths.clone()));
        assert!(gate.drop_paths(paths.clone()));
        assert!(!gate.drop_paths(paths.clone()));
        assert!(!gate.leave());
        assert!(gate.enter(paths.clone()));
        assert!(gate.drop_paths(paths));
    }
    #[test]
    fn drag_leave_emits_once_and_empty_drop_does_not_import() {
        let mut gate = DragGate::default();
        assert!(gate.enter(vec![PathBuf::from("C:/notes")]));
        assert!(gate.leave());
        assert!(!gate.leave());
        assert!(!gate.drop_paths(Vec::new()));
    }
}
