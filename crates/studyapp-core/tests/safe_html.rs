use studyapp_core::sanitize_html;

#[test]
fn formatting_and_bounded_inline_styles_survive() {
    let clean = sanitize_html(r#"<div style="text-align:center;color:#123456;font-size:24px;padding:8px"><strong>中文 &amp; 😀</strong><br><span style="background-color:rgba(1,2,3,0.5)">重点</span></div>"#).unwrap();
    assert!(clean.contains("text-align:center"));
    assert!(clean.contains("font-size:24px"));
    assert!(clean.contains("padding:8px"));
    assert!(clean.contains("<strong>中文 &amp; 😀</strong>"));
    assert!(clean.contains("background-color:rgba(1,2,3,0.5)"));
}

#[test]
fn executable_resources_namespaces_and_host_attributes_are_removed() {
    let clean = sanitize_html(r#"<p id="editor" class="cm-content" data-action="save" onclick="bad()" tabindex="0">安全<script>secret()</script><style>*{display:none}</style><iframe src="file:///private">hidden</iframe><svg><foreignObject>hidden</foreignObject></svg><math><mi>hidden</mi></math><form><input autofocus>hidden</form><template>hidden</template><a href="javascript:bad()">链接</a><img src="https://outside/image" onerror="bad()" alt="图"></p>"#).unwrap();
    assert!(clean.contains("安全"));
    assert!(clean.contains("链接"));
    assert!(clean.contains("[图片：图]"));
    for forbidden in [
        "secret", "hidden", "onclick", "src=", "href=", "data-", "class=", "id=", "tabindex",
        "<svg", "<math", "<script", "<form", "<img",
    ] {
        assert!(!clean.contains(forbidden), "{forbidden}: {clean}");
    }
}

#[test]
fn unsafe_css_and_out_of_bounds_values_never_survive() {
    let clean = sanitize_html(r#"<span style="color:red;background-image:url(file:///private);position:fixed;z-index:999999;display:none;opacity:0;font-size:9999px;margin:-20px;padding:1000px;width:9999%;font-family:evil;filter:blur(5px);background-color:var(--secret);border-color:expression(bad());text-align:right">text</span>"#).unwrap();
    assert!(clean.contains("color:red"));
    assert!(clean.contains("text-align:right"));
    for forbidden in [
        "url",
        "position",
        "z-index",
        "display",
        "opacity",
        "9999",
        "-20",
        "1000",
        "evil",
        "filter",
        "var(",
        "expression",
    ] {
        assert!(!clean.contains(forbidden), "{forbidden}: {clean}");
    }
    for style in [
        r"co\6cor:red",
        "color:r/**/ed",
        "color:red!important",
        "color:inherit",
        "color:currentColor",
        "color:red\nposition:fixed",
    ] {
        assert_eq!(
            sanitize_html(&format!("<b style=\"{style}\">text</b>")).unwrap(),
            "<b>text</b>"
        );
    }
}

#[test]
fn parser_repair_and_entity_decoding_cannot_introduce_active_markup() {
    for source in [
        "<div><b><i>malformed</b>rest</i></div>",
        "<p title='&quot; onmouseover=&quot;bad'>text</p>",
        "&lt;script&gt;bad&lt;/script&gt;",
        "<IMG SRC=JaVaScRiPt:bad><a href='file:///private'>label</a>",
        "<svg><style><img src=x onerror=bad></style></svg><b>safe</b>",
        "<math><mtext><table><mglyph><style><!--</style><img title='--><img src=x onerror=bad>'>",
    ] {
        let clean = sanitize_html(source).unwrap();
        assert_eq!(
            sanitize_html(&clean).unwrap(),
            clean,
            "not idempotent: {source}"
        );
        for forbidden in [
            " on", " src=", " href=", "<script", "<style", "<svg", "<math", "<img",
        ] {
            assert!(!clean.contains(forbidden), "{forbidden}: {clean}");
        }
    }
}

#[test]
fn limits_fail_closed_without_truncating_or_modifying_source() {
    let large = "x".repeat(65_537);
    assert!(sanitize_html(&large).is_none());
    let deep = format!("{}text{}", "<div>".repeat(40), "</div>".repeat(40));
    assert!(sanitize_html(&deep).is_none());
    assert!(sanitize_html(&"<br>".repeat(4097)).is_none());
    assert_eq!(sanitize_html("普通文字"), Some("普通文字".into()));
}

#[test]
fn inline_tags_cannot_introduce_text_or_reinterpret_markdown() {
    use studyapp_core::sanitize_inline_html as tag;
    assert_eq!(tag("<STRONG onclick='bad()'>"), Some("<strong>".into()));
    assert_eq!(tag("</strong>"), Some("</strong>".into()));
    assert_eq!(
        tag("<span style='color:red;position:fixed'>"),
        Some("<span style=\"color:red\">".into())
    );
    assert_eq!(tag("<br>"), Some("<br>".into()));
    assert!(tag(&format!("<b {}>", " ".repeat(65_537))).is_none());
    for source in [
        "<script>",
        "<img src='file:///private'>",
        "<a href='https://outside'>",
        "<b>![a](https://outside)</b>",
        "<b><i>",
        "</b onclick=bad>",
        "<svg>",
    ] {
        assert!(tag(source).is_none(), "{source}");
    }
}

#[test]
fn old_appearance_defaults_to_safe_html_and_false_round_trips() {
    let mut appearance: studyapp_core::AppearanceSettings = serde_json::from_str("{}").unwrap();
    assert!(appearance.render_html);
    appearance.render_html = false;
    let restored: studyapp_core::AppearanceSettings =
        serde_json::from_str(&serde_json::to_string(&appearance).unwrap()).unwrap();
    assert!(!restored.render_html);
}

#[test]
fn shared_browser_and_native_css_corpus() {
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/html-safety.json")).unwrap();
    for pair in cases["styles"].as_array().unwrap() {
        assert_eq!(
            studyapp_core::sanitize_html_style(pair[0].as_str().unwrap()),
            pair[1].as_str().unwrap(),
            "{}",
            pair[0]
        );
    }
    for raw in cases["html"].as_array().unwrap() {
        let clean = studyapp_core::sanitize_html(raw.as_str().unwrap()).unwrap();
        assert_eq!(studyapp_core::sanitize_html(&clean).unwrap(), clean);
        for forbidden in [
            " src=",
            " href=",
            " data-",
            " onclick=",
            " onerror=",
            "<script",
            "<svg",
            "<math",
            "<iframe",
            "<img",
            "<form",
            "<object",
        ] {
            assert!(!clean.contains(forbidden), "{clean}");
        }
    }
}

#[test]
fn native_tree_has_only_sanitized_presentation_nodes_and_keeps_ordered_starts() {
    use studyapp_core::{SafeHtmlNode, safe_html_document};
    let source = "<ol start='3' onclick='bad()'><li><span style='color:red;font-size:24px;position:fixed'>正文</span><img src='file:///private' alt='图'></li></ol>";
    let nodes = safe_html_document(source).unwrap();
    let SafeHtmlNode::Element {
        tag,
        start,
        children,
        ..
    } = &nodes[0]
    else {
        panic!("ordered list")
    };
    assert_eq!(tag, "ol");
    assert_eq!(*start, Some(3));
    assert!(!children.is_empty());
    let dump = format!("{nodes:?}");
    assert!(dump.contains("font-size:24px"));
    assert!(dump.contains("图片：图"));
    for forbidden in ["file:///", "position", "onclick", "src="] {
        assert!(!dump.contains(forbidden));
    }
    assert!(safe_html_document(&"<div>".repeat(40)).is_none());
    assert_eq!(
        studyapp_core::sanitize_html("<ol start='1000001'><li>x</li></ol>").unwrap(),
        "<ol><li>x</li></ol>"
    );
}

#[test]
fn ordered_start_attributes_use_the_shared_bounded_decimal_grammar() {
    for (raw, expected) in [
        ("0", Some("0")),
        ("0003", Some("3")),
        ("1000000", Some("1000000")),
        ("+3", None),
        ("-1", None),
        (" 3", None),
        ("3 ", None),
        ("3.0", None),
        ("1e3", None),
        ("1000001", None),
        ("", None),
    ] {
        let source = format!("<ol start=\"{raw}\"><li>entry</li></ol>");
        let attr = expected
            .map(|value| format!(" start=\"{value}\""))
            .unwrap_or_default();
        assert_eq!(
            studyapp_core::sanitize_html(&source).unwrap(),
            format!("<ol{attr}><li>entry</li></ol>"),
            "{raw}"
        );
    }
}
