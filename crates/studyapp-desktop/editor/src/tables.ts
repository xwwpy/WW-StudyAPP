import { StateEffect, Text } from "@codemirror/state";
import { EditorView, ViewPlugin, ViewUpdate } from "@codemirror/view";
import type { SyntaxNode } from "@lezer/common";

export type Sizing = "equal" | "adaptive";
export type Alignment = "source" | "left" | "center" | "right";
type Anchor = { header:string; content:string; context:string; occurrence:number };
export type TableView = { anchor:Anchor; sizing:Sizing|null; widths:number[]|null; alignments:(Alignment|null)[] };
export type TableViews = Record<string,TableView>;
type Entry = { id:string; from:number; to:number; anchor:Anchor; columns:number };
export type TableModel = Entry & { firstLine:number; widths:number[]; alignment:string[] };
export const tablePresentationChanged = StateEffect.define<void>();
let defaults:{sizing:Sizing;alignment:Alignment} = { sizing:"equal", alignment:"source" };
let records:TableViews = Object.create(null);
let entries = new Map<number,Entry>();
let enabled = false;
let cache = new WeakMap<Text,Map<number,TableModel>>();
let controller:Controls|undefined;
const alignments:Alignment[] = ["source","left","center","right"];
const key = (v:unknown):v is string => typeof v === "string" && /^[0-9a-f]{32}$/.test(v);
function valid(v:unknown):v is TableView {
  if (!v || typeof v !== "object") return false;
  const r=v as TableView, a=r.anchor;
  return !!a && key(a.header) && key(a.content) && key(a.context) && Number.isSafeInteger(a.occurrence) && a.occurrence>=0 && a.occurrence<=1000000
    && (r.sizing===null || r.sizing==="equal" || r.sizing==="adaptive")
    && (r.widths===null || Array.isArray(r.widths) && r.widths.length>0 && r.widths.length<=64 && r.widths.every(v=>Number.isInteger(v)&&v>0) && r.widths.reduce((a,b)=>a+b,0)===10000)
    && Array.isArray(r.alignments) && r.alignments.length<=64 && r.alignments.every(v=>v===null || alignments.includes(v));
}
export function setTableDocument(value:unknown, canPersist:boolean):void {
  controller?.cancel(); controller?.hide();
  if(controller) clearTimeout(controller.timer);
  records=Object.create(null); entries.clear(); cache=new WeakMap(); enabled=canPersist;
  if (value && typeof value==="object") for (const [id,record] of Object.entries(value).slice(0,128)) {
    if (key(id) && valid(record)) records[id]=structuredClone(record);
  }
}
export function setTableDefaults(value:unknown):void {
  controller?.cancel();
  const v=value as Partial<typeof defaults>|undefined;
  defaults={sizing:v?.sizing==="adaptive"?"adaptive":"equal",alignment:alignments.includes(v?.alignment as Alignment)?v!.alignment!:"source"};
  cache=new WeakMap();
}
// Presentation-only fingerprints, not authentication primitives.
function fingerprint(text:string):string {
  const input=text.length>65536 ? `${text.length}:${text.slice(0,32768)}:${text.slice(-32768)}` : text;
  const h=[2166136261,2246822519,3266489917,668265263];
  for(let i=0;i<input.length;i++) for(let j=0;j<4;j++) h[j]=Math.imul(h[j]^input.charCodeAt(i),16777619+j*2);
  return h.map(v=>(v>>>0).toString(16).padStart(8,"0")).join("");
}
function rangeFingerprint(state:EditorView["state"],from:number,to:number):string {
  const length=to-from;
  return fingerprint(length<=65536?state.sliceDoc(from,to):`${length}:${state.sliceDoc(from,from+32740)}:${state.sliceDoc(to-32740,to)}`);
}
function header(node:SyntaxNode,state:EditorView["state"]):string {
  const h=node.getChild("TableHeader"); return h?state.sliceDoc(h.from,h.to).trim():"";
}
function anchor(node:SyntaxNode,state:EditorView["state"]):Anchor {
  const title=header(node,state); let context="",occurrence=0;
  for(let previous=node.prevSibling;previous;previous=previous.prevSibling) {
    if (/Heading$/.test(previous.name) || /^ATXHeading/.test(previous.name)) { context=state.sliceDoc(previous.from,Math.min(previous.to,previous.from+4096)); break; }
    if(previous.name==="Table" && header(previous,state)===title) occurrence++;
  }
  for(let parent=node.parent;parent && parent.name!=="Document";parent=parent.parent) {
    context+=`\n${parent.name}:${state.sliceDoc(parent.from,Math.min(parent.to,parent.from+128)).split("\n")[0]}`;
  }
  return {header:fingerprint(title),content:rangeFingerprint(state,node.from,node.to),context:fingerprint(context),occurrence};
}
function freshId():string { return Array.from(crypto.getRandomValues(new Uint8Array(16)),v=>v.toString(16).padStart(2,"0")).join(""); }
function equal(columns:number):number[] { const n=Math.floor(10000/columns); return Array.from({length:columns},(_,i)=>n+(i<10000-n*columns?1:0)); }
const textMetrics=document.createElement("canvas").getContext("2d");
function adaptive(node:SyntaxNode,state:EditorView["state"],columns:number):number[] {
  if(columns>64)return equal(columns);
  const sizes=Array(columns).fill(40) as number[];
  const style=document.documentElement.style;
  const font=`${style.getPropertyValue("--editor-font-size")||"15px"} ${style.getPropertyValue("--editor-body-font")||"sans-serif"}`;

  // Bound sampling for large tables; long text wraps inside the reading width.
  const rows:SyntaxNode[]=[];
  for(let row=node.firstChild;row&&rows.length<129;row=row.nextSibling) if(row.name==="TableHeader"||row.name==="TableRow")rows.push(row);
  for(const row of rows) {
    row?.getChildren("TableCell").slice(0,columns).forEach((cell,i)=>{
      const text=state.sliceDoc(cell.from,Math.min(cell.to,cell.from+512));
      if(textMetrics) textMetrics.font=`${row.name==="TableHeader"?"650 ":""}${font}`;
      const size=textMetrics?.measureText(text).width??Array.from(text).reduce((n,ch)=>n+(ch.codePointAt(0)!>255?16:8),0);
      sizes[i]=Math.max(sizes[i],Math.min(600,size+20));
    });
  }
  const total=sizes.reduce((a,b)=>a+b,0), result=sizes.map(n=>Math.max(1,Math.floor(n/total*10000)));
  result[result.length-1]+=10000-result.reduce((a,b)=>a+b,0); return result;
}
export function tableModel(node:SyntaxNode,state:EditorView["state"]):TableModel {
  let models=cache.get(state.doc); if(!models) {models=new Map();cache.set(state.doc,models);}
  const old=models.get(node.from); if(old&&old.to===node.to)return old;
  const separator=node.getChild("TableDelimiter");
  const source=separator?state.sliceDoc(separator.from,separator.to).trim().replace(/^\||\|$/g,"").split("|").map(v=>/^\s*:.*:\s*$/.test(v)?"center":/:\s*$/.test(v)?"right":"left"):[];
  const columns=source.length||node.getChild("TableHeader")?.getChildren("TableCell").length||1;
  let entry=entries.get(node.from); const a=anchor(node,state);
  if(!entry) {
    const used=new Set(Array.from(entries.values(),e=>e.id));
    const available=Object.entries(records).filter(([id])=>!used.has(id));
    const exact=available.filter(([,r])=>r.anchor.content===a.content);
    const found=exact.find(([,r])=>r.anchor.context===a.context&&r.anchor.occurrence===a.occurrence)
      ?? (exact.length===1?exact[0]:undefined)
      ?? available.find(([,r])=>r.anchor.header===a.header&&r.anchor.context===a.context&&r.anchor.occurrence===a.occurrence);
    entry={id:found?.[0]??freshId(),from:node.from,to:node.to,anchor:a,columns}; entries.set(node.from,entry);
    if(entries.size>256) for(const [from,e] of entries) {if(!records[e.id]&&from!==node.from)entries.delete(from);if(entries.size<=256)break;}
  } else {entry.to=node.to;entry.anchor=a;entry.columns=columns;}
  const r=records[entry.id];
  const widths=r?.widths?.length===columns?r.widths:(r?.sizing??defaults.sizing)==="adaptive"?adaptive(node,state,columns):equal(columns);
  const alignment=Array.from({length:columns},(_,i)=>{
    const alignment=r?.alignments[i]??defaults.alignment; return alignment==="source"?source[i]||"left":alignment;
  });
  const model={...entry,firstLine:state.doc.lineAt(node.from).number,widths,alignment}; models.set(node.from,model); return model;
}
class Controls {
  root=document.createElement("div");
  bar=document.createElement("div");
  select=document.createElement("select");
  handles:HTMLButtonElement[]=[];
  id:string|undefined;
  drag:{id:string;column:number;start:number;width:number;widths:number[];old:TableView|undefined;pointer:number;handle:HTMLElement}|undefined;
  timer:ReturnType<typeof setTimeout>|undefined;
  frame=0;
  constructor(readonly view:EditorView,readonly save:(id:string,value:TableView|null)=>void) {
    this.root.className="md-table-controls";this.root.hidden=true;
    this.bar.className="md-table-actions";this.bar.setAttribute("role","toolbar");this.bar.setAttribute("aria-label","表格显示设置");
    this.select.setAttribute("aria-label","选择表格列");this.select.addEventListener("change",()=>this.updatePressed());
    this.bar.append(this.select,
      button("adaptive","按内容自适应列宽",()=>this.change(r=>{r.widths=null;r.sizing="adaptive";})),
      button("equal","等宽列",()=>this.change(r=>{r.widths=null;r.sizing="equal";})));
    for(const [value,label] of [["source","沿用源文对齐"],["left","左对齐"],["center","居中对齐"],["right","右对齐"]] as const) {
      const b=button(value,label,()=>this.change(r=>{r.alignments[Number(this.select.value)]=value;}));b.dataset.alignment=value;this.bar.append(b);
    }
    this.bar.append(button("reset","恢复全局默认",()=>{if(this.id){delete records[this.id];this.save(this.id,null);this.redraw();}}),button("close","收起表格设置",()=>this.hide()));
    this.root.append(this.bar);document.body.append(this.root);
    view.dom.addEventListener("pointerover",event=>{
      if(this.drag)return;
      const row=(event.target as Element).closest<HTMLElement>(".md-table-row[data-table-id]");
      if(row?.dataset.tableId){this.id=row.dataset.tableId;this.measure();}
    });
    this.root.addEventListener("keydown",event=>{
      if(event.key==="Escape"){event.preventDefault();this.cancel();this.redraw();this.hide();view.focus();}
    });
    document.addEventListener("keydown",event=>{
      if(this.drag&&event.key==="Escape"){event.preventDefault();event.stopPropagation();this.cancel();this.redraw();}
    },true);
    window.addEventListener("blur",()=>{this.cancel();this.redraw();this.hide();});
    view.scrollDOM.addEventListener("scroll",()=>this.measure(),{passive:true});
    window.addEventListener("resize",()=>this.measure());
  }
  hide():void {this.root.hidden=true;this.id=undefined;}
  redraw():void {
    cache=new WeakMap();
    if(!this.frame)this.frame=requestAnimationFrame(()=>{this.frame=0;this.view.dispatch({effects:tablePresentationChanged.of()});this.measure();});
  }
  current():Entry|undefined {return Array.from(entries.values()).find(e=>e.id===this.id);}
  value(entry:Entry):TableView {return structuredClone(records[entry.id]??{anchor:entry.anchor,sizing:null,widths:null,alignments:[]});}
  change(change:(record:TableView)=>void):void {
    const e=this.current();if(!e||!enabled)return;
    const r=this.value(e);r.anchor=e.anchor;change(r);records[e.id]=r;this.save(e.id,r);this.redraw();this.updatePressed();
  }
  updatePressed():void {
    const entry=this.current(),r=entry&&records[entry.id];
    for(const button of this.bar.querySelectorAll<HTMLButtonElement>("[data-alignment]")) button.setAttribute("aria-pressed",String((r?.alignments[Number(this.select.value)]??defaults.alignment)===button.dataset.alignment));
  }
  scheduleSync():void {clearTimeout(this.timer);this.timer=setTimeout(()=>this.sync(),500);}
  sync():void {
    clearTimeout(this.timer);if(!enabled)return;
    for(const e of entries.values()) {
      const r=records[e.id];if(!r)continue;
      const anchor={...e.anchor,header:fingerprint(this.view.state.doc.lineAt(e.from).text.trim()),content:rangeFingerprint(this.view.state,e.from,e.to)};
      if(JSON.stringify(anchor)!==JSON.stringify(r.anchor)) {r.anchor=anchor;this.save(e.id,structuredClone(r));}
    }
  }
  cancel():void {
    const d=this.drag;if(!d)return;this.drag=undefined;
    if(d.old)records[d.id]=d.old;else delete records[d.id];cache=new WeakMap();
    if(d.handle.hasPointerCapture(d.pointer))d.handle.releasePointerCapture(d.pointer);
  }
  begin(event:PointerEvent,index:number,handle:HTMLButtonElement):void {
    if(event.button!==0||!enabled)return;
    const e=this.current();if(!e)return;
    const model=cache.get(this.view.state.doc)?.get(e.from);if(!model)return;
    const row=this.view.dom.querySelector<HTMLElement>(`.md-table-header[data-table-id="${e.id}"]`);if(!row)return;
    event.preventDefault();event.stopPropagation();
    this.drag={id:e.id,column:index,start:event.clientX,width:row.getBoundingClientRect().width,widths:[...model.widths],old:records[e.id]?structuredClone(records[e.id]):undefined,pointer:event.pointerId,handle};
    handle.setPointerCapture(event.pointerId);
  }
  move(event:PointerEvent):void {
    const d=this.drag;if(!d||event.pointerId!==d.pointer)return;
    const e=this.current();if(!e||e.id!==d.id){this.cancel();return;}
    const widths=[...d.widths],pair=widths[d.column]+widths[d.column+1];
    const minimum=Math.max(1,Math.min(Math.ceil(24/Math.max(1,d.width)*10000),Math.floor(pair/3)));
    widths[d.column]=Math.max(minimum,Math.min(pair-minimum,Math.round(d.widths[d.column]+(event.clientX-d.start)/Math.max(1,d.width)*10000)));
    widths[d.column+1]=pair-widths[d.column];
    const r=this.value(e);r.widths=widths;r.anchor=e.anchor;records[e.id]=r;this.redraw();
  }
  finish(event:PointerEvent):void {
    const d=this.drag;if(!d||event.pointerId!==d.pointer)return;this.drag=undefined;
    if(d.handle.hasPointerCapture(d.pointer))d.handle.releasePointerCapture(d.pointer);
    const r=records[d.id];if(r&&JSON.stringify(r)!==JSON.stringify(d.old))this.save(d.id,structuredClone(r));
  }
  resizeKey(event:KeyboardEvent,index:number):void {
    if(!["ArrowLeft","ArrowRight"].includes(event.key))return;
    event.preventDefault();event.stopPropagation();
    const e=this.current(),model=e&&cache.get(this.view.state.doc)?.get(e.from);if(!model)return;
    this.change(r=>{
      const widths=[...model.widths],pair=widths[index]+widths[index+1];
      widths[index]=Math.max(1,Math.min(pair-1,widths[index]+(event.key==="ArrowRight"?1:-1)*(event.shiftKey?500:100)));
      widths[index+1]=pair-widths[index];r.widths=widths;
    });
  }
  measure():void {
    this.view.requestMeasure({key:this,read:()=>{
      if(!enabled||document.documentElement.dataset.source === "true")return null;
      const rows=Array.from(this.view.dom.querySelectorAll<HTMLElement>(".md-table-header[data-table-id]"));
      const viewport=this.view.scrollDOM.getBoundingClientRect();
      const visible=rows.filter(r=>{const b=r.getBoundingClientRect();return b.bottom>viewport.top&&b.top<viewport.bottom;});
      const row=visible.find(r=>r.dataset.tableId===this.id)??(!this.id?visible[0]:undefined);if(!row)return null;
      const bounds=row.getBoundingClientRect(),cells=Array.from(row.querySelectorAll<HTMLElement>(".md-table-cell"));
      return {id:row.dataset.tableId!,bounds,viewport,barHeight:this.bar.getBoundingClientRect().height||36,edges:cells.map(c=>c.getBoundingClientRect().right)};
    },write:layout=>{
      if(!layout){this.root.hidden=true;return;}
      const e=Array.from(entries.values()).find(e=>e.id===layout.id);if(!e||e.columns>64||e.columns<1){this.root.hidden=true;return;}
      const changed=this.id!==e.id,wasHidden=this.root.hidden;this.id=e.id;this.root.hidden=false;
      const barWidth=Math.min(300,window.innerWidth-16);
      const remeasure=wasHidden||this.bar.style.width!==`${barWidth}px`;
      this.bar.style.width=`${barWidth}px`;this.bar.style.left=`${Math.max(8,Math.min(window.innerWidth-barWidth-8,layout.bounds.right-barWidth))}px`;
      const top=layout.bounds.top-layout.barHeight-4>=layout.viewport.top?layout.bounds.top-layout.barHeight-4:layout.bounds.bottom+2;
      this.bar.style.top=`${Math.max(4,Math.min(window.innerHeight-layout.barHeight-4,top))}px`;
      if(changed||this.select.options.length!==e.columns){this.select.replaceChildren(...Array.from({length:e.columns},(_,i)=>new Option(`第 ${i+1} 列`,String(i))));this.updatePressed();}
      if(remeasure)requestAnimationFrame(()=>this.measure());
      this.updatePressed();
      while(this.handles.length>e.columns-1)this.handles.pop()!.remove();
      while(this.handles.length<e.columns-1){
        const index=this.handles.length,h=document.createElement("button");h.type="button";h.className="md-table-resize";
        h.setAttribute("role","separator");h.setAttribute("aria-orientation","vertical");h.setAttribute("aria-label",`调整第 ${index+1} 列宽度`);
        h.title="拖动调整列宽；方向键微调，Shift 加速，Esc 取消";
        h.addEventListener("pointerdown",event=>this.begin(event,index,h));h.addEventListener("pointermove",event=>this.move(event));h.addEventListener("pointerup",event=>this.finish(event));
        h.addEventListener("pointercancel",()=>{this.cancel();this.redraw();});h.addEventListener("lostpointercapture",()=>{this.cancel();this.redraw();});
        h.addEventListener("keydown",event=>this.resizeKey(event,index));this.handles.push(h);this.root.append(h);
      }
      const widths=cache.get(this.view.state.doc)?.get(e.from)?.widths;
      this.handles.forEach((h,i)=>{h.style.left=`${layout.edges[i]-5}px`;h.style.top=`${layout.bounds.top}px`;h.style.height=`${layout.bounds.height}px`;h.setAttribute("aria-valuemin","0");h.setAttribute("aria-valuemax","100");h.setAttribute("aria-valuenow",String(Math.round((widths?.[i]??0)/100)));});
    }});
  }
}
export function installTableControls(view:EditorView,save:(id:string,value:TableView|null)=>void):void {controller=new Controls(view,save);}
export function hideTableControls():void {controller?.cancel();controller?.hide();}
export function syncTableAnchors():void { controller?.sync(); }
export const tableUpdates=ViewPlugin.fromClass(class {
  update(update:ViewUpdate) {
    if(update.docChanged) {
      controller?.cancel();
      const next=new Map<number,Entry>();
      for(const e of entries.values()) {
        const from=update.changes.mapPos(e.from,1),to=update.changes.mapPos(e.to,-1);
        if(to>from) next.set(from,{...e,from,to});
      }
      entries=next;
    }
    if(update.docChanged || update.viewportChanged || update.geometryChanged || update.selectionSet || update.transactions.some(t=>t.effects.some(e=>e.is(tablePresentationChanged)))) controller?.measure();
    if(update.docChanged) controller?.scheduleSync();
  }
});
const icons:Record<string,string>={
  equal:'M3 4h18v16H3z M9 4v16 M15 4v16',adaptive:'M4 4v16 M20 4v16 M8 8l-4 4 4 4 M16 8l4 4-4 4 M4 12h16',
  left:'M4 5h16 M4 10h10 M4 15h16 M4 20h10',center:'M4 5h16 M7 10h10 M4 15h16 M7 20h10',right:'M4 5h16 M10 10h10 M4 15h16 M10 20h10',
  source:'M8 5L2 12l6 7 M16 5l6 7-6 7',reset:'M4 10a8 8 0 1 1 1 8 M4 4v6h6',close:'M6 6l12 12 M18 6L6 18'
};
function button(name:string,label:string,action:()=>void):HTMLButtonElement {
  const b=document.createElement("button");b.type="button";b.title=label;b.setAttribute("aria-label",label);
  b.innerHTML=`<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="${icons[name]}"/></svg>`;
  b.addEventListener("mousedown",e=>e.preventDefault());b.addEventListener("click",action);return b;
}
