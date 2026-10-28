import { defaultKeymap } from "@codemirror/commands";
import type { KeyBinding } from "@codemirror/view";

// Kept in sync with the native allowlist by the registry parity test.
export const shortcutDefaults: Readonly<Record<string, string>> = Object.freeze({
  "app.new": "Ctrl+N",
  "app.import": "Ctrl+O",
  "app.settings": "Ctrl+Comma",
  "app.library": "Ctrl+Shift+L",
  "app.navigation-toggle": "Ctrl+Shift+B",
  "app.folders-toggle": "Ctrl+Shift+E",
  "app.search": "Ctrl+F",
  "editor.save": "Ctrl+S",
  "editor.bold": "Ctrl+B",
  "editor.italic": "Ctrl+I",
  "editor.heading": "",
  "editor.heading-1": "Ctrl+1",
  "editor.heading-2": "Ctrl+2",
  "editor.heading-3": "Ctrl+3",
  "editor.heading-4": "Ctrl+4",
  "editor.heading-5": "Ctrl+5",
  "editor.heading-6": "Ctrl+6",
  "editor.paragraph": "Ctrl+0",
  "editor.ordered": "",
  "editor.insert-image": "",
  "editor.code": "",
  "editor.bullet": "",
  "editor.task": "",
  "editor.quote": "",
  "editor.strike": "",
  "editor.source": "",
  "editor.undo": "Ctrl+Z",
  "editor.redo": "Ctrl+Y;Ctrl+Shift+Z",
  "editor.undo-selection": "Ctrl+U",
  "editor.redo-selection": "Alt+U",
  "editor.move-line-up": "Alt+ArrowUp",
  "editor.move-line-down": "Alt+ArrowDown",
  "editor.copy-line-up": "Alt+Shift+ArrowUp",
  "editor.copy-line-down": "Alt+Shift+ArrowDown",
  "editor.add-cursor-above": "Ctrl+Alt+ArrowUp",
  "editor.add-cursor-below": "Ctrl+Alt+ArrowDown",
  "editor.insert-blank-line": "Ctrl+Enter",
  "editor.select-line": "Alt+L",
  "editor.select-parent-syntax": "",
  "editor.indent-less": "Ctrl+BracketLeft",
  "editor.indent-more": "Ctrl+BracketRight",
  "editor.indent-selection": "Ctrl+Alt+Backslash",
  "editor.delete-line": "Ctrl+Shift+K",
  "editor.matching-bracket": "Ctrl+Shift+Backslash",
  "editor.toggle-comment": "Ctrl+Slash",
  "editor.toggle-block-comment": "Alt+Shift+A",
  "editor.toggle-tab-focus": "Ctrl+M",
  "editor.select-all": "Ctrl+A",
  "editor.delete-group-backward": "Ctrl+Backspace",
  "editor.delete-group-forward": "Ctrl+Delete"
});

const keyNames = [
  "Enter", "Tab", "Escape", "Space", "Backspace", "Delete", "Insert", "Home", "End", "PageUp", "PageDown",
  "ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Comma", "Period", "Slash", "Backslash",
  "Semicolon", "Quote", "Backquote", "Minus", "Equal", "BracketLeft", "BracketRight"
];
const aliases: Readonly<Record<string, string>> = Object.freeze({
  return: "Enter", esc: "Escape", spacebar: "Space", del: "Delete", left: "ArrowLeft", right: "ArrowRight",
  up: "ArrowUp", down: "ArrowDown", ",": "Comma", "<": "Comma", ".": "Period", ">": "Period",
  "/": "Slash", "?": "Slash", "\\": "Backslash", "|": "Backslash", ";": "Semicolon", ":": "Semicolon",
  "'": "Quote", "\"": "Quote", "`": "Backquote", "~": "Backquote", "-": "Minus", "_": "Minus",
  "=": "Equal", "+": "Equal", "[": "BracketLeft", "{": "BracketLeft", "]": "BracketRight", "}": "BracketRight"
});

function canonicalKey(value: string): string {
  if (/^[a-z0-9]$/i.test(value)) return value.toUpperCase();
  if (/^f(?:[1-9]|1[0-9]|2[0-4])$/i.test(value)) return value.toUpperCase();
  const named = keyNames.find(key => key.toLowerCase() === value.toLowerCase());
  const alias = Object.hasOwn(aliases, value.toLowerCase()) ? aliases[value.toLowerCase()] : undefined;
  if (named || alias) return (named || alias)!;
  throw new Error(`无法识别按键：${value}`);
}

