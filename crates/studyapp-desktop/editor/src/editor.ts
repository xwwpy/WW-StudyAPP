import { installOutline, outlineUpdates, refreshOutline, setOutlineEnabled } from "./outline";
import { applyEditorTheme } from "./theme";
import { ChangeSet, Compartment, EditorSelection, EditorState, Extension, Text, Transaction } from "@codemirror/state";
import { drawSelection, EditorView, keymap, placeholder, ViewUpdate } from "@codemirror/view";
import {
  addCursorAbove, addCursorBelow, copyLineDown, copyLineUp, cursorMatchingBracket,
  deleteGroupBackward, deleteGroupForward, deleteLine, history,
  indentSelection, insertBlankLine, moveLineDown, moveLineUp, redoSelection, selectAll,
  selectLine, selectParentSyntax, toggleBlockComment, toggleComment, toggleTabFocusMode, undoSelection
} from "@codemirror/commands";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { syntaxHighlighting } from "@codemirror/language";
import { codeHighlighting, fencedLanguage } from "./languages";
import { imageMapChanged, updateImages, renderedMarkdown, rawUrlWrapping } from "./decorations";
import { formatCommand, indentMarkdown } from "./formatting";
import { documentBudget, documentLimits, resetDocumentBudget, MAX_TRANSACTION_PATCHES, stringBytes } from "./limits";
import { incompleteScalars, scalarText, splitsScalar } from "./unicode";
import { eventBinding, resolveBindings, typingKeymap } from "./shortcuts";
import { scanDocumentStatistics } from "./statistics";

interface Load { outlineEnabled?: boolean; generation: number; text: string; readonly: boolean; canRequestEdit?: boolean; }
interface Rebase extends Load { previousGeneration: number; previousRevision: number; changes: { from: number; to: number; insert: string }[]; }
interface TestAction { type: string; request?: string | number; from?: number; to?: number; text?: string; anchor?: number; head?: number; name?: string; edits?: number; width?: number; height?: number; }
interface Bridge {
  load(value: Load): void;
  rebase(value: Rebase): void;
  setReadonly(value: boolean): void;
  setCanRequestEdit(value:boolean):void;
  restoreSelection(position: number): void;
  setImages(value: {generation:number;images:Record<string,unknown>}): void;
  insertText(text:string): boolean;
  setTheme(tokens: Record<string, unknown>): void;
  setSource(value: boolean): void;
  setShortcuts(bindings: Record<string, string>): void;
  command(name: string): boolean;
  flush(value: { request: number }): Promise<void>;
  test?: (value: TestAction) => unknown;
}
declare global {
  interface Window {
    ipc?: { postMessage(message: string): void };
    studyappEditor: Bridge;
    __STUDYAPP_EDITOR_TEST__?: boolean;
  }
}

const readOnlyMode = new Compartment();
const renderingMode = new Compartment();
const shortcutsMode = new Compartment();
const themeMode = new Compartment();
let bindings = resolveBindings({});
let generation = 0;
let revision = 0;
let source = false;
let loaded = false;
let canRequestEdit = false;
let loadEpoch = 0;
let pendingFlush = false;
let unpublished: { base: EditorState; changes: ChangeSet } | undefined;
const status = document.querySelector<HTMLElement>("#editor-status")!;
const toolbar = document.querySelector<HTMLElement>("#toolbar")!;
const sourceButton = document.querySelector<HTMLButtonElement>("#source-toggle")!;
const statistics = document.querySelector<HTMLElement>("#editor-statistics")!;
const themeWarning = document.querySelector<HTMLElement>("#editor-theme-warning")!;
let statisticsDocument: Text | undefined;
let statisticsLabel = "";
let statisticsTimer: ReturnType<typeof setTimeout> | undefined;
let statisticsEpoch = 0;
let cancelStatisticsSlice: (() => void) | undefined;

