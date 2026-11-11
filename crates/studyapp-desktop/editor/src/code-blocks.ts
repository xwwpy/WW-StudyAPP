import { codeFolding, foldEffect, foldedRanges, unfoldEffect } from "@codemirror/language";
import { WidgetType, type EditorView } from "@codemirror/view";

/** Copy only on an explicit user gesture, without changing the CM selection. */
export function copyCode(view: EditorView, text: string, button: HTMLButtonElement): void {
  const doc = view.dom.ownerDocument;
  const copy = (event: ClipboardEvent) => {
    if (!event.clipboardData) return;
    event.clipboardData.setData("text/plain", text);
    event.preventDefault(); event.stopImmediatePropagation();
  };
  doc.addEventListener("copy", copy, true);
  let copied = false;
  try { copied = doc.execCommand("copy"); }
  finally { doc.removeEventListener("copy", copy, true); }
  button.textContent = copied ? "已复制" : "复制失败";
  button.setAttribute("aria-label", copied ? "代码已复制" : "复制失败，请选择代码后复制");
}

export class CodeControls extends WidgetType {
  constructor(readonly from: number, readonly to: number, readonly text: string, readonly folded: boolean) { super(); }
  eq(other: CodeControls) { return this.from === other.from && this.to === other.to && this.text === other.text && this.folded === other.folded; }
  toDOM(view: EditorView) {
    const bar = document.createElement("span");
    bar.className = "md-code-controls"; bar.contentEditable = "false";
    const button = (label: string, run: (button: HTMLButtonElement) => void) => {
      const element = document.createElement("button"); element.type = "button";
      element.textContent = label; element.setAttribute("aria-label", label + "代码");
      element.addEventListener("mousedown", event => { event.preventDefault(); event.stopPropagation(); });
      element.addEventListener("click", event => { event.preventDefault(); event.stopPropagation(); run(element); });
      bar.append(element); return element;
    };
    button("复制", button => copyCode(view, this.text, button));
    if (this.to > this.from) {
      const toggle = button(this.folded ? "展开" : "折叠", () => view.dispatch({effects:
        (this.folded ? unfoldEffect : foldEffect).of({from:this.from, to:this.to})}));
      toggle.setAttribute("aria-expanded", String(!this.folded));
    }
    return bar;
  }
  ignoreEvent() { return true; }
}

export function codeIsFolded(view: EditorView, from: number, to: number): boolean {
  let folded = false;
  foldedRanges(view.state).between(from, to, (start) => { if (start === from) folded = true; });
  return folded;
}

export const codeBlockFolding = codeFolding({
  placeholderDOM(_view, onclick) {
    const button = document.createElement("button"); button.type = "button";
    button.className = "md-code-folded"; button.textContent = "代码已折叠 · 展开";
    button.setAttribute("aria-label", "展开代码");
    button.addEventListener("click", event => { event.stopPropagation(); onclick(event); });
    return button;
  },
});
