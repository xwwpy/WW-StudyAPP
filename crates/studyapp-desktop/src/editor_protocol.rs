//! Revisioned transactions between the offline editor and native persistence.
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub struct RebaseEdit {
    pub from: usize,
    pub to: usize,
    pub insert: String,
}

/// Character edits preserve unaffected selections and undo ranges when image
/// ownership changes URLs during Save. Offsets are CodeMirror's UTF-16 units.
pub fn rebase_edits(before: &str, after: &str) -> Vec<RebaseEdit> {
    let before = normalize(before);
    let after = normalize(after);
    if before == after {
        return Vec::new();
    }
    let diff = similar::TextDiff::configure()
        .timeout(std::time::Duration::from_millis(250))
        .diff_chars(&before, &after);
    let mut edits = Vec::new();
    let mut current: Option<RebaseEdit> = None;
    let mut position = 0;
    for change in diff.iter_all_changes() {
        match change.tag() {
            similar::ChangeTag::Equal => {
                if let Some(edit) = current.take() {
                    edits.push(edit);
                }
                position += change.value().encode_utf16().count();
            }
            similar::ChangeTag::Delete => {
                let edit = current.get_or_insert_with(|| RebaseEdit {
                    from: position,
                    to: position,
                    insert: String::new(),
                });
                position += change.value().encode_utf16().count();
                edit.to = position;
            }
            similar::ChangeTag::Insert => {
                current
                    .get_or_insert_with(|| RebaseEdit {
                        from: position,
                        to: position,
                        insert: String::new(),
                    })
                    .insert
                    .push_str(change.value());
            }
        }
    }
    if let Some(edit) = current {
        edits.push(edit);
    }
    edits
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EditPatch {
    pub from_line: usize,
    pub from_column: usize,
    pub to_line: usize,
    pub to_column: usize,
    pub text: String,
}

pub struct EditorDocument {
    text: String,
    generation: u64,
    revision: u64,
    lines: Vec<usize>,
    eol: &'static str,
}

#[cfg(test)]
mod rebase_tests {
    use super::{normalize, rebase_edits};

    #[test]
    fn save_rebase_maps_disjoint_urls_in_utf16_and_preserves_untouched_text() {
        let before = "🧠 ![A](a.png)\r\nunchanged selection 中文\r![B](b.png)\n";
        let after = before
            .replace("a.png", "studyapp-media:group/alpha.png")
            .replace("b.png", "studyapp-media:group/beta.png");
        let normalized = normalize(before);
        let caret = normalized[..normalized.find("selection").unwrap()]
            .encode_utf16()
            .count();
        let changes = rebase_edits(before, &after);
        assert!(!changes.is_empty());
        assert!(
            changes
                .iter()
                .all(|change| change.to <= caret || change.from >= caret),
            "unchanged caret span was replaced"
        );
        let mut applied = normalized.encode_utf16().collect::<Vec<_>>();
        for change in changes.into_iter().rev() {
            applied.splice(change.from..change.to, change.insert.encode_utf16());
        }
        assert_eq!(String::from_utf16(&applied).unwrap(), normalize(&after));
        assert!(rebase_edits(before, before).is_empty());
    }
}

impl EditorDocument {
    pub fn new(text: String) -> Self {
        let eol = preferred_eol(&text);
        let lines = line_starts(&text);
        Self {
            text,
            generation: 1,
            revision: 0,
            lines,
            eol,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Both a successful flush and an editable refusal may settle only the
    /// exact outstanding request for the current document.
    pub fn matches_flush_response(
        &self,
        generation: u64,
        request: u64,
        pending: Option<u64>,
    ) -> bool {
        generation == self.generation && pending == Some(request)
    }
    /// Explicit recovery after a bridge failure. Keep canonical raw bytes when
    /// they already match; otherwise retain the document's preferred EOL.
    pub fn recover(&mut self, normalized: &str) -> anyhow::Result<()> {
        anyhow::ensure!(!normalized.contains('\r'), "恢复内容含有非规范化换行");
        let text = if normalize(&self.text) == normalized {
            self.text.clone()
        } else {
            normalized.replace('\n', self.eol)
        };
        self.reset(text);
        Ok(())
    }

    pub fn reset(&mut self, text: String) {
        let generation = self.generation.wrapping_add(1).max(1);
        *self = Self::new(text);
        self.generation = generation;
    }

    pub fn apply(
        &mut self,
        generation: u64,
        base: u64,
        revision: u64,
        patches: &[EditPatch],
    ) -> anyhow::Result<bool> {
        if generation != self.generation {
            return Ok(false);
        }
        anyhow::ensure!(
            base == self.revision && base.checked_add(1) == Some(revision),
            "编辑事务顺序不一致，已保留当前草稿"
        );
        anyhow::ensure!(patches.len() <= 4096, "单次编辑事务过大");
        let mut edits = Vec::with_capacity(patches.len());
        let mut previous_end = 0;
        let mut bytes = self.text.len();
        for patch in patches {
            let from = self.byte_offset(patch.from_line, patch.from_column)?;
            let to = self.byte_offset(patch.to_line, patch.to_column)?;
            anyhow::ensure!(from <= to && from >= previous_end, "编辑范围重叠或逆序");
            anyhow::ensure!(!patch.text.contains('\r'), "编辑事务必须使用规范化换行");
            let replacement = if self.eol == "\n" {
                patch.text.clone()
            } else {
                patch.text.replace('\n', self.eol)
            };
            bytes = bytes
                .checked_sub(to - from)
                .and_then(|n| n.checked_add(replacement.len()))
                .ok_or_else(|| anyhow::anyhow!("编辑长度溢出"))?;
            previous_end = to;
            edits.push((from..to, replacement));
        }
        // Validate the entire transaction before changing the canonical text.
        // No Markdown parsing or HTML rendering occurs on this hot path.
        for (range, replacement) in edits.into_iter().rev() {
            self.text.replace_range(range, &replacement);
        }
        self.lines = line_starts(&self.text);
        self.revision = revision;
        Ok(true)
    }

    pub fn verify_flush(
        &self,
        generation: u64,
        revision: u64,
        normalized: &str,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            generation == self.generation && revision == self.revision,
            "编辑器保存确认已过期"
        );
        anyhow::ensure!(
            normalize(&self.text) == normalized,
            "编辑器与草稿内容不一致，未覆盖已保存版本"
        );
        Ok(())
    }

    fn byte_offset(&self, line: usize, column: usize) -> anyhow::Result<usize> {
        let column = column
            .checked_sub(1)
            .ok_or_else(|| anyhow::anyhow!("列号必须从 1 开始"))?;
        let index = line
            .checked_sub(1)
            .ok_or_else(|| anyhow::anyhow!("行号必须从 1 开始"))?;
        let start = *self
            .lines
            .get(index)
            .ok_or_else(|| anyhow::anyhow!("编辑行号超出文档"))?;
        let end = self
            .lines
            .get(index + 1)
            .copied()
            .unwrap_or(self.text.len());
        let text = self.text[start..end].trim_end_matches(['\r', '\n']);
        let mut utf16 = 0;
        for (byte, ch) in text.char_indices() {
            if utf16 == column {
                return Ok(start + byte);
            }
            utf16 += ch.len_utf16();
            anyhow::ensure!(utf16 <= column, "编辑位置切断 Unicode 字符");
        }
        anyhow::ensure!(utf16 == column, "编辑列号超出当前行");
        Ok(start + text.len())
    }
}

pub fn normalize(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn preferred_eol(text: &str) -> &'static str {
    let crlf = text.matches("\r\n").count();
    let lf = text.bytes().filter(|b| *b == b'\n').count() - crlf;
    let cr = text.bytes().filter(|b| *b == b'\r').count() - crlf;
    if crlf > 0 && crlf >= lf && crlf >= cr {
        "\r\n"
    } else if cr > lf {
        "\r"
    } else {
        "\n"
    }
}

fn line_starts(text: &str) -> Vec<usize> {
    let bytes = text.as_bytes();
    let mut starts = vec![0];
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\r' => {
                index += 1;
                if bytes.get(index) == Some(&b'\n') {
                    index += 1;
                }
                starts.push(index);
            }
            b'\n' => {
                index += 1;
                starts.push(index);
            }
            _ => index += 1,
        }
    }
    starts
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_flush_refusals_cannot_settle_another_request_or_document() {
        let mut document = EditorDocument::new("保留".into());
        assert!(document.matches_flush_response(1, 7, Some(7)));
        assert!(!document.matches_flush_response(1, 6, Some(7)));
        assert!(!document.matches_flush_response(1, 7, None));
        document.reset("新文档".into());
        assert!(!document.matches_flush_response(1, 7, Some(7)));
        assert!(document.matches_flush_response(2, 7, Some(7)));
        assert_eq!(document.text(), "新文档");
    }

    fn patch(from: (usize, usize), to: (usize, usize), text: &str) -> EditPatch {
        EditPatch {
            from_line: from.0,
            from_column: from.1 + 1,
            to_line: to.0,
            to_column: to.1 + 1,
            text: text.into(),
        }
    }

    #[test]
    fn transactions_preserve_unicode_and_original_mixed_line_endings() {
        let mut doc = EditorDocument::new("# 标题\r\n第一行 🧠。\r\n尾行\n保留".into());
        doc.apply(1, 0, 1, &[patch((2, 4), (2, 6), "**知识**")])
            .unwrap();
        assert_eq!(doc.text(), "# 标题\r\n第一行 **知识**。\r\n尾行\n保留");
        doc.apply(1, 1, 2, &[patch((3, 2), (3, 2), "\n新行")])
            .unwrap();
        assert_eq!(
            doc.text(),
            "# 标题\r\n第一行 **知识**。\r\n尾行\r\n新行\n保留"
        );
        doc.verify_flush(1, 2, "# 标题\n第一行 **知识**。\n尾行\n新行\n保留")
            .unwrap();
    }

    #[test]
    fn multiple_edits_use_original_offsets_and_invalid_edits_are_atomic() {
        let mut doc = EditorDocument::new("a 🧠 z\nsecond\nthird".into());
        doc.apply(
            1,
            0,
            1,
            &[
                patch((1, 0), (1, 1), "First"),
                patch((3, 0), (3, 5), "Last"),
            ],
        )
        .unwrap();
        assert_eq!(doc.text(), "First 🧠 z\nsecond\nLast");
        let before = doc.text().to_owned();
        assert!(
            doc.apply(
                1,
                1,
                2,
                &[
                    patch((1, 0), (1, 1), "Q"),
                    patch((1, 7), (1, 8), "broken surrogate")
                ]
            )
            .is_err()
        );
        assert_eq!(doc.text(), before);
        assert_eq!(doc.revision(), 1);
    }

    #[test]
    fn stale_generations_are_ignored_but_revision_gaps_and_wrong_flushes_fail() {
        let mut doc = EditorDocument::new("safe".into());
        assert!(
            !doc.apply(99, 0, 1, &[patch((1, 0), (1, 4), "old library")])
                .unwrap()
        );
        assert!(doc.apply(1, 1, 2, &[patch((1, 0), (1, 4), "gap")]).is_err());
        assert!(doc.apply(1, 0, 2, &[]).is_err());
        assert_eq!(doc.text(), "safe");
        assert!(doc.verify_flush(1, 0, "different").is_err());
        assert!(doc.verify_flush(1, 1, "safe").is_err());
        assert!(doc.verify_flush(99, 0, "safe").is_err());
        doc.verify_flush(1, 0, "safe").unwrap();
    }

    #[test]
    fn explicit_recovery_keeps_matching_raw_bytes_and_resets_generation() {
        let mut doc = EditorDocument::new("a\r\nb\nc\r".into());
        doc.recover("a\nb\nc\n").unwrap();
        assert_eq!(doc.text(), "a\r\nb\nc\r");
        assert_eq!(doc.generation(), 2);
        doc.recover("a\nupdated\nc\n").unwrap();
        assert_eq!(doc.text(), "a\r\nupdated\r\nc\r\n");
        assert!(doc.recover("bad\rinput").is_err());
        assert_eq!(doc.generation(), 3);
    }

    #[test]
    fn large_document_can_edit_flush_and_recover_without_byte_ceiling() {
        let text = format!("# 大笔记\r\n{}", "中文\r\n".repeat(1_400_000));
        assert!(text.len() > 10 * 1024 * 1024);
        let mut doc = EditorDocument::new(text);
        doc.apply(1, 0, 1, &[patch((1, 2), (1, 2), "新")]).unwrap();
        let normalized = normalize(doc.text());
        doc.verify_flush(1, 1, &normalized).unwrap();
        doc.recover(&normalized).unwrap();
        assert!(doc.text().starts_with("# 新大笔记\r\n"));
        assert_eq!(doc.generation(), 2);
    }

    #[test]
    fn native_transaction_performance_on_a_large_document() {
        let mut doc =
            EditorDocument::new("# 学习笔记\n\n一段 **连续编辑** 与代码。\n".repeat(7_000));
        let mut timings = vec![];
        for revision in 1..=200 {
            let start = std::time::Instant::now();
            doc.apply(1, revision - 1, revision, &[patch((1, 2), (1, 2), "字")])
                .unwrap();
            timings.push(start.elapsed().as_secs_f64() * 1000.);
        }
        timings.sort_by(f64::total_cmp);
        eprintln!(
            "native transaction: {} bytes, median {:.2} ms, P95 {:.2} ms",
            doc.text().len(),
            timings[100],
            timings[189]
        );
        assert_eq!(doc.revision(), 200);
        assert!(
            doc.text()
                .starts_with(&format!("# {}学习笔记", "字".repeat(200)))
        );
    }

    #[test]
    fn patches_cannot_overlap_reverse_or_exceed_line_boundaries() {
        let mut doc = EditorDocument::new("abcdef\nlast".into());
        for edits in [
            vec![patch((1, 0), (1, 4), "A"), patch((1, 2), (1, 5), "B")],
            vec![patch((1, 4), (1, 1), "reverse")],
            vec![patch((0, 0), (1, 0), "zero")],
            vec![patch((1, 20), (1, 20), "out of bounds")],
        ] {
            assert!(doc.apply(1, 0, 1, &edits).is_err());
            assert_eq!(doc.text(), "abcdef\nlast");
        }
    }
}