function cancelStatistics(): void {
  statisticsEpoch++;
  clearTimeout(statisticsTimer);
  statisticsTimer = undefined;
  cancelStatisticsSlice?.();
  cancelStatisticsSlice = undefined;
}
function updateStatistics(reset = false): void {
  cancelStatistics();
  const doc = view.state.doc;
  if (statisticsDocument === doc) { statistics.textContent = statisticsLabel; return; }
  if (reset) statistics.textContent = "正在统计…";
  const epoch = statisticsEpoch;
  const steps = scanDocumentStatistics(doc.iter());
  const run = (deadline?: IdleDeadline): void => {
    if (epoch !== statisticsEpoch || doc !== view.state.doc) return;
    cancelStatisticsSlice = undefined;
    const start = performance.now();
    do {
      const result = steps.next();
      if (result.done) {
        statisticsDocument = doc;
        const value = result.value;
        statisticsLabel = `${value.words.toLocaleString("zh-CN")} 字词 · ${value.characters.toLocaleString("zh-CN")} 字符 · 约 ${value.readingMinutes} 分钟`;
        statistics.textContent = statisticsLabel;
        return;
      }
      // Check both budgets after a bounded block, including when an idle callback
      // times out, so busy windows still make progress without a full scan.
    } while (performance.now() - start < 5 && (!deadline || deadline.timeRemaining() > 1));
    queue();
  };
  const queue = (): void => {
    if (typeof window.requestIdleCallback === "function") {
      const handle = window.requestIdleCallback(run, { timeout: 500 });
      cancelStatisticsSlice = () => window.cancelIdleCallback(handle);
    } else {
      const handle = setTimeout(run, 0);
      cancelStatisticsSlice = () => clearTimeout(handle);
    }
  };
  queue();
}
function scheduleStatistics(update: ViewUpdate): void {
  if (!update.docChanged) return;
  cancelStatistics();
  statisticsTimer = setTimeout(updateStatistics, 120);
}

