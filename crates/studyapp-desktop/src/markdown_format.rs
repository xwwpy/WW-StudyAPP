use std::ops::Range;

#[derive(Clone, Copy, Debug)]
pub enum MarkdownFormat {
    Heading,
    Bold,
    Italic,
    List,
    Quote,
    Code,
    Table,
}

#[derive(Debug)]
pub struct FormattingEdit {
    pub range: Range<usize>,
    pub replacement: String,
    /// Absolute byte range in the document after applying this edit.
    pub selection: Range<usize>,
}

pub fn format_markdown(
    source: &str,
    selection: Range<usize>,
    kind: MarkdownFormat,
) -> Option<FormattingEdit> {
    let selected = source.get(selection.clone())?;
    let newline = if source.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    match kind {
        MarkdownFormat::Bold | MarkdownFormat::Italic => {
            let marker = if matches!(kind, MarkdownFormat::Bold) {
                "**"
            } else {
                "*"
            };
            let interior = if selected.is_empty() {
                "文字"
            } else {
                selected
            };
            let start = selection.start + marker.len();
            Some(FormattingEdit {
                range: selection,
                replacement: format!("{marker}{interior}{marker}"),
                selection: start..start + interior.len(),
            })
        }
        MarkdownFormat::Heading | MarkdownFormat::List | MarkdownFormat::Quote => {
            let start = source[..selection.start]
                .rfind('\n')
                .map_or(0, |pos| pos + 1);
            // A selection ending exactly at a line boundary excludes that next line.
            let anchor =
                if selection.end > selection.start && source[..selection.end].ends_with('\n') {
                    selection.end - 1
                } else {
                    selection.end
                };
            let mut end = source[anchor..]
                .find('\n')
                .map_or(source.len(), |pos| anchor + pos);
            if end > start && source.as_bytes()[end - 1] == b'\r' {
                end -= 1;
            }
            let mut replacement = String::new();
            for line in source[start..end].split_inclusive('\n') {
                let (body, ending) = if let Some(body) = line.strip_suffix("\r\n") {
                    (body, "\r\n")
                } else if let Some(body) = line.strip_suffix('\n') {
                    (body, "\n")
                } else {
                    (line, "")
                };
                let (prefix, body) = match kind {
                    MarkdownFormat::Heading => {
                        let hashes = body.bytes().take_while(|&byte| byte == b'#').count();
                        let body = if (1..=6).contains(&hashes)
                            && body.as_bytes().get(hashes) == Some(&b' ')
                        {
                            &body[hashes + 1..]
                        } else {
                            body
                        };
                        ("# ", body)
                    }
                    MarkdownFormat::List => ("- ", body.strip_prefix("- ").unwrap_or(body)),
                    _ => ("> ", body.strip_prefix("> ").unwrap_or(body)),
                };
                replacement.push_str(prefix);
                replacement.push_str(body);
                replacement.push_str(ending);
            }
            if replacement.is_empty() {
                replacement.push_str(match kind {
                    MarkdownFormat::Heading => "# ",
                    MarkdownFormat::List => "- ",
                    _ => "> ",
                });
            }
            let selection = start..start + replacement.len();
            Some(FormattingEdit {
                range: start..end,
                replacement,
                selection,
            })
        }
        MarkdownFormat::Code | MarkdownFormat::Table => {
            let before = block_separator(&source[..selection.start], newline, true);
            let after = block_separator(&source[selection.end..], newline, false);
            let (body, interior) = if matches!(kind, MarkdownFormat::Code) {
                let interior = if selected.is_empty() {
                    "代码"
                } else {
                    selected
                };
                let longest = interior
                    .split(|c| c != '`')
                    .map(str::len)
                    .max()
                    .unwrap_or(0);
                let fence = "`".repeat((longest + 1).max(3));
                let closing_newline = if interior.ends_with('\n') {
                    ""
                } else {
                    newline
                };
                let body = format!("{fence}{newline}{interior}{closing_newline}{fence}");
                let start = before.len() + fence.len() + newline.len();
                (body, start..start + interior.len())
            } else {
                let body = ["| 项目 | 内容 |", "|---|---|", "| 文字 | 文字 |"].join(newline);
                let selection = before.len()..before.len() + body.len();
                (body, selection)
            };
            let start = selection.start + interior.start;
            let end = selection.start + interior.end;
            Some(FormattingEdit {
                range: selection,
                replacement: format!("{before}{body}{after}"),
                selection: start..end,
            })
        }
    }
}

