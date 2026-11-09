import { parser } from "@lezer/markdown";
import { EditorView } from "@codemirror/view";

export interface OutlinePreference { position: "auto" | "left" | "right" | "top" | "floating"; shown: boolean; }
let applyPreference: (value: unknown) => void = () => {};
export function setOutlinePreference(value: unknown): void { applyPreference(value); }
let applyEnabled: (enabled: boolean) => void = () => {};
export function setOutlineEnabled(enabled: boolean): void { applyEnabled(enabled); }
let scheduleRefresh: (reset?: boolean) => void = () => {};
let updateCurrent: () => void = () => {};
export const outlineUpdates = EditorView.updateListener.of(update => {
  if (update.docChanged) scheduleRefresh();
  else if (update.selectionSet) updateCurrent();
});
export function refreshOutline(): void { scheduleRefresh(true); }

interface Heading {
  from: number;
  level: number;
  title: string;
  parent: number;
  children: number[];
  row: HTMLElement;
  group: HTMLElement;
}

export function installOutline(view: EditorView, onPreferenceChange: (value: OutlinePreference) => void): void {
  const toggle = document.createElement("button");
  toggle.type = "button";
  toggle.id = "outline-toggle";
  // Inline SVG uses the editor's own colors and works entirely offline.
  const icon = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  icon.setAttribute("viewBox", "0 0 20 20");
  icon.setAttribute("aria-hidden", "true");
  const path = document.createElementNS(icon.namespaceURI, "path");
  path.setAttribute("d", "M3 4h14v12H3z M8 4v12 M11 8h3 M11 11h3");
  icon.append(path);
  toggle.append(icon);
  toggle.setAttribute("aria-label", "笔记目录");
  toggle.setAttribute("aria-expanded", "false");
  toggle.setAttribute("aria-controls", "note-outline");
  document.querySelector("#toolbar")!.append(toggle);

  const panel = document.createElement("nav");
  panel.id = "note-outline";
  panel.hidden = true;
  panel.setAttribute("aria-label", "笔记目录");
  const header = document.createElement("div");
  header.className = "outline-header";
  const caption = document.createElement("span");
  caption.textContent = "大纲";
  const expand = document.createElement("button");
  expand.type = "button";
  expand.setAttribute("aria-label", "全部展开");
  expand.title = "全部展开";
  const expandIcon = icon.cloneNode(true) as SVGElement;
  expandIcon.querySelector("path")!.setAttribute("d", "m6 6 4-4 4 4m-8 8 4 4 4-4");
  expand.append(expandIcon);
  const collapse = document.createElement("button");
  collapse.type = "button";
  collapse.setAttribute("aria-label", "全部收起");
  collapse.title = "全部收起";
  const collapseIcon = icon.cloneNode(true) as SVGElement;
  collapseIcon.querySelector("path")!.setAttribute("d", "m6 2 4 4 4-4m-8 16 4-4 4 4");
  collapse.append(collapseIcon);
  const position = document.createElement("select");
  position.id = "outline-position";
  position.setAttribute("aria-label", "目录位置");
  for (const [value, label] of [["auto", "自动"], ["left", "左侧"], ["right", "右侧"], ["top", "上方"], ["floating", "浮层"]]) {
    const option = document.createElement("option");
    option.value = value;
    option.textContent = label;
    position.append(option);
  }
  const hide = document.createElement("button");
  hide.type = "button";
  hide.className = "outline-hide";
  hide.setAttribute("aria-label", "隐藏目录");
  hide.title = "隐藏目录";
  const hideIcon = icon.cloneNode(true) as SVGElement;
  hideIcon.querySelector("path")!.setAttribute("d", "M3 4h14v12H3z M8 4v12 M6 8l-2 2 2 2");
  hide.append(hideIcon);
  header.append(caption, position, hide);
  const actions = document.createElement("div");
  actions.className = "outline-actions";
  actions.append(expand, collapse);
  const tree = document.createElement("div");
  tree.className = "outline-tree";
  tree.setAttribute("role", "tree");
  tree.setAttribute("aria-label", "章节");
  panel.append(header, actions, tree);
  const editor = document.querySelector<HTMLElement>("#editor")!;
  const workspace = document.createElement("div");
  workspace.id = "editor-workspace";
  editor.before(workspace);
  workspace.append(panel, editor);
  const storageKey = "studyapp.outline.v1";
  let preference: OutlinePreference["position"] = "auto";
  let shown = false;
  try {
    const saved = JSON.parse(localStorage.getItem(storageKey) ?? "null");
    if (["auto", "left", "right", "top", "floating"].includes(saved?.position)) preference = saved.position;
    shown = saved?.shown === true;
  } catch { /* A blocked/empty profile still has a usable default layout. */ }
  position.value = preference;
  const persist = () => {
    try { localStorage.setItem(storageKey, JSON.stringify({ position: preference, shown })); } catch { /* Native preferences remain authoritative. */ }
    onPreferenceChange({ position: preference, shown });
  };
  function layout(): void {
    const { width, height } = workspace.getBoundingClientRect();
    // Observe the actual editor pane, not the app/screen: split panes and
    // sidebars can change its size without a browser resize event.
    const automatic = width >= 900 && height >= 320 ? "left" : height >= 440 ? "top" : "floating";
    // Never let a manual sidebar consume the entire document at tiny widths.
    const resolved = (preference === "left" || preference === "right") && width < 480 ? "floating" : preference === "auto" ? automatic : preference;
    workspace.dataset.outlinePosition = resolved;
    workspace.dataset.outlineShown = String(!panel.hidden);
    toggle.setAttribute("aria-expanded", String(!panel.hidden));
    toggle.setAttribute("aria-label", panel.hidden ? "显示目录" : "隐藏目录");
    toggle.title = panel.hidden ? "显示目录" : "隐藏目录";
  }
  new ResizeObserver(layout).observe(workspace);
  position.onchange = () => { preference = position.value as OutlinePreference["position"]; persist(); layout(); };

  let timer: ReturnType<typeof setTimeout>;
  let frame = 0;
  let enabled = true;
  let source = "";
  let headings: Heading[] = [];
  const folded = new Set<string>();
  const key = (heading: Heading) => `${heading.from}:${heading.title}`;
  const isFolded = (heading: Heading) => folded.has(key(heading));
  const visible = () => headings.filter(heading => {
    for (let parent = heading.parent; parent >= 0; parent = headings[parent].parent) {
      if (isFolded(headings[parent])) return false;
    }
    return true;
  });
  function focusHeading(heading: Heading): void {
    for (const item of headings) item.row.tabIndex = item === heading ? 0 : -1;
    heading.row.focus({ preventScroll: true });
    heading.row.scrollIntoView({ block: "nearest" });
  }
  function setFolded(heading: Heading, value: boolean): void {
    if (!heading.children.length) return;
    if (value) folded.add(key(heading)); else folded.delete(key(heading));
    heading.row.setAttribute("aria-expanded", String(!value));
    heading.group.hidden = value;
    const disclosure = heading.row.querySelector(".outline-disclosure")!;
    disclosure.setAttribute("aria-label", `${value ? "展开" : "收起"} ${heading.title}`);
  }
  function markCurrent(position: number): void {
    let current: Heading | undefined;
    for (const heading of headings) {
      if (heading.from > position) break;
      current = heading;
    }
    for (let parent = current?.parent ?? -1; parent >= 0; parent = headings[parent].parent) {
      if (isFolded(headings[parent])) current = headings[parent];
    }
    for (const heading of headings) {
      if (heading === current) heading.row.setAttribute("aria-current", "location");
      else heading.row.removeAttribute("aria-current");
    }
  }
  function navigate(heading: Heading): void {
    if (source !== view.state.doc.toString()) return;
    view.dispatch({ selection: { anchor: heading.from },
      effects: EditorView.scrollIntoView(heading.from, { y: "start" }) });
    markCurrent(heading.from);
    view.focus();
  }
  function render(): void {
    if (panel.hidden || !enabled) return;
    const scrollTop = tree.scrollTop;
    const focusedKey = headings.find(heading => heading.row === document.activeElement || heading.row.firstElementChild!.contains(document.activeElement));
    const previousFocus = focusedKey ? key(focusedKey) : undefined;
    source = view.state.doc.toString();
    const fragment = document.createDocumentFragment();
    const stack: number[] = [];
    headings = [];
    parser.parse(source).iterate({ enter(node) {
      const match = /^(?:ATX|Setext)Heading([1-6])$/.exec(node.name);
      if (!match) return;
      const level = Number(match[1]);
      while (stack.length && headings[stack.at(-1)!].level >= level) stack.pop();
      const parent = stack.at(-1) ?? -1;
      const title = source.slice(node.from, node.to)
        .replace(/^ {0,3}#{1,6}(?:\s+|$)/, "")
        .replace(/\s+#+\s*$/, "")
        .replace(/\n {0,3}(?:=+|-+)\s*$/, "").trim() || "（空标题）";
      const item = document.createElement("div");
      item.setAttribute("role", "treeitem");
      item.setAttribute("aria-level", String(stack.length + 1));
      item.setAttribute("aria-label", title);
      item.tabIndex = headings.length ? -1 : 0;
      const row = document.createElement("div");
      row.className = "outline-row";
      row.style.paddingLeft = `${8 + stack.length * 16}px`;
      const label = document.createElement("button");
      label.type = "button";
      label.className = "outline-label";
      label.tabIndex = -1;
      label.append(outlineTitle(title));
      label.title = label.textContent || title;
      item.setAttribute("aria-label",label.title);
      const group = document.createElement("div");
      group.setAttribute("role", "group");
      const heading: Heading = { from: node.from, level, title, parent, children: [], row: item, group };
      label.onclick = () => { if (!label.disabled) navigate(heading); };
      row.append(label);
      item.append(row, group);
      if (parent >= 0) {
        headings[parent].children.push(headings.length);
        headings[parent].group.append(item);
      } else fragment.append(item);
      headings.push(heading);
      stack.push(headings.length - 1);
      item.onkeydown = event => {
        if (event.key === "Escape") return; // The panel handles hiding/focus.
        if (source !== view.state.doc.toString()) return;
        event.stopPropagation();
        const items = visible();
        const index = items.indexOf(heading);
        switch (event.key) {
          case "ArrowDown": focusHeading(items[Math.min(index + 1, items.length - 1)]); break;
          case "ArrowUp": focusHeading(items[Math.max(index - 1, 0)]); break;
          case "Home": focusHeading(items[0]); break;
          case "End": focusHeading(items.at(-1)!); break;
          case "ArrowRight":
            if (isFolded(heading)) setFolded(heading, false);
            else if (heading.children.length) focusHeading(headings[heading.children[0]]);
            break;
          case "ArrowLeft":
            if (heading.children.length && !isFolded(heading)) setFolded(heading, true);
            else if (parent >= 0) focusHeading(headings[parent]);
            break;
          case "Enter": case " ": navigate(heading); break;
          default: return;
        }
        markCurrent(view.state.selection.main.head);
        event.preventDefault();
      };
    }});
    const validKeys = new Set(headings.map(key));
    for (const value of folded) if (!validKeys.has(value)) folded.delete(value);
    for (const heading of headings) {
      if (heading.children.length) {
        const disclosure = document.createElement("button");
        disclosure.type = "button";
        disclosure.tabIndex = -1;
        disclosure.className = "outline-disclosure";
        disclosure.textContent = "›";
        disclosure.onclick = () => {
          if (source !== view.state.doc.toString()) return;
          focusHeading(heading);
          setFolded(heading, !isFolded(heading));
          markCurrent(view.state.selection.main.head);
        };
        heading.row.firstElementChild!.prepend(disclosure);
        setFolded(heading, isFolded(heading));
      } else heading.row.firstElementChild!.classList.add("outline-leaf");
    }
    if (!headings.length) {
      const empty = document.createElement("span");
      empty.className = "outline-empty";
      empty.textContent = "暂无标题，使用 H1–H6 创建章节";
      fragment.append(empty);
    }
    expand.disabled = collapse.disabled = !headings.some(heading => heading.children.length);
    tree.replaceChildren(fragment);
    tree.scrollTop = scrollTop;
    const restore = headings.find(heading => key(heading) === previousFocus);
    if (restore) focusHeading(restore);
    markCurrent(view.state.selection.main.head);
  }
  expand.onclick = () => {
    for (const heading of headings) setFolded(heading, false);
    markCurrent(view.state.selection.main.head);
  };
  collapse.onclick = () => {
    for (const heading of headings) setFolded(heading, true);
    if (headings.length) focusHeading(headings[0]);
    markCurrent(view.state.selection.main.head);
  };
  updateCurrent = () => {
    if (!panel.hidden) markCurrent(view.state.selection.main.head);
  };
  view.scrollDOM.addEventListener("scroll", () => {
    if (frame || panel.hidden) return;
    frame = requestAnimationFrame(() => {
      frame = 0;
      const height = Math.max(0, view.scrollDOM.getBoundingClientRect().top + 16 - view.documentTop);
      markCurrent(view.lineBlockAtHeight(height).from);
    });
  }, { passive: true });
  // The host restores durable native state after a fresh/incognito WebView.
  // Restoration is not a user edit: do not emit another save, touch the
  // document/selection, steal focus or write the browser store.
  applyPreference = value => {
    if (!value || typeof value !== "object" || Array.isArray(value)) return;
    const saved = value as Partial<OutlinePreference>;
    if (!["auto", "left", "right", "top", "floating"].includes(saved.position ?? "") || typeof saved.shown !== "boolean") return;
    preference = saved.position!;
    shown = saved.shown;
    position.value = preference;
    panel.hidden = !enabled || !shown;
    layout();
    render();
  };
  applyEnabled = value => {
    if (value === enabled) return;
    enabled = value;
    clearTimeout(timer);
    toggle.hidden = !value;
    panel.hidden = !value || !shown;
    layout();
    tree.replaceChildren();
    headings = [];
    folded.clear();
    render();
  };
  scheduleRefresh = (reset = false) => {
    clearTimeout(timer);
    if (reset) folded.clear();
    for (const button of tree.querySelectorAll<HTMLButtonElement>("button")) button.disabled = true;
    if (!panel.hidden) timer = setTimeout(render, 180);
  };
  function show(value: boolean): void {
    clearTimeout(timer);
    shown = value;
    panel.hidden = !enabled || !shown;
    persist();
    layout();
    render();
    if (!value) toggle.focus();
  }
  toggle.onclick = () => show(Boolean(panel.hidden));
  hide.onclick = () => show(false);
  panel.addEventListener("keydown", event => {
    if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); show(false); }
  });
  panel.hidden = !shown;
  layout();
  render();
}

/** Local text/emphasis/code only; never interpret headings as HTML. */
function outlineTitle(source:string):DocumentFragment {
  const result=document.createDocumentFragment();
  const styles:Record<string,string>={StrongEmphasis:"strong",Emphasis:"em",InlineCode:"code",Strikethrough:"s"};
  const visit=(node:import("@lezer/common").SyntaxNode,parent:Node)=>{
    if (["EmphasisMark","CodeMark","StrikethroughMark"].includes(node.name)) return;
    const tag=styles[node.name];
    const target=tag?document.createElement(tag):parent;
    if(tag)parent.appendChild(target);
    if(!node.firstChild){target.appendChild(document.createTextNode(source.slice(node.from,node.to)));return;}
    let at=node.from;
    for(let child:import("@lezer/common").SyntaxNode|null=node.firstChild;child;child=child.nextSibling){
      if(child.from>at)target.appendChild(document.createTextNode(source.slice(at,child.from)));
      visit(child,target);at=child.to;
    }
    if(at<node.to)target.appendChild(document.createTextNode(source.slice(at,node.to)));
  };
  visit(parser.parse(source).topNode,result);
  return result;
}
