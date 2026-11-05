//! Pure bounded presentation model, shared by native summary surfaces.
use std::collections::BTreeMap;
use studyapp_core::{RgbaColor, SafeHtmlNode, parse_rgba_color};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Font {
    pub color: RgbaColor,
    pub background: Option<RgbaColor>,
    pub size: f32,
    pub weight: f32,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub family: Option<String>,
    pub line_height: f32,
    pub tracking: f32,
}
impl Font {
    pub fn base(size: f32, color: RgbaColor) -> Self {
        Self {
            color,
            background: None,
            size,
            weight: 400.,
            italic: false,
            underline: false,
            strike: false,
            family: None,
            line_height: size * 1.6,
            tracking: 0.,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Dimension {
    Pixels(f32),
    Percent(f32),
}
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct BoxStyle {
    pub background: Option<RgbaColor>,
    pub border_color: Option<RgbaColor>,
    pub border: f32,
    pub border_style: String,
    pub radius: f32,
    pub margin: [f32; 4],
    pub padding: [f32; 4],
    pub width: Option<Dimension>,
    pub max_width: Option<Dimension>,
    pub height: Option<f32>,
    pub align: String,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Kind {
    Column,
    Flow,
    Row,
    Cell,
}
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Node {
    Run(usize),
    Break,
    Box {
        kind: Kind,
        style: BoxStyle,
        children: Vec<Node>,
    },
}
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Run {
    pub text: String,
    pub font: Font,
    pub separator: &'static str,
}
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Document {
    pub root: Node,
    pub runs: Vec<Run>,
    pub plain: String,
}
fn px(value: &str) -> Option<f32> {
    value.strip_suffix("px").unwrap_or(value).parse().ok()
}
fn edges(value: &str) -> [f32; 4] {
    let v: Vec<_> = value.split_whitespace().filter_map(px).collect();
    match v.as_slice() {
        [a] => [*a; 4],
        [a, b] => [*a, *b, *a, *b],
        [a, b, c] => [*a, *b, *c, *b],
        [a, b, c, d] => [*a, *b, *c, *d],
        _ => [0.; 4],
    }
}
fn dimension(value: &str) -> Option<Dimension> {
    if let Some(v) = value.strip_suffix('%') {
        v.parse::<f32>().ok().map(|n| Dimension::Percent(n / 100.))
    } else {
        px(value).map(Dimension::Pixels)
    }
}
pub(super) fn color(value: &str) -> Option<RgbaColor> {
    if let Some(hex) = value.strip_prefix('#') {
        let expanded = if matches!(hex.len(), 3 | 4) {
            format!("#{}", hex.chars().flat_map(|c| [c, c]).collect::<String>())
        } else {
            value.into()
        };
        return parse_rgba_color(&expanded);
    }
    if let Some(body) = value
        .strip_prefix("rgba(")
        .or_else(|| value.strip_prefix("rgb("))
        .and_then(|s| s.strip_suffix(')'))
    {
        let v: Vec<_> = body.split(',').map(str::trim).collect();
        if !matches!(v.len(), 3 | 4) {
            return None;
        }
        let r = v[0].parse::<u8>().ok()?;
        let g = v[1].parse::<u8>().ok()?;
        let b = v[2].parse::<u8>().ok()?;
        let alpha = if v.len() == 4 {
            (v[3].parse::<f32>().ok()? * 255.).round() as u8
        } else {
            255
        };
        return Some(RgbaColor::new(
            (r as u32) << 16 | (g as u32) << 8 | b as u32,
            alpha,
        ));
    }
    let rgb = match value {
        "transparent" => return Some(RgbaColor::new(0, 0)),
        "black" => 0,
        "white" => 0xffffff,
        "red" => 0xff0000,
        "green" => 0x008000,
        "blue" => 0x0000ff,
        "yellow" => 0xffff00,
        "gray" | "grey" => 0x808080,
        "silver" => 0xc0c0c0,
        "maroon" => 0x800000,
        "purple" => 0x800080,
        "fuchsia" => 0xff00ff,
        "lime" => 0x00ff00,
        "olive" => 0x808000,
        "navy" => 0x000080,
        "teal" => 0x008080,
        "aqua" => 0x00ffff,
        "orange" => 0xffa500,
        "pink" => 0xffc0cb,
        "brown" => 0xa52a2a,
        _ => return None,
    };
    Some(RgbaColor::opaque(rgb))
}
fn styles(tag: &str, css: &str, parent: &Font) -> (Font, BoxStyle) {
    let mut font = parent.clone();
    let mut bx = BoxStyle::default();
    match tag {
        "b" | "strong" | "th" => font.weight = 700.,
        "i" | "em" => font.italic = true,
        "u" => font.underline = true,
        "s" | "del" => font.strike = true,
        "code" | "pre" | "kbd" => font.family = Some("monospace".into()),
        "mark" => font.background = Some(RgbaColor::opaque(0xfef08a)),
        "small" | "sub" | "sup" => {
            font.size = (font.size * 0.8).max(8.);
            font.line_height = font.size * 1.6;
        }
        _ => {}
    }
    if let Some(level) = tag
        .strip_prefix('h')
        .and_then(|s| s.parse::<u8>().ok())
        .filter(|n| (1..=6).contains(n))
    {
        font.weight = 700.;
        font.size = (parent.size * (1.8 - (level as f32 - 1.) * 0.15)).min(72.);
        font.line_height = font.size * 1.5;
        bx.margin = [8., 0., 4., 0.];
    }
    let declarations: BTreeMap<_, _> = css.split(';').filter_map(|s| s.split_once(':')).collect();
    if let Some(v) = declarations.get("font-size").and_then(|s| px(s)) {
        font.size = v;
        font.line_height = v * 1.6;
    }
    for (key, value) in declarations {
        match key {
            "color" => {
                if let Some(v) = color(value) {
                    font.color = v;
                }
            }
            "background-color" => {
                bx.background = color(value);
                font.background = bx.background;
            }
            "font-weight" => {
                font.weight = match value {
                    "bold" => 700.,
                    "normal" => 400.,
                    _ => value.parse().unwrap_or(400.),
                }
            }
            "font-style" => font.italic = matches!(value, "italic" | "oblique"),
            "font-family" => font.family = Some(value.into()),
            "text-decoration" => {
                font.underline = value.contains("underline");
                font.strike = value.contains("line-through");
            }
            "line-height" => {
                if let Some(n) = px(value) {
                    font.line_height = if value.ends_with("px") {
                        n
                    } else {
                        font.size * n
                    };
                }
            }
            "letter-spacing" => font.tracking = px(value).unwrap_or(0.),
            "text-align" => bx.align = value.into(),
            "padding" => bx.padding = edges(value),
            "margin" => bx.margin = edges(value),
            "width" => bx.width = dimension(value),
            "max-width" => bx.max_width = dimension(value),
            "height" => bx.height = px(value),
            "border-color" => bx.border_color = color(value),
            "border-width" => bx.border = px(value).unwrap_or(0.),
            "border-style" => bx.border_style = value.into(),
            "border-radius" => bx.radius = px(value).unwrap_or(0.),
            _ => {}
        }
    }
    if bx.border_style.is_empty() || bx.border_style == "none" {
        bx.border = 0.;
    }
    (font, bx)
}
struct Builder {
    runs: Vec<Run>,
    separator: &'static str,
    last_space: bool,
}
impl Builder {
    fn boundary(&mut self, separator: &'static str) {
        if !self.runs.is_empty() {
            self.separator = separator;
        }
        self.last_space = true;
    }
    fn text(&mut self, value: &str, font: &Font, pre: bool) -> Option<Vec<Node>> {
        let mut text = String::new();
        for ch in value.chars() {
            if !pre && ch.is_ascii_whitespace() {
                if !self.last_space {
                    text.push(' ');
                }
                self.last_space = true;
            } else {
                text.push(ch);
                self.last_space = false;
            }
        }
        let mut out = Vec::new();
        for part in text.split_word_bounds() {
            if self.runs.len() >= 8192 {
                return None;
            }
            if part.contains('\n') && pre {
                for (i, line) in part.split('\n').enumerate() {
                    if i > 0 {
                        out.push(Node::Break);
                        self.boundary("\n");
                    }
                    if !line.is_empty() {
                        out.extend(self.text(line, font, true)?);
                    }
                }
                continue;
            }
            let index = self.runs.len();
            self.runs.push(Run {
                text: part.into(),
                font: font.clone(),
                separator: std::mem::take(&mut self.separator),
            });
            out.push(Node::Run(index));
        }
        Some(out)
    }
    fn nodes(
        &mut self,
        nodes: &[SafeHtmlNode],
        font: &Font,
        pre: bool,
        list: Option<u32>,
    ) -> Option<Vec<Node>> {
        let mut out = Vec::new();
        let mut list_index = list;
        for node in nodes {
            match node {
                SafeHtmlNode::Text(value) => out.extend(self.text(value, font, pre)?),
                SafeHtmlNode::Element {
                    tag,
                    style,
                    start,
                    children,
                } => {
                    if tag == "br" {
                        self.boundary("\n");
                        out.push(Node::Break);
                        continue;
                    }
                    let (next, mut bx) = styles(tag, style, font);
                    let kind = match tag.as_str() {
                        "table" | "thead" | "tbody" | "tfoot" | "ul" | "ol" | "dl" => Kind::Column,
                        "tr" => Kind::Row,
                        "td" | "th" => Kind::Cell,
                        "div" | "blockquote" => {
                            if children.iter().any(is_block) {
                                Kind::Column
                            } else {
                                Kind::Flow
                            }
                        }
                        _ => Kind::Flow,
                    };
                    let block = is_block(node) && !matches!(tag.as_str(), "td" | "th");
                    if block {
                        self.boundary("\n");
                    }
                    if matches!(tag.as_str(), "p" | "blockquote" | "pre") {
                        bx.margin[2] = bx.margin[2].max(4.);
                    }
                    if tag == "blockquote" {
                        bx.padding[3] = bx.padding[3].max(12.);
                    }
                    let mut content = Vec::new();
                    if tag == "li" {
                        let marker = if let Some(n) = list_index {
                            list_index = Some(n.saturating_add(1));
                            format!("{n}. ")
                        } else {
                            "• ".into()
                        };
                        content.extend(self.text(&marker, &next, false)?);
                    }
                    let child_list = if tag == "ol" {
                        Some(start.unwrap_or(1))
                    } else {
                        None
                    };
                    content.extend(self.nodes(children, &next, pre || tag == "pre", child_list)?);
                    if matches!(tag.as_str(), "td" | "th") {
                        self.boundary("\t");
                    }
                    out.push(Node::Box {
                        kind,
                        style: bx,
                        children: content,
                    });
                    if block {
                        self.boundary("\n");
                    }
                }
            }
        }
        Some(out)
    }
}
fn is_block(node: &SafeHtmlNode) -> bool {
    matches!(node,SafeHtmlNode::Element {tag,..} if matches!(tag.as_str(),"div"|"p"|"pre"|"blockquote"|"h1"|"h2"|"h3"|"h4"|"h5"|"h6"|"ul"|"ol"|"li"|"table"|"thead"|"tbody"|"tfoot"|"tr"|"dl"|"dt"|"dd"|"hr"))
}
impl Document {
    pub fn from_markdown(source: &str, size: f32, color: RgbaColor) -> Option<Self> {
        if source.len() > 65_536 {
            return None;
        }
        use pulldown_cmark::{Event, Options, Parser};
        let options =
            Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
        if !Parser::new_ext(source, options)
            .any(|e| matches!(e, Event::Html(_) | Event::InlineHtml(_)))
        {
            return None;
        }
        let safe = crate::markdown_content::markdown_preview_with_html(source, true);
        let mut html = String::new();
        pulldown_cmark::html::push_html(
            &mut html,
            Parser::new_ext(&safe, options).map(|event| match event {
                Event::TaskListMarker(checked) => {
                    Event::Text(if checked { "☑ " } else { "☐ " }.into())
                }
                _ => event,
            }),
        );
        let nodes = studyapp_core::safe_html_document(&html)?;
        let mut builder = Builder {
            runs: Vec::new(),
            separator: "",
            last_space: true,
        };
        let children = builder.nodes(&nodes, &Font::base(size, color), false, None)?;
        let mut plain = String::new();
        for run in &builder.runs {
            plain.push_str(run.separator);
            plain.push_str(&run.text);
        }
        Some(Self {
            root: Node::Box {
                kind: Kind::Column,
                style: BoxStyle::default(),
                children,
            },
            runs: builder.runs,
            plain,
        })
    }
    pub fn selected(&self, ranges: &[Option<std::ops::Range<usize>>]) -> String {
        let mut text = String::new();
        for (run, range) in self.runs.iter().zip(ranges) {
            if let Some(range) = range
                && !range.is_empty()
            {
                if !text.is_empty() {
                    text.push_str(run.separator);
                }
                text.push_str(&run.text[range.clone()]);
            }
        }
        text
    }
}
#[cfg(test)]
mod tests {
    use super::{Document, color};
    use studyapp_core::RgbaColor;
    #[test]
    fn inline_styles_become_real_run_metrics_and_colors_without_resource_attributes() {
        let source = "普通 <span style='color:rgba(255,0,0,0.5);font-size:24px;font-weight:700;line-height:2;text-decoration:underline;font-family:monospace'>重点中文🙂</span> 结束";
        let doc = Document::from_markdown(source, 14., RgbaColor::opaque(0)).unwrap();
        assert!(doc.plain.contains("重点中文🙂"));
        for run in doc.runs.iter().filter(|r| r.font.size == 24.) {
            assert_eq!(run.font.color, RgbaColor::new(0xff0000, 128));
            assert_eq!(run.font.weight, 700.);
            assert!(run.font.underline);
            assert_eq!(run.font.line_height, 48.);
        }
        assert!(doc.runs.iter().any(|r| r.font.size == 24.));
        assert_eq!(color("#1234"), Some(RgbaColor::new(0x112233, 0x44)));
    }
    #[test]
    fn mixed_markdown_preserves_lists_tasks_fences_and_safe_visible_copy() {
        let source = "3. **第三项**\n4. 第四项\n\n- [x] 完成\n\n<span style='color:red'>正文</span>\n\n```html\n<img src='file:///private'>\n```\n\n![图](file:///private)";
        let doc = Document::from_markdown(source, 14., RgbaColor::opaque(0)).unwrap();
        for expected in ["3. ", "4. ", "☑", "<img src='file:///private'>", "图片：图"] {
            assert!(doc.plain.contains(expected), "{}", doc.plain);
        }
        let ranges: Vec<_> = doc.runs.iter().map(|r| Some(0..r.text.len())).collect();
        assert_eq!(doc.selected(&ranges), doc.plain);
        assert!(Document::from_markdown("**普通 Markdown**", 14., RgbaColor::opaque(0)).is_none());
    }
}
