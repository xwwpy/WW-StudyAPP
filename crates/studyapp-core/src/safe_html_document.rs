//! A presentation-only tree derived from the same bounded sanitizer as WebView HTML.
use html5ever::{QualName, local_name, namespace_url, ns, parse_fragment, tendril::TendrilSink};
use markup5ever_rcdom::{Handle, NodeData, RcDom};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SafeHtmlNode {
    Text(String),
    Element {
        tag: String,
        style: String,
        start: Option<u32>,
        children: Vec<Self>,
    },
}
fn collect(node: &Handle, depth: usize, remaining: &mut usize) -> Option<Vec<SafeHtmlNode>> {
    if depth > 32 || *remaining == 0 {
        return None;
    }
    *remaining -= 1;
    if let NodeData::Text { contents } = &node.data {
        return Some(vec![SafeHtmlNode::Text(contents.borrow().to_string())]);
    }
    let mut children = Vec::new();
    for child in node.children.borrow().iter() {
        children.extend(collect(child, depth + 1, remaining)?);
    }
    if let NodeData::Element { name, attrs, .. } = &node.data {
        if name.local.as_ref() == "html" {
            return Some(children);
        }
        let attrs = attrs.borrow();
        let style = attrs
            .iter()
            .find(|a| a.name.local.as_ref() == "style")
            .map(|a| a.value.to_string())
            .unwrap_or_default();
        let start = attrs
            .iter()
            .find(|a| a.name.local.as_ref() == "start")
            .and_then(|a| a.value.parse().ok());
        return Some(vec![SafeHtmlNode::Element {
            tag: name.local.to_string(),
            style,
            start,
            children,
        }]);
    }
    Some(children)
}
/// No resources, links, host attributes or executable nodes can enter this tree.
/// Limit failures return `None`; the caller must retain the original as literal text.
pub fn safe_html_document(source: &str) -> Option<Vec<SafeHtmlNode>> {
    let clean = crate::sanitize_html(source)?;
    let dom = parse_fragment(
        RcDom::default(),
        Default::default(),
        QualName::new(None, ns!(html), local_name!("div")),
        vec![],
    )
    .one(clean);
    collect(&dom.document, 0, &mut 4096)
}
