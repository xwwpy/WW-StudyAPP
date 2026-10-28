//! Immutable, GPUI-independent presentation prepared with a storage scan.
use super::{CopyState, StorageReport};
use std::collections::HashMap;
use studyapp_desktop::presentation::display_path;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CopyOrder {
    #[default]
    Largest,
    Name,
}

#[derive(Clone, Debug)]
pub struct StorageCategory {
    pub title: &'static str,
    pub bytes: String,
    pub byte_count: u64,
    pub description: &'static str,
}

#[derive(Clone, Debug)]
pub struct StorageIssueView {
    pub path: String,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct ManagedCopyView {
    pub note_id: String,
    pub title: String,
    pub path: String,
    pub bytes: String,
    pub byte_count: u64,
    search_key: String,
    pub state: CopyState,
    pub deletable: bool,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct StoragePresentation {
    pub logical_label: String,
    pub scanned_label: String,
    pub categories: Vec<StorageCategory>,
    pub image_summary: String,
    pub image_locations: Vec<String>,
    pub issues: Vec<StorageIssueView>,
    pub copies: Vec<ManagedCopyView>,
    copy_indices: HashMap<String, usize>,
}

impl StoragePresentation {
    pub fn copy_rows(&self, query: &str, order: CopyOrder) -> Vec<usize> {
        let query = query.trim().to_lowercase();
        let mut rows = self
            .copies
            .iter()
            .enumerate()
            .filter(|(_, copy)| query.is_empty() || copy.search_key.contains(&query))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        rows.sort_by(|&a, &b| {
            let left = &self.copies[a];
            let right = &self.copies[b];
            let primary = match order {
                CopyOrder::Largest => right.byte_count.cmp(&left.byte_count),
                CopyOrder::Name => left.search_key.cmp(&right.search_key),
            };
            primary
                .then_with(|| left.search_key.cmp(&right.search_key))
                .then_with(|| left.note_id.cmp(&right.note_id))
        });
        rows
    }

    pub fn prepare(report: &StorageReport) -> Self {
        let timestamp = chrono::DateTime::from_timestamp(report.scanned_at, 0)
            .map(|time| {
                time.with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M:%S")
                    .to_string()
            })
            .unwrap_or_else(|| "尚未统计".into());
        let categories = [
            (
                "数据库与历史",
                report.database_bytes,
                "笔记历史、学习记录与数据库日志",
            ),
            (
                "托管笔记",
                report.managed_bytes,
                "当前资料库自有的 Markdown 副本",
            ),
            (
                "本地图片",
                report.image_bytes,
                "库内图片及索引登记的当前、历史位置",
            ),
            ("草稿", report.draft_bytes, "未提交的笔记草稿，支持多份草稿"),
            (
                "保留与恢复文件",
                report.retained_bytes,
                "发布暂存、原始副本及未归属文件",
            ),
            ("备份", report.backup_bytes, "资料库目录内的数据库备份"),
            (
                "配置与其他",
                report.auxiliary_bytes,
                "设置版本、图片索引与其他文件",
            ),
        ]
        .into_iter()
        .map(|(title, bytes, description)| StorageCategory {
            title,
            bytes: size_label(bytes),
            byte_count: bytes,
            description,
        })
        .collect::<Vec<_>>();
        let copies = report
            .copies
            .iter()
            .map(|copy| ManagedCopyView {
                note_id: copy.note_id.clone(),
                title: copy.title.clone(),
                path: display_path(&copy.path),
                bytes: size_label(copy.bytes),
                byte_count: copy.bytes,
                search_key: copy.title.to_lowercase(),
                state: copy.state,
                deletable: copy.deletable,
                reason: copy.reason.clone(),
            })
            .collect::<Vec<_>>();
        let copy_indices = copies
            .iter()
            .enumerate()
            .map(|(ix, copy)| (copy.note_id.clone(), ix))
            .collect();
        Self {
            logical_label: format!(
                "已统计逻辑大小 {}{}",
                size_label(report.logical_bytes()),
                if report.incomplete {
                    "（统计不完整）"
                } else {
                    ""
                }
            ),
            scanned_label: format!("统计时间：{timestamp} · 包含已登记的外置图片"),
            categories,
            image_summary: format!(
                "已登记图片位置：{} 处；只统计已知资产，不扫描外置目录中的其他文件。",
                report.image_locations.len()
            ),
            image_locations: report
                .image_locations
                .iter()
                .map(|path| display_path(path))
                .collect(),
            issues: report
                .issues
                .iter()
                .map(|issue| StorageIssueView {
                    path: display_path(&issue.path),
                    message: issue.message.clone(),
                })
                .collect(),
            copies,
            copy_indices,
        }
    }

    pub fn copy_by_id(&self, note_id: &str) -> Option<&ManagedCopyView> {
        self.copy_indices.get(note_id).map(|&ix| &self.copies[ix])
    }

    /// Restore a viewport by stable note identity after refresh, using the
    /// actual filtered/sorted projection. A removed item falls to its nearest
    /// surviving neighbor; only an exact match keeps the intra-row offset.
    pub fn relocate_copy(
        &self,
        previous: &Self,
        previous_rows: &[usize],
        rows: &[usize],
        old_top: usize,
    ) -> Option<(usize, bool)> {
        let old_index = *previous_rows.get(old_top)?;
        let indices = rows
            .iter()
            .enumerate()
            .map(|(row, &ix)| (self.copies[ix].note_id.as_str(), row))
            .collect::<HashMap<_, _>>();
        if let Some(&next) = indices.get(previous.copies[old_index].note_id.as_str()) {
            return Some((next, true));
        }
        previous_rows[old_top + 1..]
            .iter()
            .chain(previous_rows[..old_top].iter().rev())
            .find_map(|&ix| indices.get(previous.copies[ix].note_id.as_str()).copied())
            .map(|next| (next, false))
    }
}

fn size_label(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.)
    } else {
        format!("{:.1} MiB", bytes as f64 / (1024. * 1024.))
    }
}

