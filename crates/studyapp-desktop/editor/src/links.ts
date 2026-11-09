import { EditorSelection } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { syntaxTree } from "@codemirror/language";

/** Only explicit web/mail navigation and local fragment jumps, never file/UNC. */
export function safeHref(value:string):string|undefined {
  if(!value || value.length>8192 || /[\u0000- \u007f]/.test(value))return;
  if(value.startsWith("#"))return value;
  try {
    const url=new URL(value);
    if(!["http:","https:","mailto:"].includes(url.protocol)||url.username||url.password)return;
    if(url.protocol!=="mailto:"&&!url.hostname)return;
    return url.href;
  } catch {return;}
}

export function installLinks(view:EditorView,open:(url:string)=>void):void {
  const navigate=(value:string)=>{
    const href=safeHref(value);if(!href)return;
    if(!href.startsWith("#")){open(href);return;}
    let target:string;try{target=decodeURIComponent(href.slice(1));}catch{return;}
    const html=[...view.contentDOM.querySelectorAll<HTMLElement>("[data-studyapp-anchor]")].find(node=>node.dataset.studyappAnchor===target);
    if(html){html.scrollIntoView({block:"start"});return;}
    const slug=(title:string)=>title.toLowerCase().replace(/[`*_~]/g,"").replace(/[^\p{L}\p{N}\p{M}\s_-]/gu,"").trim().replace(/\s/g,"-");
    const seen=new Map<string,number>();let position:number|undefined;
    syntaxTree(view.state).iterate({enter:node=>{
      if(!/^(?:ATX|Setext)Heading[1-6]$/.test(node.name))return;
      const title=view.state.sliceDoc(node.from,node.to).replace(/^ {0,3}#{1,6}\s*/,"").replace(/\s+#+\s*$/,"").replace(/\n[=-]+\s*$/,"").trim();
      const base=slug(title),count=seen.get(base)||0;seen.set(base,count+1);
      if(position===undefined && (target===title || target===(count?`${base}-${count}`:base)))position=node.from;
    }});
    if(position!==undefined)view.dispatch({selection:EditorSelection.cursor(position),scrollIntoView:true});
  };
  view.dom.addEventListener("click",event=>{
    const element=(event.target as Element).closest<HTMLElement>("a,[data-studyapp-href]");
    if(!element)return;
    event.preventDefault();event.stopPropagation();
    if(event.button!==0 || (!view.state.readOnly&&!event.ctrlKey&&!event.metaKey))return;
    const href=element.dataset.studyappHref||element.getAttribute("href");if(href)navigate(href);
  },true);
  view.dom.addEventListener("keydown",event=>{
    if(event.key!=="Enter")return;
    const element=(event.target as Element).closest<HTMLElement>("a,[data-studyapp-href]");if(!element)return;
    event.preventDefault();event.stopPropagation();const href=element.dataset.studyappHref||element.getAttribute("href");if(href)navigate(href);
  });
}
