import { StateField, type Range } from "@codemirror/state";
import { TreeFragment, type Tree } from "@lezer/common";
import { markdownLanguage } from "@codemirror/lang-markdown";
import { Decoration, EditorView } from "@codemirror/view";

/** Height-affecting line styles must exist before CM measures a viewport.
 * A viewport plugin adds them too late when scrolling into unmeasured code,
 * causing repeated height corrections and occasionally an empty viewport.
 * Use a lightweight, incrementally reused Markdown tree without code parsers.
 */
function layout(state: import("@codemirror/state").EditorState, tree: Tree) {
  const lines = new Map<number, Set<string>>();
  const line = (position: number, classes: string) => {
    const from = state.doc.lineAt(position).from;
    let values = lines.get(from); if (!values) lines.set(from, values = new Set());
    for (const name of classes.split(" ")) values.add(name);
  };
  tree.iterate({enter: cursor => {
    const {name, from, to} = cursor;
    if (/^(?:ATX|Setext)Heading[1-6]$/.test(name)) line(from, `md-heading md-heading-${name.at(-1)}`);
    if (name === "Blockquote") {
      for(let n=state.doc.lineAt(from).number, last=state.doc.lineAt(to).number; n<=last; n++) line(state.doc.line(n).from,"md-quote");
    }
    if (name !== "FencedCode" && name !== "CodeBlock") return;
    for(let n=state.doc.lineAt(from).number,last=state.doc.lineAt(to).number;n<=last;n++) line(state.doc.line(n).from,"md-code-line");
    line(from,"md-code-start"); line(to,"md-code-end");
    const active = !state.readOnly && state.selection.ranges.some(range => range.from <= to && range.to >= from);
    if(name === "FencedCode" && !active) for(const fence of cursor.node.getChildren("CodeMark")) line(fence.from,"md-code-fence");
    return false;
  }});
  const ranges: Range<Decoration>[] = [];
  for(const [from, classes] of lines) ranges.push(Decoration.line({class:[...classes].join(" ")}).range(from));
  return Decoration.set(ranges,true);
}
export const markdownBlockLayout = StateField.define<{
  tree: Tree; fragments: readonly TreeFragment[]; decorations: ReturnType<typeof Decoration.set>;
}>({
  create(state) {
    const tree = markdownLanguage.parser.parse(state.doc.toString());
    return {tree,fragments:TreeFragment.addTree(tree),decorations:layout(state,tree)};
  },
  update(old,tr) {
    if (!tr.docChanged && !tr.selection && tr.state.readOnly === tr.startState.readOnly) return old;
    let {tree,fragments} = old;
    if(tr.docChanged) {
      const changes: {fromA:number;toA:number;fromB:number;toB:number}[] = [];
      tr.changes.iterChangedRanges((fromA,toA,fromB,toB)=>changes.push({fromA,toA,fromB,toB}));
      fragments = TreeFragment.applyChanges(fragments,changes);
      tree = markdownLanguage.parser.parse(tr.state.doc.toString(),fragments);
      fragments = TreeFragment.addTree(tree);
    }
    return {tree,fragments,decorations:layout(tr.state,tree)};
  },
  provide: field => EditorView.decorations.from(field,value=>value.decorations),
});
