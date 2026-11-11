//! Pure page bounds; filtering and grouping happen before slicing the page.
#[derive(Debug, PartialEq, Eq)]
pub struct RecordPage {
    pub index: usize,
    pub pages: usize,
    pub range: std::ops::Range<usize>,
}
impl RecordPage {
    pub fn new(total: usize, requested: usize, size: usize) -> Self {
        let size = size.max(1);
        let pages = total.div_ceil(size).max(1);
        let index = requested.min(pages - 1);
        let start = index * size;
        Self {
            index,
            pages,
            range: start..start.saturating_add(size).min(total),
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RecordGrouping {
    #[default]
    None,
    Date,
    Note,
    Status,
}
pub fn grouped_records(
    positions: &[usize],
    grouping: RecordGrouping,
    mut label: impl FnMut(usize) -> String,
) -> Vec<(String, usize)> {
    let mut rows: Vec<_> = positions
        .iter()
        .map(|&position| (label(position), position))
        .collect();
    if grouping != RecordGrouping::None {
        // Stable sorting keeps newest-first order within every category.
        rows.sort_by(|a, b| {
            if grouping == RecordGrouping::Date {
                b.0.cmp(&a.0)
            } else {
                a.0.cmp(&b.0)
            }
        });
    }
    rows
}
#[cfg(test)]
mod grouping_tests {
    use super::*;
    #[test]
    fn groups_precede_pagination_without_duplicate_records() {
        let labels = ["2026-11-10", "2026-11-12", "2026-11-10", "2026-11-12"];
        let rows = grouped_records(&[3, 2, 1, 0], RecordGrouping::Date, |i| labels[i].into());
        assert_eq!(
            rows.iter().map(|row| row.1).collect::<Vec<_>>(),
            [3, 1, 2, 0]
        );
        let mut seen = vec![];
        for page in 0..2 {
            for row in &rows[RecordPage::new(rows.len(), page, 2).range] {
                seen.push(row.1);
            }
        }
        seen.sort_unstable();
        assert_eq!(seen, [0, 1, 2, 3]);
    }
}
#[cfg(test)]
mod tests {
    use super::RecordPage;
    #[test]
    fn pages_are_disjoint_complete_and_clamped_after_filtering() {
        let mut rows = vec![];
        for page in 0..3 {
            rows.extend(RecordPage::new(125, page, 50).range);
        }
        assert_eq!(rows, (0..125).collect::<Vec<_>>());
        assert_eq!(
            RecordPage::new(12, 8, 50),
            RecordPage {
                index: 0,
                pages: 1,
                range: 0..12
            }
        );
        assert_eq!(RecordPage::new(0, usize::MAX, 0).range, 0..0);
        assert_eq!(RecordPage::new(100, 1, 50).range, 50..100);
    }
}
