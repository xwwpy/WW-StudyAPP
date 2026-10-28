import { Language, StreamLanguage, HighlightStyle } from "@codemirror/language";
import { rust } from "@codemirror/lang-rust";
import { python } from "@codemirror/lang-python";
import { javascript } from "@codemirror/lang-javascript";
import { cpp } from "@codemirror/lang-cpp";
import { java } from "@codemirror/lang-java";
import { go } from "@codemirror/lang-go";
import { html } from "@codemirror/lang-html";
import { css } from "@codemirror/lang-css";
import { json } from "@codemirror/lang-json";
import { yaml } from "@codemirror/lang-yaml";
import { sql } from "@codemirror/lang-sql";
import { csharp } from "@codemirror/legacy-modes/mode/clike";
import { toml } from "@codemirror/legacy-modes/mode/toml";
import { shell } from "@codemirror/legacy-modes/mode/shell";
import { tags } from "@lezer/highlight";

const languages = new Map<string, Language>();
function register(names: string[], language: Language) { for (const name of names) languages.set(name, language); }
register(["rust", "rs"], rust().language);
register(["python", "py"], python().language);
register(["javascript", "js", "jsx"], javascript({ jsx: true }).language);
register(["typescript", "ts", "tsx"], javascript({ jsx: true, typescript: true }).language);
register(["c", "h", "cpp", "c++", "cc", "cxx", "hpp"], cpp().language);
register(["csharp", "c#", "cs"], StreamLanguage.define(csharp));
register(["java"], java().language);
register(["go", "golang"], go().language);
register(["html", "htm", "xml"], html().language);
register(["css"], css().language);
register(["json", "jsonc"], json().language);
register(["yaml", "yml"], yaml().language);
register(["toml"], StreamLanguage.define(toml));
register(["sql"], sql().language);
register(["shell", "sh", "bash", "zsh"], StreamLanguage.define(shell));

export function fencedLanguage(info: string): Language | null {
  return languages.get(info.trim().split(/\s+/)[0].toLowerCase()) ?? null;
}

export const codeHighlighting = HighlightStyle.define([
  { tag: [tags.keyword, tags.modifier, tags.controlKeyword, tags.operatorKeyword], class: "tok-keyword" },
  { tag: [tags.string, tags.special(tags.string), tags.regexp], class: "tok-string" },
  { tag: [tags.number, tags.bool, tags.null], class: "tok-number" },
  { tag: [tags.comment, tags.docComment], class: "tok-comment" },
  { tag: [tags.typeName, tags.className, tags.namespace], class: "tok-type" },
  { tag: [tags.propertyName, tags.attributeName], class: "tok-property" },
  { tag: [tags.tagName], class: "tok-tag" },
  { tag: [tags.function(tags.variableName), tags.function(tags.propertyName)], class: "tok-function" },
  { tag: [tags.operator, tags.punctuation], class: "tok-punctuation" }
]);

// Share the parser registry with the native, keyboard-searchable selector.
export const fenceLanguageNames = [...languages.keys()];

export function normalizeFenceLanguage(value: string): string {
  const token = value.trim().toLowerCase();
  return languages.has(token) ? token : "";
}

export function completeFenceLanguage(value: string): string[] {
  const query = value.trim().toLowerCase();
  return fenceLanguageNames.filter(name => !query || name.includes(query))
    .sort((a, b) => {
      const rank = (name: string) => name === query ? 0 : name.startsWith(query) ? 1 : 2;
      return rank(a) - rank(b) || a.length - b.length || a.localeCompare(b);
    }).slice(0, 5);
}
