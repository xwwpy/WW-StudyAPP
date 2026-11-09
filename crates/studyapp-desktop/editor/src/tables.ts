import { isolateHistory, invertedEffects } from "@codemirror/commands";
import { EditorSelection, StateEffect, Text, Transaction } from "@codemirror/state";
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
let creating:{dialog:HTMLDialogElement;view:EditorView}|undefined;
export function closeTableDialog(focus=false):void {
  const active=creating;creating=undefined;if(!active)return;
  active.dialog.onclose=null;active.dialog.close();active.dialog.remove();if(focus)active.view.focus();
}
type TableSnapshot={entry:Entry;record:TableView|null};
const tableStructure=StateEffect.define<{from:number;before:TableSnapshot|null;after:TableSnapshot|null}>({map:(value,changes)=>({...value,from:changes.mapPos(value.from,-1)})});
export const tableHistory=invertedEffects.of(transaction=>transaction.effects.filter(effect=>effect.is(tableStructure)).map(effect=>tableStructure.of({...effect.value,before:effect.value.after,after:effect.value.before})));

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
  closeTableDialog();
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
  width=document.createElement("input");
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
      const b=button(value,label,()=>this.change(r=>{const index=Number(this.select.value); if(index<0)r.alignments=Array(this.current()!.columns).fill(value);else r.alignments[index]=value;}));b.dataset.alignment=value;this.bar.append(b);
    }
    this.bar.append(button("reset","恢复全局默认",()=>{if(this.id){delete records[this.id];this.save(this.id,null);this.redraw();}}),button("close","收起表格设置",()=>this.hide()));
    this.width.type="number";this.width.min="0.01";this.width.max="100";this.width.step="1";
    this.width.setAttribute("aria-label","所选列宽度百分比");this.width.title="列宽（%），其他列按比例调整";
    this.width.addEventListener("change",()=>{
      const entry=this.current(),index=Number(this.select.value),percent=Number(this.width.value);
      const model=entry&&cache.get(this.view.state.doc)?.get(entry.from);
      if(!entry||!model||index<0||entry.columns<2||!Number.isFinite(percent)||percent<=0||percent>=100)return;
      this.change(record=>{
        const total=Math.max(1,10000-model.widths[index]);
        const width=Math.min(10000-(entry.columns-1),Math.max(1,Math.round(percent*100)));
        const remaining=10000-width;
        const values=model.widths.map((value,i)=>i===index?width:1+Math.floor(value/total*(remaining-(entry.columns-1))));
        const target=index===0?1:0;values[target]+=10000-values.reduce((a,b)=>a+b,0);record.widths=values;
      });
    });
    const more=document.createElement("details"),summary=document.createElement("summary"),panel=document.createElement("div");
    more.className="md-table-more";summary.textContent="⋯";summary.setAttribute("aria-label","更多列操作");summary.title="更多列操作";panel.className="md-table-more-panel";more.append(summary,panel);this.bar.append(more);
    const label=document.createElement("label");label.textContent="列宽（%）";label.append(this.width);panel.append(label);
    more.addEventListener("toggle",()=>{if(!more.open)return;requestAnimationFrame(()=>{const bounds=summary.getBoundingClientRect(),height=panel.getBoundingClientRect().height;panel.style.left=`${Math.max(8,Math.min(window.innerWidth-280,bounds.right-260))}px`;panel.style.top=`${bounds.bottom+height+8<window.innerHeight?bounds.bottom+4:Math.max(4,bounds.top-height-4)}px`;});});
    for(const [operation,label] of [["before","左侧插入列"],["after","右侧插入列"],["duplicate","复制此列"],["move-left","此列左移"],["move-right","此列右移"],["delete","删除此列"]] as const) {
      const control=button(operation,label,()=>{const entry=this.current();if(entry)editColumn(this.view,entry,Number(this.select.value),operation);});
      control.dataset.tableTextAction="true";panel.append(control);
    }
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
  hide():void {this.root.hidden=true;this.id=undefined;for(const details of this.root.querySelectorAll("details"))details.open=false;}
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
    const column=Number(this.select.value);
    for(const button of this.bar.querySelectorAll<HTMLButtonElement>("[data-alignment]")) {
      const chosen=(index:number)=>(r?.alignments[index]??defaults.alignment)===button.dataset.alignment;
      button.setAttribute("aria-pressed",String(column<0?Array.from({length:entry?.columns??0},(_,i)=>i).every(chosen):chosen(column)));
    }
    this.width.disabled=column<0 || !entry || entry.columns<2;
    const widths=entry&&cache.get(this.view.state.doc)?.get(entry.from)?.widths;
    if(document.activeElement!==this.width)this.width.value=column>=0&&widths?String(widths[column]/100):"";
    for(const button of this.bar.querySelectorAll<HTMLButtonElement>("[data-table-text-action]"))button.disabled=this.view.state.readOnly||column<0;
  }
  persist(id:string,value:TableView|null):void {if(enabled)this.save(id,value?structuredClone(value):null);}
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
      const barWidth=Math.min(332,window.innerWidth-16);
      const remeasure=wasHidden||this.bar.style.width!==`${barWidth}px`;
      this.bar.style.width=`${barWidth}px`;this.bar.style.left=`${Math.max(8,Math.min(window.innerWidth-barWidth-8,layout.bounds.right-barWidth))}px`;
      const top=layout.bounds.top-layout.barHeight-4>=layout.viewport.top?layout.bounds.top-layout.barHeight-4:layout.bounds.bottom+2;
      this.bar.style.top=`${Math.max(4,Math.min(window.innerHeight-layout.barHeight-4,top))}px`;
      if(changed||this.select.options.length!==e.columns+1){const selected=this.select.value;this.select.replaceChildren(...Array.from({length:e.columns},(_,i)=>new Option(`第 ${i+1} 列`,String(i))),new Option("全部列","-1"));if(!changed&&[...this.select.options].some(option=>option.value===selected))this.select.value=selected;this.updatePressed();}
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
    if(update.docChanged||update.state.readOnly)closeTableDialog();
    if(update.docChanged) {
      controller?.cancel();
      const next=new Map<number,Entry>();
      for(const e of entries.values()) {
        const from=update.changes.mapPos(e.from,1),to=update.changes.mapPos(e.to,-1);
        if(to>from) next.set(from,{...e,from,to});
      }
      entries=next;
    }
    for(const transaction of update.transactions)for(const effect of transaction.effects)if(effect.is(tableStructure)) {
      const {before,after,from}=effect.value;
      if(before){delete records[before.entry.id];for(const [position,entry]of entries)if(entry.id===before.entry.id)entries.delete(position);}
      if(after){entries.set(from,{...after.entry,from,to:from+after.entry.to-after.entry.from});if(after.record)records[after.entry.id]=structuredClone(after.record);}
      cache=new WeakMap();
      const id=after?.entry.id??before?.entry.id;if(id)controller?.persist(id,after?.record??null);
    }
    if(update.docChanged || update.viewportChanged || update.geometryChanged || update.selectionSet || update.transactions.some(t=>t.effects.some(e=>e.is(tablePresentationChanged)))) controller?.measure();
    if(update.docChanged) controller?.scheduleSync();
  }
});
const icons:Record<string,string>={
  equal:'M3 4h18v16H3z M9 4v16 M15 4v16',adaptive:'M4 4v16 M20 4v16 M8 8l-4 4 4 4 M16 8l4 4-4 4 M4 12h16',
  left:'M4 5h16 M4 10h10 M4 15h16 M4 20h10',center:'M4 5h16 M7 10h10 M4 15h16 M7 20h10',right:'M4 5h16 M10 10h10 M4 15h16 M10 20h10',
  before:'M12 4v16 M4 12h16',after:'M12 4v16 M4 12h16',duplicate:'M8 8h12v12H8z M4 16V4h12',delete:'M6 6h12v14H6z M4 4h16',
  'move-left':'m13 5-7 7 7 7 M6 12h14','move-right':'m11 5 7 7-7 7 M4 12h14',
  source:'M8 5L2 12l6 7 M16 5l6 7-6 7',reset:'M4 10a8 8 0 1 1 1 8 M4 4v6h6',close:'M6 6l12 12 M18 6L6 18'
};
function button(name:string,label:string,action:()=>void):HTMLButtonElement {
  const b=document.createElement("button");b.type="button";b.title=label;b.setAttribute("aria-label",label);
  b.innerHTML=`<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="${icons[name]}"/></svg>`;
  b.addEventListener("mousedown",e=>e.preventDefault());b.addEventListener("click",action);return b;
}