function post(message: object): void {
  try { window.ipc?.postMessage(JSON.stringify(message)); }
  catch {
    status.textContent = "无法连接笔记存储，请保留当前内容";
    status.classList.add("error");
  }
}
function error(message: string): void {
  status.textContent = message;
  status.classList.add("error");
  post({ type: "error", generation, revision, message });
}
function warning(message: string): void {
  status.textContent = message;
  status.classList.add("warning");
}
function clearWarning(): void {
  if (status.classList.contains("warning")) {
    status.classList.remove("warning");
    status.textContent = "";
  }
}
function updateToolbar(): void {
  for (const button of toolbar.querySelectorAll<HTMLButtonElement>("button[data-command],button[data-menu-command]")) {
    const command = button.dataset.command ?? button.dataset.menuCommand;
    button.disabled = command !== "source" && (view.state.readOnly || pendingFlush);
    const binding = bindings[`editor.${command}`];
    button.title = `${button.getAttribute("aria-label") ?? ""}${binding ? ` · ${binding.replaceAll(";", " / ")}` : ""}`;
  }
  sourceButton.setAttribute("aria-pressed", String(source));
  if (!status.classList.contains("error") && !status.classList.contains("warning")) status.textContent = pendingFlush ? "正在确认输入…" : view.state.readOnly ? "只读" : "";
}
function readonlyExtensions(value: boolean): Extension {
  return [EditorState.readOnly.of(value), EditorView.editable.of(!value)];
}
function publishPatchSet(base: Text, changes: ChangeSet): void {
  const patches: { fromLine: number; fromColumn: number; toLine: number; toColumn: number; text: string }[] = [];
  changes.iterChanges((from, to, _newFrom, _newTo, inserted) => {
    const first = base.lineAt(from), last = base.lineAt(to);
    patches.push({
      fromLine: first.number, fromColumn: from - first.from + 1,
      toLine: last.number, toColumn: to - last.from + 1, text: inserted.toString()
    });
  });
  if (!patches.length) return;
  const baseRevision = revision++;
  post({ type: "change", generation, baseRevision, revision, changes: patches });
}
function publishBatches(base: EditorState, changes: ChangeSet): void {
  const budget = base.field(documentBudget);
  const shrinking: { from: number; to: number; insert: Text }[] = [];
  const growing: typeof shrinking = [];
  changes.iterChanges((from, to, _newFrom, _newTo, insert) => {
    const firstLine = base.doc.lineAt(from).number - 1;
    const lastLine = base.doc.lineAt(to).number - 1;
    let removedBytes = stringBytes(base.doc.sliceString(from, to));
    for (let line = firstLine; line < lastLine; line++) removedBytes += budget.newlineWidths[line] - 1;
    const insertedBytes = stringBytes(insert.toString()) + (insert.lines - 1) * (budget.preferredWidth - 1);
    (insertedBytes < removedBytes ? shrinking : growing).push({ from, to, insert });
  });
  let applied = ChangeSet.empty(base.doc.length);
  let current = base.doc;
  // Keep original patches and untouched raw EOLs. Shrinking first ensures an
  // intermediate native revision cannot exceed either endpoint's byte budget.
  // Mapping each batch through prior batches keeps its line/column base exact.
  for (const group of [shrinking, growing]) {
    for (let offset = 0; offset < group.length; offset += MAX_TRANSACTION_PATCHES) {
      const batch = ChangeSet.of(group.slice(offset, offset + MAX_TRANSACTION_PATCHES), base.doc.length).map(applied);
      publishPatchSet(current, batch);
      current = batch.apply(current);
      applied = applied.compose(batch);
    }
  }
}
function publishChanges(update: ViewUpdate): void {
  if (!loaded) return;
  for (const transaction of update.transactions) {
    if (!transaction.docChanged) continue;
    // Windows can deliver an astral character as separate UTF-16 key events.
    // Compose those transient edits before crossing Rust's scalar-string IPC
    // boundary; never emit an unpaired surrogate or a split-scalar position.
    const pending = unpublished
      ? { base: unpublished.base, changes: unpublished.changes.compose(transaction.changes) }
      : { base: transaction.startState, changes: transaction.changes };
    unpublished = pending;
    let patches = 0;
    let valid = true;
    pending.changes.iterChanges((from, to, _newFrom, _newTo, inserted) => {
      patches++;
      valid &&= scalarText(inserted.toString()) && !splitsScalar(pending.base.doc, from) && !splitsScalar(pending.base.doc, to);
    });
    if (!valid) continue;
    unpublished = undefined;
    if (patches > MAX_TRANSACTION_PATCHES) publishBatches(pending.base, pending.changes);
    else publishPatchSet(pending.base.doc, pending.changes);
  }
}
const editorCommands: Readonly<Record<string, (view: EditorView) => boolean>> = Object.freeze({
  "undo-selection": undoSelection,
  "redo-selection": redoSelection,
  "move-line-up": moveLineUp,
  "move-line-down": moveLineDown,
  "copy-line-up": copyLineUp,
  "copy-line-down": copyLineDown,
  "add-cursor-above": addCursorAbove,
  "add-cursor-below": addCursorBelow,
  "insert-blank-line": insertBlankLine,
  "select-line": selectLine,
  "select-parent-syntax": selectParentSyntax,
  "indent-less": view => indentMarkdown(view, true),
  "indent-more": view => indentMarkdown(view),
  "indent-selection": indentSelection,
  "delete-line": deleteLine,
  "matching-bracket": cursorMatchingBracket,
  "toggle-comment": toggleComment,
  "toggle-block-comment": toggleBlockComment,
  "toggle-tab-focus": toggleTabFocusMode,
  "select-all": selectAll,
  "delete-group-backward": deleteGroupBackward,
  "delete-group-forward": deleteGroupForward
});
function command(name: string): boolean {
  if (typeof name !== "string" || !loaded || view.composing || pendingFlush) return false;
  name = name.startsWith("editor.") ? name.slice(7) : name;
  if (name === "source") { setSource(!source); return true; }
  if (name === "save" && !view.state.readOnly) { post({ type: "save", generation, revision }); return true; }
  if (view.state.readOnly) return false;
  if (name === "insert-image") { post({type:"requestInsertImage",generation}); return true; }
  const result = Object.hasOwn(editorCommands, name) ? editorCommands[name](view) : formatCommand(view, name);
  if (result) view.focus();
  return result;
}
function shortcutLookup(values: Record<string, string>): Map<string, string> {
  const lookup = new Map<string, string>();
  for (const [action, binding] of Object.entries(values)) {
    for (const alternative of binding.split(";").filter(Boolean)) lookup.set(alternative, action);
  }
  return lookup;
}
let activeShortcutLookup = shortcutLookup(bindings);
const knownShortcutKeys = new Set(activeShortcutLookup.keys());
function handleShortcut(event: KeyboardEvent, lookup: Map<string, string>): boolean {
  if (event.defaultPrevented || view.composing) return false;
  const binding = eventBinding(event);
  const action = binding && lookup.get(binding);
  if (!action) {
    // Chromium emits beforeinput(historyUndo/historyRedo) for its own Ctrl+Z/Y
    // defaults. Consuming retired bindings prevents that hidden fallback.
    if (binding && knownShortcutKeys.has(binding)) {
      event.preventDefault();
      return true;
    }
    return false;
  }
  // Once matched, consume even a readonly/no-op command, so browser commands
  // cannot activate and a physical keydown cannot save or route twice.
  event.preventDefault();
  if (event.repeat || pendingFlush) return true;
  if (action.startsWith("app.")) post({ type: "shortcut", action, generation });
  else command(action);
  return true;
}
function shortcutExtensions(values: Record<string, string>): Extension {
  const lookup = shortcutLookup(values);
  return EditorView.domEventHandlers({ keydown: event => handleShortcut(event, lookup) });
}
function setShortcuts(values: Record<string, string>): void {
  let resolved: Record<string, string>;
  try { resolved = resolveBindings(values); }
  catch (cause) {
    // Invalid configuration never changes live bindings or poisons note saving.
    warning(`快捷键配置无效：${cause instanceof Error ? cause.message : String(cause)}`);
    return;
  }
  view.dispatch({ effects: shortcutsMode.reconfigure(shortcutExtensions(resolved)) });
  bindings = resolved;
  activeShortcutLookup = shortcutLookup(resolved);
  for (const binding of activeShortcutLookup.keys()) knownShortcutKeys.add(binding);
  clearWarning();
  updateToolbar();
}
function extensions(readonly: boolean, rawText = ""): Extension[] {
  return [
    outlineUpdates, readOnlyMode.of(readonlyExtensions(readonly)), renderingMode.of(source ? [] : renderedMarkdown),
    themeMode.of(EditorView.theme({})), rawUrlWrapping,
    documentLimits(rawText, warning, clearWarning),
    markdown({ base: markdownLanguage, codeLanguages: fencedLanguage, completeHTMLTags: false, pasteURLAsLink: false }),
    syntaxHighlighting(codeHighlighting), history(), drawSelection(), EditorView.lineWrapping,
    EditorState.allowMultipleSelections.of(true),
    placeholder("从这里开始记录，支持 Markdown…"),
    EditorView.contentAttributes.of({ "aria-label": "笔记正文", spellcheck: "false", tabindex: "0" }),
    EditorView.updateListener.of(publishChanges), EditorView.updateListener.of(scheduleStatistics),
    shortcutsMode.of(shortcutExtensions(bindings)),
    keymap.of([...typingKeymap, {key: "Tab", run: view => indentMarkdown(view), shift: view => indentMarkdown(view, true)}]),
    EditorView.domEventHandlers({
      click(event, editor) {
        if (!loaded || !canRequestEdit || !editor.state.readOnly || pendingFlush || event.button !== 0 ||
            (event.target as Element).closest("button,input,select,a,.md-language-control,.cm-panels,.cm-tooltip")) return false;
        if (!(event.target as Element).closest(".cm-content")) return false;
        const position = editor.posAtCoords({x:event.clientX,y:event.clientY});
        if (position !== null) post({type:"requestEdit",generation,position});
        return false;
      },
      paste: (() => {
        // Each loaded document owns its readers. Keep File objects queued and
        // encode one at a time, so completion order cannot reorder the images.
        const epoch = loadEpoch, requestGeneration = generation;
        const queue: File[] = [];
        let reading = false, count = 0, bytes = 0;
        const active = () => epoch === loadEpoch && loaded && !view.state.readOnly && !pendingFlush;
        function next(): void {
          if (!active()) { queue.length = 0; reading = false; count = bytes = 0; return; }
          const file = queue.shift();
          if (!file) { reading = false; count = bytes = 0; return; }
          reading = true;
          const reader = new FileReader();
          reader.addEventListener("load", () => {
            if (active() && typeof reader.result === "string") {
              post({type:"pasteImage",generation:requestGeneration,name:file.name,mime:file.type,base64:reader.result.slice(reader.result.indexOf(",")+1)});
            }
            next();
          });
          const failed = () => { if (active()) warning("无法读取剪贴板图片，请重新复制后粘贴"); next(); };
          reader.addEventListener("error", failed);
          reader.addEventListener("abort", failed);
          try { reader.readAsDataURL(file); } catch { failed(); }
        }
        return (event: ClipboardEvent): boolean => {
          const files = [...(event.clipboardData?.files ?? [])].filter(file => /^image\/(?:png|jpeg|gif|webp|bmp)$/.test(file.type));
          if (!files.length) {
            // WebView2 exposes PNG-only Windows clipboard entries as a trusted
            // empty paste. Ask the host for that image format only; never read
            // ordinary text or request ambient browser clipboard permission.
            if (event.isTrusted && event.clipboardData?.types.length === 0 && view.hasFocus) {
              event.preventDefault();
              if (active()) post({type:"requestNativeImagePaste",generation:requestGeneration});
              return true;
            }
            return false;
          }
          event.preventDefault();
          if (!active()) return true;
          if (files.some(file => file.size > 10 * 1024 * 1024)) { warning("图片超过 10 MiB，请先缩小图片"); return true; }
          const incomingBytes = files.reduce((total, file) => total + file.size, 0);
          if (count + files.length > 20 || bytes + incomingBytes > 20 * 1024 * 1024) {
            warning("每次最多粘贴 20 张图片、合计 20 MiB，请等待当前图片处理完成");
            return true;
          }
          count += files.length;
          bytes += incomingBytes;
          queue.push(...files);
          if (!reading) next();
          return true;
        };
      })(),
      dragover(event) { if (event.dataTransfer?.types.includes("Files")) { event.preventDefault(); return true; } return false; },
      drop(event) { if (event.dataTransfer?.files.length) { event.preventDefault(); return true; } return false; }
    }),
    // Transactions during IME must arrive before the flush barrier. Once frozen,
    // even programmatic commands cannot mutate the acknowledged document.
    EditorState.transactionFilter.of(transaction => transaction.docChanged && transaction.startState.readOnly ? [] : transaction)
  ];
}
const view = new EditorView({
  state: EditorState.create({ doc: "", extensions: extensions(true) }),
  parent: document.querySelector("#editor")!
});

