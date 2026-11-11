import { parseAsync, renderDocument, HElement } from "docx-preview";
import { safeHref } from "./links";

export interface OriginalDocument {kind:"pdf"|"docx";data:string;}
export interface DocumentResult {generation:number;request:number;pages?:number[][];image?:string;error?:string;retry?:boolean;passwordRequired?:boolean;}
type RequestPage=(value:{request:number;page:number;width:number;password:string})=>void;
const MAX_BYTES=32*1024*1024;
const IMAGE=/^data:image\/(?:png|jpeg|gif|webp|bmp);base64,[a-zA-Z0-9+/=]+$/;
const EMPTY_IMAGE="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==";
let current:OriginalReader|undefined;
let sequence=0;
export function closeDocument():void {current?.destroy();current=undefined;delete document.documentElement.dataset.document;}
export function documentResult(value:DocumentResult):void {current?.receive(value);}
export function showDocument(value:OriginalDocument|undefined,open:(url:string)=>void,request:RequestPage,identity?:unknown):void {
  closeDocument();if(!value)return;
  if(!["pdf","docx"].includes(value.kind)||typeof value.data!=="string"||value.data.length>Math.ceil(MAX_BYTES/3)*4)throw new Error("原文档格式或大小无效");
  document.documentElement.dataset.document=value.kind;current=new OriginalReader(value,open,request,identity);
}

