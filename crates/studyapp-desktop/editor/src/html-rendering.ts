import { Facet, StateEffect, StateField, type EditorState, type Range } from "@codemirror/state";
import { syntaxTree } from "@codemirror/language";
import { Decoration, EditorView, ViewPlugin, WidgetType } from "@codemirror/view";
import { htmlPolicy, inertHtml, sanitizeHtml, sanitizeStyle } from "./safe-html";
export const htmlConfiguration=Facet.define<boolean,boolean>({combine:values=>values.at(-1)??true});
const parsed=StateEffect.define<void>();
type Region={from:number;to:number;tag?:string;style?:string;inside?:[number,number];source?:string;block?:boolean};
class HtmlWidget extends WidgetType {
  constructor(readonly source:string,readonly block:boolean){super();}
  eq(other:HtmlWidget){return other.source===this.source && other.block===this.block;}
  toDOM(){const dom=document.createElement(this.block?"div":"span");dom.className="md-html-fragment";const clean=sanitizeHtml(this.source);if(clean)dom.append(clean);else dom.textContent=this.source;return dom;}
  // Let the browser own selection inside rendered HTML. CodeMirror's public
  // widget contract also exempts its selectionchange from source-position
  // reconciliation. Click still reaches the existing request-edit boundary.
  ignoreEvent(event:Event){return event.type!=="click" && event.type!=="contextmenu";}
}
const inlineTags=new Set(["span","strong","b","em","i","u","s","del","mark","small","sub","sup","code","kbd"]);
function regions(state:EditorState):Region[]{
  const result:Region[]=[], parents=new Set<number>();
  syntaxTree(state).iterate({enter:cursor=>{
    if(result.length>=htmlPolicy.nodes) return false;
    if(cursor.name==="FencedCode" || cursor.name==="CodeBlock" || cursor.name==="InlineCode") return false;
    if(cursor.name==="HTMLBlock") {
      if(cursor.to-cursor.from<=htmlPolicy.inputBytes) result.push({from:cursor.from,to:cursor.to,source:state.sliceDoc(cursor.from,cursor.to),block:true});
      return false;
    }
    if(cursor.name!=="HTMLTag") return;
    const parent=cursor.node.parent;
    if(!parent || parent.to-parent.from>htmlPolicy.inputBytes || parents.has(parent.from)) return;
    parents.add(parent.from);
    const stack:{tag:string;from:number;to:number;style:string}[]=[], local:Region[]=[];
    let oversized=false;
    for(let child=parent.firstChild;child;child=child.nextSibling){
      if(child.name!=="HTMLTag") continue;
      const source=state.sliceDoc(child.from,child.to);
      if(source.length>htmlPolicy.inputBytes || local.length>=htmlPolicy.nodes-result.length) {oversized=true;break;}
      const closing=/^<\/\s*([a-z][\w-]*)\s*>$/i.exec(source);
      if(closing){
        const tag=closing[1].toLowerCase(),index=stack.map(open=>open.tag).lastIndexOf(tag);
        if(index<0) continue;
        const [open]=stack.splice(index);
        if(child.to-open.from>htmlPolicy.inputBytes) continue;
        if(inlineTags.has(tag)) local.push({from:open.from,to:child.to,tag,style:open.style,inside:[open.to,child.from]});
        else { // Resource, namespace and unknown groups pass through the same sanitizer.
          for(let i=local.length-1;i>=0;i--) if(local[i].from>=open.from) local.splice(i,1);
          local.push({from:open.from,to:child.to,source:state.sliceDoc(open.from,child.to)});
        }
        continue;
      }
      const match=/^<\s*([a-z][\w-]*)\b/i.exec(source);
      if(!match){local.push({from:child.from,to:child.to,source});continue;}
      const tag=match[1].toLowerCase();
      if(["br","hr","img","input","meta","link","source","embed","base"].includes(tag) || /\/\s*>$/.test(source)){
        local.push({from:child.from,to:child.to,source});continue;
      }
      if(stack.length>=htmlPolicy.depth) {oversized=true;break;}
      const fragment=inertHtml(source),element=fragment?.firstElementChild;
      const style=element?.namespaceURI==="http://www.w3.org/1999/xhtml" && element.localName===tag ? sanitizeStyle(element.getAttribute("style")||"") : "";
      stack.push({tag,from:child.from,to:child.to,style});
    }
    if(!oversized) result.push(...local);
  }});
  return result;
}
function decorate(state:EditorState,models:Region[]):ReturnType<typeof Decoration.set>{
  const ranges:Range<Decoration>[]=[];
  for(const region of models){
    if(!state.readOnly && state.selection.ranges.some(r=>r.from<=region.to && r.to>=region.from)) continue;
    if(region.inside){
      const [from,to]=region.inside;
      if(to>from) ranges.push(Decoration.mark({tagName:region.tag,attributes:region.style?{style:region.style}:undefined}).range(from,to));
      if(from>region.from) ranges.push(Decoration.replace({}).range(region.from,from));
      if(to<region.to) ranges.push(Decoration.replace({}).range(to,region.to));
    }else if(region.source!==undefined){
      ranges.push(Decoration.replace({widget:new HtmlWidget(region.source,!!region.block),block:!!region.block}).range(region.from,region.to));
    }
  }
  return Decoration.set(ranges,true);
}
const field=StateField.define<{models:Region[];tree:ReturnType<typeof syntaxTree>;decorations:ReturnType<typeof Decoration.set>}>({
  create(state){const models=state.facet(htmlConfiguration)?regions(state):[];return {models,tree:syntaxTree(state),decorations:decorate(state,models)};},
  update(old,tr){
    const enabled=tr.state.facet(htmlConfiguration),tree=syntaxTree(tr.state);
    const rebuild=tr.docChanged || tree!==old.tree || tr.startState.facet(htmlConfiguration)!==enabled || tr.effects.some(e=>e.is(parsed));
    if(!rebuild && !tr.selection && tr.state.readOnly===tr.startState.readOnly) return old;
    const models=rebuild?enabled?regions(tr.state):[]:old.models;
    return {models,tree,decorations:decorate(tr.state,models)};
  },
  provide:field=>EditorView.decorations.from(field,value=>value.decorations),
});
// Background incremental parsing may advance without a document edit. Reconcile
// direct decorations after the update, never recursively dispatch during it.
const reconcile=ViewPlugin.fromClass(class {
  queued=false;destroyed=false;
  update(update:{view:EditorView;state:EditorState}){
    if(this.queued || syntaxTree(update.state)===update.state.field(field).tree) return;
    this.queued=true;queueMicrotask(()=>{this.queued=false;if(!this.destroyed)update.view.dispatch({effects:parsed.of()});});
  }
  destroy(){this.destroyed=true;}
});
export const htmlRendering=[field,reconcile];