function setReadonly(value: boolean): void {
  view.dispatch({ effects: readOnlyMode.reconfigure(readonlyExtensions(Boolean(value))) });
  updateToolbar();
}
function setSource(value: boolean): void {
  source = Boolean(value);
  document.documentElement.dataset.source = String(source);
  view.dispatch({ effects: renderingMode.reconfigure(source ? [] : renderedMarkdown) });
  updateToolbar();
}
function load(value: Load): void {
  if (!Number.isSafeInteger(value?.generation) || value.generation < 0 || typeof value.text !== "string") {
    error("笔记载入参数无效"); return;
  }
  loaded = false;
  updateImages({});
  unpublished = undefined;
  loadEpoch++;
  generation = value.generation;
  canRequestEdit = value.canRequestEdit === true;
  revision = 0;
  pendingFlush = false;
  status.textContent = "";
  status.classList.remove("error", "warning");
  // CM's Text uses LF internally. Only host canonical text owns raw CRLF/mixed EOL.
  view.setState(EditorState.create({ doc: value.text.replace(/\r\n?/g, "\n"), extensions: extensions(Boolean(value.readonly), value.text) }));
  setOutlineEnabled(value.outlineEnabled !== false);
  refreshOutline();
  view.scrollDOM.scrollTop = 0;
  loaded = true;
  updateToolbar();
  updateStatistics(true);
}
function rebase(value: Rebase): void {
  try {
    if (!loaded || !view.state.readOnly || pendingFlush || unpublished
      || value.previousGeneration !== generation || value.previousRevision !== revision
      || !Number.isSafeInteger(value.generation) || value.generation <= generation
      || typeof value.text !== "string"
      || !Array.isArray(value.changes)) throw new Error("保存内容的编辑状态已变化");
    let end = 0;
    for (const change of value.changes) {
      if (!Number.isSafeInteger(change.from) || !Number.isSafeInteger(change.to)
        || change.from < end || change.to < change.from || change.to > view.state.doc.length
        || typeof change.insert !== "string" || !scalarText(change.insert) || change.insert.includes("\r")
        || splitsScalar(view.state.doc, change.from) || splitsScalar(view.state.doc, change.to)) throw new Error("保存内容的映射无效");
      end = change.to;
    }
    const changes = ChangeSet.of(value.changes, view.state.doc.length);
    if (changes.apply(view.state.doc).toString() !== value.text.replace(/\r\n?/g, "\n")) throw new Error("保存内容校验失败");
    // This trusted host transaction maps the existing selection, history and
    // top visible line. It is not an editable interval or an undoable user edit.
    loaded = false;
    view.dispatch({ changes, filter: false, annotations: Transaction.addToHistory.of(false),
      effects: [view.scrollSnapshot().map(changes)!, resetDocumentBudget(value.text), readOnlyMode.reconfigure(readonlyExtensions(value.readonly))] });
    generation = value.generation;
    revision = 0;
    loadEpoch++;
    canRequestEdit = false;
    loaded = true;
    updateToolbar();
  } catch (reason) {
    loaded = true;
    error(reason instanceof Error ? reason.message : "保存内容映射失败");
  }
}

