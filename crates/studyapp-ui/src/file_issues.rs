//! Directory-first failure lists. Error details are only mounted as hover hints.
use crate::{components::*, controls::Button};
use gpui_kit::{
    component::{Sizable, button::ButtonVariants},
    *,
};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileIssueGroup {
    pub directory: String,
    pub files: Vec<(String, String)>,
}
pub fn group_file_issues(
    issues: impl IntoIterator<Item = (String, String)>,
) -> Vec<FileIssueGroup> {
    let mut groups: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for (path, message) in issues {
        let normalized = path.replace(char::from(92), "/");
        let (directory, name) = normalized
            .rsplit_once('/')
            .unwrap_or(("其它位置", &normalized));
        let directory = if directory.len() == 2 && directory.ends_with(':') {
            format!("{directory}/")
        } else {
            directory.to_owned()
        };
        groups
            .entry(directory)
            .or_default()
            .push((name.to_owned(), message));
    }
    groups
        .into_iter()
        .map(|(directory, mut files)| {
            files.sort();
            files.dedup();
            FileIssueGroup { directory, files }
        })
        .collect()
}
pub fn file_issue_list(id: &'static str, groups: &[FileIssueGroup]) -> Div {
    column()
        .gap_3()
        .children(groups.iter().enumerate().map(|(index, group)| {
            column()
                .gap_1()
                .child(
                    muted(format!(
                        "{} · {} 项未统计",
                        group.directory,
                        group.files.len()
                    ))
                    .text_sm(),
                )
                .children(
                    group
                        .files
                        .iter()
                        .enumerate()
                        .map(|(file, (name, message))| {
                            Button::new(SharedString::from(format!("{id}-{index}-{file}")))
                                .small()
                                .ghost()
                                .label(name.clone())
                                .tooltip(message.clone())
                                .justify_start()
                                .w_full()
                        }),
                )
        }))
}
#[cfg(test)]
mod tests {
    use super::group_file_issues;
    #[test]
    fn failures_group_by_full_directory_and_keep_details_out_of_labels() {
        let groups = group_file_issues([
            (r"C:\one\same.md".into(), "permission denied".into()),
            (r"C:\two\same.md".into(), "locked".into()),
            (r"C:\one\other.md".into(), "missing".into()),
        ]);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].files.len(), 2);
        assert_eq!(groups[1].files[0].0, "same.md");
        assert_eq!(groups[1].files[0].1, "locked");
    }
}
