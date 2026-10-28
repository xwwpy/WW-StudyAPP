use std::{collections::VecDeque, ops::Range};

use pulldown_cmark::{Event, Options, Parser};

#[derive(Debug)]
pub struct MarkdownBlock {
    pub range: Range<usize>,
    pub html: String,
}

/// Parse once for the whole document so reference definitions resolve globally.
/// Gaps (including definitions and blank lines) belong to the preceding block;
/// any leading gap belongs to the first block. Source is never serialized back.
pub fn markdown_blocks(source: &str) -> Vec<MarkdownBlock> {
    let parser = Parser::new_ext(source, markdown_options()).into_offset_iter();
    let mut groups = Vec::new();
    let mut events = Vec::new();
    let mut start = 0;
    let mut depth = 0usize;
    for (event, range) in parser {
        if events.is_empty() {
            start = range.start;
        }
        match &event {
            Event::Start(_) => depth += 1,
            Event::End(_) => depth = depth.saturating_sub(1),
            _ => {}
        }
        events.push(event);
        if depth == 0 {
            groups.push((
                start,
                crate::presentation::safe_markdown_events(events.drain(..)),
            ));
        }
    }
    if groups.is_empty() {
        return if source.is_empty() {
            vec![]
        } else {
            vec![MarkdownBlock {
                range: 0..source.len(),
                html: String::new(),
            }]
        };
    }
    let mut blocks = Vec::with_capacity(groups.len());
    for (index, (start, html)) in groups.iter().enumerate() {
        let end = groups.get(index + 1).map_or(source.len(), |next| next.0);
        blocks.push(MarkdownBlock {
            range: (if index == 0 { 0 } else { *start })..end,
            html: html.clone(),
        });
    }
    blocks
}

pub(crate) fn markdown_options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS
}

const MAX_HISTORY_ENTRIES: usize = 100;
const MAX_HISTORY_BYTES: usize = 4 * 1024 * 1024;