/** Bounded creation, one undoable Markdown transaction. Rows include the header. */
export function insertTable(view:EditorView,rows:number,columns:number):boolean {
  if(view.state.readOnly || !Number.isInteger(rows) || !Number.isInteger(columns) || rows<1 || rows>1000 || columns<1 || columns>64 || rows*columns>4096)return false;
  const range=view.state.selection.main,prefix=range.from?"\n\n":"";
  const row=(cells:string[])=>`| ${cells.join(" | ")} |`;
  const lines=[row(Array.from({length:columns},(_,i)=>`列 ${i+1}`)),row(Array(columns).fill("---")),...Array.from({length:rows-1},()=>row(Array(columns).fill("内容")))];
  const insert=prefix+lines.join("\n")+"\n\n",from=range.from+prefix.length+2;
  view.dispatch({changes:{from:range.from,to:range.to,insert},selection:EditorSelection.range(from,from+3),annotations:[Transaction.userEvent.of("input.table"),isolateHistory.of("full")],scrollIntoView:true});
  return true;
}

export function createTableDialog(view:EditorView):boolean {
  if(view.state.readOnly)return false;
  if(document.querySelector("#table-create-dialog"))return true;
  const dialog=document.createElement("dialog");dialog.id="table-create-dialog";dialog.setAttribute("aria-label","创建表格");
  const form=document.createElement("form");form.method="dialog";
  const title=document.createElement("h3");title.textContent="创建表格";form.append(title);
  const number=(label:string,value:number,max:number)=>{
    const wrapper=document.createElement("label");wrapper.textContent=label;
    const input=document.createElement("input");input.type="number";input.min="1";input.max=String(max);input.step="1";input.value=String(value);input.required=true;input.setAttribute("aria-label",label);wrapper.append(input);form.append(wrapper);return input;
  };
  const rows=number("行数（含表头）",2,1000),columns=number("列数",2,64);
  const error=document.createElement("p");error.setAttribute("role","alert");form.append(error);
  const cancel=document.createElement("button");cancel.type="button";cancel.textContent="取消";cancel.onclick=()=>dialog.close();
  const submit=document.createElement("button");submit.type="submit";submit.textContent="创建";
  form.append(cancel,submit);dialog.append(form);document.body.append(dialog);
  creating={dialog,view};
  form.onsubmit=event=>{event.preventDefault();if(view.state.readOnly){closeTableDialog();return;}if(!form.reportValidity())return;
    if(Number(rows.value)*Number(columns.value)>4096){error.textContent="最多 4,096 个单元格，请减少行数或列数。";return;}
    if(insertTable(view,Number(rows.value),Number(columns.value))){dialog.close();view.focus();}
  };
  dialog.onclose=()=>{if(creating?.dialog===dialog)creating=undefined;dialog.remove();view.focus();};dialog.showModal();rows.focus();rows.select();return true;
}

