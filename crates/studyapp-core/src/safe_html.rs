//! Bounded, offline formatting-only HTML. Never reuse untrusted DOM/attributes.
use html5ever::{QualName, local_name, namespace_url, ns, parse_fragment, tendril::TendrilSink};
use markup5ever_rcdom::{Handle, NodeData, RcDom};
use serde::Deserialize;
use std::{collections::BTreeMap, sync::OnceLock};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Policy {
    input_bytes: usize,
    output_bytes: usize,
    nodes: usize,
    depth: usize,
    tags: Vec<String>,
    blocked: Vec<String>,
    colors: Vec<String>,
    color_properties: Vec<String>,
    enums: BTreeMap<String, Vec<String>>,
    numbers: BTreeMap<String, NumberRule>,
    integer_attributes: BTreeMap<String, BTreeMap<String, [u32; 2]>>,
}
#[derive(Deserialize)]
struct NumberRule {
    count: usize,
    units: BTreeMap<String, [f64; 2]>,
}
fn policy() -> &'static Policy {
    static POLICY: OnceLock<Policy> = OnceLock::new();
    POLICY.get_or_init(|| {
        serde_json::from_str(include_str!("html_policy.json")).expect("HTML policy")
    })
}
fn decimal(value: &str) -> Option<f64> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return None;
    }
    value.parse::<f64>().ok().filter(|v| v.is_finite())
}
fn safe_color(value: &str) -> bool {
    if policy().colors.iter().any(|v| v == value) {
        return true;
    }
    if let Some(hex) = value.strip_prefix('#') {
        return matches!(hex.len(), 3 | 4 | 6 | 8) && hex.bytes().all(|b| b.is_ascii_hexdigit());
    }
    let (body, alpha) = if let Some(v) = value.strip_prefix("rgba(") {
        (v, true)
    } else if let Some(v) = value.strip_prefix("rgb(") {
        (v, false)
    } else {
        return false;
    };
    let Some(body) = body.strip_suffix(')') else {
        return false;
    };
    let parts: Vec<_> = body.split(',').map(str::trim).collect();
    if parts.len() != if alpha { 4 } else { 3 } {
        return false;
    }
    parts[..3]
        .iter()
        .all(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) && v.parse::<u8>().is_ok())
        && (!alpha || decimal(parts[3]).is_some_and(|v| (0.0..=1.0).contains(&v)))
}
fn safe_number(value: &str, rule: &NumberRule) -> bool {
    let parts: Vec<_> = value.split_ascii_whitespace().collect();
    !parts.is_empty()
        && parts.len() <= rule.count
        && parts.iter().all(|value| {
            rule.units.iter().any(|(unit, [min, max])| {
                value
                    .strip_suffix(unit)
                    .and_then(decimal)
                    .is_some_and(|v| (*min..=*max).contains(&v))
            }) || (*value == "0"
                && rule
                    .units
                    .values()
                    .any(|[min, max]| (*min..=*max).contains(&0.0)))
        })
}
/// Only finite static formatting declarations; no selectors, URLs or CSS escape syntax.
pub fn sanitize_html_style(source: &str) -> String {
    if source.len() > 2048 {
        return String::new();
    }
    let mut declarations = BTreeMap::new();
    for declaration in source.split(';').take(64) {
        let Some((key, value)) = declaration.split_once(':') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim().to_ascii_lowercase();
        if value.is_empty()
            || value.len() > 96
            || !value.is_ascii()
            || value
                .bytes()
                .any(|b| b.is_ascii_control() || b"\\{}!@\"'<>/".contains(&b))
        {
            continue;
        }
        let valid = if policy().color_properties.contains(&key) {
            safe_color(&value)
        } else if let Some(values) = policy().enums.get(&key) {
            values.contains(&value)
        } else if let Some(rule) = policy().numbers.get(&key) {
            safe_number(&value, rule)
        } else {
            false
        };
        if valid {
            declarations.insert(key, value);
        }
    }
    declarations
        .into_iter()
        .map(|(k, v)| format!("{k}:{v}"))
        .collect::<Vec<_>>()
        .join(";")
}
fn escaped(output: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&#39;"),
            _ => output.push(ch),
        }
    }
}
fn visit(node: &Handle, depth: usize, remaining: &mut usize, output: &mut String) -> Option<()> {
    if depth > policy().depth || *remaining == 0 {
        return None;
    }
    *remaining -= 1;
    let mut close = None;
    let mut pre_content_start = None;
    match &node.data {
        NodeData::Text { contents } => escaped(output, &contents.borrow()),
        NodeData::Element { name, attrs, .. } => {
            if name.ns != ns!(html)
                || policy()
                    .blocked
                    .iter()
                    .any(|tag| tag == name.local.as_ref())
            {
                return Some(());
            }
            let tag = name.local.as_ref();
            if tag == "img" {
                output.push_str("[图片：");
                let attrs = attrs.borrow();
                if let Some(alt) = attrs
                    .iter()
                    .find(|a| a.name.ns == ns!() && a.name.local.as_ref() == "alt")
                {
                    escaped(output, &alt.value);
                } else {
                    output.push_str("图片");
                }
                output.push(']');
            } else if policy().tags.iter().any(|v| v == tag) {
                output.push('<');
                output.push_str(tag);
                if let Some(style) = attrs
                    .borrow()
                    .iter()
                    .find(|a| a.name.ns == ns!() && a.name.local.as_ref() == "style")
                {
                    let style = sanitize_html_style(&style.value);
                    if !style.is_empty() {
                        output.push_str(" style=\"");
                        escaped(output, &style);
                        output.push('"');
                    }
                }
                if let Some(rules) = policy().integer_attributes.get(tag) {
                    for (key, [min, max]) in rules {
                        if let Some(value) = attrs
                            .borrow()
                            .iter()
                            .find(|a| a.name.ns == ns!() && a.name.local.as_ref() == key)
                            .filter(|a| {
                                !a.value.is_empty() && a.value.bytes().all(|b| b.is_ascii_digit())
                            })
                            .and_then(|a| a.value.parse::<u32>().ok())
                            .filter(|n| (*min..=*max).contains(n))
                        {
                            output.push_str(&format!(" {key}=\"{value}\""));
                        }
                    }
                }
                output.push('>');
                if tag == "pre" {
                    pre_content_start = Some(output.len());
                }
                if !matches!(tag, "br" | "hr") {
                    close = Some(tag);
                }
            }
        }
        _ => {}
    }
    if output.len() > policy().output_bytes {
        return None;
    }
    for child in node.children.borrow().iter() {
        visit(child, depth + 1, remaining, output)?;
    }
    if let Some(start) = pre_content_start
        && output[start..].starts_with(char::from(10))
    {
        // HTML parsers discard one LF immediately after <pre>. An empty safe
        // span protects an already-parsed leading LF on subsequent passes.
        output.insert_str(start, "<span></span>");
    }
    if let Some(tag) = close {
        output.push_str("</");
        output.push_str(tag);
        output.push('>');
    }
    (output.len() <= policy().output_bytes).then_some(())
}
/// Returns `None` on a resource limit. Callers must display the original as text.
/// All links become labels and images become placeholders; no I/O is performed.
pub fn sanitize_html(source: &str) -> Option<String> {
    if source.len() > policy().input_bytes {
        return None;
    }
    let dom = parse_fragment(
        RcDom::default(),
        Default::default(),
        QualName::new(None, ns!(html), local_name!("div")),
        vec![],
    )
    .one(source);
    let mut output = String::new();
    let mut remaining = policy().nodes;
    visit(&dom.document, 0, &mut remaining, &mut output)?;
    Some(output)
}

