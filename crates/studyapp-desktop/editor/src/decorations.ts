import { Range, StateEffect, Text, Transaction } from "@codemirror/state";
import { syntaxTree } from "@codemirror/language";
import { markdownLanguage } from "@codemirror/lang-markdown";
import { Decoration, DecorationSet, EditorView, ViewPlugin, ViewUpdate, WidgetType } from "@codemirror/view";
import { completeFenceLanguage, normalizeFenceLanguage } from "./languages";
import { isolateHistory } from "@codemirror/commands";
import type { SyntaxNode } from "@lezer/common";

type TableModel = { columns:number; alignment:string[]; firstLine:number };
function tableModel(node:SyntaxNode,state:EditorView["state"]):TableModel {
  const separator=node.getChild("TableDelimiter");
  const alignment=separator ? state.sliceDoc(separator.from,separator.to).trim().replace(/^\||\|$/g,"").split("|").map(value=>/^\s*:.*:\s*$/.test(value)?"center":/:\s*$/.test(value)?"right":"left") : [];
  return {
    columns:alignment.length || node.getChild("TableHeader")?.getChildren("TableCell").length || 1,
    alignment,
    firstLine:state.doc.lineAt(node.from).number,
  };
}

export const imageMapChanged = StateEffect.define<void>();
const languageFocusChanged = StateEffect.define<void>();
let resolvedImages: ReadonlyMap<string, string> = new Map();
export function updateImages(images: Record<string, unknown>): void {
  const next = new Map<string,string>();
  let total = 0;
  for (const [destination,value] of Object.entries(images)) {
    if (destination.length > 4096 || typeof value !== "string" || value.length > 14*1024*1024) continue;
    if (!/^data:image\/(?:png|jpeg|gif|webp|bmp);base64,[A-Za-z0-9+/]+={0,2}$/.test(value)) continue;
    const encoded = value.slice(value.indexOf(",")+1);
    const bytes = encoded.length*3/4 - (encoded.endsWith("==") ? 2 : encoded.endsWith("=") ? 1 : 0);
    if (bytes > 10*1024*1024 || total+bytes > 64*1024*1024) continue;
    total += bytes; next.set(destination,value);
  }
  resolvedImages = next;
}
class ImageWidget extends WidgetType {
  constructor(readonly source:string,readonly alt:string) { super(); }
  eq(other:ImageWidget) { return this.source === other.source && this.alt === other.alt; }
  toDOM() {
    const image = document.createElement("img");
    image.className = "md-image"; image.src = this.source; image.alt = this.alt;
    image.draggable = false;
    return image;
  }
  ignoreEvent() { return false; }
}

class MissingImageWidget extends WidgetType {
  constructor(readonly alt:string) { super(); }
  eq(other:MissingImageWidget) { return this.alt === other.alt; }
  toDOM() {
    const placeholder = document.createElement("span");
    placeholder.className = "md-image-placeholder";
    placeholder.setAttribute("role","img");
    placeholder.setAttribute("aria-label",`图片未找到：${this.alt || "图片"}`);
    placeholder.title = "图片未找到，选中后可编辑 Markdown 引用";
    placeholder.textContent = `▧ ${this.alt || "图片未找到"}`;
    return placeholder;
  }
  ignoreEvent() { return false; }
}

