import type { MarkdownConfig } from "@lezer/markdown";

/** Only parsing changes: an unindented physical line leaves list paragraphs.
 * Continued, quoted and fenced content retain the upstream Markdown rules.
 * Lezer closes non-continuing composite contexts on the next advance; no source
 * edits, private parser state or synthetic newlines are needed in the editor.
 */
export const listParagraphBoundaries: MarkdownConfig = {
  parseBlock: [{
    name: "ListParagraphBoundary",
    endLeaf(cx, line) {
      if (line.basePos !== 0 || line.indent !== 0 || line.pos >= line.text.length) return false;
      for (let depth = cx.depth - 1; depth > 0; depth--) {
        if (cx.parentType(depth).name === "ListItem") return true;
      }
      return false;
    }
  }]
};
