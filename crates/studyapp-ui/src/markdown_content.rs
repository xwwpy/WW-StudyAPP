//! Offline Markdown presentation for compact learning text.

pub fn markdown_preview(source: &str) -> String {
    preview(source, false, false)
}

/// HTML is sanitized within the original Markdown token boundaries. Keeping the
/// Markdown stream preserves ordered starts, task lists, tables and fenced code.
pub fn markdown_preview_with_html(source: &str, enabled: bool) -> String {
    use std::{cell::RefCell, collections::VecDeque};
    struct CachedPreview {
        source: String,
        rendered: String,
    }
    thread_local! {
        static CACHE: RefCell<VecDeque<CachedPreview>> = const { RefCell::new(VecDeque::new()) };
    }
    if !enabled || !source.contains('<') || source.len() > 65_536 {
        return preview(source, false, enabled);
    }
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(index) = cache.iter().position(|entry| entry.source == source) {
            let entry = cache.remove(index).expect("cached preview index");
            let rendered = entry.rendered.clone();
            cache.push_back(entry);
            return rendered;
        }
        let rendered = preview(source, false, true);
        if rendered.len() <= 524_288 {
            if cache.len() >= 16 {
                cache.pop_front();
            }
            cache.push_back(CachedPreview {
                source: source.into(),
                rendered: rendered.clone(),
            });
        }
        rendered
    })
}

fn preview(source: &str, safe_links: bool, render_html: bool) -> String {
    use pulldown_cmark::{Event, Tag};
    let mut events = parser(source).into_offset_iter().peekable();
    let mut output = String::new();
    let mut cursor = 0;
    while let Some((event, mut range)) = events.next() {
        match event {
            Event::Start(Tag::Link { ref dest_url, .. })
                if safe_links
                    && (dest_url.starts_with("https://") || dest_url.starts_with("http://")) => {}
            Event::Html(_) | Event::InlineHtml(_) => {
                if matches!(event, Event::Html(_)) {
                    while let Some((Event::Html(_), next)) = events.peek() {
                        if next.start != range.end {
                            break;
                        }
                        range.end = next.end;
                        events.next();
                    }
                }
                output.push_str(&source[cursor..range.start]);
                if render_html {
                    let raw = &source[range.clone()];
                    let safe = if matches!(event, Event::InlineHtml(_)) {
                        studyapp_core::sanitize_inline_html(raw)
                    } else {
                        studyapp_core::sanitize_html(raw).map(|clean| {
                            // A div HTML block ends at a physical blank line in
                            // CommonMark. Encode text newlines so a sanitized
                            // pre block cannot be split and reinterpreted.
                            format!("<div>{}</div>\n", clean.replace('\n', "&#10;"))
                        })
                    };
                    if let Some(safe) = safe {
                        output.push_str(&safe);
                        cursor = range.end;
                        continue;
                    }
                }
                // Preserve the boundary as literal text: removing it (or adding
                // whitespace) can create a new inline or reference-style image.
                push_literal(&mut output, &source[range.clone()]);
                cursor = range.end;
            }
            Event::Start(tag @ (Tag::Image { .. } | Tag::Link { .. })) => {
                let image = matches!(tag, Tag::Image { .. });
                let mut depth = 1;
                let mut label = String::new();
                for (event, _) in events.by_ref() {
                    match event {
                        Event::Start(_) => depth += 1,
                        Event::End(_) => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        Event::Text(text) | Event::Code(text) => label.push_str(&text),
                        Event::SoftBreak | Event::HardBreak => label.push(' '),
                        _ => {}
                    }
                }
                output.push_str(&source[cursor..range.start]);
                if image {
                    output.push_str("\\[图片：");
                } else if label.trim().is_empty() {
                    // An empty link must not join surrounding image tokens.
                    label = "链接".into();
                }
                // Labels cannot become new images, raw HTML, or formatting.
                push_literal(&mut output, &label);
                if image {
                    output.push_str("\\]");
                }
                cursor = range.end;
            }
            _ => {}
        }
    }
    output.push_str(&source[cursor..]);
    output
}