async function flush(value: { request: number }): Promise<void> {
  if (!Number.isSafeInteger(value?.request) || value.request < 0) { error("保存请求参数无效"); return; }
  const epoch = loadEpoch;
  const requestGeneration = generation;
  pendingFlush = true;
  updateToolbar();
  // Blurring the focused contenteditable commits a native IME composition.
  // CodeMirror then consumes its pending DOM mutations on the following tasks.
  view.contentDOM.blur();
  const deadline = performance.now() + 2500;
  let stableRevision = revision;
  let stableTicks = 0;
  while (stableTicks < 2) {
    await new Promise(resolve => setTimeout(resolve, 20));
    if (epoch !== loadEpoch) return;
    if (performance.now() > deadline) {
      pendingFlush = false;
      if (unpublished && !view.composing) {
        const rollback = incompleteScalars(view.state.doc);
        if (rollback.length) {
          view.dispatch({ changes: rollback, annotations: Transaction.addToHistory.of(false) });
          const message = "最后一个字符输入未完成，已取消该次字符；请重新输入后再保存。";
          warning(message);
          updateToolbar();
          post({ type: "flushRejected", generation: requestGeneration, request: value.request, message });
          return;
        }
      }
      updateToolbar();
      error("输入法或 Unicode 字符尚未完成，请完成当前输入后重试保存");
      return;
    }
    if (!view.composing && !unpublished && stableRevision === revision) stableTicks++;
    else stableTicks = 0;
    stableRevision = revision;
  }
  setReadonly(true);
  pendingFlush = false;
  updateToolbar();
  post({ type: "flush", generation: requestGeneration, request: value.request, revision, normalizedText: view.state.doc.toString() });
}

