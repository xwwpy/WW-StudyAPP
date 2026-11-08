//! Disjoint visible rectangles for native compositors that lack rounded regions.
use gpui_kit::{Bounds, Pixels, point, px};
use studyapp_ui::bubble::OverlayShape;

pub fn visible_rectangles(editor: Bounds<Pixels>, shapes: &[OverlayShape]) -> Vec<Bounds<Pixels>> {
    let mut visible = vec![editor];
    for shape in shapes {
        let hole = match shape {
            OverlayShape::RoundedRect { bounds, .. } => *bounds,
            OverlayShape::Triangle(points) => Bounds::from_corners(
                point(
                    points.iter().map(|p| p.x).min().unwrap(),
                    points.iter().map(|p| p.y).min().unwrap(),
                ),
                point(
                    points.iter().map(|p| p.x).max().unwrap(),
                    points.iter().map(|p| p.y).max().unwrap(),
                ),
            ),
        };
        visible = visible
            .into_iter()
            .flat_map(|rect| {
                let cut = rect.intersect(&hole);
                if cut.size.width <= px(0.) || cut.size.height <= px(0.) {
                    return vec![rect];
                }
                [
                    Bounds::from_corners(rect.origin, point(rect.right(), cut.top())),
                    Bounds::from_corners(point(rect.left(), cut.bottom()), rect.bottom_right()),
                    Bounds::from_corners(
                        point(rect.left(), cut.top()),
                        point(cut.left(), cut.bottom()),
                    ),
                    Bounds::from_corners(
                        point(cut.right(), cut.top()),
                        point(rect.right(), cut.bottom()),
                    ),
                ]
                .into_iter()
                .filter(|r| r.size.width > px(0.) && r.size.height > px(0.))
                .collect()
            })
            .collect();
    }
    visible
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::size;
    #[test]
    fn overlapping_holes_are_a_union_and_never_restore_hidden_pixels() {
        let editor = Bounds::new(point(px(20.), px(30.)), size(px(100.), px(100.)));
        let holes = [
            OverlayShape::card(Bounds::new(point(px(40.), px(50.)), size(px(50.), px(50.)))),
            OverlayShape::card(Bounds::new(point(px(60.), px(70.)), size(px(50.), px(50.)))),
        ];
        let visible = visible_rectangles(editor, &holes);
        let area: f32 = visible
            .iter()
            .map(|r| f32::from(r.size.width) * f32::from(r.size.height))
            .sum();
        assert_eq!(area, 5900.);
        for (i, a) in visible.iter().enumerate() {
            assert!(
                a.left() >= editor.left()
                    && a.top() >= editor.top()
                    && a.right() <= editor.right()
                    && a.bottom() <= editor.bottom()
            );
            for b in &visible[i + 1..] {
                let overlap = a.intersect(b);
                assert!(overlap.size.width <= px(0.) || overlap.size.height <= px(0.));
            }
        }
        assert!(visible_rectangles(editor, &[OverlayShape::card(editor)]).is_empty());
        assert_eq!(visible_rectangles(editor, &[]), vec![editor]);
    }
}
