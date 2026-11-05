import { StateEffect } from "@codemirror/state";
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