toolbar.addEventListener("mousedown", event => {
  // Keep the exact cross-paragraph selection while formatting with the toolbar.
  if ((event.target as Element).closest("button")) event.preventDefault();
});
const formatToggle = document.querySelector<HTMLButtonElement>("#format-toggle")!;
const formatMenu = document.querySelector<HTMLElement>("#format-menu")!;
function closeFormatMenu(): void { formatMenu.hidden = true; formatToggle.setAttribute("aria-expanded", "false"); }
formatToggle.addEventListener("click", () => { formatMenu.hidden = !formatMenu.hidden; formatToggle.setAttribute("aria-expanded", String(!formatMenu.hidden)); });
formatToggle.addEventListener("keydown", event => {
  if (!["ArrowDown", "ArrowUp", "Enter", " "].includes(event.key) || event.isComposing) return;
  event.preventDefault();
  formatMenu.hidden = false;
  formatToggle.setAttribute("aria-expanded", "true");
  const buttons = [...formatMenu.querySelectorAll<HTMLButtonElement>("button:not(:disabled)")];
  (event.key === "ArrowUp" ? buttons.at(-1) : buttons[0])?.focus();
});
document.addEventListener("mousedown", event => { if (!(event.target as Element).closest("#toolbar")) closeFormatMenu(); });
formatMenu.addEventListener("keydown", event => {
  const buttons = [...formatMenu.querySelectorAll<HTMLButtonElement>("button:not(:disabled)")];
  const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
  if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); closeFormatMenu(); view.focus(); }
  if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
    event.preventDefault();
    const next = event.key === "Home" ? 0 : event.key === "End" ? buttons.length-1 : (index+(event.key === "ArrowDown" ? 1 : -1)+buttons.length)%buttons.length;
    buttons[next]?.focus();
  }
});
toolbar.addEventListener("click", event => {
  const button = (event.target as Element).closest<HTMLButtonElement>("button[data-command],button[data-menu-command]");
  if (button && !button.disabled) { command((button.dataset.command ?? button.dataset.menuCommand)!); closeFormatMenu(); }
});
document.addEventListener("keydown", event => {
  // Native divider capture can coexist with WebView keyboard focus. Forward
  if ((event.target as Element).closest(".md-language-control")) return;
  // Escape before CodeMirror consumes it, without changing editor semantics.
  if (event.key === "Escape" && !event.isComposing && event.keyCode !== 229 && !view.composing && !event.repeat) {
    post({ type: "cancelResize", generation });
  }
}, { capture: true });
document.addEventListener("keydown", event => {
  // Toolbar focus uses exactly the same configured bindings. Focused content
  // was already handled by the compartment; don't dispatch a second time.
  if (!(event.target as Element).closest(".md-language-control") && !view.hasFocus) handleShortcut(event, activeShortcutLookup);
});
document.addEventListener("click", event => {
  if ((event.target as Element).closest("a")) event.preventDefault();
});
window.addEventListener("error", event => error(`编辑器错误：${event.message}`));
window.addEventListener("unhandledrejection", () => error("编辑器操作未完成，请保留当前内容并重试"));

