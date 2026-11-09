//! In-memory explorer built only from the guarded inventory's accepted paths.
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub directory: bool,
    pub bytes: u64,
    pub files: u64,
    pub children: Vec<usize>,
}
#[derive(Clone, Debug, Default)]
pub struct Tree {
    pub entries: Vec<Entry>,
    pub roots: Vec<usize>,
    lookup: HashMap<PathBuf, usize>,
}
impl Tree {
    fn ensure(&mut self, root: &Path, path: &Path, directory: bool) -> usize {
        if let Some(&index) = self.lookup.get(path) {
            return index;
        }
        let parent = path
            .parent()
            .filter(|_| path != root && path.starts_with(root))
            .map(|parent| self.ensure(root, parent, true));
        let index = self.entries.len();
        self.entries.push(Entry {
            path: path.into(),
            name: path
                .file_name()
                .unwrap_or(path.as_os_str())
                .to_string_lossy()
                .into_owned(),
            directory,
            bytes: 0,
            files: 0,
            children: Vec::new(),
        });
        self.lookup.insert(path.into(), index);
        if let Some(parent) = parent {
            self.entries[parent].children.push(index);
        } else {
            self.roots.push(index);
        }
        index
    }
    pub fn record(&mut self, root: &Path, path: &Path, directory: bool, bytes: u64) {
        let index = self.ensure(root, path, directory);
        if directory {
            return;
        }
        self.entries[index].bytes = bytes;
        self.entries[index].files = 1;
        let mut parent = path.parent();
        while let Some(path) = parent.filter(|path| path.starts_with(root)) {
            if let Some(&index) = self.lookup.get(path) {
                let entry = &mut self.entries[index];
                entry.bytes = entry.bytes.saturating_add(bytes);
                entry.files += 1;
            }
            if path == root {
                break;
            }
            parent = path.parent();
        }
    }
    pub fn finish(&mut self) {
        let entries = &self.entries;
        let compare = |a: &usize, b: &usize| {
            entries[*b]
                .directory
                .cmp(&entries[*a].directory)
                .then_with(|| {
                    entries[*a]
                        .name
                        .to_lowercase()
                        .cmp(&entries[*b].name.to_lowercase())
                })
                .then_with(|| entries[*a].path.cmp(&entries[*b].path))
        };
        let mut children = self
            .entries
            .iter()
            .map(|entry| entry.children.clone())
            .collect::<Vec<_>>();
        for row in &mut children {
            row.sort_by(compare);
        }
        self.roots.sort_by(compare);
        for (entry, children) in self.entries.iter_mut().zip(children) {
            entry.children = children;
        }
        self.lookup = HashMap::new();
    }
    pub fn append(&mut self, source: &Tree, label: &str) {
        for entry in &source.entries {
            if self.lookup.contains_key(&entry.path) {
                continue;
            }
            let index = self.entries.len();
            let mut entry = entry.clone();
            entry.children.clear();
            if source
                .roots
                .iter()
                .any(|&root| source.entries[root].path == entry.path)
            {
                entry.name = format!("{label} · {}", entry.name);
            }
            self.lookup.insert(entry.path.clone(), index);
            self.entries.push(entry);
        }
    }
    pub fn aggregate(&mut self) {
        self.roots.clear();
        for index in 0..self.entries.len() {
            let parent = self.entries[index]
                .path
                .parent()
                .and_then(|path| self.lookup.get(path))
                .copied()
                .filter(|&parent| self.entries[parent].directory);
            if let Some(parent) = parent {
                self.entries[parent].children.push(index);
            } else {
                self.roots.push(index);
            }
        }
        fn sum(tree: &mut Tree, index: usize) -> (u64, u64) {
            if tree.entries[index].directory {
                let (mut bytes, mut files) = (0u64, 0u64);
                for child in tree.entries[index].children.clone() {
                    let (b, f) = sum(tree, child);
                    bytes = bytes.saturating_add(b);
                    files = files.saturating_add(f);
                }
                tree.entries[index].bytes = bytes;
                tree.entries[index].files = files;
            }
            (tree.entries[index].bytes, tree.entries[index].files)
        }
        for root in self.roots.clone() {
            sum(self, root);
        }
        self.finish();
    }
    /// Work is proportional to visible rows; collapsed subtrees are not walked.
    pub fn visible(&self, expanded: &HashSet<PathBuf>) -> Vec<(usize, usize)> {
        let mut result = Vec::new();
        let mut pending = self
            .roots
            .iter()
            .rev()
            .map(|&index| (index, 0))
            .collect::<Vec<_>>();
        while let Some((index, depth)) = pending.pop() {
            result.push((index, depth));
            let entry = &self.entries[index];
            if entry.directory && expanded.contains(&entry.path) {
                pending.extend(entry.children.iter().rev().map(|&index| (index, depth + 1)));
            }
        }
        result
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn folders_aggregate_only_recorded_members_and_expand_one_level() {
        let root = Path::new("/owned");
        let mut tree = Tree::default();
        tree.record(root, Path::new("/owned/a/note.md"), false, 20);
        tree.record(root, Path::new("/owned/a/image.png"), false, 30);
        tree.record(root, Path::new("/owned/empty"), true, 0);
        tree.finish();
        let node = &tree.entries[tree.roots[0]];
        assert_eq!((node.bytes, node.files), (50, 2));
        let mut expanded = HashSet::new();
        assert_eq!(tree.visible(&expanded).len(), 1);
        expanded.insert(root.to_path_buf());
        assert_eq!(tree.visible(&expanded).len(), 3);
        expanded.insert(root.join("a"));
        assert_eq!(tree.visible(&expanded).len(), 5);
        assert!(
            tree.entries
                .iter()
                .any(|entry| entry.path == root.join("empty") && entry.bytes == 0)
        );
    }
    #[test]
    fn unrelated_registered_files_do_not_gain_unscanned_siblings() {
        let mut tree = Tree::default();
        tree.record(
            Path::new("/registry"),
            Path::new("/elsewhere/only.png"),
            false,
            7,
        );
        tree.finish();
        assert_eq!(tree.entries.len(), 1);
        assert_eq!(tree.entries[0].bytes, 7);
        let mut combined = Tree::default();
        combined.append(&tree, "已登记图片");
        combined.aggregate();
        assert_eq!(combined.visible(&HashSet::new()).len(), 1);
    }
}

#[cfg(test)]
mod aggregate_tests {
    use super::*;
    #[test]
    fn individual_owned_files_merge_into_collapsed_folders_without_counting_siblings() {
        let root = Path::new("/install");
        let mut combined = Tree::default();
        for name in ["app.exe", "receipt.json"] {
            let mut tree = Tree::default();
            tree.record(root, &root.join(name), false, 7);
            tree.finish();
            combined.append(&tree, "程序文件");
        }
        combined.aggregate();
        assert_eq!(combined.roots.len(), 1);
        let entry = &combined.entries[combined.roots[0]];
        assert_eq!(entry.path, root);
        assert_eq!((entry.bytes, entry.files), (14, 2));
        assert_eq!(combined.visible(&HashSet::new()).len(), 1);
    }
    #[test]
    fn collapsed_large_trees_walk_only_the_requested_levels() {
        let root = Path::new("/library");
        let mut source = Tree::default();
        for index in 0..10000 {
            source.record(
                root,
                &root.join(format!("folder-{}/file-{index}", index % 100)),
                false,
                10,
            );
        }
        source.finish();
        let mut tree = Tree::default();
        tree.append(&source, "资料库");
        tree.aggregate();
        assert_eq!(tree.entries[tree.roots[0]].bytes, 100000);
        let mut expanded = HashSet::new();
        assert_eq!(tree.visible(&expanded).len(), 1);
        expanded.insert(root.to_path_buf());
        assert_eq!(tree.visible(&expanded).len(), 101);
        expanded.insert(root.join("folder-0"));
        assert_eq!(tree.visible(&expanded).len(), 201);
    }
}