/// Canonical Markdown bytes plus snapshot history grouped by `checkpoint`.
/// History retains at most 100 snapshots and 4 MiB across both stacks. Text
/// itself is never truncated; documents above that budget disable history.
pub struct MarkdownBuffer {
    text: String,
    undo: VecDeque<String>,
    redo: VecDeque<String>,
    editing: bool,
}
impl MarkdownBuffer {
    pub fn new(text: String) -> Self {
        Self {
            text,
            undo: VecDeque::new(),
            redo: VecDeque::new(),
            editing: false,
        }
    }
    pub fn text(&self) -> &str {
        &self.text
    }
    /// Returns false for invalid UTF-8 boundaries, reversed ranges, or no change.
    pub fn replace(&mut self, range: Range<usize>, text: &str) -> bool {
        let Some(previous) = self.text.get(range.clone()) else {
            return false;
        };
        if previous == text {
            return false;
        }
        if !self.editing {
            self.undo.push_back(self.text.clone());
        }
        self.redo.clear();
        self.text.replace_range(range, text);
        self.editing = true;
        self.bound_history();
        true
    }
    pub fn checkpoint(&mut self) {
        self.editing = false;
    }
    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop_back() else {
            return false;
        };
        self.redo
            .push_back(std::mem::replace(&mut self.text, previous));
        self.editing = false;
        self.bound_history();
        true
    }
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop_back() else {
            return false;
        };
        self.undo.push_back(std::mem::replace(&mut self.text, next));
        self.editing = false;
        self.bound_history();
        true
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    fn bound_history(&mut self) {
        if self.text.len() > MAX_HISTORY_BYTES {
            self.undo.clear();
            self.redo.clear();
            return;
        }
        let mut bytes: usize = self
            .undo
            .iter()
            .chain(self.redo.iter())
            .map(String::len)
            .sum();
        while self.undo.len() + self.redo.len() > MAX_HISTORY_ENTRIES || bytes > MAX_HISTORY_BYTES {
            let removed = self.undo.pop_front().or_else(|| self.redo.pop_front());
            let Some(removed) = removed else {
                break;
            };
            bytes -= removed.len();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn markdown_partitions_exact_bytes_and_keeps_top_level_constructs() {
        for source in [
            "",
            " \r\n\r\n",
            "\r\n# 中文😀\r\n\r\n段落\r\n",
            "- outer\n  - inner\n\n    nested\n\nnext\n",
            "```rust\n# inside\n\nstill code\n```\n\nafter",
            "```\nunclosed\n\n# still code",
            "| a | b |\n|---|---|\n| 1 | 2 |\n\nafter",
            "[ref]: https://example.org\n\n[visible][ref]\n\n[other]: /target\n",
        ] {
            let blocks = markdown_blocks(source);
            let mut end = 0;
            for block in &blocks {
                assert_eq!(block.range.start, end);
                assert!(source.get(block.range.clone()).is_some());
                end = block.range.end;
            }
            assert_eq!(end, source.len(), "{source:?}");
            assert_eq!(
                blocks
                    .iter()
                    .map(|b| &source[b.range.clone()])
                    .collect::<String>(),
                source
            );
        }
        assert_eq!(markdown_blocks("- outer\n  - inner\n\nnext").len(), 2);
        assert_eq!(markdown_blocks("```\na\n\nb\n```").len(), 1);
    }
    #[test]
    fn markdown_rendering_resolves_references_and_sanitizes_resources() {
        let source = "[visible][ref]\n\n![alt](https://tracker.invalid)\n\n<script>bad()</script>\n\n[ref]: javascript:bad\n";
        let html = markdown_blocks(source)
            .iter()
            .map(|b| b.html.as_str())
            .collect::<String>();
        assert!(html.contains("<p>visible</p>"), "{html}");
        assert!(!html.contains("[ref]"));
        assert!(!html.contains("tracker"));
        assert!(!html.contains("<script"));
        assert!(!html.contains("javascript:"));
        assert_eq!(html, crate::presentation::safe_markdown_html(source));
    }
    #[test]
    fn markdown_buffer_groups_edits_and_validates_unicode_ranges() {
        let mut b = MarkdownBuffer::new("中😀\r\ntext".into());
        assert!(!b.replace(1..3, "bad"));
        assert!(!b.replace(99..99, "bad"));
        assert!(!b.replace(Range { start: 7, end: 3 }, "bad"));
        assert!(!b.can_undo());
        assert!(!b.can_redo());
        assert!(b.replace(0..3, "中文"));
        assert!(b.replace(0..6, "字"));
        assert_eq!(b.text(), "字😀\r\ntext");
        b.checkpoint();
        assert!(b.replace(3..7, "!"));
        assert!(b.undo());
        assert_eq!(b.text(), "字😀\r\ntext");
        assert!(b.undo());
        assert_eq!(b.text(), "中😀\r\ntext");
        assert!(!b.undo());
        assert!(b.redo());
        assert!(b.redo());
        assert_eq!(b.text(), "字!\r\ntext");
        assert!(!b.redo());
        assert!(b.undo());
        assert!(b.replace(0..3, "新"));
        assert!(!b.can_redo());
    }
    #[test]
    fn markdown_history_is_bounded_and_noop_does_not_create_history() {
        let mut b = MarkdownBuffer::new(String::new());
        assert!(!b.replace(0..0, ""));
        assert!(!b.undo());
        for _ in 0..300 {
            b.checkpoint();
            assert!(b.replace(b.text().len()..b.text().len(), "a"));
        }
        let mut count = 0;
        while b.undo() {
            count += 1;
        }
        assert!(count > 0 && count <= 100);
        let large = "x".repeat(5 * 1024 * 1024);
        b.checkpoint();
        assert!(b.replace(0..b.text().len(), &large));
        assert!(!b.can_undo());
        assert!(!b.can_redo());
    }
    #[test]
    fn markdown_replacement_repartitions_changed_block_lengths_without_losing_neighbors() {
        let mut buffer = MarkdownBuffer::new("# 标题\r\n\r\nold\r\n\r\nlast😀\r\n".into());
        let blocks = markdown_blocks(buffer.text());
        assert_eq!(blocks.len(), 3);
        assert!(buffer.replace(
            blocks[1].range.clone(),
            "a much longer 中文 paragraph\r\n\r\n"
        ));
        let blocks = markdown_blocks(buffer.text());
        assert_eq!(&buffer.text()[blocks[2].range.clone()], "last😀\r\n");
        buffer.checkpoint();
        assert!(buffer.replace(blocks[1].range.clone(), "短\r\n\r\n"));
        assert_eq!(buffer.text(), "# 标题\r\n\r\n短\r\n\r\nlast😀\r\n");
        assert!(buffer.undo());
        assert!(buffer.text().contains("a much longer 中文 paragraph"));
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "# 标题\r\n\r\nold\r\n\r\nlast😀\r\n");
    }

    #[test]
    fn markdown_history_budget_applies_to_combined_undo_and_redo() {
        let mut buffer = MarkdownBuffer::new("x".repeat(1024 * 1024));
        for _ in 0..12 {
            buffer.checkpoint();
            assert!(buffer.replace(
                0..1,
                if buffer.text().starts_with('x') {
                    "y"
                } else {
                    "x"
                }
            ));
        }
        assert!(buffer.undo());
        let retained: usize = buffer
            .undo
            .iter()
            .chain(buffer.redo.iter())
            .map(String::len)
            .sum();
        assert!(retained <= MAX_HISTORY_BYTES);
        assert!(buffer.can_redo());
        let mut count = 0;
        while buffer.undo() {
            count += 1;
        }
        assert!(count <= 3);
    }
}
