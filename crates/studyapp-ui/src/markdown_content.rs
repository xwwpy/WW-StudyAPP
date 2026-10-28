//! Offline Markdown presentation for compact learning text.

pub fn markdown_preview(source: &str) -> String {
    use pulldown_cmark::{Event, Tag};
    let mut events = parser(source).into_offset_iter();
    let mut output = String::new();
    let mut cursor = 0;
    while let Some((event, range)) = events.next() {
        match event {
            Event::Html(_) | Event::InlineHtml(_) => {
                output.push_str(&source[cursor..range.start]);
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

fn parser(source: &str) -> pulldown_cmark::Parser<'_> {
    use pulldown_cmark::Options;
    pulldown_cmark::Parser::new_ext(
        source,
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS,
    )
}

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
