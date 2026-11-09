import { EditorSelection, StateEffect, Transaction } from "@codemirror/state";
import { syntaxTree } from "@codemirror/language";
import type { EditorView } from "@codemirror/view";
import { isolateHistory } from "@codemirror/commands";
import type { SyntaxNode } from "@lezer/common";

// Closed vocabulary shared with core::ListMarker; never accept arbitrary markup.
const symbols:Record<string,string> = {disc:"•",circle:"◦",square:"▪",hollowSquare:"▫",triangle:"▸",hollowTriangle:"▹",diamond:"◆",hollowDiamond:"◇",arrow:"➤",dash:"–",plus:"+",star:"✦"};
const defaults = ["disc","square","triangle","diamond","circle","hollowSquare","hollowTriangle","hollowDiamond"];
let markers = defaults;
export const listPresentationChanged = StateEffect.define<void>();
export function setListMarkers(value:unknown):void {
  markers = Array.isArray(value) && value.length === 8 && value.every(v=>typeof v==="string" && Object.hasOwn(symbols,v)) ? [...value] : defaults;
}
export function listDepth(node:SyntaxNode):number {
  let depth=0;
  for(let ancestor=node.parent;ancestor;ancestor=ancestor.parent) {
    if(ancestor.name==="BulletList" || ancestor.name==="OrderedList")depth++;
  }
  return depth;
}
export function listSymbol(depth:number):string { return symbols[markers[Math.max(0,depth-1)%8]]; }

/** Leave an empty item immediately; nested items move exactly one parent up. */
export function leaveList(view: EditorView, backward = false): boolean {
  const { state } = view;
  if (state.readOnly) return false;
  const changes: {from:number;to:number;insert:string}[] = [];
  const tree = syntaxTree(state);
  for (const range of state.selection.ranges) {
    if (!range.empty) return false;
    const line = state.doc.lineAt(range.head);
    const prefix = /^([ \t]*)([-+*]|\d+[.)])(?:[ \t]+(?:\[[ xX]\][ \t]*)?)?/.exec(line.text);
    if (!prefix || (prefix[0].length < line.text.length && !/[ \t]$/.test(prefix[0]))) return false;
    const empty = !line.text.slice(prefix[0].length).trim();
    if (empty ? range.head !== line.to : !backward || range.head !== line.from + prefix[0].length) return false;
    let item: SyntaxNode | null = null;
    for (let node: SyntaxNode | null = tree.resolveInner(line.from + prefix[1].length, 1); node; node = node.parent) {
      if (node.name === "FencedCode" || node.name === "CodeBlock") return false;
      if (!item && node.name === "ListItem") item = node;
    }
    if (!item) return false;
    let parent = item.parent?.parent;
    while (parent && parent.name !== "ListItem") parent = parent.parent;
    const indent = parent ? /^[ \t]*/.exec(state.doc.lineAt(parent.from).text)![0] : "";
    const insert = parent ? indent + prefix[2] + " " : "";
    changes.push({from:line.from,to:line.from+prefix[0].length,insert});
  }
  const unique = [...new Map(changes.map(change=>[change.from,change])).values()].sort((a,b)=>a.from-b.from);
  const changeSet = state.changes(unique);
  view.dispatch({changes:changeSet, selection:EditorSelection.create(state.selection.ranges.map(range=>EditorSelection.cursor(changeSet.mapPos(range.head,1))),state.selection.mainIndex),
    annotations:[Transaction.userEvent.of(backward?"delete.backward":"input.list"),isolateHistory.of("full")],scrollIntoView:true});
  return true;
}
