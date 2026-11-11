//! App-specific list paragraph boundaries. Synthetic separators are parse-only;
//! every returned range points into the unchanged original source.
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use std::{borrow::Cow, collections::BTreeSet, ops::Range};

type OffsetEvent<'a> = (Event<'a>, Range<usize>);

pub(crate) struct MarkdownParser<'a> {
    events: std::vec::IntoIter<OffsetEvent<'a>>,
}
impl<'a> MarkdownParser<'a> {
    pub fn into_offset_iter(self) -> impl Iterator<Item = OffsetEvent<'a>> {
        self.events
    }
}
impl<'a> Iterator for MarkdownParser<'a> {
    type Item = Event<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        self.events.next().map(|(event, _)| event)
    }
}

pub(crate) fn parser(source: &str) -> MarkdownParser<'_> {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let events: Vec<_> = Parser::new_ext(source, options)
        .into_offset_iter()
        .collect();
    let Some((virtual_source, inserted)) = virtual_document(source, &events) else {
        return MarkdownParser {
            events: events.into_iter(),
        };
    };
    let original_offset =
        |offset: usize| offset - inserted.partition_point(|position| *position < offset);
    let events = Parser::new_ext(&virtual_source, options)
        .into_offset_iter()
        .map(|(event, range)| {
            (
                event.into_static(),
                original_offset(range.start)..original_offset(range.end),
            )
        })
        .collect::<Vec<_>>();
    MarkdownParser {
        events: events.into_iter(),
    }
}

/// Used only by a third-party display fallback without an event API. The
/// returned separators affect layout, never the stored document or offsets.
pub(crate) fn display_source(source: &str) -> Cow<'_, str> {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let events: Vec<_> = Parser::new_ext(source, options)
        .into_offset_iter()
        .collect();
    virtual_document(source, &events).map_or(Cow::Borrowed(source), |(text, _)| Cow::Owned(text))
}
fn virtual_document(source: &str, events: &[OffsetEvent<'_>]) -> Option<(String, Vec<usize>)> {
    let mut starts = vec![0];
    starts.extend(source.match_indices('\n').map(|(offset, _)| offset + 1));
    let mut items = Vec::new();
    let mut code = 0;
    let mut quote = 0;
    let mut boundaries = BTreeSet::new();
    for (event, range) in events {
        match event {
            Event::Start(Tag::Item) => items.push(range.start),
            Event::End(TagEnd::Item) => {
                items.pop();
            }
            Event::Start(Tag::CodeBlock(_)) => code += 1,
            Event::End(TagEnd::CodeBlock) => code -= 1,
            Event::Start(Tag::BlockQuote(_)) => quote += 1,
            Event::End(TagEnd::BlockQuote(_)) => quote -= 1,
            Event::Text(_)
            | Event::Code(_)
            | Event::InlineHtml(_)
            | Event::SoftBreak
            | Event::HardBreak
                if code == 0 && quote == 0 && !items.is_empty() =>
            {
                let first = starts
                    .partition_point(|start| *start <= range.start)
                    .saturating_sub(1);
                for &start in &starts[first..] {
                    if start >= range.end {
                        break;
                    }
                    if start <= *items.last().expect("inside list item") {
                        continue;
                    }
                    if source
                        .as_bytes()
                        .get(start)
                        .is_some_and(|byte| !matches!(byte, b' ' | b'\t' | b'\r' | b'\n' | b'>'))
                    {
                        boundaries.insert(start);
                    }
                }
            }
            _ => {}
        }
    }
    if boundaries.is_empty() {
        return None;
    }
    // Parse one complete virtual document so reference definitions, inline
    // markup and document-wide constructs retain their normal visibility.
    let mut virtual_source = String::with_capacity(source.len() + boundaries.len());
    let mut inserted = Vec::with_capacity(boundaries.len());
    let mut cursor = 0;
    for boundary in boundaries {
        virtual_source.push_str(&source[cursor..boundary]);
        inserted.push(virtual_source.len());
        virtual_source.push('\n');
        cursor = boundary;
    }
    virtual_source.push_str(&source[cursor..]);
    Some((virtual_source, inserted))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn html(source: &str) -> String {
        let mut result = String::new();
        pulldown_cmark::html::push_html(&mut result, parser(source));
        result
    }
    #[test]
    fn unindented_body_leaves_bullet_ordered_task_and_nested_lists() {
        for list in ["- item", "1. item", "- [x] item", "- parent\n  - item"] {
            let source = format!("{list}\n正文 **outside**");
            let rendered = html(&source);
            assert!(rendered.contains("</li>"), "{rendered}");
            assert!(
                rendered.contains("<p>正文 <strong>outside</strong></p>"),
                "{rendered}"
            );
            assert!(rendered.find("</li>").unwrap() < rendered.find("正文").unwrap());
        }
    }
    #[test]
    fn continuations_code_quotes_and_references_keep_markdown_semantics() {
        let source = "- [reference][target]\n  continuation\nbody [reference][target]\n\n[target]: https://example.test/\n";
        let rendered = html(source);
        assert_eq!(
            rendered.matches("href=\"https://example.test/\"").count(),
            2
        );
        assert!(rendered.contains("continuation</li>"), "{rendered}");
        assert!(rendered.contains("<p>body"));
        let code = "- item\n\n  ```text\n  literal\n  ```\nbody";
        assert!(html(code).contains("<code class=\"language-text\">literal\n</code>"));
        let quote = "> - item\n>   continuation\n> lazy";
        let mut upstream = String::new();
        pulldown_cmark::html::push_html(&mut upstream, Parser::new(quote));
        assert_eq!(html(quote), upstream);
    }
    #[test]
    fn mapped_offsets_still_address_original_utf8_crlf_and_inline_tokens() {
        let source = "- 中文\r\n正文 **bold** and [link](https://example.test/)\r\n- next\r\n尾部";
        let original = source.as_bytes().to_vec();
        for (_, range) in parser(source).into_offset_iter() {
            assert!(range.start <= range.end && range.end <= source.len());
            assert!(source.is_char_boundary(range.start) && source.is_char_boundary(range.end));
        }
        let links: Vec<_> = parser(source)
            .into_offset_iter()
            .filter_map(|(event, range)| {
                matches!(event, Event::Start(Tag::Link { .. })).then(|| &source[range])
            })
            .collect();
        assert_eq!(links, ["[link](https://example.test/)"]);
        assert_eq!(source.as_bytes(), original);
        assert!(html(source).contains("<p>正文 <strong>bold</strong>"));
        assert!(html(source).contains("<p>尾部</p>"));
    }
}