/// A single formatting tag for a Markdown inline stream. Never returns text or
/// children which could be reinterpreted as Markdown resource/link syntax.
pub fn sanitize_inline_html(source: &str) -> Option<String> {
    if source.len() > policy().input_bytes {
        return None;
    }
    let source = source.trim();
    if let Some(closing) = source.strip_prefix("</").and_then(|v| v.strip_suffix('>')) {
        let tag = closing.trim().to_ascii_lowercase();
        return (tag.bytes().all(|b| b.is_ascii_alphanumeric())
            && policy().tags.contains(&tag)
            && !matches!(tag.as_str(), "br" | "hr"))
        .then(|| format!("</{tag}>"));
    }
    let tag = source
        .strip_prefix('<')?
        .split(|c: char| !c.is_ascii_alphanumeric())
        .next()?
        .to_ascii_lowercase();
    if !policy().tags.contains(&tag) {
        return None;
    }
    let clean = sanitize_html(source)?;
    let opening = if matches!(tag.as_str(), "br" | "hr") {
        clean.as_str()
    } else {
        clean.strip_suffix(&format!("</{tag}>"))?
    };
    (opening.starts_with(&format!("<{tag}"))
        && opening.ends_with('>')
        && opening.bytes().filter(|b| *b == b'>').count() == 1)
        .then(|| opening.to_owned())
}

/// Resource discovery only. Returned paths are resolved by the bounded image worker,
/// never by adopting an untrusted HTML element or granting WebView file access.
pub fn html_image_sources(source: &str) -> Vec<String> {
    if source.len() > policy().input_bytes {
        return Vec::new();
    }
    let dom = parse_fragment(
        RcDom::default(),
        Default::default(),
        QualName::new(None, ns!(html), local_name!("div")),
        vec![],
    )
    .one(source);
    // Keep the root alive: RcDom node destruction drains descendant lists.
    let mut stack = vec![(dom.document.clone(), 0)];
    let mut remaining = policy().nodes;
    let mut result = Vec::new();
    while let Some((node, depth)) = stack.pop() {
        if remaining == 0 || result.len() >= 200 {
            break;
        }
        remaining -= 1;
        if depth > policy().depth {
            continue;
        }
        if let NodeData::Element { name, attrs, .. } = &node.data {
            if name.ns != ns!(html)
                || policy()
                    .blocked
                    .iter()
                    .any(|tag| tag == name.local.as_ref())
            {
                continue;
            }
            if name.local.as_ref() == "img"
                && let Some(value) = attrs
                    .borrow()
                    .iter()
                    .find(|attr| attr.name.ns == ns!() && attr.name.local.as_ref() == "src")
                    .map(|attr| attr.value.to_string())
                    .filter(|value| !value.is_empty() && value.len() <= 8192)
            {
                result.push(value);
            }
        }
        stack.extend(
            node.children
                .borrow()
                .iter()
                .rev()
                .cloned()
                .map(|child| (child, depth + 1)),
        );
    }
    result
}

#[cfg(test)]
mod resource_discovery_tests {
    #[test]
    fn images_are_discovered_in_html_namespace_only_and_entities_are_decoded() {
        let sources = super::html_image_sources(
            "<img src='https://example.com/a.png?x=1&amp;y=2'><script><img src=bad></script><svg><image href=bad /></svg>",
        );
        assert_eq!(sources, vec!["https://example.com/a.png?x=1&y=2"]);
        assert!(super::html_image_sources(&"x".repeat(65537)).is_empty());
    }
}
