//! Real project menus: independent appearance, keyboard/submenu behavior and geometry.
//! Hosts retain gpui-component's focus/weak-ownership design; see LICENSE-APACHE.
mod context_menu;
mod dropdown_menu;
mod menu_item;
mod popup_menu;
pub use context_menu::{ContextMenu, ContextMenuExt, ContextMenuState};
pub use dropdown_menu::{DropdownMenu, DropdownMenuPopover};
pub use popup_menu::{PopupMenu, PopupMenuItem, PopupMenuLayout};
use studyapp_core::{NoticeDensity, NoticeLayout};

#[derive(Clone, Copy, Debug)]
struct MenuMetrics {
    row_height: f32,
    padding: f32,
    gap: f32,
}
impl MenuMetrics {
    fn new(layout: NoticeLayout, density: NoticeDensity) -> Self {
        match (layout, density) {
            (NoticeLayout::Primary, NoticeDensity::Comfortable) => Self {
                row_height: 28.,
                padding: 6.,
                gap: 3.,
            },
            _ => Self {
                row_height: 20.,
                padding: 3.,
                gap: 1.,
            },
        }
    }
}

fn menu_extent(
    width: f32,
    height: f32,
    min_width: Option<f32>,
    max_width: Option<f32>,
    max_height: Option<f32>,
    viewport_width: f32,
    viewport_height: f32,
) -> (f32, f32) {
    let available_width = (viewport_width - 8.).max(0.);
    let available_height = (viewport_height - 8.).max(0.);
    let width = width
        .max(min_width.unwrap_or(0.))
        .min(max_width.unwrap_or(f32::MAX))
        .min(available_width);
    (
        width.max(0.),
        height
            .min(max_height.unwrap_or(f32::MAX))
            .min(available_height)
            .max(0.),
    )
}

fn next_actionable(
    items: impl Iterator<Item = bool>,
    selected: Option<usize>,
    backwards: bool,
) -> Option<usize> {
    let indices: Vec<_> = items
        .enumerate()
        .filter_map(|(i, enabled)| enabled.then_some(i))
        .collect();
    if backwards {
        selected
            .and_then(|selected| indices.iter().rev().find(|i| **i < selected).copied())
            .or_else(|| indices.last().copied())
    } else {
        selected
            .and_then(|selected| indices.iter().find(|i| **i > selected).copied())
            .or_else(|| indices.first().copied())
    }
}

#[cfg(test)]
mod tests {
    use super::{MenuMetrics, menu_extent, next_actionable};
    use studyapp_core::{NoticeDensity, NoticeLayout};
    #[test]
    fn compact_changes_actual_command_density() {
        let standard = MenuMetrics::new(NoticeLayout::Primary, NoticeDensity::Comfortable);
        let compact = MenuMetrics::new(NoticeLayout::Alternative, NoticeDensity::Comfortable);
        assert!(compact.row_height < standard.row_height);
        assert_eq!(
            MenuMetrics::new(NoticeLayout::Primary, NoticeDensity::Compact).row_height,
            compact.row_height
        );
        assert!(compact.padding < standard.padding && compact.gap < standard.gap);
    }
    #[test]
    fn menu_extents_respect_real_viewport_and_explicit_constraints() {
        assert_eq!(
            menu_extent(300., 500., None, None, None, 240., 180.),
            (232., 172.)
        );
        assert_eq!(
            menu_extent(300., 500., Some(340.), Some(360.), Some(220.), 1200., 800.),
            (340., 220.)
        );
        assert_eq!(
            menu_extent(300., 500., Some(400.), Some(280.), None, 2., 2.),
            (0., 0.)
        );
    }
    #[test]
    fn keyboard_wrap_skips_disabled_labels_and_separators() {
        let items = [false, false, true, false, true, false];
        for (selected, backwards, expected) in [
            (None, false, Some(2)),
            (None, true, Some(4)),
            (Some(2), false, Some(4)),
            (Some(4), false, Some(2)),
            (Some(2), true, Some(4)),
            (Some(4), true, Some(2)),
        ] {
            assert_eq!(
                next_actionable(items.into_iter(), selected, backwards),
                expected
            );
        }
        assert_eq!(next_actionable([false; 3].into_iter(), None, false), None);
        assert_eq!(next_actionable([].into_iter(), Some(0), true), None);
    }
}
