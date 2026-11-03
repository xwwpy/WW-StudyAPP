import { parser } from "@lezer/markdown";
import { EditorView } from "@codemirror/view";

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

export function installOutline(view: EditorView): void {
  const toggle = document.createElement("button");
  toggle.type = "button";
  toggle.id = "outline-toggle";
  toggle.textContent = "目录⌄";
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
  expand.textContent = "全部展开";
  const collapse = document.createElement("button");
  collapse.type = "button";
  collapse.textContent = "全部收起";
  header.append(caption, expand, collapse);
  const tree = document.createElement("div");
  tree.className = "outline-tree";
  tree.setAttribute("role", "tree");
  tree.setAttribute("aria-label", "章节");
  panel.append(header, tree);
  document.querySelector("#editor")!.before(panel);

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
      label.textContent = title;
      label.title = title;
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
  applyEnabled = value => {
    if (value === enabled) return;
    enabled = value;
    clearTimeout(timer);
    toggle.hidden = !value;
    panel.hidden = true;
    toggle.setAttribute("aria-expanded", "false");
    tree.replaceChildren();
    headings = [];
    folded.clear();
  };
  scheduleRefresh = (reset = false) => {
    clearTimeout(timer);
    if (reset) folded.clear();
    for (const button of tree.querySelectorAll<HTMLButtonElement>("button")) button.disabled = true;
    if (!panel.hidden) timer = setTimeout(render, 180);
  };
  toggle.onclick = () => {
    clearTimeout(timer);
    panel.hidden = !panel.hidden;
    toggle.setAttribute("aria-expanded", String(!panel.hidden));
    render();
  };
}
