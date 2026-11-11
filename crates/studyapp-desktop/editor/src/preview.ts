import { EditorSelection } from "@codemirror/state";
import { EditorView } from "@codemirror/view";

/** Resolve the visible DOM caret, not a second scan of CM's measured line tree.
 * The preview may have just resized or replaced hidden Markdown markers. Browser
 * caret hit testing uses that actual layout, including widgets and wrapped text.
 */
export function previewCaret(view: EditorView, event: MouseEvent): number | null {
  // A drag is a selection/copy operation, not an instruction to open a draft.
  const selection = view.contentDOM.ownerDocument.getSelection();
  if (selection && !selection.isCollapsed) return null;
  return domCaret(view, event);
}

function domCaret(view: EditorView, event: MouseEvent): number | null {
  const content = view.contentDOM;
  if (!content.isConnected || !content.getClientRects().length) return null;
  const doc = content.ownerDocument;
  const pane = view.scrollDOM.getBoundingClientRect(), box = content.getBoundingClientRect();
  const left = Math.max(0, pane.left, box.left), right = Math.min(doc.documentElement.clientWidth, pane.right, box.right);
  const top = Math.max(0, pane.top, box.top), bottom = Math.min(doc.documentElement.clientHeight, pane.bottom, box.bottom);
  if (right <= left || bottom <= top) return null;
  // A drag may leave the pane. Keep its caret at the visible edge while CM's
  // existing gesture lifecycle handles auto-scroll; do not reset the selection.
  const x = Math.max(left + .1, Math.min(right - .1, event.clientX));
  const y = Math.max(top + .1, Math.min(bottom - .1, event.clientY));
  const caret = doc.caretPositionFromPoint?.(x, y);
  const range = caret ? null : doc.caretRangeFromPoint?.(x, y);
  const node = caret?.offsetNode ?? range?.startContainer;
  const offset = caret?.offset ?? range?.startOffset ?? 0;
  if (node && content.contains(node)) return view.posAtDOM(node, offset);
  // Engines without a caret API and non-text widgets still have a stable DOM
  // boundary. Never use a stale node from a detached/previous document.
  const target = event.target;
  return target instanceof Node && content.contains(target) ? view.posAtDOM(target, 0) : null;
}

/** Keep CodeMirror's gesture/drag lifecycle, but resolve readonly positions from
 * the rendered DOM. Its measured line tree can lag behind replaced Markdown
 * widgets during a resize; both mousedown and click must avoid that scan. */
export const previewMouseSelection = EditorView.mouseSelectionStyle.of((view, event) => {
  if (!view.state.readOnly || event.button !== 0) return null;
  let start = domCaret(view, event);
  let initial = view.state.selection;
  const clicks = Math.max(1, Math.min(3, event.detail));
  const rangeAt = (pos: number) => {
    if (clicks === 2) return view.state.wordAt(pos) ?? EditorSelection.cursor(pos);
    if (clicks === 3) {
      const line = view.state.doc.lineAt(pos);
      return EditorSelection.range(line.from, Math.min(view.state.doc.length, line.to + 1));
    }
    return EditorSelection.cursor(pos);
  };
  return {
    update(update) {
      if (update.docChanged) {
        if (start !== null) start = update.changes.mapPos(start);
        initial = initial.map(update.changes);
      }
    },
    get(current, extend, multiple) {
      const pos = domCaret(view, current);
      if (start === null) return initial;
      if (pos === null) return view.state.selection;
      let range = rangeAt(pos);
      if (start !== pos && !extend) {
        const origin = rangeAt(start);
        const from = Math.min(origin.from, range.from), to = Math.max(origin.to, range.to);
        range = start < pos ? EditorSelection.range(from, to) : EditorSelection.range(to, from);
      }
      if (extend) return initial.replaceRange(initial.main.extend(range.from, range.to));
      if (multiple) {
        const removed = clicks === 1 && initial.ranges.length > 1
          ? initial.ranges.findIndex(range => range.from <= pos && range.to >= pos) : -1;
        if (removed >= 0) return EditorSelection.create(initial.ranges.filter((_, index) => index !== removed),
          Math.max(0, initial.mainIndex - (removed <= initial.mainIndex ? 1 : 0)));
        return initial.addRange(range);
      }
      return EditorSelection.create([range]);
    },
  };
});

/** Terminate selection on lost native releases, without clearing copied text. */
export function installGestureTermination(view: EditorView): void {
  const doc = view.dom.ownerDocument;
  let selecting = false;
  const end = () => {
    if (!selecting) return;
    selecting = false;
    doc.dispatchEvent(new MouseEvent("mouseup", {button:0, buttons:0, bubbles:true}));
  };
  view.contentDOM.addEventListener("mousedown", event => { selecting = event.button === 0; }, true);
  doc.addEventListener("mouseup", () => { selecting = false; }, true);
  doc.addEventListener("mousemove", event => { if (!(event.buttons & 1)) end(); }, true);
  window.addEventListener("blur", end);
  doc.addEventListener("visibilitychange", () => { if (doc.hidden) end(); });
  for (const name of ["pointercancel", "lostpointercapture", "dragend"]) doc.addEventListener(name, end, true);
}