#[cfg(test)]
mod tests {
    use super::super::{ManagedCopy, StorageIssue};
    use super::*;

    fn report(count: usize) -> StorageReport {
        StorageReport {
            database_bytes: 1024,
            managed_bytes: 2048,
            scanned_at: 1_700_000_000,
            copies: (0..count)
                .map(|ix| ManagedCopy {
                    note_id: format!("copy-{ix}"),
                    title: format!("托管副本 {ix}"),
                    path: format!(r"\\?\C:\library\notes\copy-{ix}.md"),
                    bytes: 2048,
                    state: CopyState::Present,
                    deletable: true,
                    reason: None,
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn storage_copy_query_filters_and_sorts_without_reordering_report() {
        let mut source = report(3);
        source.copies[0].title = "Alpha".into();
        source.copies[0].bytes = 12;
        source.copies[1].title = "beta".into();
        source.copies[1].bytes = 9000;
        source.copies[2].title = "Alpine".into();
        source.copies[2].bytes = 100;
        let view = StoragePresentation::prepare(&source);
        assert_eq!(view.copy_rows(" AL ", CopyOrder::Largest), vec![2, 0]);
        assert_eq!(view.copy_rows("", CopyOrder::Name), vec![0, 2, 1]);
        assert_eq!(
            view.copy_rows("not present", CopyOrder::Largest),
            Vec::<usize>::new()
        );
        assert_eq!(view.copies[0].note_id, "copy-0");
    }

    #[test]
    fn storage_presentation_prepares_totals_and_display_paths() {
        let report = report(1);
        let view = StoragePresentation::prepare(&report);
        assert_eq!(view.logical_label, "已统计逻辑大小 3.0 KiB");
        assert!(view.scanned_label.starts_with("统计时间："));
        assert_eq!(view.categories.len(), 7);
        let copy = view.copy_by_id("copy-0").unwrap();
        assert_eq!(copy.path, r"C:\library\notes\copy-0.md");
        assert_eq!(copy.bytes, "2.0 KiB");
        assert_eq!(report.copies[0].path, r"\\?\C:\library\notes\copy-0.md");
    }

    #[test]
    fn storage_presentation_rows_scale_without_materializing_copies() {
        for count in [100, 1000, 10_000] {
            let view = StoragePresentation::prepare(&report(count));
            let rows = view.copy_rows("", CopyOrder::Largest);
            assert_eq!(rows.len(), count);
            assert_eq!(rows.iter().take(8).count(), 8);
            assert!(
                view.copy_by_id(&format!("copy-{}", count - 1))
                    .unwrap()
                    .deletable
            );
        }
    }

    #[test]
    fn storage_presentation_preserves_copy_identity_after_cleanup() {
        let mut report = report(2);
        report.copies[1].state = CopyState::Cleaned;
        report.copies[1].deletable = false;
        report.copies[1].bytes = 0;
        report.copies[1].reason = Some("副本已清理，历史与学习记录仍保留。".into());
        let view = StoragePresentation::prepare(&report);
        let cleaned = view.copy_by_id("copy-1").unwrap();
        assert_eq!(cleaned.state, CopyState::Cleaned);
        assert!(!cleaned.deletable);
        assert_eq!(cleaned.bytes, "0 B");
        assert!(cleaned.reason.as_ref().unwrap().contains("历史"));
        assert!(view.copy_rows("", CopyOrder::Largest).contains(&1));
    }

    #[test]
    fn storage_presentation_path_visibility_only_changes_path_rows() {
        let mut report = report(1);
        report.image_locations = vec![r"\\?\C:\images".into(), r"\\?\D:\images".into()];
        report.incomplete = true;
        report.issues.push(StorageIssue {
            path: r"\\?\D:\images".into(),
            message: "无法访问".into(),
        });
        let view = StoragePresentation::prepare(&report);
        assert!(view.logical_label.ends_with("（统计不完整）"));
        assert_eq!(view.copy_rows("", CopyOrder::Name), vec![0]);
        assert_eq!(view.issues[0].path, r"D:\images");
        assert_eq!(view.image_locations[0], r"C:\images");
    }

    #[test]
    fn storage_presentation_restores_sorted_anchor_by_note_identity() {
        let mut source = report(3);
        let previous = StoragePresentation::prepare(&source);
        let old_rows = previous.copy_rows("", CopyOrder::Name);
        source.copies.swap(0, 2);
        source.copies[0].bytes = 0;
        let next = StoragePresentation::prepare(&source);
        let rows = next.copy_rows("", CopyOrder::Largest);
        let (position, exact) = next.relocate_copy(&previous, &old_rows, &rows, 2).unwrap();
        assert!(exact);
        assert_eq!(next.copies[rows[position]].note_id, "copy-2");
    }

    #[test]
    fn copy_anchor_is_independent_of_overview_paths_and_resolved_issues() {
        let mut source = report(3);
        source.image_locations = vec!["C:/images".into()];
        source.incomplete = true;
        source.issues.push(StorageIssue {
            path: "D:/offline".into(),
            message: "暂不可用".into(),
        });
        let previous = StoragePresentation::prepare(&source);
        let old_rows = previous.copy_rows("", CopyOrder::Name);
        source.issues.clear();
        source.image_locations.clear();
        source.incomplete = false;
        let next = StoragePresentation::prepare(&source);
        let rows = next.copy_rows("", CopyOrder::Name);
        assert_eq!(
            next.relocate_copy(&previous, &old_rows, &rows, 1),
            Some((1, true))
        );
    }

    #[test]
    fn removed_copy_anchor_uses_next_surviving_identity_then_previous_or_empty_state() {
        let mut source = report(4);
        let previous = StoragePresentation::prepare(&source);
        let old_rows = previous.copy_rows("", CopyOrder::Name);
        source.copies.remove(1);
        let next = StoragePresentation::prepare(&source);
        let rows = next.copy_rows("", CopyOrder::Name);
        assert_eq!(
            next.relocate_copy(&previous, &old_rows, &rows, 1),
            Some((1, false))
        );
        assert_eq!(next.copies[rows[1]].note_id, "copy-2");
        source.copies.pop();
        let shorter = StoragePresentation::prepare(&source);
        let rows = shorter.copy_rows("", CopyOrder::Name);
        assert_eq!(
            shorter.relocate_copy(&previous, &old_rows, &rows, 3),
            Some((1, false))
        );
        source.copies.clear();
        let empty = StoragePresentation::prepare(&source);
        assert_eq!(empty.relocate_copy(&previous, &old_rows, &[], 3), None);
    }
}