function setImages(value: {generation:number;images:Record<string,unknown>}): void {
  if (!loaded || value?.generation !== generation || !value.images || typeof value.images !== "object" || Array.isArray(value.images)) return;
  updateImages(value.images);
  view.dispatch({effects:imageMapChanged.of()});
}
function insertText(text:string): boolean {
  if (typeof text !== "string" || !loaded || view.state.readOnly || pendingFlush || view.composing) return false;
  view.dispatch({...view.state.replaceSelection(text.replace(/\r\n?/g,"\n")),annotations:Transaction.userEvent.of("input.paste")});
  view.focus(); return true;
}
function restoreSelection(position: number): void {
  if (!loaded || view.state.readOnly || !Number.isSafeInteger(position)) return;
  let caret = Math.max(0, Math.min(view.state.doc.length, position));
  if (splitsScalar(view.state.doc, caret)) caret--;
  view.dispatch({selection:EditorSelection.cursor(caret),scrollIntoView:true});
  view.focus();
}
function setTheme(tokens: Record<string, unknown>): void {
  themeWarning.hidden = !applyEditorTheme(tokens);
  // A changed style module invalidates CodeMirror's measured text geometry,
  // including its cursor layer, without replacing document/history/selection.
  view.dispatch({effects:themeMode.reconfigure(EditorView.theme({}))});
  view.requestMeasure();
}
applyEditorTheme({});

