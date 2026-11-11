import { EditorView, type ViewUpdate } from "@codemirror/view";

let refresh = () => {};
export const actionbarUpdates = EditorView.updateListener.of((update: ViewUpdate) => {
  if (update.selectionSet || update.docChanged || update.viewportChanged || update.focusChanged || update.transactions.some(t => t.reconfigured)) refresh();
});
export function installActionbar(view: EditorView, command: (name: string) => boolean, allowed: () => boolean): {
  configure(tokens: Record<string, unknown>): void; update(): void;
} {
  const bar = document.createElement("div");
  bar.id = "editor-actionbar"; bar.setAttribute("role", "toolbar"); bar.setAttribute("aria-label", "选中文本操作"); bar.hidden = true;
  let fixed = false, scheduled = false;
  const copy = document.createElement("button"); copy.type = "button"; copy.textContent = "复制";
  copy.addEventListener("click", () => { view.focus(); document.execCommand("copy"); });
  bar.append(copy);
  for (const [name, label] of [["bold", "加粗"], ["italic", "斜体"], ["code", "行内代码"], ["link", "链接"]]) {
    const button = document.createElement("button"); button.type = "button"; button.textContent = label; button.dataset.selectionCommand = name;
    button.addEventListener("click", () => command(name)); bar.append(button);
  }
  // Preserve the original multi-range selection, including native rendered text.
  bar.addEventListener("mousedown", event => event.preventDefault());
  const host = document.querySelector("#editor")!;
  host.parentElement!.insertBefore(bar, host);
  const update = () => {
    if (scheduled) return; scheduled = true;
    requestAnimationFrame(() => {
      scheduled = false;
      const selected = view.state.selection.ranges.some(range => !range.empty);
      bar.hidden = !selected || !allowed() || (!view.hasFocus && !bar.contains(document.activeElement));
      if (bar.hidden) return;
      for (const button of bar.querySelectorAll<HTMLButtonElement>("[data-selection-command]")) button.hidden = view.state.readOnly;
      bar.classList.toggle("actionbar-fixed", fixed);
      if (fixed) { bar.style.left = ""; bar.style.top = ""; return; }
      const range = view.state.selection.main;
      const caret = view.coordsAtPos(range.head) ?? view.coordsAtPos(range.anchor);
      if (!caret) { bar.hidden = true; return; }
      const bounds = bar.getBoundingClientRect(), area = view.scrollDOM.getBoundingClientRect();
      const x = Math.max(8, Math.min(caret.left - bounds.width / 2, innerWidth - bounds.width - 8));
      const above = caret.top - bounds.height - 8;
      const y = above >= area.top + 8 ? above : Math.min(caret.bottom + 8, innerHeight - bounds.height - 8);
      bar.style.left = `${x}px`; bar.style.top = `${Math.max(area.top + 8, y)}px`;
    });
  };
  view.scrollDOM.addEventListener("scroll", update, { passive: true });
  window.addEventListener("resize", update);
  refresh = update;
  return { update, configure(tokens) {
    tokens = tokens && typeof tokens === "object" && !Array.isArray(tokens) ? tokens : {};
    fixed = tokens.actionbarLayout === "alternative";
    const valid = (value: unknown): value is string => typeof value === "string" && /^#[a-f\d]{6}([a-f\d]{2})?$/i.test(value);
    for (const [token, css] of [["actionbarBackground", "--actionbar-background"], ["actionbarBorder", "--actionbar-border"], ["actionbarText", "--actionbar-text"]]) {
      if (valid(tokens[token])) bar.style.setProperty(css, tokens[token]);
    }
    const radius = Number(tokens.actionbarRadius); if (Number.isFinite(radius)) bar.style.borderRadius = `${Math.max(0, Math.min(32, radius))}px`;
    for (const [token, property, low, high] of [
      ["actionbarBorderWidth", "borderWidth", 0, 4], ["actionbarWidth", "width", 200, 640], ["actionbarHeight", "maxHeight", 48, 480]
    ] as const) {
      const value = Number(tokens[token]);
      if (Number.isFinite(value)) bar.style[property] = `${Math.max(low, Math.min(high, value))}px`;
    }
    update();
  } };
}