function decodeMarkdown(value:string):string {
  // Character references are inert text; angle brackets are never parsed as HTML.
  const decoder = document.createElement("textarea");
  decoder.innerHTML = value.replaceAll("<","&lt;").replaceAll(">","&gt;");
  return decoder.value.replace(/\\([!"#$%&'()*+,\-./:;<=>?@[\]\\^_`{|}~])/g,"$1");
}
function referenceLabel(value:string):string {
  return decodeMarkdown(value).trim().replace(/\s+/g," ").toUpperCase();
}

class LanguageWidget extends WidgetType {
  constructor(readonly from: number, readonly to: number, readonly token: string) { super(); }
  eq(other: LanguageWidget) { return this.from === other.from && this.to === other.to && this.token === other.token; }
  toDOM(view: EditorView) {
    const wrapper = document.createElement("span");
    wrapper.className = "md-language-control";
    const input = document.createElement("input");
    input.className = "md-language-picker";
    input.dataset.languageFrom = String(this.from);
    input.setAttribute("role", "combobox");
    input.setAttribute("aria-label", "代码语言");
    input.setAttribute("aria-autocomplete", "list");
    input.setAttribute("aria-expanded", "false");
    input.placeholder = "纯文本";
    input.value = this.token === "text" ? "" : this.token;
    input.autocomplete = "off";
    input.spellcheck = false;
    const list = document.createElement("span");
    list.className = "md-language-completions";
    list.setAttribute("role", "listbox");
    list.id = `language-options-${this.from}`;
    input.setAttribute("aria-controls", list.id);
    let choices: string[] = [], selected = -1, dirty = false, finished = false;
    const show = () => {
      choices = completeFenceLanguage(input.value);
      selected = -1;
      list.replaceChildren();
      for (const [index, choice] of choices.entries()) {
        const option = document.createElement("span");
        option.setAttribute("role", "option");
        option.id = `${list.id}-${index}`;
        option.textContent = choice;
        option.addEventListener("mousedown", event => { event.preventDefault(); input.value = choice; dirty = true; commit(true); });
        list.append(option);
      }
      list.hidden = choices.length === 0;
      input.setAttribute("aria-expanded", String(choices.length > 0));
    };
    const commit = (focus: boolean) => {
      if (finished) return;
      finished = true;
      list.hidden = true;
      input.setAttribute("aria-expanded", "false");
      if (dirty && !view.state.readOnly && view.state.sliceDoc(this.from, this.to) === this.token) {
        const value = normalizeFenceLanguage(input.value);
        const adjacentMetadata = value && this.from === this.to && /\S/.test(view.state.sliceDoc(this.to,this.to+1));
        const changes = view.state.changes({ from: this.from, to: this.to, insert: value + (adjacentMetadata ? " " : "") });
        view.dispatch({ changes, selection: view.state.selection.map(changes), annotations: [Transaction.userEvent.of("input.language"), isolateHistory.of("full")] });
      }
      if (focus) view.focus();
    };
    input.addEventListener("focus", () => { finished = false; dirty = false; });
    input.addEventListener("input", () => { dirty = true; show(); });
    input.addEventListener("blur", () => {
      commit(false);
      // CodeMirror's hasFocus already becomes false while this input owns
      // focus, so leaving it need not produce another CM focusChanged update.
      // Wait until focus has reached its destination (and any DOM update that
      // removed this widget has finished) before reconciling the decoration.
      queueMicrotask(() => {
        if (view.dom.isConnected) view.dispatch({ effects: languageFocusChanged.of() });
      });
    });
    input.addEventListener("keydown", event => {
      event.stopPropagation();
      if (event.isComposing || event.keyCode === 229) return;
      if (event.key === "Escape") {
        event.preventDefault(); finished = true; list.hidden = true;
        input.value = this.token === "text" ? "" : this.token;
        input.setAttribute("aria-expanded", "false"); view.focus();
      } else if (event.key === "Enter") {
        event.preventDefault();
        if (selected >= 0) { input.value = choices[selected]; dirty = true; }
        commit(true);
      } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        if (!choices.length) show();
        if (!choices.length) return;
        selected = (selected + (event.key === "ArrowDown" ? 1 : -1) + choices.length) % choices.length;
        for (const [index, option] of [...list.children].entries()) option.setAttribute("aria-selected", String(index === selected));
        input.setAttribute("aria-activedescendant", `${list.id}-${selected}`);
      }
    });
    list.hidden = true;
    wrapper.append(input, list);
    return wrapper;
  }
  ignoreEvent() { return true; }
}

class BulletWidget extends WidgetType {
  eq() { return true; }
  toDOM() {
    const span = document.createElement("span");
    span.className = "md-bullet";
    span.textContent = "•";
    span.setAttribute("aria-hidden", "true");
    return span;
  }
}

class EmptyCellWidget extends WidgetType {
  constructor(readonly columns:number, readonly alignment:string) { super(); }
  eq(other:EmptyCellWidget) { return this.columns === other.columns && this.alignment === other.alignment; }
  toDOM() {
    const cell = document.createElement("span");
    cell.className = "md-table-cell md-empty-cell";
    cell.style.width = `${100/this.columns}%`;
    cell.style.textAlign = this.alignment;
    cell.setAttribute("aria-label","空单元格");
    return cell;
  }
}

class TaskWidget extends WidgetType {
  constructor(readonly from: number, readonly checked: boolean, readonly disabled: boolean) { super(); }
  eq(other: TaskWidget) { return this.from === other.from && this.checked === other.checked && this.disabled === other.disabled; }
  toDOM() {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "md-task";
    button.setAttribute("role", "checkbox");
    button.setAttribute("aria-checked", String(this.checked));
    button.setAttribute("aria-label", "切换任务完成状态");
    button.dataset.taskFrom = String(this.from);
    button.disabled = this.disabled;
    button.textContent = this.checked ? "✓" : "";
    button.addEventListener("keydown", event => {
      if (event.key === "Enter" || event.key === " ") {
        // A widget button lives inside the editor's event tree. Consume these
        // before CodeMirror interprets Enter/Space as text at its old cursor.
        event.preventDefault();
        event.stopPropagation();
        button.click();
      }
    });
    return button;
  }
  ignoreEvent() { return false; }
}

// This policy belongs to URL source tokens, including source mode. Keeping it
// separate from rich rendering also leaves prose and code word breaking alone.
export const rawUrlWrapping = ViewPlugin.fromClass(class {
  decorations: DecorationSet;
  tree;
  constructor(view: EditorView) {
    this.tree = syntaxTree(view.state);
    this.decorations = this.build(view);
  }
  update(update: ViewUpdate) {
    const tree = syntaxTree(update.state);
    if (update.docChanged || update.viewportChanged || tree !== this.tree) {
      this.tree = tree;
      this.decorations = this.build(update.view);
    }
  }
  build(view: EditorView): DecorationSet {
    const ranges: Range<Decoration>[] = [];
    const seen = new Set<number>();
    for (const visible of view.visibleRanges) this.tree.iterate({
      from: visible.from, to: visible.to,
      enter: node => {
        if (node.name === "URL" && node.to > node.from && !seen.has(node.from)) {
          seen.add(node.from);
          ranges.push(Decoration.mark({ class: "md-raw-url" }).range(node.from,node.to));
        }
      }
    });
    return Decoration.set(ranges,true);
  }
}, { decorations: plugin => plugin.decorations });

export const renderedMarkdown = ViewPlugin.fromClass(class {
  decorations: DecorationSet;
  tree;
  referenceDocument?: Text;
  referenceDestinations?: Map<string,string>;
  constructor(view: EditorView) {
    this.tree = syntaxTree(view.state);
    this.decorations = this.build(view);
  }
  update(update: ViewUpdate) {
    const tree = syntaxTree(update.state);
    if (update.transactions.some(transaction => transaction.effects.some(effect => effect.is(imageMapChanged) || effect.is(languageFocusChanged))) || update.docChanged || update.selectionSet || update.focusChanged || update.viewportChanged || tree !== this.tree ||
        update.startState.readOnly !== update.state.readOnly) {
      this.tree = tree;
      this.decorations = this.build(update.view);
    }
  }
  build(view: EditorView): DecorationSet {
    const ranges: Range<Decoration>[] = [];
    const lines = new Set<string>();
    const seen = new Set<string>();
    const tables = new Map<number,TableModel>();
    const { state } = view;
    const references = () => {
      if (this.referenceDocument !== state.doc) {
        this.referenceDocument = state.doc;
        this.referenceDestinations = undefined;
      }
      if (!this.referenceDestinations) {
        this.referenceDestinations = new Map();
        // Resolve distant definitions too. The full parse is lazy, cached per
        // document and needed only when an actual reference image is visible.
        const tree = this.tree.length === state.doc.length ? this.tree : markdownLanguage.parser.parse(state.doc.toString());
        tree.iterate({enter: cursor => {
          if (cursor.name !== "LinkReference") return;
          const label = cursor.node.getChild("LinkLabel"), url = cursor.node.getChild("URL");
          if (!label || !url) return;
          const key = referenceLabel(state.sliceDoc(label.from+1,label.to-1));
          if (!this.referenceDestinations!.has(key)) this.referenceDestinations!.set(key,decodeMarkdown(state.sliceDoc(url.from,url.to).replace(/^<|>$/g,"")));
        }});
      }
      return this.referenceDestinations;
    };
    const focusedElement = view.root.activeElement;
    // The language input is a focusable widget inside its code block, rather
    // than CM's contentDOM. Retain that block while typing a language, but a
    // stale selection alone must not keep controls visible after leaving it.
    const focusedLanguage = view.dom.ownerDocument.hasFocus() && focusedElement instanceof HTMLInputElement &&
      view.dom.contains(focusedElement) && focusedElement.matches(".md-language-picker")
      ? Number(focusedElement.dataset.languageFrom) : -1;
    const active = (from: number, to: number) => !state.readOnly &&
      state.selection.ranges.some(range => range.from <= to && range.to >= from);
    const mark = (from: number, to: number, className: string, attributes?: Record<string, string>) => {
      if (to > from) ranges.push(Decoration.mark({ class: className, attributes }).range(from, to));
    };
    const hide = (from: number, to: number) => {
      // ViewPlugin replacements may never remove line breaks. Markdown link
      // titles can span lines, so retain each newline and hide only line text.
      for (let position = from; position < to;) {
        const current = state.doc.lineAt(position);
        const end = Math.min(to, current.to);
        if (end > position) ranges.push(Decoration.replace({}).range(position, end));
        position = current.to + 1;
      }
    };
    const line = (position: number, className: string, attributes?: Record<string,string>) => {
      const from = state.doc.lineAt(position).from;
      const key = `${from}:${className}`;
      if (!lines.has(key)) {
        lines.add(key);
        ranges.push(Decoration.line({ class: className, attributes }).range(from));
      }
    };
    const listLines = new Map<number,{depth:number;marker:boolean}>();
    for (const visible of view.visibleRanges) {
      this.tree.iterate({
        from: visible.from, to: visible.to,
        enter: cursor => {
          const { name, from, to } = cursor;
          const key = `${name}:${from}:${to}`;
          if (seen.has(key)) return;
          seen.add(key);
          const node = cursor.node;
          const parent = node.parent;
          if (/^(ATXHeading|SetextHeading)[1-6]$/.test(name)) {
            line(from, `md-heading md-heading-${name.at(-1)}`);
          } else if (name === "StrongEmphasis") {
            mark(from, to, "md-strong");
          } else if (name === "Emphasis") {
            mark(from, to, "md-emphasis");
          } else if (name === "Strikethrough") {
            mark(from, to, "md-strike");
          } else if (name === "InlineCode") {
            mark(from, to, "md-inline-code");
          } else if (name === "Blockquote") {
            const first = state.doc.lineAt(Math.max(from, visible.from)).number;
            const last = state.doc.lineAt(Math.min(to, visible.to)).number;
            for (let i = first; i <= last; i++) line(state.doc.line(i).from, "md-quote");
          } else if (name === "FencedCode" || name === "CodeBlock") {
            const first = state.doc.lineAt(Math.max(from, visible.from)).number;
            const last = state.doc.lineAt(Math.min(to, visible.to)).number;
            for (let i = first; i <= last; i++) line(state.doc.line(i).from, "md-code-line");
            line(from,"md-code-start");
            line(to,"md-code-end");
            if (name === "FencedCode") {
              const opening = state.doc.lineAt(from);
              const fence = node.getChild("CodeMark");
              const codeFocused = view.hasFocus || focusedLanguage >= from && focusedLanguage <= to;
              if (fence && active(from, to) && codeFocused) {
                const token = /^(\s*)(\S*)/.exec(state.sliceDoc(fence.to, opening.to))!;
                let start = fence.to + token[1].length;
                // Attribute metadata is never mistaken for a language after a
                // language is cleared. Unknown plain language tags stay intact.
                const metadata = /[={}\[\]"']/.test(token[2]);
                const value = metadata ? "" : token[2];
                if (metadata) start = fence.to;
                const bottom = state.doc.lineAt(to).to;
                ranges.push(Decoration.widget({ widget: new LanguageWidget(start, start + value.length, value), side: 1 }).range(bottom));
              }
            }
          } else if (name === "HeaderMark" && parent && !active(parent.from, parent.to)) {
            const end = state.doc.lineAt(to).to;
            hide(from, Math.min(end, to + (state.sliceDoc(to, to + 1) === " " ? 1 : 0)));
          } else if (["EmphasisMark", "StrikethroughMark", "CodeMark"].includes(name) && parent && !active(parent.from, parent.to)) {
            hide(from, to);
            if (parent.name === "FencedCode") line(from, "md-code-fence");
          } else if (name === "CodeInfo" && parent && !active(parent.from, parent.to)) {
            mark(from, to, "md-code-language");
          } else if (name === "ListItem") {
            let depth = 0;
            for (let ancestor = node.parent; ancestor; ancestor = ancestor.parent) {
              if (ancestor.name === "BulletList" || ancestor.name === "OrderedList") depth++;
            }
            const first = state.doc.lineAt(Math.max(from,visible.from)).number;
            const last = state.doc.lineAt(Math.min(to,visible.to)).number;
            for (let i = first; i <= last; i++) {
              const current = state.doc.line(i);
              if ((listLines.get(current.from)?.depth ?? 0) < depth) listLines.set(current.from,{depth,marker:current.number===state.doc.lineAt(from).number});
            }
          } else if (name === "ListMark") {
            const current = state.doc.lineAt(from);
            if (!active(current.from, current.to) && /^[-+*]$/.test(state.sliceDoc(from, to))) {
              const task = parent?.getChild("Task");
              if (task) hide(from, Math.min(to + 1, current.to));
              else ranges.push(Decoration.replace({ widget: new BulletWidget() }).range(from, to));
            } else mark(from, to, "md-list-marker");
          } else if (name === "QuoteMark") {
            const current = state.doc.lineAt(from);
            if (!active(current.from, current.to)) hide(from, Math.min(to + 1, current.to));
          } else if (name === "TaskMarker") {
            const current = state.doc.lineAt(from);
            if (!active(current.from, current.to)) {
              ranges.push(Decoration.replace({ widget: new TaskWidget(from, /\[[xX]\]/.test(state.sliceDoc(from, to)), state.readOnly) }).range(from, to));
            }
          } else if (name === "Image" && !active(from,to)) {
            const url = node.getChild("URL");
            const marks = node.getChildren("LinkMark");
            const alt = marks.length >= 2 ? decodeMarkdown(state.sliceDoc(marks[0].to,marks[1].from)) : "";
            const label = node.getChild("LinkLabel");
            const key = label ? state.sliceDoc(label.from+1,label.to-1) || alt : alt;
            const destination = url ? decodeMarkdown(state.sliceDoc(url.from,url.to).replace(/^<|>$/g,"")) : references().get(referenceLabel(key));
            const source = destination ? resolvedImages.get(destination) : undefined;
            if (state.doc.lineAt(from).number === state.doc.lineAt(to).number) {
              ranges.push(Decoration.replace({widget:source ? new ImageWidget(source,alt) : new MissingImageWidget(alt)}).range(from,to));
            } else {
              // ViewPlugin replacements cannot span line breaks. Keep those
              // breaks in the document and hide each source fragment separately.
              ranges.push(Decoration.widget({widget:source ? new ImageWidget(source,alt) : new MissingImageWidget(alt),side:1}).range(from));
              hide(from,to);
            }
            return false;
          } else if (name === "Link" || name === "Autolink") {
            const urlNode = node.getChild("URL");
            const url = urlNode ? state.sliceDoc(urlNode.from, urlNode.to) : "";
            const safe = /^(https?:\/\/|mailto:)/i.test(url);
            mark(from, to, "md-link", { title: safe ? `${url}（选中以编辑链接）` : "选中以编辑链接" });
            if (!active(from, to) && name === "Link") {
              for (let child = node.firstChild; child; child = child.nextSibling) {
                if (["LinkMark", "URL", "LinkTitle"].includes(child.name)) hide(child.from, child.to);
              }
            }
          } else if (name === "TableHeader" || name === "TableRow") {
            if (!parent) return;
            let model=tables.get(parent.from);
            if (!model) {model=tableModel(parent,state);tables.set(parent.from,model);}
            const {columns,alignment}=model;
            const rowNumber = state.doc.lineAt(from).number-model.firstLine;
            line(from, `md-table-row${name === "TableHeader" ? " md-table-header" : rowNumber%2===0 ? " md-table-stripe" : ""}`, {"data-table-columns":String(columns)});
            const delimiters = node.getChildren("TableDelimiter");
            // Include cell whitespace so there are no anonymous text flex items.
            // The same source ranges remain decorated while the caret is inside.
            let start = from;
            let index = 0;
            for (const [delimiterIndex,delimiter] of delimiters.entries()) {
              if (delimiter.from > start || delimiterIndex > 0) {
                if (delimiter.from === start) ranges.push(Decoration.widget({widget:new EmptyCellWidget(columns,alignment[index]||"left"),side:-1}).range(start));
                else mark(start,delimiter.from,"md-table-cell",{style:`width:${100/columns}%;text-align:${alignment[index]||"left"}`});
                index++;
              }
              hide(delimiter.from,delimiter.to);
              start = delimiter.to;
            }
            if (start < to) { mark(start,to,"md-table-cell",{style:`width:${100/columns}%;text-align:${alignment[index]||"left"}`}); index++; }
            for (; index < columns; index++) {
              ranges.push(Decoration.widget({widget:new EmptyCellWidget(columns,alignment[index]||"left"),side:1}).range(to));
            }
          } else if (name === "TableDelimiter") {
            const current = state.doc.lineAt(from);
            if (parent?.name === "Table" && !active(current.from, current.to)) {
              hide(from, to);
              if (parent?.name === "Table") line(from, "md-table-separator");
            }
          } else if (name === "HorizontalRule") {
            line(from, "md-rule");
            if (!active(from, to)) hide(from, to);
          }
        }
      });
    }
    for (const [position,{depth,marker}] of listLines) {
      const current = state.doc.lineAt(position);
      const indentation = /^\s*/.exec(current.text)![0].length;
      if (indentation) hide(position,position+indentation);
      line(position,`md-list-line${marker ? "" : " md-list-continuation"}`,{"data-list-depth":String(depth),style:`--list-depth:${depth}`});
    }
    return Decoration.set(ranges, true);
  }
}, {
  decorations: plugin => plugin.decorations,
  eventHandlers: {
    mousedown(event) {
      if ((event.target as Element).closest(".md-task")) { event.preventDefault(); return true; }
      return false;
    },
    click(event, view) {
      const target = (event.target as Element).closest<HTMLButtonElement>("[data-task-from]");
      if (!target || view.state.readOnly) return false;
      const from = Number(target.dataset.taskFrom);
      const original = view.state.sliceDoc(from, from + 3);
      if (!/^\[[ xX]\]$/.test(original)) return false;
      const hadFocus = document.activeElement === target;
      view.dispatch({
        changes: { from: from + 1, to: from + 2, insert: original[1] === " " ? "x" : " " },
        annotations: Transaction.userEvent.of("input.task")
      });
      if (hadFocus) view.dom.querySelector<HTMLButtonElement>(`[data-task-from="${from}"]`)?.focus({ preventScroll: true });
      event.preventDefault();
      return true;
    }
  }
});
