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
/// Each directory retains its full location as identity; equal basenames under
/// different drives/parents cannot share expansion state or failure totals.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileIssueDirectory {
    pub path: String,
    pub name: String,
    pub count: usize,
    pub directories: BTreeMap<String, FileIssueDirectory>,
    pub files: Vec<(String, String)>,
}

pub fn issue_tree(groups: &[FileIssueGroup]) -> Vec<FileIssueDirectory> {
    let mut roots: BTreeMap<String, FileIssueDirectory> = BTreeMap::new();
    for group in groups {
        let directory = group.directory.replace(char::from(92), "/");
        let parts: Vec<_> = directory
            .split('/')
            .filter(|part| !part.is_empty())
            .collect();
        let (root, rest) = if directory.starts_with("//") && parts.len() >= 2 {
            (format!("//{}/{}", parts[0], parts[1]), &parts[2..])
        } else if directory.starts_with('/') {
            ("/".to_owned(), parts.as_slice())
        } else if parts.first().is_some_and(|part| part.ends_with(':')) {
            (format!("{}/", parts[0]), &parts[1..])
        } else {
            (
                parts.first().copied().unwrap_or("其它位置").to_owned(),
                parts.get(1..).unwrap_or_default(),
            )
        };
        let mut node = roots
            .entry(root.clone())
            .or_insert_with(|| FileIssueDirectory {
                path: root.clone(),
                name: root,
                ..Default::default()
            });
        node.count += group.files.len();
        for part in rest {
            let path = format!(
                "{}{part}",
                if node.path.ends_with('/') {
                    node.path.clone()
                } else {
                    format!("{}/", node.path)
                }
            );
            node = node
                .directories
                .entry(path.clone())
                .or_insert_with(|| FileIssueDirectory {
                    path,
                    name: (*part).into(),
                    ..Default::default()
                });
            node.count += group.files.len();
        }
        node.files.extend(group.files.iter().cloned());
        node.files.sort();
    }
    roots.into_values().collect()
}

#[derive(IntoElement)]
pub struct FileIssueTree {
    id: &'static str,
    roots: Vec<FileIssueDirectory>,
}
pub fn file_issue_list(id: &'static str, groups: &[FileIssueGroup]) -> FileIssueTree {
    FileIssueTree {
        id,
        roots: issue_tree(groups),
    }
}
fn directory_view(
    id: &'static str,
    directory: FileIssueDirectory,
    collapsed: &Entity<std::collections::HashSet<String>>,
    cx: &App,
) -> Div {
    let closed = collapsed.read(cx).contains(&directory.path);
    let state = collapsed.clone();
    let path = directory.path.clone();
    let mut result = column().gap_1().child(
        Button::new(SharedString::from(format!(
            "{id}-directory-{}",
            directory.path
        )))
        .small()
        .ghost()
        .justify_start()
        .w_full()
        .label(format!(
            "{} {} · {} 项未统计",
            if closed { "▸" } else { "▾" },
            directory.name,
            directory.count
        ))
        .tooltip(directory.path.clone())
        .on_click(move |_, window, cx| {
            state.update(cx, |closed, cx| {
                if !closed.remove(&path) {
                    closed.insert(path.clone());
                }
                cx.notify();
            });
            window.refresh();
        }),
    );
    if !closed {
        result = result.child(
            column()
                .pl_4()
                .gap_1()
                .children(
                    directory
                        .directories
                        .into_values()
                        .map(|child| directory_view(id, child, collapsed, cx)),
                )
                .children(directory.files.into_iter().enumerate().map(
                    |(index, (name, reason))| {
                        Button::new(SharedString::from(format!(
                            "{id}-file-{}-{index}",
                            directory.path
                        )))
                        .small()
                        .ghost()
                        .label(name)
                        .tooltip(reason)
                        .justify_start()
                        .w_full()
                    },
                )),
        );
    }
    result
}
impl RenderOnce for FileIssueTree {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(
            SharedString::from(format!("{}-collapsed-directories", self.id)),
            cx,
            |_, _| std::collections::HashSet::<String>::new(),
        );
        column().gap_2().children(
            self.roots
                .into_iter()
                .map(|directory| directory_view(self.id, directory, &state, cx)),
        )
    }
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

#[cfg(test)]
mod tree_tests {
    use super::{group_file_issues, issue_tree};
    #[test]
    fn aggregates_each_level_and_keeps_equal_names_separate() {
        let groups = group_file_issues([
            ("C:/one/shared/a.md".into(), "locked".into()),
            ("C:/one/shared/deeper/b.md".into(), "missing".into()),
            ("C:/two/shared/a.md".into(), "denied".into()),
            ("D:/one/shared/a.md".into(), "busy".into()),
        ]);
        let roots = issue_tree(&groups);
        assert_eq!(roots.len(), 2);
        assert_eq!(roots[0].path, "C:/");
        assert_eq!(roots[0].count, 3);
        let one = &roots[0].directories["C:/one"];
        assert_eq!(one.count, 2);
        let shared = &one.directories["C:/one/shared"];
        assert_eq!(shared.count, 2);
        assert_eq!(shared.files[0], ("a.md".into(), "locked".into()));
        assert_eq!(shared.directories["C:/one/shared/deeper"].count, 1);
        assert_eq!(roots[0].directories["C:/two"].count, 1);
    }
    #[test]
    fn unix_unc_and_relative_locations_have_distinct_roots() {
        let roots = issue_tree(&group_file_issues([
            ("/home/notes/a.md".into(), "error".into()),
            ("//server/share/notes/b.md".into(), "error".into()),
            ("relative/notes/c.md".into(), "error".into()),
        ]));
        assert_eq!(
            roots
                .iter()
                .map(|root| root.path.as_str())
                .collect::<Vec<_>>(),
            ["/", "//server/share", "relative"]
        );
        assert!(roots.iter().all(|root| root.count == 1));
    }
}
