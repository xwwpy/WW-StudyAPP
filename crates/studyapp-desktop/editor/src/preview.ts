import type { EditorView } from "@codemirror/view";

/** Resolve the visible DOM caret, not a second scan of CM's measured line tree.
 * The preview may have just resized or replaced hidden Markdown markers. Browser
 * caret hit testing uses that actual layout, including widgets and wrapped text.
 */
export function previewCaret(view: EditorView, event: MouseEvent): number | null {
  const content = view.contentDOM;
  if (!content.isConnected || !content.getClientRects().length) return null;
  const doc = content.ownerDocument;
  // A drag is a selection/copy operation, not an instruction to open a draft.
  const selection = doc.getSelection();
  if (selection && !selection.isCollapsed) return null;
  const caret = doc.caretPositionFromPoint?.(event.clientX, event.clientY);
  const range = caret ? null : doc.caretRangeFromPoint?.(event.clientX, event.clientY);
  const node = caret?.offsetNode ?? range?.startContainer;
  const offset = caret?.offset ?? range?.startOffset ?? 0;
  if (node && content.contains(node)) return view.posAtDOM(node, offset);
  // Engines without a caret API and non-text widgets still have a stable DOM
  // boundary. Never use a stale node from a detached/previous document.
  const target = event.target;
  return target instanceof Node && content.contains(target) ? view.posAtDOM(target, 0) : null;
}