const bridge: Bridge = { load, rebase, setReadonly, setCanRequestEdit: value => {canRequestEdit = value === true;}, restoreSelection, setTheme, setImages, insertText, setSource, setShortcuts, command, flush };
if (window.__STUDYAPP_EDITOR_TEST__ === true) {
  const observedKeys: Array<{key:string;code:string;binding:string|undefined;keyCode:number;prevented:boolean}> = [];
  // Metadata only: native smoke tests must never copy clipboard contents into
  // diagnostic artifacts. This observer does not alter the real paste event.
  const observedPastes: Array<{trusted:boolean;types:string[];files:Array<{type:string;size:number}>;prevented:boolean}> = [];
  document.addEventListener("paste", event => {
    const observed = {trusted:event.isTrusted,types:[...(event.clipboardData?.types ?? [])],files:[...(event.clipboardData?.files ?? [])].map(file => ({type:file.type,size:file.size})),prevented:false};
    observedPastes.push(observed);
    if (observedPastes.length > 8) observedPastes.shift();
    queueMicrotask(() => {observed.prevented = event.defaultPrevented;});
  }, true);
  document.addEventListener("keydown", event => {
    const observed = {key:event.key,code:event.code,binding:eventBinding(event),keyCode:event.keyCode,prevented:false};
    observedKeys.push(observed);
    if (observedKeys.length > 8) observedKeys.shift();
    queueMicrotask(() => {observed.prevented = event.defaultPrevented;});
  }, true);
  const snapshot = () => ({
    generation, revision, text: view.state.doc.toString(), normalizedText: view.state.doc.toString(),
    utf8Bytes: view.state.field(documentBudget).bytes, rawBytes: view.state.field(documentBudget).rawBytes,
    source, readonly: view.state.readOnly, composing: view.composing,
    scrollTop: view.scrollDOM.scrollTop, scrollLeft: view.scrollDOM.scrollLeft,
    canRequestEdit,
    bindings, editorFocused:view.hasFocus,
    activeElement:document.activeElement?.tagName+"."+document.activeElement?.className,
    observedKeys, observedPastes, pendingFlush,
    bodyClickPosition: view.posAtCoords({x:75 / devicePixelRatio,y:105 / devicePixelRatio}),
    theme: {
      mode: document.documentElement.dataset.theme,
      background: getComputedStyle(document.documentElement).getPropertyValue("--editor-background").trim(),
      surface: getComputedStyle(document.documentElement).getPropertyValue("--editor-surface").trim(),
      text: getComputedStyle(document.documentElement).getPropertyValue("--editor-text").trim(),
      readingWidth: getComputedStyle(document.documentElement).getPropertyValue("--editor-reading-width").trim(),
      contentBackground: getComputedStyle(view.scrollDOM).backgroundColor
    },
    images: view.contentDOM.querySelectorAll("img.md-image").length,
    imagePlaceholders: view.contentDOM.querySelectorAll(".md-image-placeholder").length,
    selection: { anchor: view.state.selection.main.anchor, head: view.state.selection.main.head },
    renderedLines: view.contentDOM.querySelectorAll(".cm-line").length,
    domNodes: view.dom.querySelectorAll("*").length,
    headings: view.contentDOM.querySelectorAll(".md-heading").length,
    highlights: view.contentDOM.querySelectorAll('[class*="tok-"]').length
  });
  bridge.test = async (action: TestAction) => {
    let result: unknown;
    const position = (value: number | undefined) => Math.max(0, Math.min(view.state.doc.length, value ?? 0));
    if (action.type === "snapshot") result = snapshot();
    else if (action.type === "layout") {
      // Setup only: acknowledge the host's actual viewport and a completed CM
      // measurement (including its scroll target/anchor corrections). Ordinary
      // snapshots intentionally remain immediate so continuity checks see jumps.
      const deadline = performance.now() + 4000;
      await new Promise<void>((resolve, reject) => {
        const timeout = setTimeout(() => reject(new Error("Editor layout completion timed out")), 4000);
        const measure = () => view.requestMeasure({
          read: () => ({ width: innerWidth, height: innerHeight }),
          write: layout => {
            if (Math.abs(layout.width - (action.width ?? NaN)) <= 1 && Math.abs(layout.height - (action.height ?? NaN)) <= 1) {
              clearTimeout(timeout);
              resolve();
            } else if (performance.now() < deadline) requestAnimationFrame(measure);
          }
        });
        requestAnimationFrame(measure);
      });
      result = { ...snapshot(), layout: { width: innerWidth, height: innerHeight } };
    }
    else if (action.type === "select") {
      view.dispatch({ selection: EditorSelection.range(position(action.anchor), position(action.head ?? action.anchor)), scrollIntoView: true });
      view.focus();
      result = snapshot();
    } else if (action.type === "replace") {
      view.dispatch({
        changes: { from: position(action.from), to: position(action.to ?? action.from), insert: (action.text ?? "").replace(/\r\n?/g, "\n") },
        annotations: Transaction.userEvent.of("input.test")
      });
      result = snapshot();
    } else if (action.type === "command") {
      command(action.name ?? "");
      result = snapshot();
    } else if (action.type === "performance") {
      const edits = Math.max(1, Math.min(1000, action.edits ?? 200));
      const samples: number[] = [];
      view.dispatch({ selection: EditorSelection.cursor(0), scrollIntoView: true });
      view.focus();
      for (let i = 0; i < edits; i++) {
        await new Promise(resolve => requestAnimationFrame(resolve));
        const start = performance.now();
        view.dispatch({ changes: { from: i, insert: "x" }, selection: EditorSelection.cursor(i + 1), annotations: Transaction.userEvent.of("input.type") });
        // Force layout so samples include decoration/DOM update cost, not only state changes.
        view.contentDOM.getBoundingClientRect();
        samples.push(performance.now() - start);
      }
      samples.sort((a, b) => a - b);
      result = { edits, medianMs: samples[Math.floor(samples.length / 2)], p95Ms: samples[Math.ceil(samples.length * .95) - 1],
        renderedLines: view.contentDOM.querySelectorAll(".cm-line").length, domNodes: view.dom.querySelectorAll("*").length };
    } else throw new Error(`Unknown test action: ${action.type}`);
    if (action.request !== undefined) post({ type: "test", generation, request: action.request, result });
    return result;
  };
}
window.studyappEditor = bridge;
updateToolbar();
post({ type: "ready" });

installOutline(view);
