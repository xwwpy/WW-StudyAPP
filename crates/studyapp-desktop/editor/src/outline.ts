import { parser } from "@lezer/markdown";
import { EditorView } from "@codemirror/view";

let scheduleRefresh: () => void = () => {};
export const outlineUpdates = EditorView.updateListener.of(update => {
  if (update.docChanged) scheduleRefresh();
});

export function refreshOutline(): void {
  scheduleRefresh();
}

export function installOutline(view: EditorView): void {
  const toggle = document.createElement("button");
  toggle.type = "button";
  toggle.id = "outline-toggle";
  toggle.textContent = "目录";
  toggle.setAttribute("aria-label", "笔记目录");
  toggle.setAttribute("aria-expanded", "false");
  toggle.setAttribute("aria-controls", "note-outline");
  document.querySelector("#toolbar")!.append(toggle);

  const panel = document.createElement("nav");
  panel.id = "note-outline";
  panel.hidden = true;
  panel.setAttribute("aria-label", "笔记目录");
  document.querySelector("#editor")!.before(panel);
  let timer: ReturnType<typeof setTimeout>;

  function render(): void {
    if (panel.hidden) return;
    const text = view.state.doc.toString();
    const fragment = document.createDocumentFragment();
    let count = 0;
    // Parse the entire current document so headings beyond CM's viewport parse
    // budget remain navigable. Debouncing coalesces continuous typing.
    parser.parse(text).iterate({ enter(node) {
      const match = /^(?:ATX|Setext)Heading([1-6])$/.exec(node.name);
      if (!match) return;
      count++;
      const position = node.from;
      const button = document.createElement("button");
      button.type = "button";
      button.textContent = text.slice(node.from, node.to)
        .replace(/^ {0,3}#{1,6}\s+/, "")
        .replace(/\s+#+\s*$/, "")
        .replace(/\n {0,3}(?:=+|-+)\s*$/, "").trim();
      button.style.paddingLeft = `${8 + (Number(match[1]) - 1) * 12}px`;
      button.onclick = () => {
        if (button.disabled || view.state.doc.toString() !== text) return;
        view.dispatch({ selection: { anchor: position },
          effects: EditorView.scrollIntoView(position, { y: "start" }) });
        view.focus();
      };
      fragment.append(button);
    }});
    if (!count) {
      const empty = document.createElement("span");
      empty.textContent = "暂无标题";
      fragment.append(empty);
    }
    panel.replaceChildren(fragment);
  }

  scheduleRefresh = () => {
    clearTimeout(timer);
    for (const button of panel.querySelectorAll<HTMLButtonElement>("button")) button.disabled = true;
    if (!panel.hidden) timer = setTimeout(render, 300);
  };
  toggle.onclick = () => {
    clearTimeout(timer);
    panel.hidden = !panel.hidden;
    toggle.setAttribute("aria-expanded", String(!panel.hidden));
    render();
  };
}