// Generated Office markup is still untrusted. Only bounded passive nodes are
// recreated; no original node, event, frame, script, font or remote resource is adopted.
const TAGS=new Set("div section article header footer p span a b strong i em u s strike sup sub small br hr tab stop table thead tbody tfoot tr td th colgroup col ul ol li dl dt dd h1 h2 h3 h4 h5 h6 img svg g path rect circle ellipse line polyline polygon text tspan defs clipPath math mrow mi mn mo ms mtext mspace msup msub msubsup mfrac msqrt mroot mtable mtr mtd munderover mover munder mfenced menclose mstyle".toLowerCase().split(" "));
const PROPS=new Set(("color background background-color opacity font font-family font-size font-style font-weight font-variant font-variant-caps font-kerning font-stretch line-height letter-spacing word-spacing text-align text-align-last text-decoration text-decoration-line text-decoration-color text-decoration-style text-transform text-indent text-shadow text-underline-position vertical-align white-space overflow-wrap word-break hyphens tab-size direction unicode-bidi border border-top border-right border-bottom border-left border-color border-width border-style border-collapse border-spacing border-radius padding padding-left padding-right padding-top padding-bottom margin margin-left margin-right margin-top margin-bottom width min-width max-width height min-height max-height display box-sizing position top right bottom left float clear overflow overflow-x overflow-y transform transform-origin clip-path z-index flex flex-flow flex-direction flex-wrap flex-grow flex-shrink flex-basis align-items align-self justify-content order column-count column-gap column-rule column-width columns break-before break-after break-inside page-break-before page-break-after page-break-inside list-style-type list-style-position counter-reset counter-increment counter-set content fill fill-opacity stroke stroke-width stroke-opacity box-shadow").split(" "));
function cleanDeclarations(source:CSSStyleDeclaration):string {
  const target=document.createElement("span").style;
  for(const key of source) {
    const value=source.getPropertyValue(key);
    if(!PROPS.has(key)||value.length>4096||/(?:url|expression|var|attr|image-set|paint)\s*\(|[{}@\\]/i.test(value))continue;
    if(key==="position"&&!/^(?:static|relative|absolute)$/.test(value))continue;
    target.setProperty(key,value);
  }
  return target.cssText;
}
function cleanStyles(source:string):string {
  if(source.length>2*1024*1024)throw new Error("文档样式过大");
  const sheet=new CSSStyleSheet();sheet.replaceSync(source);let count=0;
  const visit=(rules:CSSRuleList):string=>[...rules].map(rule=>{
    if(++count>20000)throw new Error("文档样式数量过多");
    if(rule instanceof CSSStyleRule) {
      if(rule.selectorText.length>8192||/:(?:host|root)|::(?:slotted|part)|@/i.test(rule.selectorText))return "";
      return `${rule.selectorText}{${cleanDeclarations(rule.style)}}`;
    }
    if(rule instanceof CSSMediaRule && /^(?:not )?print$/.test(rule.conditionText))return `@media ${rule.conditionText}{${visit(rule.cssRules)}}`;
    return "";
  }).join("\n");
  return visit(sheet.cssRules);
}
function officeFactory():((value:HElement|Node|string)=>Node) {
  let count=0,imageBytes=0;const owned=new WeakSet<Node>();
  const h=(value:HElement|Node|string):Node=>{
    if(++count>100000)throw new Error("文档内容过多，请拆分后导入");
    if(typeof value==="string")return document.createTextNode(value);
    if(value instanceof Node)return owned.has(value)?value:document.createTextNode(value.textContent||"");
    if(value.tagName==="#comment")return document.createComment("");
    if(value.tagName==="#fragment") {const fragment=document.createDocumentFragment();for(const child of value.children||[])fragment.append(h(child));owned.add(fragment);return fragment;}
    const tag=value.tagName.toLowerCase();
    const element=tag==="style"?document.createElement("style"):TAGS.has(tag)?(["http://www.w3.org/2000/svg","http://www.w3.org/1998/Math/MathML"].includes(value.ns||"")?document.createElementNS(value.ns!,value.tagName):document.createElement(tag)):document.createElement("span");
    owned.add(element);
    if(value.className&&String(value.className).length<=8192)element.setAttribute("class",String(value.className));
    if(value.style&&"style" in element) {
      const style=document.createElement("span").style;
      if(typeof value.style==="string")style.cssText=value.style;else Object.assign(style,value.style);
      element.setAttribute("style",cleanDeclarations(style));
    }
    for(const key of ["id","title","alt","colSpan","rowSpan","start","value","width","height","viewBox","d","points","x","y","x1","x2","y1","y2","cx","cy","r","rx","ry","transform","fill","stroke"]) {
      const property=value[key];if(property===undefined||String(property).length>32768)continue;
      if(["fill","stroke"].includes(key)&&/url\s*\(/i.test(String(property)))continue;
      element.setAttribute(key==="colSpan"?"colspan":key==="rowSpan"?"rowspan":key,String(property));
    }
    if(tag==="img"&&typeof value.src==="string"&&IMAGE.test(value.src)) {imageBytes+=value.src.length;if(imageBytes>64*1024*1024)throw new Error("文档图片内容过大");element.setAttribute("src",value.src);}
    if(tag==="a"&&value.href) {const href=safeHref(String(value.href));if(href){element.setAttribute("href",href);element.setAttribute("rel","noreferrer noopener");}}
    for(const child of value.children||[])element.append(h(child));
    return element;
  };
  return h;
}

// The renderer also mutates styles and image properties after factory calls.
// Recreate its final detached output so none of those mutations bypass policy.
function passiveWordNodes(nodes:Node[]):Node[] {
  let count=0;
  const value=(node:Node,depth=0):HElement|string=>{
    if(++count>100000||depth>128)throw new Error("文档结构过大或嵌套过深");
    if(node.nodeType===Node.TEXT_NODE)return node.textContent||"";
    if(!(node instanceof Element))return {tagName:"#comment"};
    if(node.tagName.toLowerCase()==="style")return {tagName:"style",children:[cleanStyles(node.textContent||"")]};
    const result:HElement={tagName:node.tagName.toLowerCase(),ns:node.namespaceURI||undefined,className:node.getAttribute("class")||undefined,style:node.getAttribute("style")||undefined};
    for(const name of ["id","title","alt","colspan","rowspan","start","value","width","height","viewBox","d","points","x","y","x1","x2","y1","y2","cx","cy","r","rx","ry","transform","fill","stroke","href","src"]) {
      const attribute=node.getAttribute(name);if(attribute!==null)result[name==="colspan"?"colSpan":name==="rowspan"?"rowSpan":name]=attribute;
    }
    result.children=[...node.childNodes].map(child=>value(child,depth+1));return result;
  };
  const recreate=officeFactory();return nodes.map(node=>recreate(value(node)));
}

interface DocumentPosition {page:number;offset:number;scale:string;}
const SCALES=new Set(["fit","0.5","0.75","1","1.25","1.5","2"]);
class OriginalReader {
  root=document.createElement("section");toolbar=document.createElement("div");pages=document.createElement("div");status=document.createElement("span");
  pageInput=document.createElement("input");total=document.createElement("span");zoom=document.createElement("select");
  dead=false;epoch=0;activePage=1;scale="fit";password="";
  sizes:number[][]=[];slots:HTMLElement[]=[];wanted=new Set<number>();running=false;
  observer:IntersectionObserver|undefined;resize:ResizeObserver;resizeTimer:ReturnType<typeof setTimeout>|undefined;
  pending=new Map<number,{resolve:(value:DocumentResult)=>void;timer:ReturnType<typeof setTimeout>}>();
  wordRoot:ShadowRoot|undefined;wordContent:HTMLElement|undefined;
  positionKey:string|undefined;initialPosition:DocumentPosition|undefined;positionEpoch=0;
  positionTimer:ReturnType<typeof setTimeout>|undefined;
  readonly savePosition=():void=>{
    if(!this.positionKey||this.initialPosition||!this.slots.length||this.dead)return;
    const top=this.pages.getBoundingClientRect().top;
    const index=this.slots.findIndex(slot=>slot.getBoundingClientRect().bottom>top+16);
    if(index<0)return;
    const bounds=this.slots[index].getBoundingClientRect();
    if(!bounds.height)return;
    // Only display metadata: no document bytes, source text or opening password.
    const position:DocumentPosition={page:index+1,offset:Math.max(0,Math.min(1,(top-bounds.top)/bounds.height)),scale:this.scale};
    try{localStorage.setItem(this.positionKey,JSON.stringify(position));}catch{/* Reading remains available without browser storage. */}
  };
  constructor(readonly value:OriginalDocument,readonly open:(url:string)=>void,readonly request:RequestPage,identity?:unknown) {
    if(typeof identity==="string"&&identity&&identity.length<=8192) {
      this.positionKey=`studyapp.document-reading.v1:${value.kind}:${identity}`;
      try{
        const saved:DocumentPosition=JSON.parse(localStorage.getItem(this.positionKey)||"null");
        if(saved&&Number.isSafeInteger(saved.page)&&saved.page>0&&saved.page<=2000&&Number.isFinite(saved.offset)&&saved.offset>=0&&saved.offset<=1&&SCALES.has(saved.scale)) {
          this.initialPosition=saved;this.scale=saved.scale;
        }
      }catch{/* Invalid or unavailable storage never prevents opening a document. */}
    }

    this.root.id="original-document";this.root.setAttribute("aria-label",value.kind==="pdf"?"PDF 原文阅读":"Word 原文阅读");
    this.toolbar.className="document-toolbar";this.toolbar.setAttribute("role","toolbar");this.toolbar.setAttribute("aria-label","文档阅读控制");
    this.pages.className="document-pages";this.pages.tabIndex=0;this.pages.setAttribute("aria-label","文档页面");
    this.status.setAttribute("role","status");this.status.textContent="正在打开原文档…";
    const button=(text:string,label:string,run:()=>void)=>{const e=document.createElement("button");e.type="button";e.textContent=text;e.setAttribute("aria-label",label);e.onclick=run;return e;};
    this.pageInput.type="number";this.pageInput.min="1";this.pageInput.value="1";this.pageInput.setAttribute("aria-label","文档页码");this.pageInput.onchange=()=>this.go(Number(this.pageInput.value));
    for(const [value,label] of [["fit","适合宽度"],["0.5","50%"],["0.75","75%"],["1","100%"],["1.25","125%"],["1.5","150%"],["2","200%"]])this.zoom.append(new Option(label,value));
    this.zoom.setAttribute("aria-label","文档缩放");this.zoom.value=this.scale;this.zoom.onchange=()=>{this.scale=this.zoom.value;this.layout();this.schedulePositionSave();};
    this.toolbar.append(button("‹","上一页",()=>this.go(this.activePage-1)),this.pageInput,this.total,button("›","下一页",()=>this.go(this.activePage+1)),this.zoom,this.status);
    this.root.append(this.toolbar,this.pages);document.querySelector("#editor-workspace")!.append(this.root);
    this.resize=new ResizeObserver(()=>{if(!this.wordContent&&this.scale!=="fit")return;clearTimeout(this.resizeTimer);this.resizeTimer=setTimeout(()=>this.layout(),100);});this.resize.observe(this.pages);
    this.pages.addEventListener("scroll",()=>{const top=this.pages.getBoundingClientRect().top;const index=this.slots.findIndex(slot=>slot.getBoundingClientRect().bottom>top+16);if(index>=0){this.activePage=index+1;if(document.activeElement!==this.pageInput)this.pageInput.value=String(index+1);}this.schedulePositionSave();},{passive:true});
    window.addEventListener("pagehide",this.savePosition);window.addEventListener("beforeunload",this.savePosition);
    void this.start().catch(error=>this.fail(error));
  }
  schedulePositionSave():void {clearTimeout(this.positionTimer);this.positionTimer=setTimeout(this.savePosition,120);}
  restorePosition():void {
    const saved=this.initialPosition;if(!saved||!this.slots.length)return;
    const epoch=++this.positionEpoch;
    requestAnimationFrame(()=>requestAnimationFrame(()=>{
      if(this.dead||epoch!==this.positionEpoch)return;
      const index=Math.min(saved.page,this.slots.length)-1,slot=this.slots[index];
      const bounds=slot.getBoundingClientRect();
      this.pages.scrollTop+=bounds.top-this.pages.getBoundingClientRect().top+saved.offset*bounds.height;
      this.activePage=index+1;this.pageInput.value=String(this.activePage);this.initialPosition=undefined;
    }));
  }
  fail(error:unknown):void {if(!this.dead){this.status.textContent=error instanceof Error?`文档未能显示：${error.message}`:"文档未能显示，请检查文件格式。";this.status.classList.add("error");}}
  receive(value:DocumentResult):void {
    const pending=this.pending.get(value.request);if(!pending)return;clearTimeout(pending.timer);this.pending.delete(value.request);pending.resolve(value);
  }
  ask(page:number,width:number):Promise<DocumentResult> {
    if(this.dead)return Promise.resolve({generation:0,request:0,error:"文档已关闭"});
    return new Promise(resolve=>{const request=++sequence;const timer=setTimeout(()=>{this.pending.delete(request);resolve({generation:0,request,error:"页面加载超时，请重新打开笔记；也可以导出原文后使用其他阅读器查看。"});},30000);this.pending.set(request,{resolve,timer});this.request({request,page,width:Math.max(96,Math.min(3200,Math.round(width))),password:this.password});});
  }
  async start():Promise<void> {
    if(this.dead)return;
    if(this.value.kind==="docx") {
      const decoded=atob(this.value.data);if(decoded.length>MAX_BYTES)throw new Error("文件超过 32 MiB");
      await this.word(Uint8Array.from(decoded,char=>char.charCodeAt(0)));return;
    }
    const result=await this.ask(0,1000);if(this.dead)return;
    if(result.retry){this.resizeTimer=setTimeout(()=>void this.start().catch(error=>this.fail(error)),150);return;}
    if(result.passwordRequired){this.passwordForm();return;}
    if(result.error)throw new Error(result.error);
    if(!result.pages?.length||result.pages.length>2000||!result.pages.every(size=>size.length===2&&size.every(n=>Number.isFinite(n)&&n>0)))throw new Error("文档页数或页面尺寸无效");
    this.sizes=result.pages;this.total.textContent=`/ ${this.sizes.length}`;this.pageInput.max=String(this.sizes.length);
    this.slots=this.sizes.map((_,index)=>{const slot=document.createElement("div");slot.className="document-page";slot.dataset.page=String(index+1);slot.setAttribute("aria-label",`第 ${index+1} 页`);this.pages.append(slot);return slot;});
    this.layout();this.status.textContent="PDF 原页面 · 可翻页和缩放";
  }
  passwordForm():void {
    const form=document.createElement("form"),label=document.createElement("label"),input=document.createElement("input"),submit=document.createElement("button");
    form.className="document-password";label.textContent="此 PDF 需要打开密码";input.type="password";input.maxLength=256;input.required=true;input.autocomplete="off";input.setAttribute("aria-label","PDF 打开密码");submit.type="submit";submit.textContent="打开";
    label.append(input);form.append(label,submit);this.pages.replaceChildren(form);
    form.onsubmit=event=>{event.preventDefault();this.password=input.value;input.value="";form.remove();void this.start().catch(error=>this.fail(error));};
    this.status.textContent=(this.password?"密码未被接受，请重试。":"")+"密码只用于本次阅读，不写入资料库或日志。";
  }
  layout():void {
    if(this.dead)return;
    if(this.wordContent){this.wordLayout();return;}
    this.epoch++;this.observer?.disconnect();this.wanted.clear();
    for(let i=0;i<this.slots.length;i++) {
      const slot=this.slots[i],[w,h]=this.sizes[i];const scale=this.scale==="fit"?Math.max(.01,(this.pages.clientWidth-32)/w):Number(this.scale);
      slot.replaceChildren();delete slot.dataset.rendered;slot.style.width=`${w*scale}px`;slot.style.height=`${h*scale}px`;
    }
    this.observer=new IntersectionObserver(entries=>{
      for(const entry of entries){const slot=entry.target as HTMLElement,page=Number(slot.dataset.page);if(entry.isIntersecting)this.wanted.add(page);else{this.wanted.delete(page);slot.replaceChildren();delete slot.dataset.rendered;}}
      void this.pump();
    },{root:this.pages,rootMargin:"200px 0px"});
    for(const slot of this.slots)this.observer.observe(slot);
    this.restorePosition();
  }
  async pump():Promise<void> {
    if(this.running||this.dead)return;this.running=true;
    try {
      for(const page of this.wanted) {
        const epoch=this.epoch,slot=this.slots[page-1];if(!slot||slot.dataset.rendered===String(epoch))continue;
        const result=await this.ask(page,slot.clientWidth*Math.min(2,devicePixelRatio||1));
        if(this.dead||epoch!==this.epoch||!this.wanted.has(page))continue;
        if(result.retry){this.resizeTimer=setTimeout(()=>void this.pump(),150);break;}
        if(result.error) {slot.textContent=result.error;slot.dataset.rendered=String(epoch);continue;}
        if(!result.image||result.image.length>24*1024*1024||!IMAGE.test(result.image))throw new Error("页面图像无效");
        const image=document.createElement("img");image.src=result.image;image.alt=`第 ${page} 页`;image.draggable=false;slot.replaceChildren(image);slot.dataset.rendered=String(epoch);
      }
    } catch(error){this.fail(error);} finally {this.running=false;}
  }
  go(page:number):void {if(!Number.isInteger(page)||!this.slots.length)return;const index=Math.max(0,Math.min(this.slots.length-1,page-1));this.slots[index].scrollIntoView({block:"start"});this.activePage=index+1;this.pageInput.value=String(index+1);}
  async word(bytes:Uint8Array):Promise<void> {
    const host=document.createElement("div");host.className="document-word";this.pages.append(host);
    const shadow=host.attachShadow({mode:"open"});this.wordRoot=shadow;
    const parsed=await parseAsync(bytes,{useBase64URL:true,renderAltChunks:false,ignoreFonts:true});if(this.dead)return;
    let imageBytes=0;const known=new Set<string>();
    const bounded=(method:string)=>{const load=parsed[method].bind(parsed);parsed[method]=async(...args:unknown[])=>{
      const source=await load(...args);if(typeof source!=="string"||!IMAGE.test(source))return EMPTY_IMAGE;
      if(!known.has(source)){imageBytes+=source.length;known.add(source);}if(imageBytes>64*1024*1024)throw new Error("文档图片内容过大");return source;
    };};
    bounded("loadDocumentImage");bounded("loadNumberingImage");
    const nodes=await renderDocument(parsed,{inWrapper:true,hideWrapperOnPrint:false,ignoreWidth:false,ignoreHeight:false,ignoreFonts:true,breakPages:true,ignoreLastRenderedPageBreak:false,renderHeaders:true,renderFooters:true,renderFootnotes:true,renderEndnotes:true,renderAltChunks:false,useBase64URL:true,h:officeFactory()});
    if(this.dead)return;
    const content=document.createElement("div");content.className="word-content";this.wordContent=content;
    for(const node of passiveWordNodes(nodes)) {
      if(node instanceof HTMLStyleElement){const style=document.createElement("style");style.textContent=cleanStyles(node.textContent||"");shadow.append(style);}else content.append(node);
    }
    const style=document.createElement("style");style.textContent=":host{all:initial;display:block;position:relative;overflow:hidden;color:#000;background:#e4e6ea;font:12pt serif}.word-content{position:absolute;top:0;left:0;transform-origin:top left}.docx-wrapper{background:transparent!important;padding:12px!important;width:max-content!important;box-sizing:border-box;align-items:flex-start!important}.docx-wrapper>section.docx{margin-bottom:12px!important}a{cursor:pointer}img{max-width:100%}*{cursor:auto}";
    shadow.append(style,content);this.resize.observe(content);
    shadow.addEventListener("click",event=>{
      const link=(event.target as Element).closest("a");if(!link)return;event.preventDefault();const href=safeHref(link.getAttribute("href")||"");if(!href)return;
      if(href.startsWith("#")){let id:string;try{id=decodeURIComponent(href.slice(1));}catch{return;}shadow.getElementById(id)?.scrollIntoView({block:"start"});}else this.open(href);
    });
    this.slots=[...shadow.querySelectorAll<HTMLElement>("section.docx")];
    if(!this.slots.length)throw new Error("DOCX 没有可显示的正文页面");
    if(this.slots.length>2000)throw new Error("文档超过 2,000 页，请拆分后导入");
    this.total.textContent=`/ ${this.slots.length}`;this.pageInput.max=String(this.slots.length);
    this.status.textContent="DOCX 原版式 · 使用本机字体，分页可能与 Word 略有差异";this.wordLayout();
  }
  wordLayout():void {
    const content=this.wordContent,host=content?(content.getRootNode() as ShadowRoot).host as HTMLElement:undefined;if(!content||!host)return;
    content.style.transform="none";const width=Math.max(1,content.scrollWidth),height=Math.max(1,content.scrollHeight);
    const scale=this.scale==="fit"?Math.max(.01,(this.pages.clientWidth-24)/width):Number(this.scale);
    content.style.transform=`scale(${scale})`;host.style.width=`${width*scale}px`;host.style.height=`${height*scale}px`;
    this.restorePosition();
  }
  destroy():void {this.savePosition();clearTimeout(this.positionTimer);window.removeEventListener("pagehide",this.savePosition);window.removeEventListener("beforeunload",this.savePosition);this.positionEpoch++;this.dead=true;this.epoch++;this.password="";clearTimeout(this.resizeTimer);this.resize.disconnect();this.observer?.disconnect();for(const [request,pending] of this.pending){clearTimeout(pending.timer);pending.resolve({generation:0,request,error:"文档已关闭"});}this.pending.clear();this.root.remove();}
}
