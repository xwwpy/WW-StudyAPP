//! Fixed shortcut allowlist shared by settings validation and native dispatch.
use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, BTreeSet};

pub struct ShortcutAction {
    pub id: &'static str,
    pub label: &'static str,
    pub group: &'static str,
    pub default: &'static str,
}

macro_rules! actions {
    ($(($id:literal, $label:literal, $group:literal, $default:literal)),* $(,)?) => {
        pub const SHORTCUT_ACTIONS: &[ShortcutAction] = &[
            $(ShortcutAction { id: $id, label: $label, group: $group, default: $default }),*
        ];
    };
}

actions![
    ("app.new", "新建笔记", "应用", "Ctrl+N"),
    ("app.import", "导入 Markdown", "应用", "Ctrl+O"),
    ("app.settings", "打开设置", "应用", "Ctrl+Comma"),
    ("app.library", "打开资料库", "应用", "Ctrl+Shift+L"),
    (
        "app.navigation-toggle",
        "显示或隐藏导航栏",
        "应用",
        "Ctrl+Shift+B"
    ),
    (
        "app.folders-toggle",
        "显示或隐藏文件夹栏",
        "应用",
        "Ctrl+Shift+E"
    ),
    ("app.search", "搜索笔记", "应用", "Ctrl+F"),
    ("editor.save", "保存笔记版本 / 设置", "应用", "Ctrl+S"),
    ("editor.bold", "粗体", "格式", "Ctrl+B"),
    ("editor.italic", "斜体", "格式", "Ctrl+I"),
    ("editor.heading", "标题", "格式", ""),
    ("editor.heading-1", "1 级标题", "格式", "Ctrl+1"),
    ("editor.heading-2", "2 级标题", "格式", "Ctrl+2"),
    ("editor.heading-3", "3 级标题", "格式", "Ctrl+3"),
    ("editor.heading-4", "4 级标题", "格式", "Ctrl+4"),
    ("editor.heading-5", "5 级标题", "格式", "Ctrl+5"),
    ("editor.heading-6", "6 级标题", "格式", "Ctrl+6"),
    ("editor.paragraph", "正文", "格式", "Ctrl+0"),
    ("editor.ordered", "有序列表", "格式", ""),
    ("editor.insert-image", "插入图片", "格式", ""),
    ("editor.code", "行内代码", "格式", ""),
    ("editor.bullet", "列表", "格式", ""),
    ("editor.task", "任务列表", "格式", ""),
    ("editor.quote", "引用", "格式", ""),
    ("editor.strike", "删除线", "格式", ""),
    ("editor.source", "切换源码模式", "笔记", ""),
    ("editor.undo", "撤销", "编辑", "Ctrl+Z"),
    ("editor.redo", "重做", "编辑", "Ctrl+Y;Ctrl+Shift+Z"),
    ("editor.undo-selection", "撤销选区", "编辑", "Ctrl+U"),
    ("editor.redo-selection", "重做选区", "编辑", "Alt+U"),
    ("editor.move-line-up", "上移行", "编辑", "Alt+ArrowUp"),
    ("editor.move-line-down", "下移行", "编辑", "Alt+ArrowDown"),
    (
        "editor.copy-line-up",
        "向上复制行",
        "编辑",
        "Alt+Shift+ArrowUp"
    ),
    (
        "editor.copy-line-down",
        "向下复制行",
        "编辑",
        "Alt+Shift+ArrowDown"
    ),
    (
        "editor.add-cursor-above",
        "在上方添加光标",
        "编辑",
        "Ctrl+Alt+ArrowUp"
    ),
    (
        "editor.add-cursor-below",
        "在下方添加光标",
        "编辑",
        "Ctrl+Alt+ArrowDown"
    ),
    ("editor.insert-blank-line", "插入空行", "编辑", "Ctrl+Enter"),
    ("editor.select-line", "选择当前行", "编辑", "Alt+L"),
    ("editor.select-parent-syntax", "选择上一级语法", "编辑", ""),
    ("editor.indent-less", "减少缩进", "编辑", "Ctrl+BracketLeft"),
    (
        "editor.indent-more",
        "增加缩进",
        "编辑",
        "Ctrl+BracketRight"
    ),
    (
        "editor.indent-selection",
        "整理选区缩进",
        "编辑",
        "Ctrl+Alt+Backslash"
    ),
    ("editor.delete-line", "删除当前行", "编辑", "Ctrl+Shift+K"),
    (
        "editor.matching-bracket",
        "跳到匹配括号",
        "编辑",
        "Ctrl+Shift+Backslash"
    ),
    ("editor.toggle-comment", "切换行注释", "编辑", "Ctrl+Slash"),
    (
        "editor.toggle-block-comment",
        "切换块注释",
        "编辑",
        "Alt+Shift+A"
    ),
    (
        "editor.toggle-tab-focus",
        "切换 Tab 焦点模式",
        "编辑",
        "Ctrl+M"
    ),
    ("editor.select-all", "全选正文", "编辑", "Ctrl+A"),
    (
        "editor.delete-group-backward",
        "向前删除一个词",
        "编辑",
        "Ctrl+Backspace"
    ),
    (
        "editor.delete-group-forward",
        "向后删除一个词",
        "编辑",
        "Ctrl+Delete"
    ),
];

