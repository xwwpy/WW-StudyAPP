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
        Self { index, pages, range: start..start.saturating_add(size).min(total) }
    }
}
#[cfg(test)]
mod tests {
    use super::RecordPage;
    #[test]
    fn pages_are_disjoint_complete_and_clamped_after_filtering() {
        let mut rows = vec![];
        for page in 0..3 { rows.extend(RecordPage::new(125, page, 50).range); }
        assert_eq!(rows, (0..125).collect::<Vec<_>>());
        assert_eq!(RecordPage::new(12, 8, 50), RecordPage { index:0, pages:1, range:0..12 });
        assert_eq!(RecordPage::new(0, usize::MAX, 0).range, 0..0);
        assert_eq!(RecordPage::new(100, 1, 50).range, 50..100);
    }
}
