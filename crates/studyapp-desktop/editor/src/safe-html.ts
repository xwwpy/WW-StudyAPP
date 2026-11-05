import policy from "../../../studyapp-core/src/html_policy.json";
export { policy as htmlPolicy };
const bytes = (s:string) => new TextEncoder().encode(s).length;
const decimal = (s:string) => /^[\d.]+$/.test(s) && Number.isFinite(Number(s));
function color(s:string):boolean {
  if (policy.colors.includes(s) || /^#(?:[\da-f]{3}|[\da-f]{4}|[\da-f]{6}|[\da-f]{8})$/.test(s)) return true;
  const match=/^(rgb|rgba)\(([^)]*)\)$/.exec(s);
  if (!match) return false;
  const parts=match[2].split(",").map(s=>s.trim()), alpha=match[1]==="rgba";
  return parts.length===(alpha?4:3) && parts.slice(0,3).every(s=>/^\d+$/.test(s) && Number(s)<=255)
    && (!alpha || decimal(parts[3]) && Number(parts[3])>=0 && Number(parts[3])<=1);
}
export function sanitizeStyle(source:string):string {
  if (bytes(source)>2048) return "";
  const accepted=new Map<string,string>();
  for (const declaration of source.split(";").slice(0,64)) {
    const colon=declaration.indexOf(":"); if(colon<0) continue;
    const key=declaration.slice(0,colon).trim().toLowerCase(),value=declaration.slice(colon+1).trim().toLowerCase();
    if(!value || value.length>96 || /[^\x20-\x7e]|[\{}!@"'<>/]/.test(value)) continue;
    const enums=Object.hasOwn(policy.enums,key) ? (policy.enums as Record<string,string[]>)[key] : undefined;
    const rule=Object.hasOwn(policy.numbers,key) ? (policy.numbers as Record<string,{count:number;units:Record<string,number[]>}>)[key] : undefined;
    const valid=policy.colorProperties.includes(key) ? color(value) : enums ? enums.includes(value) : rule ? (()=>{
      const parts=value.split(/[\t\n\f\r ]+/);
      return parts.length<=rule.count && parts.every(part=>Object.entries(rule.units).some(([unit,[min,max]])=>{
        const s=unit ? part.endsWith(unit) ? part.slice(0,-unit.length) : "" : part;
        return decimal(s) && Number(s)>=min && Number(s)<=max || part==="0" && min<=0 && max>=0;
      }));
    })() : false;
    if(valid) accepted.set(key,value);
  }
  return [...accepted].sort(([a],[b])=>a<b?-1:a>b?1:0).map(([k,v])=>`${k}:${v}`).join(";");
}
/** Parsing stays in inert template contents. No parsed element is ever adopted. */
export function inertHtml(source:string):DocumentFragment|null {
  if(bytes(source)>policy.inputBytes) return null;
  const inert=document.createElement("template");
  inert.innerHTML=source;
  return inert.content;
}
export function sanitizeHtml(source:string):DocumentFragment|null {
  const parsed=inertHtml(source); if(!parsed) return null;
  const result=document.createDocumentFragment(); let remaining=policy.nodes-2, output=0;
  const visit=(node:Node,parent:Node,depth:number):boolean=>{
    if(depth>policy.depth || --remaining<0) return false;
    if(node.nodeType===Node.TEXT_NODE) { const text=node.textContent||""; output+=bytes(text)*6; parent.appendChild(document.createTextNode(text)); }
    else if(node instanceof Element) {
      const tag=node.localName;
      if(node.namespaceURI!=="http://www.w3.org/1999/xhtml" || policy.blocked.includes(tag)) return true;
      if(tag==="img") { const text=`[图片：${node.getAttribute("alt")??"图片"}]`; output+=bytes(text)*6; parent.appendChild(document.createTextNode(text)); return output<=policy.outputBytes; }
      let target=parent;
      if(policy.tags.includes(tag)) {
        const clean=document.createElement(tag),style=sanitizeStyle(node.getAttribute("style")||"");
        if(style) clean.setAttribute("style",style);
        output+=tag.length*2+style.length+20;
        parent.appendChild(clean); target=clean;
      }
      for(const child of node.childNodes) if(!visit(child,target,depth+1)) return false;
    }
    return output<=policy.outputBytes;
  };
  for(const node of parsed.childNodes) if(!visit(node,result,2)) return null;
  return result;
}
