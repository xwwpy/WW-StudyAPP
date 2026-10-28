# Offline continuous Markdown editor

This directory owns the single CodeMirror 6 contenteditable surface used by the native GPUI/Wry host. The Markdown document is always the editable source of truth. A viewport decoration plugin styles headings, emphasis, lists, quotes, tasks, tables, links and code directly in that document. Syntax is revealed around the selection. There are no paragraph input/preview pairs and no HTML-rendered duplicate document.

## Build and test

Use Node.js 22 or later:

```text
npm ci
npm run check
npm run build
npm test
npm run bundle:check
```

The lockfile pins all dependencies; esbuild produces deterministic `dist/index.html`, `dist/editor.js`, `dist/editor.css` and `dist/THIRD_PARTY_LICENSES.txt`. `bundle:check` rebuilds in memory and compares those files without replacing them. Every library actually present in the JavaScript bundle must have a license notice; a missing license fails the build. Generated assets are committed so Rust consumers need no Node.js runtime.

Tests serve only these three assets from a temporary loopback HTTP server and launch an isolated headless browser. They use installed Microsoft Edge on Windows; on other machines install Playwright's Chromium using `npx playwright install chromium`. They never attach to or terminate the user's browser or desktop application. Reports and screenshots are written under ignored `test-results/`.

On the development Windows sandbox, esbuild's dependency resolver may receive access denied while inspecting a parent directory. Running the same build with the tool's approved sandbox escalation resolves it; no source or dependency workaround is required.

## Host bridge

Load the assets from `https://studyapp.editor/index.html`. The only outgoing transport is `window.ipc.postMessage(JSON.stringify(message))`.

```typescript
window.studyappEditor.load({ generation: 1, text: rawMarkdown, readonly: false });
window.studyappEditor.setReadonly(true);
window.studyappEditor.setSource(true);
window.studyappEditor.command("bold");
window.studyappEditor.flush({ request: 17 });
```

`load` creates a fresh document and history, resets revision to zero, preserves the current source-mode preference, and invalidates any previous flush. Generation and request IDs must be nonnegative JavaScript-safe integers. `setSource` reconfigures decorations without changing text, selection or undo history.

Commands: `heading`, `bold`, `italic`, `code`, `strike`, `bullet`, `task`, `quote`, `undo`, `redo`, `source`, `save`. Ctrl/Cmd+S emits `save` for the native save path. The toolbar keeps the existing selection while formatting across paragraphs. Inline formatting applies separate markers to each selected nonempty line, preserving blank paragraphs and heading/list prefixes; nested emphasis toggles use the syntax tree. Code spans choose a backtick delimiter longer than any selected backtick run. Formatting actions each form one undo step.

Messages:

```typescript
{ type: "ready" }
{ type: "change", generation, baseRevision, revision, changes: [
  { fromLine, fromColumn, toLine, toColumn, text }
] }
{ type: "flush", generation, request, revision, normalizedText }
{ type: "save", generation, revision }
{ type: "error", generation, revision, message }
```

Every document-changing CodeMirror transaction increments the revision once, including undo, redo and IME updates. All change positions are relative to that transaction's **pre-change** document. Lines and UTF-16 columns are **one-based**. Changes are emitted in ascending order; the Rust host applies them in reverse order. Inserted text uses LF.

CodeMirror normalizes CRLF and CR to LF at load. Only the native host retains original newline bytes and chooses the EOL for inserted line breaks. Full text is serialized only for a requested flush or explicit test snapshot, never for normal typing notifications.

The frontend rejects edits whose resulting raw UTF-8 document would exceed 10 MiB or whose transaction would emit over 4096 patches. Rejection is atomic and displays a local warning; it sends no fatal `error`, changes no revision and leaves the document editable. A StateField maintains normalized bytes, raw bytes and the width of each original newline. Typing counts only inserted/deleted fragments; newline edits update the corresponding line-width spans. Preferred inserted EOL follows the native rule: CRLF when its count is nonzero and at least both LF and CR counts, otherwise CR when CR exceeds LF, otherwise LF. A valid document at the limit may still delete content and resume editing.