/// Supply only missing line breaks; do not rewrite neighboring source bytes.
fn block_separator(neighbor: &str, newline: &str, preceding: bool) -> String {
    if neighbor.is_empty() {
        return String::new();
    }
    let count = if preceding {
        neighbor
            .chars()
            .rev()
            .filter(|&c| c != '\r')
            .take_while(|&c| c == '\n')
            .count()
    } else {
        neighbor
            .chars()
            .filter(|&c| c != '\r')
            .take_while(|&c| c == '\n')
            .count()
    };
    newline.repeat(2usize.saturating_sub(count))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn apply(
        source: &str,
        selection: Range<usize>,
        kind: MarkdownFormat,
    ) -> (String, Range<usize>) {
        let edit = format_markdown(source, selection, kind).unwrap();
        let mut result = source.to_owned();
        result.replace_range(edit.range, &edit.replacement);
        assert!(result.get(edit.selection.clone()).is_some());
        (result, edit.selection)
    }
    #[test]
    fn inline_formats_preserve_unicode_and_select_interior() {
        let (text, selection) = apply("中😀文", 3..7, MarkdownFormat::Bold);
        assert_eq!(text, "中**😀**文");
        assert_eq!(&text[selection], "😀");
        let (text, selection) = apply("", 0..0, MarkdownFormat::Italic);
        assert_eq!(&text[selection], "文字");
        assert_eq!(text, "*文字*");
        assert!(format_markdown("中", 1..2, MarkdownFormat::Bold).is_none());
        assert!(format_markdown("abc", Range { start: 3, end: 2 }, MarkdownFormat::List).is_none());
    }
    #[test]
    fn prefixes_apply_to_complete_selected_lines_preserving_crlf() {
        let (text, _) = apply(
            "before\r\n中文 line\r\nsecond\r\nafter",
            11..25,
            MarkdownFormat::Quote,
        );
        assert_eq!(text, "before\r\n> 中文 line\r\n> second\r\nafter");
        let (text, _) = apply("alpha\nbeta\ngamma", 2..6, MarkdownFormat::Heading);
        assert_eq!(text, "# alpha\nbeta\ngamma");
        let (text, _) = apply("## existing\nnext", 5..5, MarkdownFormat::Heading);
        assert_eq!(text, "# existing\nnext");
        let (text, _) = apply("alpha\nbeta\ngamma", 1..8, MarkdownFormat::List);
        assert_eq!(text, "- alpha\n- beta\ngamma");
    }
    #[test]
    fn code_uses_fence_longer_than_embedded_backticks_and_separates_neighbors() {
        let source = "before SELECT after";
        let (text, selection) = apply(source, 7..13, MarkdownFormat::Code);
        assert_eq!(text, "before \n\n```\nSELECT\n```\n\n after");
        assert_eq!(&text[selection], "SELECT");
        let (text, selection) = apply("a```b\r\n", 0..7, MarkdownFormat::Code);
        assert!(text.starts_with("````\r\n"));
        assert!(text.contains("\r\n````"));
        assert_eq!(&text[selection], "a```b\r\n");
    }
    #[test]
    fn tables_are_separate_blocks_at_a_midline_cursor() {
        let (text, selection) = apply("beforeafter", 6..6, MarkdownFormat::Table);
        assert!(text.starts_with("before\n\n|"));
        assert!(text.ends_with("\n\nafter"));
        assert!(text[selection].contains("|---|"));
        let (text, _) = apply("a\r\nb", 4..4, MarkdownFormat::Table);
        assert!(!text.replace("\r\n", "").contains('\n'));
    }
}
