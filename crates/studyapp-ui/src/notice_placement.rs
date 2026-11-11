//! Pure feedback-space arbitration. No card may cover another card's actions.
use studyapp_core::{NoticeLayout, NoticePosition, NoticeStyle, NotificationAppearance};
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}
impl Rect {
    fn intersects(self, other: Self) -> bool {
        self.x < other.x + other.width + 8.
            && other.x < self.x + self.width + 8.
            && self.y < other.y + other.height + 8.
            && other.y < self.y + self.height + 8.
    }
}
#[derive(Clone, Copy)]
pub(crate) struct Candidate {
    pub id: u64,
    pub style: NoticeStyle,
    pub width: f32,
    pub height: f32,
}
#[derive(Default)]
pub(crate) struct Placement {
    pub floating: Vec<(u64, Rect)>,
    pub inline: Vec<u64>,
}
impl Placement {
    pub fn ids(&self) -> std::collections::BTreeSet<u64> {
        self.inline
            .iter()
            .copied()
            .chain(self.floating.iter().map(|(id, _)| *id))
            .collect()
    }
}
pub(crate) fn place(
    candidates: &[Candidate],
    appearance: &NotificationAppearance,
    width: f32,
    height: f32,
) -> Placement {
    let mut result = Placement::default();
    if !width.is_finite() || !height.is_finite() || width < 64. || height < 96. {
        return result;
    }
    if let Some(modal) = candidates
        .iter()
        .find(|item| item.style == NoticeStyle::ModalCard)
    {
        result.inline.push(modal.id);
        return result;
    }
    let mut top = crate::design::TITLE_BAR_HEIGHT + 8.;
    for item in candidates.iter().filter(|item| {
        item.style == NoticeStyle::Banner && appearance.layout(item.style) == NoticeLayout::Primary
    }) {
        if item.height.is_finite() && item.height > 0. && top + item.height < height * 0.45 {
            result.inline.push(item.id);
            top += item.height + 8.;
        }
    }
    let mut toast_y = top;
    let mut corner_top = top;
    let mut corner_bottom = height - 16.;
    let full_snackbar = appearance.layout(NoticeStyle::Snackbar) == NoticeLayout::Alternative;
    let mut snackbar_bottom = height - if full_snackbar { 0. } else { 16. };
    // Every region has its own queue; this orders only contested screen space.
    for style in [
        NoticeStyle::Banner,
        NoticeStyle::Snackbar,
        NoticeStyle::CornerCard,
        NoticeStyle::Bubble,
        NoticeStyle::Hud,
    ] {
        let mut items: Vec<_> = candidates
            .iter()
            .filter(|item| item.style == style && !result.inline.contains(&item.id))
            .collect();
        if style == NoticeStyle::Banner && appearance.layout(style) == NoticeLayout::Primary {
            continue;
        }
        if style.region().is_some_and(|region| {
            appearance.region_settings(region).stack == studyapp_core::NoticeStack::NewestFirst
        }) {
            items.reverse();
        }
        let staggered = style.region().is_some_and(|region| {
            appearance.region_settings(region).stack == studyapp_core::NoticeStack::Staggered
        });
        for (index, item) in items.into_iter().enumerate() {
            if !item.width.is_finite() || !item.height.is_finite() || item.height <= 0. {
                continue;
            }
            let full = style == NoticeStyle::Banner
                || (style == NoticeStyle::Snackbar
                    && appearance.layout(style) == NoticeLayout::Alternative);
            let edge_to_edge = style == NoticeStyle::Snackbar && full_snackbar;
            let w = if edge_to_edge {
                width
            } else if full {
                width - 32.
            } else {
                item.width.min(width - 32.).max(0.)
            };
            let h = item.height;
            let (x, y) = match style {
                NoticeStyle::Banner | NoticeStyle::Bubble | NoticeStyle::Hud => {
                    ((width - w) * 0.5, toast_y)
                }
                NoticeStyle::Snackbar => ((width - w) * 0.5, snackbar_bottom - h),
                _ => {
                    let left = matches!(
                        appearance.position,
                        NoticePosition::TopLeft | NoticePosition::BottomLeft
                    );
                    let upper = matches!(
                        appearance.position,
                        NoticePosition::TopLeft | NoticePosition::TopRight
                    );
                    (
                        if left { 16. } else { width - w - 16. },
                        if upper { corner_top } else { corner_bottom - h },
                    )
                }
            };
            let step = if staggered && !full {
                (index as f32 * 18.).min(((width - w) * 0.5 - 16.).max(0.))
            } else {
                0.
            };
            let x = if style == NoticeStyle::CornerCard
                && matches!(
                    appearance.position,
                    NoticePosition::TopLeft | NoticePosition::BottomLeft
                ) {
                x + step
            } else {
                x - step
            };
            let rect = Rect {
                x,
                y,
                width: w,
                height: h,
            };
            if y < top
                || y + h > height - if edge_to_edge { 0. } else { 16. }
                || result
                    .floating
                    .iter()
                    .any(|(_, other)| rect.intersects(*other))
            {
                continue;
            }
            result.floating.push((item.id, rect));
            match style {
                NoticeStyle::Snackbar => snackbar_bottom = y - 8.,
                NoticeStyle::CornerCard => {
                    if matches!(
                        appearance.position,
                        NoticePosition::TopLeft | NoticePosition::TopRight
                    ) {
                        corner_top = y + h + 8.;
                    } else {
                        corner_bottom = y - 8.;
                    }
                }
                _ => toast_y = y + h + 8.,
            }
        }
    }
    result
}
#[cfg(test)]
mod tests {
    use super::{Candidate, place};
    use studyapp_core::{NoticeStyle as S, NotificationAppearance};
    #[test]
    fn different_regions_run_together_without_overlaps_and_narrow_space_queues() {
        let items = [
            Candidate {
                id: 1,
                style: S::Bubble,
                width: 300.,
                height: 44.,
            },
            Candidate {
                id: 2,
                style: S::CornerCard,
                width: 360.,
                height: 260.,
            },
            Candidate {
                id: 3,
                style: S::Snackbar,
                width: 360.,
                height: 64.,
            },
        ];
        let appearance = NotificationAppearance::default();
        let wide = place(&items, &appearance, 1200., 800.);
        assert_eq!(wide.floating.len(), 3);
        for (i, (_, rect)) in wide.floating.iter().enumerate() {
            assert!(
                wide.floating
                    .iter()
                    .skip(i + 1)
                    .all(|(_, other)| !rect.intersects(*other))
            );
        }
        let narrow = place(&items, &appearance, 400., 350.);
        assert!(narrow.floating.len() < 3);
        assert!(
            narrow
                .floating
                .iter()
                .all(|(_, rect)| rect.x >= 16. && rect.x + rect.width <= 384.)
        );
    }
    #[test]
    fn inline_banner_reserves_space_and_modal_preempts_without_becoming_a_card() {
        let mut items = vec![
            Candidate {
                id: 1,
                style: S::Banner,
                width: 360.,
                height: 80.,
            },
            Candidate {
                id: 2,
                style: S::Bubble,
                width: 300.,
                height: 44.,
            },
        ];
        let appearance = NotificationAppearance::default();
        let normal = place(&items, &appearance, 1000., 800.);
        assert_eq!(normal.inline, [1]);
        assert!(normal.floating[0].1.y >= crate::design::TITLE_BAR_HEIGHT + 88.);
        items.push(Candidate {
            id: 3,
            style: S::ModalCard,
            width: 500.,
            height: 300.,
        });
        let modal = place(&items, &appearance, 1000., 800.);
        assert_eq!(modal.inline, [3]);
        assert!(modal.floating.is_empty());
    }
    #[test]
    fn staggered_regions_shift_cards_without_covering_any_actions() {
        let items: Vec<_> = (0..3)
            .map(|id| Candidate {
                id,
                style: S::CornerCard,
                width: 300.,
                height: 90.,
            })
            .collect();
        let mut appearance = NotificationAppearance::default();
        appearance.regions.insert(
            studyapp_core::NoticeRegion::Corner,
            studyapp_core::NoticeRegionSettings {
                stack: studyapp_core::NoticeStack::Staggered,
                max_visible: 3,
            },
        );
        let placed = place(&items, &appearance, 1200., 800.);
        assert_eq!(placed.floating.len(), 3);
        assert_ne!(placed.floating[0].1.x, placed.floating[1].1.x);
        for (index, (_, rect)) in placed.floating.iter().enumerate() {
            assert!(
                placed
                    .floating
                    .iter()
                    .skip(index + 1)
                    .all(|(_, other)| !rect.intersects(*other))
            );
        }
    }
    #[test]
    fn alternative_snackbar_reaches_both_side_edges_and_the_bottom() {
        let mut appearance = NotificationAppearance::default();
        appearance
            .layouts
            .insert(S::Snackbar, studyapp_core::NoticeLayout::Alternative);
        let placed = place(
            &[Candidate {
                id: 1,
                style: S::Snackbar,
                width: 300.,
                height: 60.,
            }],
            &appearance,
            800.,
            600.,
        );
        let rect = placed.floating[0].1;
        assert_eq!((rect.x, rect.width, rect.y + rect.height), (0., 800., 600.));
    }
}