function reserved(key: string, ctrl: boolean, alt: boolean, shift: boolean): boolean {
  // Windows shell, IME switches and clipboard remain owned by the platform.
  if (alt && ["Tab", "Escape", "F4", "Space"].includes(key)) return true;
  if (ctrl && ["Escape", "Space"].includes(key)) return true;
  if (ctrl && alt && key === "Delete") return true;
  if (shift && key === "Space") return true;
  if (ctrl && !alt && !shift && ["C", "X", "V", "Insert"].includes(key)) return true;
  if (shift && !ctrl && !alt && ["Insert", "Delete"].includes(key)) return true;
  // Ordinary caret and selection navigation is never replaced by a command.
  if (ctrl && !alt && ["ArrowLeft", "ArrowRight", "Home", "End"].includes(key)) return true;
  if (alt && !ctrl && ["ArrowLeft", "ArrowRight"].includes(key)) return true;
  if (!ctrl && !alt && ["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End", "PageUp", "PageDown", "Tab", "Enter", "Backspace"].includes(key)) return true;
  return false;
}

export function normalizeBinding(value: string): string {
  if (typeof value !== "string" || value.length > 512) throw new Error("快捷键必须是最多 512 字符的文本");
  if (!value.trim()) return "";
  const alternatives = value.split(";");
  if (alternatives.length > 8) throw new Error("每个操作最多设置 8 个快捷键");
  const result: string[] = [];
  for (const alternative of alternatives) {
    const parts = alternative.split("+").map(part => part.trim());
    const modifiers = new Set<string>();
    let key: string | undefined;
    for (const part of parts) {
      const lower = part.toLowerCase();
      const modifier = lower === "ctrl" || lower === "control" ? "Ctrl" : lower === "alt" ? "Alt" : lower === "shift" ? "Shift" : undefined;
      if (modifier) {
        if (modifiers.has(modifier)) throw new Error("快捷键修饰键重复");
        modifiers.add(modifier);
      } else {
        if (key || !part) throw new Error("快捷键格式无效");
        key = canonicalKey(part);
      }
    }
    if (!key) throw new Error("快捷键缺少按键");
    const ctrl = modifiers.has("Ctrl"), alt = modifiers.has("Alt"), shift = modifiers.has("Shift");
    if (!ctrl && !alt && !/^F\d+$/.test(key)) throw new Error("请使用 Ctrl、Alt 组合键或功能键");
    if (reserved(key, ctrl, alt, shift)) throw new Error("此快捷键由系统、输入法或文本导航保留");
    const normalized = [...["Ctrl", "Alt", "Shift"].filter(modifier => modifiers.has(modifier)), key].join("+");
    if (result.includes(normalized)) throw new Error("同一操作的快捷键重复");
    result.push(normalized);
  }
  return result.join(";");
}

export function resolveBindings(value: Record<string, string>): Record<string, string> {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("快捷键配置必须是对象");
  const resolved: Record<string, string> = { ...shortcutDefaults };
  for (const [action, binding] of Object.entries(value)) {
    if (!Object.hasOwn(shortcutDefaults, action)) throw new Error(`未知快捷键操作：${action}`);
    resolved[action] = normalizeBinding(binding);
  }
  // Added defaults must not invalidate an older user's custom digit bindings.
  // An explicitly supplied new heading binding still participates in conflicts.
  const explicit = new Set(Object.values(value).flatMap(binding => normalizeBinding(binding).split(";").filter(Boolean)));
  for (const action of ["editor.paragraph", ...Array.from({length:6},(_,i) => `editor.heading-${i+1}`)]) {
    if (!Object.hasOwn(value,action) && explicit.has(resolved[action])) resolved[action] = "";
  }
  const used = new Map<string, string>();
  for (const [action, binding] of Object.entries(resolved)) {
    for (const alternative of binding.split(";").filter(Boolean)) {
      const previous = used.get(alternative);
      if (previous) throw new Error(`${alternative} 同时用于 ${previous} 和 ${action}`);
      used.set(alternative, action);
    }
  }
  return resolved;
}

export function eventBinding(event: KeyboardEvent): string | undefined {
  if (event.metaKey || event.isComposing || event.keyCode === 229 || event.getModifierState("AltGraph")) return undefined;
  try {
    // Windows reports Shift+Digit1 as key="!" (and similarly @#$%^&*()).
    // The configured digit identifies that physical number key.
    const token = event.shiftKey && /^Digit[0-9]$/.test(event.code)
      ? event.code.slice(-1) : event.key === " " ? "Space" : event.key;
    const key = canonicalKey(token);
    return [event.ctrlKey ? "Ctrl" : "", event.altKey ? "Alt" : "", event.shiftKey ? "Shift" : "", key].filter(Boolean).join("+");
  } catch { return undefined; }
}

// Keep typing, caret/selection movement, Escape and Tab's ordinary indent
// behavior. Every other modifier command comes exclusively from the registry.
export const typingKeymap: readonly KeyBinding[] = [
  ...defaultKeymap.filter(binding => {
    const key = binding.key;
    if (!key) return false; // This application is Windows-only.
    return !/Mod-|Ctrl-|Alt-/.test(key) ||
      ["Alt-ArrowLeft", "Alt-ArrowRight", "Mod-ArrowLeft", "Mod-ArrowRight", "Mod-Home", "Mod-End"].includes(key);
  })
];
