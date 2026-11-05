import { EditorSelection } from "@codemirror/state";
import { Decoration, EditorView, layer, RectangleMarker, ViewPlugin, type ViewUpdate } from "@codemirror/view";

// Native selection paints the primary range above opaque Markdown surfaces.
// Secondary marks do the same without tinting/obscuring the foreground glyphs;
// public CodeMirror layers retain additional carets and their measured geometry.
const secondaryMarks = (view: EditorView) => Decoration.set(view.state.selection.ranges
  .filter(range => range !== view.state.selection.main && !range.empty)
  .map(range => Decoration.mark({class:"cm-secondary-selected-text"}).range(range.from, range.to)), true);
export const renderedSelection = [
  layer({
    above: true,
    class: "cm-secondary-selections",
    markers: view => view.state.selection.ranges.flatMap(range =>
      range === view.state.selection.main || !range.empty ? [] : RectangleMarker.forRange(view,
        "cm-secondary-cursor", EditorSelection.cursor(range.head, range.assoc))),
    update: update => update.docChanged || update.selectionSet || update.viewportChanged || update.focusChanged,
  }),
  ViewPlugin.fromClass(class {
    decorations;
    constructor(view: EditorView) { this.decorations = secondaryMarks(view); }
    update(update: ViewUpdate) {
      if (update.docChanged || update.selectionSet) this.decorations = secondaryMarks(update.view);
    }
  }, {decorations: plugin => plugin.decorations}),
];

let closeOnChange = () => {};
export const copyMenuUpdates = EditorView.updateListener.of(update => {
  if (update.docChanged || update.selectionSet || update.transactions.some(transaction => transaction.reconfigured)) closeOnChange();
});

// HTML widgets contain selectable rendered text, not a one-to-one CM source
// mapping. Keep their native DOM selection rather than copying hidden HTML.
function htmlSelection(view:EditorView):string|null {
  const selection=view.contentDOM.ownerDocument.getSelection();
  if(!selection || selection.isCollapsed || !selection.anchorNode || !selection.focusNode ||
      !view.contentDOM.contains(selection.anchorNode) || !view.contentDOM.contains(selection.focusNode)) return null;
  const widget=(node:Node)=> (node instanceof Element ? node : node.parentElement)?.closest(".md-html-fragment");
  return widget(selection.anchorNode) || widget(selection.focusNode) ? selection.toString() : null;
}

export function installCopyMenu(view: EditorView, failed: (message: string) => void): void {
  const menu = document.createElement("div");
  menu.id = "editor-copy-menu";
  menu.setAttribute("role", "menu");
  menu.setAttribute("aria-label", "文本操作");
  menu.hidden = true;
  const copy = document.createElement("button");
  copy.type = "button";
  copy.setAttribute("role", "menuitem");
  copy.textContent = "复制";
  menu.append(copy);
  document.body.append(menu);
  const close = () => { menu.hidden = true; };
  const open = (x: number, y: number) => {
    copy.disabled = !htmlSelection(view) && view.state.selection.ranges.every(range => range.empty);
    menu.hidden = false;
    const box = menu.getBoundingClientRect();
    menu.style.left = `${Math.max(8, Math.min(x, innerWidth - box.width - 8))}px`;
    menu.style.top = `${Math.max(8, Math.min(y, innerHeight - box.height - 8))}px`;
  };
  // Preserve the editor's DOM selection. Copy is a deliberate user gesture,
  // using the browser's normal copy event, never ambient clipboard permission.
  menu.addEventListener("mousedown", event => event.preventDefault());
  copy.addEventListener("click", () => {
    const rendered = htmlSelection(view);
    if (!copy.disabled && (rendered || view.state.selection.ranges.some(range => !range.empty))) {
      if (!rendered) view.focus();
      if (!document.execCommand("copy")) failed("复制未完成，请按 Ctrl+C（macOS 为 ⌘C）复制所选文字。");
    }
    close();
  });
  // Only a user copy event is handled; no ambient clipboard API or permission.
  document.addEventListener("copy", event => {
    const text=htmlSelection(view);
    if(!text) return;
    if(event.clipboardData) {
      event.clipboardData.setData("text/plain",text);
      event.preventDefault();
    }
    event.stopPropagation();
  },true);
  view.dom.addEventListener("contextmenu", event => {
    if (!(event.target as Element).closest(".cm-content")) return;
    event.preventDefault();
    const caret = view.coordsAtPos(view.state.selection.main.head);
    open(event.clientX || caret?.left || 8, event.clientY || caret?.bottom || 8);
  });
  view.dom.addEventListener("keydown", event => {
    if (event.isComposing) return;
    if (event.key === "ContextMenu" || (event.key === "F10" && event.shiftKey)) {
      event.preventDefault(); event.stopPropagation();
      const caret = view.coordsAtPos(view.state.selection.main.head);
      open(caret?.left ?? 8, caret?.bottom ?? 8);
    } else if (!menu.hidden && event.key === "Escape") {
      event.preventDefault(); event.stopPropagation(); close();
    } else if (!menu.hidden && (event.key === "Enter" || event.key === " ")) {
      event.preventDefault(); event.stopPropagation(); copy.click();
    } else if (!menu.hidden && event.key.startsWith("Arrow")) {
      close();
    }
  }, true);
  closeOnChange = close;
  document.addEventListener("pointerdown", event => { if (!menu.contains(event.target as Node)) close(); });
  view.scrollDOM.addEventListener("scroll", close, { passive: true });
  window.addEventListener("resize", close);
}