function splitCells(source:string):string[] {
  let cells:string[]=[],at=0,slashes=0;
  for(let i=0;i<source.length;i++) {
    if(source[i]==="|" && slashes%2===0){cells.push(source.slice(at,i).trim());at=i+1;}
    slashes=source[i]==="\\"?slashes+1:0;
  }
  cells.push(source.slice(at).trim());
  if(/^\s*\|/.test(source))cells.shift();
  if(/\|\s*$/.test(source) && cells.at(-1)==="")cells.pop();
  return cells;
}
type ColumnOperation="before"|"after"|"duplicate"|"delete"|"move-left"|"move-right";
function editColumn(view:EditorView,entry:Entry,index:number,operation:ColumnOperation):boolean {
  if(view.state.readOnly || !Number.isInteger(index) || index<0 || index>=entry.columns)return false;
  const order=Array.from({length:entry.columns},(_,i)=>i);
  if(["before","after","duplicate"].includes(operation)) {
    if(order.length>=64)return false;
    order.splice(index+(operation==="before"?0:1),0,operation==="duplicate"?index:-1);
  } else if(operation==="delete")order.splice(index,1);
  else {const other=index+(operation==="move-left"?-1:1);if(other<0||other>=order.length)return false;[order[index],order[other]]=[order[other],order[index]];}
  const prefix=view.state.doc.lineAt(entry.from).text.slice(0,entry.from-view.state.doc.lineAt(entry.from).from);
  const source=view.state.sliceDoc(entry.from,entry.to);
  const rows=source.split("\n").map((line,i)=>splitCells(i&&line.startsWith(prefix)?line.slice(prefix.length):line));
  const insert=order.length?rows.map((cells,row)=>`${row?prefix:""}| ${order.map(column=>column<0?row===1?"---":"":cells[column]||(row===1?"---":"")).join(" | ")} |`).join("\n"):"";
  const old=records[entry.id]?structuredClone(records[entry.id]):null;
  const before={entry:{...entry},record:old};
  let record=old?structuredClone(old):null;
  if(record) {
    record.alignments=order.map(column=>column<0?null:old!.alignments[column]??null);
    if(record.widths) {
      const raw=order.map(column=>column<0?10000/Math.max(1,order.length):record!.widths![column]);
      const total=raw.reduce((a,b)=>a+b,0),rest=10000-order.length;
      record.widths=raw.map(value=>1+Math.floor(value/total*rest));
      if(order.length)record.widths[order.length-1]+=10000-record.widths.reduce((a,b)=>a+b,0);
    }
  }
  const after=order.length?{entry:{...entry,to:entry.from+insert.length,columns:order.length},record}:null;
  view.dispatch({changes:{from:entry.from,to:entry.to,insert},effects:tableStructure.of({from:entry.from,before,after}),selection:EditorSelection.cursor(entry.from),annotations:[Transaction.userEvent.of("input.table"),isolateHistory.of("full")],scrollIntoView:true});
  return true;
}
