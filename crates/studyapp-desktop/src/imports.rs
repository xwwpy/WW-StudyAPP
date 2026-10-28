use std::{collections::HashSet, path::PathBuf};

pub struct ImportSelection {
    pub files: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

/// Bounded traversal of user-selected paths. Links are never followed.
pub fn collect(paths: Vec<PathBuf>) -> ImportSelection {
    let mut output = ImportSelection {
        files: vec![],
        warnings: vec![],
    };
    let mut stack: Vec<_> = paths.into_iter().rev().map(|path| (path, 0)).collect();
    let mut seen = HashSet::new();
    let mut inspected = 0;
    while let Some((path, depth)) = stack.pop() {
        inspected += 1;
        if inspected > 10_000 || output.files.len() >= 200 {
            output
                .warnings
                .push("本次最多检查 10,000 个目录项、导入 200 份笔记，其余请分批导入".into());
            break;
        }
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => {
                output
                    .warnings
                    .push(format!("无法读取：{}", path.display()));
                continue;
            }
        };
        let is_link = metadata.file_type().is_symlink();
        #[cfg(windows)]
        let is_link = {
            use std::os::windows::fs::MetadataExt;
            is_link || metadata.file_attributes() & 0x400 != 0
        };
        if is_link {
            output
                .warnings
                .push(format!("跳过链接：{}", path.display()));
            continue;
        }
        if metadata.is_dir() {
            if depth >= 8 {
                output
                    .warnings
                    .push(format!("目录超过 8 层，已跳过：{}", path.display()));
                continue;
            }
            match std::fs::read_dir(&path) {
                Ok(entries) => {
                    let mut children = Vec::new();
                    for entry in entries.take(10_001) {
                        match entry {
                            Ok(entry) => children.push(entry.path()),
                            Err(_) => output
                                .warnings
                                .push(format!("目录中有无法读取的项目：{}", path.display())),
                        }
                    }
                    children.sort();
                    stack.extend(children.into_iter().rev().map(|path| (path, depth + 1)));
                }
                Err(_) => output
                    .warnings
                    .push(format!("无法读取目录：{}", path.display())),
            }
        } else if metadata.is_file()
            && path
                .extension()
                .and_then(|p| p.to_str())
                .is_some_and(|p| p.eq_ignore_ascii_case("md") || p.eq_ignore_ascii_case("markdown"))
        {
            match std::fs::canonicalize(&path) {
                Ok(canonical) => {
                    let key = canonical.to_string_lossy().to_string();
                    let key = if cfg!(windows) {
                        key.to_lowercase()
                    } else {
                        key
                    };
                    if seen.insert(key) {
                        output.files.push(canonical);
                    }
                }
                Err(_) => output
                    .warnings
                    .push(format!("无法定位文件：{}", path.display())),
            }
        } else if depth == 0 {
            output
                .warnings
                .push(format!("仅支持 Markdown 文件或目录：{}", path.display()));
        }
    }
    output.warnings.truncate(20);
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_drop_recurses_deduplicates_and_reports_invalid_top_level_paths() {
        let root = tempfile::tempdir().unwrap().keep();
        let nested = root.join("nested");
        std::fs::create_dir(&nested).unwrap();
        let note = nested.join("笔记.MD");
        std::fs::write(&note, "# study").unwrap();
        let unsupported = root.join("image.png");
        std::fs::write(&unsupported, "fixture").unwrap();
        let result = collect(vec![root, note, unsupported]);
        assert_eq!(result.files.len(), 1);
        assert!(result.warnings.iter().any(|w| w.contains("仅支持")));
    }

    #[test]
    fn large_directory_is_bounded_without_deleting_any_sources() {
        let root = tempfile::tempdir().unwrap().keep();
        for i in 0..205 {
            std::fs::write(root.join(format!("{i:03}.md")), "fixture").unwrap();
        }
        let result = collect(vec![root.clone()]);
        assert_eq!(result.files.len(), 200);
        assert!(!result.warnings.is_empty());
        assert_eq!(std::fs::read_dir(root).unwrap().count(), 205);
    }
}