pub fn default_bindings() -> BTreeMap<String, String> {
    SHORTCUT_ACTIONS
        .iter()
        .map(|action| (action.id.to_owned(), action.default.to_owned()))
        .collect()
}

/// Newly introduced defaults yield to an older explicit custom binding.
/// Existing actions and explicit new bindings retain strict conflict checking.
pub fn resolved_bindings(bindings: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut resolved = default_bindings();
    let explicit: BTreeSet<String> = bindings
        .values()
        .filter_map(|value| normalize_binding(value).ok())
        .flat_map(|value| {
            value
                .split(';')
                .filter(|key| !key.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .collect();
    for action in ["editor.paragraph".to_owned()]
        .into_iter()
        .chain((1..=6).map(|level| format!("editor.heading-{level}")))
    {
        if !bindings.contains_key(&action)
            && resolved
                .get(&action)
                .is_some_and(|binding| explicit.contains(binding))
        {
            resolved.insert(action, String::new());
        }
    }
    resolved.extend(bindings.clone());
    resolved
}

#[derive(Debug, PartialEq, Eq)]
pub enum CaptureKey {
    Pending,
    Cancel,
    Binding(String),
    Invalid(String),
}

/// Capture consumes every event while armed, including invalid or modifier keys.
pub fn capture_key(key: &str, ctrl: bool, alt: bool, shift: bool, platform: bool) -> CaptureKey {
    if matches!(key.to_ascii_lowercase().as_str(), "escape" | "esc") {
        return CaptureKey::Cancel;
    }
    if matches!(
        key.to_ascii_lowercase().as_str(),
        "ctrl" | "control" | "alt" | "shift" | "meta" | "super" | "win" | "command" | "fn"
    ) {
        return CaptureKey::Pending;
    }
    if platform {
        return CaptureKey::Invalid("Windows 系统组合键不能录制".into());
    }
    match key_combination(key, ctrl, alt, shift) {
        Ok(binding) => CaptureKey::Binding(binding),
        Err(error) => CaptureKey::Invalid(error.to_string()),
    }
}

/// Format a live platform event before applying the binding delimiter grammar.
/// Windows GPUI consumes Shift into punctuation (for example `+` with shift=false),
/// so recover it from the symbol before canonicalizing the key token. Explicit
/// registry tokens such as Equal keep the modifier flags supplied by the caller.
pub fn key_combination(key: &str, ctrl: bool, alt: bool, shift: bool) -> Result<String> {
    let (key, symbol_shift) = match key {
        "!" => ("1", true),
        "@" => ("2", true),
        "#" => ("3", true),
        "$" => ("4", true),
        "%" => ("5", true),
        "^" => ("6", true),
        "&" => ("7", true),
        "*" => ("8", true),
        "(" => ("9", true),
        ")" => ("0", true),
        "+" => ("Equal", true),
        "_" => ("Minus", true),
        ":" => ("Semicolon", true),
        "\"" => ("Quote", true),
        "<" => ("Comma", true),
        ">" => ("Period", true),
        "?" => ("Slash", true),
        "|" => ("Backslash", true),
        "{" => ("BracketLeft", true),
        "}" => ("BracketRight", true),
        "~" => ("Backquote", true),
        other => (other, false),
    };
    let key = normalize_key(key)?;
    normalize_binding(&format!(
        "{}{}{}{}",
        if ctrl { "Ctrl+" } else { "" },
        if alt { "Alt+" } else { "" },
        if shift || symbol_shift { "Shift+" } else { "" },
        key
    ))
}

pub fn record_binding(previous: &str, captured: &str, append: bool) -> Result<String> {
    if append && !previous.trim().is_empty() {
        normalize_binding(&format!("{previous};{captured}"))
    } else {
        normalize_binding(captured)
    }
}

/// Return every participant, including conflicts against omitted default bindings.
pub fn binding_conflicts(bindings: &BTreeMap<String, String>) -> BTreeMap<String, Vec<String>> {
    let resolved = resolved_bindings(bindings);
    let mut owners: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (action, binding) in resolved {
        for alias in binding.split(';').filter(|alias| !alias.trim().is_empty()) {
            if let Ok(alias) = normalize_binding(alias) {
                let owners = owners.entry(alias).or_default();
                if !owners.contains(&action) {
                    owners.push(action.clone());
                }
            }
        }
    }
    let mut conflicts: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for owners in owners.values().filter(|owners| owners.len() > 1) {
        for action in owners {
            let others = conflicts.entry(action.clone()).or_default();
            for other in owners.iter().filter(|other| *other != action) {
                if !others.contains(other) {
                    others.push(other.clone());
                }
            }
        }
    }
    conflicts
}

/// Missing actions use defaults, while an explicit empty value unbinds an action.
pub fn validate_bindings(bindings: &BTreeMap<String, String>) -> Result<()> {
    let label = |id: &str| {
        SHORTCUT_ACTIONS
            .iter()
            .find(|action| action.id == id)
            .map(|action| action.label)
            .unwrap_or("未识别操作")
    };
    let mut resolved = resolved_bindings(bindings);
    for (action, binding) in bindings {
        if !SHORTCUT_ACTIONS.iter().any(|known| known.id == action) {
            bail!("包含不支持的快捷操作，请恢复默认后重新设置");
        }
        resolved.insert(
            action.clone(),
            normalize_binding(binding)
                .with_context(|| format!("「{}」的快捷键无效", label(action)))?,
        );
    }
    let mut used = BTreeMap::new();
    for (action, binding) in &resolved {
        for alternative in binding.split(';').filter(|value| !value.is_empty()) {
            if let Some(previous) = used.insert(alternative, action) {
                bail!(
                    "{alternative} 同时用于「{}」和「{}」",
                    label(previous),
                    label(action)
                );
            }
        }
    }
    Ok(())
}

/// Canonical key tokens also accept GPUI/browser names and punctuation.
pub fn normalize_key(value: &str) -> Result<String> {
    let lower = value.to_ascii_lowercase();
    if lower.len() == 1 && lower.as_bytes()[0].is_ascii_alphanumeric() {
        return Ok(lower.to_ascii_uppercase());
    }
    if let Some(number) = lower
        .strip_prefix('f')
        .and_then(|value| value.parse::<u8>().ok())
        && (1..=24).contains(&number)
        && lower == format!("f{number}")
    {
        return Ok(format!("F{number}"));
    }
    let key = match lower.as_str() {
        "enter" | "return" => "Enter",
        "tab" => "Tab",
        "escape" | "esc" => "Escape",
        "space" | "spacebar" | " " => "Space",
        "backspace" => "Backspace",
        "delete" | "del" => "Delete",
        "insert" => "Insert",
        "home" => "Home",
        "end" => "End",
        "pageup" => "PageUp",
        "pagedown" => "PageDown",
        "arrowleft" | "left" => "ArrowLeft",
        "arrowright" | "right" => "ArrowRight",
        "arrowup" | "up" => "ArrowUp",
        "arrowdown" | "down" => "ArrowDown",
        "comma" | "," | "<" => "Comma",
        "period" | "." | ">" => "Period",
        "slash" | "/" | "?" => "Slash",
        "backslash" | "\\" | "|" => "Backslash",
        "semicolon" | ";" | ":" => "Semicolon",
        "quote" | "'" | "\"" => "Quote",
        "backquote" | "`" | "~" => "Backquote",
        "minus" | "-" | "_" => "Minus",
        "equal" | "=" | "+" => "Equal",
        "bracketleft" | "[" | "{" => "BracketLeft",
        "bracketright" | "]" | "}" => "BracketRight",
        _ => bail!("无法识别按键：{value}"),
    };
    Ok(key.to_owned())
}

fn reserved(key: &str, ctrl: bool, alt: bool, shift: bool) -> bool {
    (alt && matches!(key, "Tab" | "Escape" | "F4" | "Space"))
        || (ctrl && matches!(key, "Escape" | "Space"))
        || (ctrl && alt && key == "Delete")
        || (shift && key == "Space")
        || (ctrl && !alt && !shift && matches!(key, "C" | "X" | "V" | "Insert"))
        || (shift && !ctrl && !alt && matches!(key, "Insert" | "Delete"))
        || (ctrl && !alt && matches!(key, "ArrowLeft" | "ArrowRight" | "Home" | "End"))
        || (alt && !ctrl && matches!(key, "ArrowLeft" | "ArrowRight"))
        || (!ctrl
            && !alt
            && matches!(
                key,
                "ArrowLeft"
                    | "ArrowRight"
                    | "ArrowUp"
                    | "ArrowDown"
                    | "Home"
                    | "End"
                    | "PageUp"
                    | "PageDown"
                    | "Tab"
                    | "Enter"
                    | "Backspace"
            ))
}

pub fn normalize_binding(binding: &str) -> Result<String> {
    if binding.len() > 512 {
        bail!("快捷键最多 512 字符");
    }
    if binding.trim().is_empty() {
        return Ok(String::new());
    }
    let mut result = Vec::new();
    for alternative in binding.split(';') {
        if result.len() >= 8 {
            bail!("每个操作最多设置 8 个快捷键");
        }
        let mut modifiers = BTreeSet::new();
        let mut key = None;
        for part in alternative.split('+').map(str::trim) {
            let modifier = match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => Some("Ctrl"),
                "alt" => Some("Alt"),
                "shift" => Some("Shift"),
                _ => None,
            };
            if let Some(modifier) = modifier {
                if !modifiers.insert(modifier) {
                    bail!("快捷键修饰键重复");
                }
            } else {
                if key.is_some() || part.is_empty() {
                    bail!("快捷键格式无效");
                }
                key = Some(normalize_key(part)?);
            }
        }
        let Some(key) = key else {
            bail!("快捷键缺少按键")
        };
        let ctrl = modifiers.contains("Ctrl");
        let alt = modifiers.contains("Alt");
        let shift = modifiers.contains("Shift");
        if !ctrl && !alt && !(key.starts_with('F') && key.len() > 1) {
            bail!("请使用 Ctrl、Alt 组合键或功能键");
        }
        if reserved(&key, ctrl, alt, shift) {
            bail!("此快捷键由系统、输入法或文本导航保留");
        }
        let mut parts: Vec<&str> = ["Ctrl", "Alt", "Shift"]
            .into_iter()
            .filter(|modifier| modifiers.contains(modifier))
            .collect();
        parts.push(&key);
        let normalized = parts.join("+");
        if result.contains(&normalized) {
            bail!("同一操作的快捷键重复");
        }
        result.push(normalized);
    }
    Ok(result.join(";"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_complete_unique_and_normalized() {
        let bindings = default_bindings();
        assert_eq!(bindings.len(), SHORTCUT_ACTIONS.len());
        validate_bindings(&bindings).unwrap();
        for action in SHORTCUT_ACTIONS {
            assert_eq!(normalize_binding(action.default).unwrap(), action.default);
        }
    }

    #[test]
    fn normalizes_aliases_order_and_function_keys() {
        assert_eq!(
            normalize_binding("shift + control + s ; f8").unwrap(),
            "Ctrl+Shift+S;F8"
        );
        assert_eq!(
            normalize_binding("alt+ctrl+[").unwrap(),
            "Ctrl+Alt+BracketLeft"
        );
        assert_eq!(normalize_binding("   ").unwrap(), "");
        assert_eq!(normalize_key("?").unwrap(), "Slash");
    }

    #[test]
    fn rejects_invalid_reserved_or_repeated_keys() {
        for binding in [
            "S",
            "Shift+S",
            "Ctrl",
            "Ctrl++S",
            "Ctrl+Ctrl+S",
            "Ctrl+S;",
            "Ctrl+Z;control+z",
            "Win+S",
            "Meta+S",
            "Alt+F4",
            "Alt+Tab",
            "Alt+Space",
            "Ctrl+Space",
            "Ctrl+Shift+Space",
            "Ctrl+Alt+Delete",
            "Ctrl+Shift+Escape",
            "Ctrl+C",
            "Shift+Insert",
            "Ctrl+Home",
            "Alt+ArrowLeft",
            "F25",
            "F01",
            "Ctrl+DoesNotExist",
        ] {
            assert!(normalize_binding(binding).is_err(), "{binding}");
        }
    }

    #[test]
    fn validates_effective_defaults_unknown_actions_and_unbinding() {
        validate_bindings(&BTreeMap::new()).unwrap();
        let mut bindings = BTreeMap::from([("editor.save".to_owned(), "ctrl+b".to_owned())]);
        assert!(validate_bindings(&bindings).is_err());
        bindings.insert("editor.bold".into(), "".into());
        validate_bindings(&bindings).unwrap();
        bindings.insert("app.arbitrary-code".into(), "Ctrl+Q".into());
        assert!(validate_bindings(&bindings).is_err());
    }

    #[test]
    fn additive_heading_defaults_preserve_old_digit_customizations() {
        let mut old = BTreeMap::from([("editor.bold".to_owned(), "Ctrl+1".to_owned())]);
        validate_bindings(&old).unwrap();
        assert_eq!(resolved_bindings(&old)["editor.heading-1"], "");
        assert_eq!(resolved_bindings(&old)["editor.heading-2"], "Ctrl+2");
        old.insert("editor.heading-1".into(), "Ctrl+1".into());
        assert!(validate_bindings(&old).is_err());
    }
}

#[cfg(test)]
mod recording_tests {
    use super::*;

    #[test]
    fn punctuation_capture_and_dispatch_use_canonical_key_tokens() {
        for (key, shift, expected) in [
            (";", false, "Ctrl+Semicolon"),
            ("=", false, "Ctrl+Equal"),
            ("+", false, "Ctrl+Shift+Equal"),
            ("+", true, "Ctrl+Shift+Equal"),
            (":", false, "Ctrl+Shift+Semicolon"),
            ("!", false, "Ctrl+Shift+1"),
            ("?", false, "Ctrl+Shift+Slash"),
            ("{", false, "Ctrl+Shift+BracketLeft"),
            ("Semicolon", false, "Ctrl+Semicolon"),
        ] {
            assert_eq!(
                capture_key(key, true, false, shift, false),
                CaptureKey::Binding(expected.into()),
                "{key}"
            );
            assert_eq!(
                key_combination(key, true, false, shift).unwrap(),
                expected,
                "dispatch {key}"
            );
            assert_eq!(normalize_binding(expected).unwrap(), expected);
        }
    }

    #[test]
    fn recording_consumes_commands_ignores_modifiers_and_cancels_escape() {
        assert_eq!(
            capture_key("Control", true, false, false, false),
            CaptureKey::Pending
        );
        assert_eq!(
            capture_key("Shift", false, false, true, false),
            CaptureKey::Pending
        );
        assert_eq!(
            capture_key("escape", true, false, false, false),
            CaptureKey::Cancel
        );
        assert_eq!(
            capture_key("s", true, false, false, false),
            CaptureKey::Binding("Ctrl+S".into())
        );
        assert!(matches!(
            capture_key("s", false, false, false, false),
            CaptureKey::Invalid(_)
        ));
    }

    #[test]
    fn live_conflicts_include_every_alias_owner_and_save_rejects_them() {
        let mut bindings = default_bindings();
        bindings.insert("app.new".into(), "control+s;F8".into());
        bindings.insert("app.import".into(), "f8".into());
        let conflicts = binding_conflicts(&bindings);
        assert_eq!(conflicts["app.new"].len(), 2);
        assert!(conflicts["editor.save"].contains(&"app.new".to_owned()));
        assert!(conflicts["app.import"].contains(&"app.new".to_owned()));
        assert!(validate_bindings(&bindings).is_err());
    }

    #[test]
    fn recording_appends_normalized_alternatives_without_losing_existing() {
        assert_eq!(
            record_binding("control+y", "Ctrl+Shift+Z", true).unwrap(),
            "Ctrl+Y;Ctrl+Shift+Z"
        );
        assert_eq!(record_binding("Ctrl+Y", "F8", false).unwrap(), "F8");
        assert!(record_binding("Ctrl+Y", "Ctrl+Y", true).is_err());
    }
}