fn push_literal(output: &mut String, text: &str) {
    for ch in text.chars() {
        if ch.is_ascii_punctuation() {
            output.push('\\');
        }
        output.push(ch);
    }
}

pub fn markdown_excerpt(source: &str, limit: usize) -> String {
    use pulldown_cmark::{Event, TagEnd};
    let mut text = String::new();
    for event in parser(source) {
        match event {
            Event::Text(value) | Event::Code(value) => text.push_str(&value),
            Event::SoftBreak | Event::HardBreak | Event::Rule => text.push(' '),
            Event::End(
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::Item
                | TagEnd::CodeBlock
                | TagEnd::TableCell,
            ) => text.push(' '),
            _ => {}
        }
    }
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(limit)
        .collect()
}

use crate::markdown_parser::parser;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_preserves_code_task_lists_and_ordered_starts() {
        let source = "```rust\nlet x = 1;\n```\n\n3. 第三项\n4. 第四项\n\n- [x] 完成\n- [ ] 未完成\n\n|列|值|\n|--|--|\n|中|😀|\n";
        assert_eq!(markdown_preview(source), source);
        let safe = markdown_preview(
            "[**中**](https://example.com) ![图](file:///private.png) <img src=\"remote\">\n<script>alert(1)</script>",
        );
        assert!(!safe.contains("https://"));
        assert!(!safe.contains("file://"));
        assert!(safe.contains("\\<img"));
        assert!(safe.contains("\\<script"));
        assert!(safe.contains("中"));
        assert!(safe.contains("图片：图"));
    }

    #[test]
    fn summary_formats_lists_code_and_emphasis_without_loading_external_content() {
        let safe = markdown_preview(
            "## 今日\n\n- **中文🧠**\n- `code`\n\n[链接](https://example.com) ![图](https://example.com/a.png)\n<script>alert(1)</script>",
        );
        let mut html = String::new();
        pulldown_cmark::html::push_html(&mut html, parser(&safe));
        assert!(html.contains("<h2>今日</h2>"));
        assert!(html.contains("<strong>中文🧠</strong>"));
        assert!(html.contains("<code>code</code>"));
        assert!(html.contains("<ul>"));
        assert!(html.contains("链接"));
        assert!(html.contains("[图片：图]"));
        assert!(!html.contains("https://"));
        assert!(!html.contains("<script"));
    }

    #[test]
    fn summary_excerpt_is_readable_and_unicode_safe() {
        assert_eq!(
            markdown_excerpt("# 今日\n\n- **中文🧠**\n- `code`", 80),
            "今日 中文🧠 code"
        );
        assert_eq!(markdown_excerpt("**中文🧠abc**", 3), "中文🧠");
        assert_eq!(markdown_excerpt("普通摘要", 80), "普通摘要");
        assert_eq!(
            markdown_excerpt(
                "![图](https://example.com/a) [链接](https://example.com)",
                80
            ),
            "图 链接"
        );
        assert_eq!(markdown_excerpt("<script>alert(1)</script>", 80), "");
    }

    #[test]
    fn preview_preserves_empty_cells_and_cannot_reinterpret_link_labels() {
        use pulldown_cmark::{Event, Tag};
        let table = "| a | b | c |\n|---|---|---|\n| x | | z |";
        assert_eq!(markdown_preview(table), table);
        for source in [
            "[![图](https://img.example/a)](https://example.com)",
            "[\\!\\[图\\]\\(https://example.com\\)](https://target.example)",
            "![图][image]\n\n[image]: https://example.com/a.png",
            "中文<b>粗体</b>与 <https://example.com>",
            "![图]<span></span>(https://example.com/a.png)",
            "[标签]<span></span>(https://example.com)",
            "![图]<span></span>[ref]\n\n[ref]: https://example.com/a.png",
            "![图][]()(https://example.com/a.png)",
            "![图][ ]()[ref]\n\n[ref]: https://example.com/a.png",
        ] {
            let safe = markdown_preview(source);
            assert!(
                parser(&safe).all(|event| !matches!(
                    event,
                    Event::Html(_)
                        | Event::InlineHtml(_)
                        | Event::Start(Tag::Image { .. } | Tag::Link { .. })
                )),
                "unsafe interpretation: {safe}"
            );
        }
    }
}