If native error recovery changes the canonical raw line endings of the whole document, reload that canonical text with a fresh generation so the frontend's exact raw-byte budget remains synchronized.

Flush blurs the editing surface to commit native IME input, waits for composition and pending DOM transactions to settle, freezes the document as readonly, then sends the acknowledgement. If the IME fails to settle within 2.5 seconds, an error is sent and the existing content stays available. The host must call `setReadonly(false)` after completing or cancelling a save when editing should resume. A newer load silently cancels old pending flushes.

## Security and supported syntax

All scripts and parser dependencies are bundled. CSP denies remote resources, connections, frames, media and executable Markdown HTML. Markdown HTML remains text; image URLs are not fetched. Links are decorated spans with safe HTTP(S)/mailto tooltips, never navigable anchors. Editing a link reveals its Markdown target. This version intentionally provides no browser navigation or external-link IPC.

Fences support Rust, Python, JavaScript/JSX, TypeScript/TSX, C/C++, C#, Java, Go, HTML, CSS, JSON, YAML, TOML, SQL and shell, with common aliases. Unknown fences are plain code. GFM tables use the header's column count to align editable cell text, including rows with missing trailing cells; active rows expose delimiters. Tasks are keyboard-accessible checkbox buttons: Enter or Space changes the original `[ ]` / `[x]` marker and participates in normal undo history. Tab indents in the editing surface; Escape followed by Tab leaves the editor, and Ctrl+M toggles CodeMirror's Tab-focus mode.

The Lezer parser is incremental; visual decorations are built only for the current viewport. CodeMirror's scroller is the only scrolling document surface. Tables, inline syntax and fence decorations do not replace multiline text with independent editors.

## Explicit test interface

Set `window.__STUDYAPP_EDITOR_TEST__ = true` in a trusted initialization script before the bundle runs. Otherwise the bridge has **no `test` method**. The native release host must not enable this flag.

`await window.studyappEditor.test(action)` supports:

```typescript
{ type: "snapshot", request?: "smoke-1" }
{ type: "select", anchor: 0, head: 12, request?: "smoke-2" }
{ type: "replace", from: 0, to: 12, text: "replacement", request?: "smoke-3" }
{ type: "command", name: "undo", request?: "smoke-4" }
{ type: "performance", edits: 200, request?: "perf-1" }
```

Selection/replacement positions are zero-based absolute UTF-16 offsets in normalized text. Replace uses a real CodeMirror transaction and respects readonly. Snapshot returns `generation`, `revision`, `text`, `normalizedText`, `utf8Bytes`, `rawBytes`, `source`, `readonly`, `composing`, `selection: {anchor, head}`, `renderedLines`, `domNodes`, `headings` and `highlights`. With a supplied request, a `{type:"test", generation, request, result}` report is also sent over IPC.

The performance action schedules up to 1,000 incremental insertions, measuring transaction/decorations/DOM updates plus forced layout. It reports median and P95 milliseconds, rendered lines and DOM node count. These are browser edit/layout measurements, not physical keyboard-to-display or native GPU latency.

The browser suite covers one editing surface, direct syntax styling/reveal, cross-paragraph keyboard selection, formatting, nested style toggles, backticks, undo/redo, source/history preservation, Unicode/UTF-16/mixed EOL patches, all supported fence families, mouse/keyboard task toggles, tables with missing cells, multiline links, untrusted HTML/links/images, malformed Markdown, CDP-driven IME composition/flush, readonly, generation reset, stale flush cancellation, production test isolation, oversized paste/patch rejection, exact mixed-EOL byte budgets, editing at the 10 MiB limit and a 250 KiB-plus document with 200 edits. Native WebView2 lifecycle and real installed IME behavior also need host integration testing.
