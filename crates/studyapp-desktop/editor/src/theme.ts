// Only declarative, bounded semantic tokens cross the native/WebView boundary.
const color = (value:unknown): value is string => typeof value === "string" && /^#(?:[a-f0-9]{6}|[a-f0-9]{8})$/i.test(value);
const fonts = {sans:'"Segoe UI Variable Text", "Microsoft YaHei UI", "Segoe UI", sans-serif',serif:'Georgia, "Noto Serif CJK SC", "SimSun", serif',mono:'"Cascadia Mono", Consolas, monospace'};
const luminance = (hex:string) => [1,3,5].map(offset => parseInt(hex.slice(offset,offset+2),16)/255).map(value => value<=.04045 ? value/12.92 : ((value+.055)/1.055)**2.4).reduce((sum,value,index)=>sum+value*[.2126,.7152,.0722][index],0);
// CSS source-over in sRGB. Keep raw tokens for painting; use opaque actual
// backdrops only for contrast. Ignoring AA would hide fully transparent text.
const over = (foreground:string, background:string) => {
  const alpha=foreground.length===9 ? parseInt(foreground.slice(7,9),16)/255 : 1;
  return "#"+[1,3,5].map(offset=>Math.round(parseInt(foreground.slice(offset,offset+2),16)*alpha+parseInt(background.slice(offset,offset+2),16)*(1-alpha)).toString(16).padStart(2,"0")).join("");
};
const contrast = (a:string,b:string) => {
  const background=over(b,"#ffffff"), foreground=over(a,background);
  return (Math.max(luminance(foreground),luminance(background))+.05)/(Math.min(luminance(foreground),luminance(background))+.05);
};
function readable(value:string, backgrounds:string[]):string {
  const passes = (candidate:string) => backgrounds.every(background => contrast(candidate,background)>=4.5);
  if (passes(value)) return value;
  const score = (candidate:string) => Math.min(...backgrounds.map(background=>contrast(candidate,background)));
  const target = score("#ffffff")>score("#000000") ? 255 : 0;
  for (let step=1;step<=100;step++) {
    const result = "#"+[1,3,5].map(offset=>Math.floor((parseInt(value.slice(offset,offset+2),16)*(100-step)+target*step)/100).toString(16).padStart(2,"0")).join("");
    if (passes(result)) return result;
  }
  for (let gray=0;gray<=255;gray++) {
    const candidate="#"+gray.toString(16).padStart(2,"0").repeat(3);
    if(passes(candidate)) return candidate;
  }
  return target ? "#ffffff" : "#000000";
}
const record = (value:unknown):Record<string,unknown> => value && typeof value==="object" && !Array.isArray(value) ? value as Record<string,unknown> : {};
export function applyEditorTheme(tokens:Record<string,unknown>):boolean {
  tokens = record(tokens);
  const root = document.documentElement;
  const set = (name:string,value:string) => root.style.setProperty(`--editor-${name.replaceAll("_","-")}`,value);
  if (tokens.mode === "light" || tokens.mode === "dark") root.dataset.theme = tokens.mode;
  for (const name of ["background","surface","text","muted","border","accent","selection","codeBackground","codeText"]) {
    if (color(tokens[name])) set(name.replace(/[A-Z]/g,match=>"-"+match.toLowerCase()),tokens[name]);
  }
  const computed = getComputedStyle(root);
  const base = (name:string,fallback:string) => { const value=computed.getPropertyValue(`--editor-${name}`).trim(); return color(value) ? value : fallback; };
  const surface=over(base("surface","#f7f8fa"),"#ffffff"),text=base("text","#263344"),accent=base("accent","#177f80");
  const token = (name:string,fallback:string) => color(tokens[name]) ? tokens[name] : fallback;
  const markdown = record(tokens.markdown), overrides=record(markdown.colors);
  const pick = (name:string,fallback:string) => color(overrides[name]) ? overrides[name] : fallback;
  const toolbarBackground=token("toolbarBackground",surface),quoteBackground=pick("quote_background",token("quoteBackground",surface));
  const codeBackground=pick("code_background",token("codeBackground",surface));
  const inlineBackground=color(overrides.code_background) ? codeBackground : token("inlineCodeBackground",codeBackground);
  const tableHeader=token("tableHeader",surface),tableBackground=pick("table_background",surface);
  const stripe=markdown.table_stripes===false ? tableBackground : pick("table_stripe",surface);
  const quoteSurface=over(quoteBackground,surface),codeSurface=over(codeBackground,surface),inlineSurface=over(inlineBackground,surface);
  // Rows replace, rather than overlay, the table background in editor.css.
  const tableSurface=over(tableBackground,surface),stripeSurface=over(stripe,surface),headerSurface=over(tableHeader,surface);
  let lowContrast=false;
  // Explicit saved colors are author choices. Guard derived colors and report
  // low contrast without replacing a valid override or blocking persistence.
  const foreground = (name:string,fallback:string,backgrounds:string[]) => {
    const value=color(overrides[name]) ? overrides[name] : readable(fallback,backgrounds);
    if (backgrounds.some(background=>contrast(value,background)<4.5)) lowContrast=true;
    return value;
  };
  const selection=pick("selection_background",token("selection",luminance(surface)<.18?"#3E6187":"#A3C9EE"));
  const selectionBackdrops=[surface,quoteSurface,codeSurface,tableSurface,stripeSurface];
  if (selectionBackdrops.some(background=>contrast(selection,background)<1.5)) lowContrast=true;
  const selectionText=foreground("selection_text",token("selectionText",text),selectionBackdrops.map(background=>over(selection,background)));
  const generalBackgrounds=[surface,quoteSurface,tableSurface,stripeSurface];
  const general = (name:string,fallback:string) => foreground(name,fallback,generalBackgrounds);
  const body=pick("body",text);
  if (contrast(body,surface)<4.5) lowContrast=true;
  const values:Record<string,string> = {
    selection,selection_text:selectionText,text:body,heading:foreground("heading",text,[surface]),link:general("link",accent),
    muted:general("muted",base("muted",text)),quote:foreground("quote",text,[quoteSurface]),quote_background:quoteBackground,
    toolbar_background:toolbarBackground,ui_text:readable(text,[surface,over(toolbarBackground,surface)]),ui_muted:readable(base("muted",text),[surface,over(toolbarBackground,surface)]),
    danger:readable(token("danger","#ac3a36"),[surface]),warning:readable(token("warning","#91631d"),[surface]),
    code_background:codeBackground,code_text:foreground("code",token("codeText",text),[codeBackground]),
    inline_code_background:inlineBackground,inline_code_text:foreground("code",token("codeText",text),[inlineBackground]),
    table:foreground("table",text,[tableSurface,stripeSurface,headerSurface]),table_header:foreground("table_header",text,[headerSurface]),
    table_header_background:tableHeader,table_background:tableBackground,table_stripe:stripe,list:general("list",accent),
    grid:pick("grid",token("grid",base("border",text))),code_border:pick("code_border",token("codeBorder",base("border",text))),
    check_text:readable(text,[accent]),check_background:surface,
  };
  for (const [name,value] of Object.entries(values)) set(name,value);
  const dark = luminance(codeSurface)<.18;
  for (const [name,light,darkColor] of [
    ["keyword","#854191","#d6a4e8"],["string","#28683e","#9bcea1"],["number","#91531a","#efbd83"],
    ["comment","#586b79","#b1bdc9"],["type","#7c5a19","#e7ce8b"],["property","#17677b","#8dcbd9"],
    ["tag","#9b4446","#f0aaad"],["function","#24648c","#99caec"],["punctuation","#536473","#c3cbd5"],
  ]) {
    set(`syntax_${name}`,foreground(`syntax_${name}`,dark?darkColor:light,[codeSurface]));
    set(`source_syntax_${name}`,foreground(`syntax_${name}`,luminance(surface)<.18?darkColor:light,[surface]));
  }
  const bounded=(value:unknown,min:number,max:number,fallback:number) => typeof value==="number" && Number.isFinite(value) && value>=min && value<=max ? value : fallback;
  const bodySize=bounded(markdown.body_size,12,28,bounded(tokens.fontSize,12,24,15));
  set("font_size",`${bodySize}px`);
  set("code_size",`${bounded(markdown.code_size,12,26,Math.max(12,bodySize-1))}px`);
  set("line_height",String(bounded(markdown.line_height,130,240,bounded(tokens.spacing,1.3,2.4,1.75)*100)/100));
  set("reading_width",`${bounded(markdown.reading_width,480,1600,720)}px`);
  set("code_radius",`${bounded(markdown.code_radius,0,24,8)}px`);
  set("grid_width",markdown.table_grid===false?"0px":"1px");
  set("code_border_width",markdown.code_border===false?"0px":"1px");
  if (markdown.table_stripes===false) set("table_stripe",tableBackground);
  set("body_font",fonts[markdown.body_font as keyof typeof fonts] ?? fonts.sans);
  set("mono_font",fonts[markdown.mono_font as keyof typeof fonts] ?? fonts.mono);
  return lowContrast;
}