/// Keeps only the latest release's sanitized source; TextView retains its parsed
/// state under the stable element id when that source does not change.
pub fn release_notes(source: &str) -> gpui_kit::SharedString {
    use std::cell::RefCell;
    thread_local! {
        static CACHE: RefCell<Option<(String, gpui_kit::SharedString)>> = const { RefCell::new(None) };
    }
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some((old, rendered)) = cache.as_ref()
            && old == source
        {
            return rendered.clone();
        }
        let rendered: gpui_kit::SharedString = preview(source, true, false).into();
        *cache = Some((source.to_owned(), rendered.clone()));
        rendered
    })
}

#[cfg(test)]
mod release_tests {
    #[test]
    fn mixed_release_markdown_keeps_formatting_and_safe_links() {
        let source = "# Changes\n\n- **Bold** and `code`\n\n```rust\nlet x = 1;\n```\n\n[Help](https://example.com/help)\n\n![remote](https://example.com/a.png)\n\n[bad](javascript:alert)\n<script>bad</script>";
        let safe = super::release_notes(source);
        assert!(safe.contains("# Changes"));
        assert!(safe.contains("**Bold**"));
        assert!(safe.contains("```rust"));
        assert!(safe.contains("[Help](https://example.com/help)"));
        assert!(!safe.contains("![remote]"));
        assert!(!safe.contains("javascript:"));
        assert!(!safe.contains("<script>"));
        assert_eq!(safe, super::release_notes(source));
    }
}

#[cfg(test)]
mod html_tests {
    use super::*;
    use pulldown_cmark::{Event, Tag};
    #[test]
    fn safe_html_keeps_markdown_order_tasks_and_fenced_html() {
        let source = "3. **第三项**\n4. 第四项\n\n- [x] 完成\n\n<span style='color:red' onclick='bad()'>**重点**</span>\n\n```html\n<img src='file:///private'>\n```";
        let safe = markdown_preview_with_html(source, true);
        assert!(safe.contains("3. **第三项**"));
        assert!(safe.contains("- [x] 完成"));
        assert!(safe.contains("<span style=\"color:red\">**重点**</span>"));
        assert!(!safe.contains("onclick"));
        assert!(safe.contains("```html\n<img src='file:///private'>\n```"));
        assert_eq!(
            markdown_preview_with_html(source, false),
            markdown_preview(source)
        );
    }
    #[test]
    fn native_html_blocks_remain_one_safe_block_without_resource_reinterpretation() {
        for source in [
            "<div style='color:red'>\n<strong>正文</strong>\n</div>",
            "<unknown>![a](https://outside)</unknown>",
            "<a href='https://outside'>![a](file:///private)</a>",
            "![图]<span></span>(https://outside)",
            "![图]<script></script>[ref]\n\n[ref]:https://outside",
        ] {
            let safe = markdown_preview_with_html(source, true);
            assert!(
                parser(&safe).all(|event| !matches!(
                    event,
                    Event::Start(Tag::Image { .. } | Tag::Link { .. })
                )),
                "{safe}"
            );
            for (event, _) in parser(&safe).into_offset_iter() {
                if let Event::Html(raw) | Event::InlineHtml(raw) = event {
                    for forbidden in ["src=", "href=", "onclick=", "<script"] {
                        assert!(!raw.contains(forbidden), "{safe}");
                    }
                }
            }
        }
    }
}
