import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { createServer } from "node:http";
import { readFile, mkdir, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";
import { parser as markdownParser } from "@lezer/markdown";
import { normalizeBinding, resolveBindings, shortcutDefaults } from "../src/shortcuts.ts";
import { documentStatistics } from "../src/statistics.ts";

const root = fileURLToPath(new URL("../dist/", import.meta.url));
let server, browser, base;
before(async () => {
  const assets = new Map();
  for (const name of ["index.html", "editor.js", "editor.css"]) {
    try { assets.set(name, await readFile(root + name)); }
    catch (error) { if (error.code !== "ENOENT") throw error; }
  }
  server = createServer(async (req, res) => {
    const name = new URL(req.url, "http://localhost").pathname.slice(1) || "index.html";
    if (!["index.html", "editor.js", "editor.css"].includes(name)) {
      res.writeHead(404).end(); return;
    }
    if (assets.has(name)) {
      res.setHeader("Content-Type", name.endsWith(".js") ? "text/javascript" : name.endsWith(".css") ? "text/css" : "text/html");
      res.end(assets.get(name));
    } else {
      res.setHeader("Content-Type", "text/html");
      res.end("<!doctype html><title>Frontend is not implemented</title>");
    }
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  base = `http://127.0.0.1:${server.address().port}/`;
  const edge = "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe";
  browser = await chromium.launch({ headless: true, ...(existsSync(edge) ? { executablePath: edge } : {}) });
});
after(async () => { await browser?.close(); await new Promise(resolve => server?.close(resolve)); });

async function setup(text = "", testEnabled = true) {
  const page = await browser.newPage({ viewport: { width: 1100, height: 800 } });
  const errors = [];
  const externalRequests = [];
  page.on("pageerror", error => errors.push(error.message));
  page.on("request", request => { if (!request.url().startsWith(base)) externalRequests.push(request.url()); });
  await page.addInitScript(({ testEnabled }) => {
    window.__STUDYAPP_EDITOR_TEST__ = testEnabled;
    window.messages = [];
    window.ipc = { postMessage: message => window.messages.push(JSON.parse(message)) };
  }, { testEnabled });
  await page.goto(base);
  const api = await page.evaluate(() => typeof window.studyappEditor);
  assert.equal(api, "object", `offline editor bridge must exist; page errors: ${JSON.stringify(errors)}; html: ${api !== "object" ? (await page.content()).slice(0, 600) : ""}`);
  await page.evaluate(text => window.studyappEditor.load({ generation: 41, text, readonly: false }), text);
  await page.waitForTimeout(80);
  return { page, errors, externalRequests };
}
async function action(page, value) { return page.evaluate(value => window.studyappEditor.test(value), value); }
async function state(page) { return action(page, { type: "snapshot" }); }
async function select(page, anchor, head = anchor) {
  const result = await action(page, { type: "select", anchor, head });
  // view.focus() changes DOM focus immediately; CM reconciles focusChanged
  // asynchronously. Wait for its rendered focus state before inspecting any
  // focus-dependent decorations rather than racing the browser's timer.
  await page.locator(".cm-editor.cm-focused").waitFor({ state: "attached" });
  return result;
}
async function shortcuts(page, bindings) {
  assert.equal(await page.evaluate(() => typeof window.studyappEditor.setShortcuts), "function", "shortcut bridge must support live reconfiguration");
  await page.evaluate(bindings => window.studyappEditor.setShortcuts(bindings), bindings);
}

test("passive first readonly inspection never invokes corrective CodeMirror geometry", async () => {
  const { page, errors } = await setup();
  const text = `# 默认样式

尚未选择任何预设的只读正文。
`;
  for (const tokens of [null, {}, { mode: "light" }, { mode: "dark" }]) {
    await page.evaluate(({ text, tokens }) => {
      window.studyappEditor.setTheme(tokens);
      window.studyappEditor.load({ generation: 42, text, readonly: true });
    }, { text, tokens });
    await page.waitForTimeout(80);
    const before = await action(page, { type: "passiveVisibility" });
    assert.equal(before.text, text);
    assert.equal(before.readonly, true);
    assert.equal(before.revision, 0);
    assert.ok(before.editor.width > 500);
    assert.ok(before.editor.height > 100);
    assert.ok(before.firstLine.height > 0);
    assert.equal(before.firstLine.visibility, "visible");
  }
  const source = await readFile(new URL("../src/editor.ts", import.meta.url), "utf8");
  const passive = source.split('action.type === "passiveVisibility"')[1].split('else if (action.type === "snapshot")')[0];
  assert.doesNotMatch(passive, /view\.(?:requestMeasure|dispatch|focus|posAtCoords|coordsAtPos)\(/);
  assert.deepEqual(errors, []);
  await page.close();
});

test("layout completion acknowledges readonly preview resize before the continuity baseline", async () => {
  const rust = await readFile(new URL("../../src/appearance_preview.rs", import.meta.url), "utf8");
  const text = rust.match(/PREVIEW_SAMPLE: &str = r#"([\s\S]*?)"#;/)[1].replaceAll(String.fromCharCode(13), "");
  const { page, errors } = await setup(text);
  await page.setViewportSize({ width: 830, height: 371 });
  await page.evaluate(text => {
    window.studyappEditor.setTheme({ markdown: { reading_width: 640 } });
    window.studyappEditor.load({ generation: 42, text, readonly: true });
  }, text);
  // Anchor a stable table row: adding list examples must not move the caret
  // into an atomic HTML widget that has no resize-driven scroll delta.
  await action(page, { type: "select", anchor: text.indexOf("| 概念与定义") });
  const full = await action(page, { type: "layout", width: 830, height: 371 });
  assert.ok(full.scrollTop > 50);
  // Hold the real browser measurement scheduler across the same full-width to
  // split-pane transition as the native fixture. No color interaction occurs.
  await page.evaluate(() => {
    const frame = window.requestAnimationFrame.bind(window), pending = [];
    window.requestAnimationFrame = callback => { pending.push(callback); return 999999; };
    window.releasePreviewFrames = () => {
      window.requestAnimationFrame = frame;
      pending.forEach(callback => frame(callback));
    };
  });
  await page.setViewportSize({ width: 459, height: 371 });
  const pending = await state(page);
  await page.evaluate(() => window.releasePreviewFrames());
  const opened = await action(page, { type: "layout", width: 459, height: 371 });
  assert.ok(Math.abs(opened.scrollTop - pending.scrollTop) >= 5,
    `fixture must expose pending resize scroll: ${JSON.stringify({ pending, opened })}`);
  const hovered = await state(page);
  assert.deepEqual(hovered.theme, opened.theme);
  assert.equal(hovered.generation, opened.generation);
  assert.equal(hovered.text, text);
  assert.equal(hovered.readonly, true);
  assert.ok(Math.abs(hovered.scrollTop - opened.scrollTop) < 5);
  assert.deepEqual(opened.layout, { width: 459, height: 371 });
  assert.deepEqual(errors, []);
  await assert.rejects(action(page, { type: "layout", width: 830, height: 371 }), /layout.*timed out/);
  await page.close();
});

test("editor semantic surfaces use new tokens and reset to legacy fallbacks", async () => {
  const { page } = await setup("# Heading\n\n`inline`\n\n> Quote\n\n| A | B |\n| --- | --- |\n| C | D |\n\n```js\nconst answer = 42;\n```\n\nEnd");
  await select(page, (await state(page)).text.length);
  await page.evaluate(() => window.studyappEditor.setTheme({
    background: "#ededed", surface: "#ffffff", text: "#202020", border: "#b0b0b0",
    toolbarBackground: "#fefefe", codeBackground: "#f1f2f3", codeBorder: "#b2b3b4",
    inlineCodeBackground: "#e5e6e7", quoteBackground: "#f5f6f7", tableHeader: "#e8e9ea", grid: "#c1c2c3"
  }));
  const styles = () => {
    const css = selector => getComputedStyle(document.querySelector(selector));
    return { toolbar: css("#toolbar").backgroundColor, document: css(".cm-scroller").backgroundColor,
      code: css(".md-code-line").backgroundColor, codeBorder: css(".md-code-line").borderLeftColor,
      inline: css(".md-inline-code").backgroundColor, quote: css(".md-quote").backgroundColor,
      table: css(".md-table-header").backgroundColor, grid: css(".md-table-row").borderLeftColor };
  };
  assert.deepEqual(await page.evaluate(styles), { toolbar: "rgb(254, 254, 254)", document: "rgb(255, 255, 255)",
    code: "rgb(241, 242, 243)", codeBorder: "rgb(178, 179, 180)", inline: "rgb(229, 230, 231)",
    quote: "rgb(245, 246, 247)", table: "rgb(232, 233, 234)", grid: "rgb(193, 194, 195)" });
  await page.evaluate(() => window.studyappEditor.setTheme({ surface: "#fafafa", border: "#cccccc", codeBackground: "#eeeeee" }));
  assert.deepEqual(await page.evaluate(styles), { toolbar: "rgb(250, 250, 250)", document: "rgb(250, 250, 250)",
    code: "rgb(238, 238, 238)", codeBorder: "rgb(204, 204, 204)", inline: "rgb(238, 238, 238)",
    quote: "rgb(250, 250, 250)", table: "rgb(250, 250, 250)", grid: "rgb(204, 204, 204)" });
  await page.close();
});

test("explicit low contrast reading colors are preserved with an independent warning", async () => {
  const { page } = await setup("# Heading\n\nBody `code`\n\nEnd");
  await select(page, (await state(page)).text.length);
  await page.evaluate(() => window.studyappEditor.setTheme({ surface: "#ffffff", text: "#fafafa",
    markdown: { colors: { heading: "#eeeeee", code: "#dddddd", code_background: "#ffffff" } } }));
  assert.equal(await page.locator(".cm-scroller").evaluate(node => getComputedStyle(node).color), "rgb(250, 250, 250)");
  assert.equal(await page.locator(".md-heading").evaluate(node => getComputedStyle(node).color), "rgb(238, 238, 238)");
  assert.equal(await page.locator(".md-inline-code").evaluate(node => getComputedStyle(node).color), "rgb(221, 221, 221)");
  assert.match(await page.locator("#editor-theme-warning").innerText(), /对比度/);
  await page.evaluate(() => window.studyappEditor.load({ generation: -1, text: "invalid", readonly: false }));
  assert.match(await page.locator("#editor-status").innerText(), /无效/);
  assert.equal((await state(page)).text, "# Heading\n\nBody `code`\n\nEnd");
  await page.evaluate(() => window.studyappEditor.setTheme({ surface: "#ffffff", text: "#202020" }));
  assert.equal(await page.locator("#editor-theme-warning").isVisible(), false);
  assert.match(await page.locator("#editor-status").innerText(), /无效/);
  await page.close();
});

test("reading defaults and compact toolbar preserve content selection and undo", async () => {
  const text = "# Heading\n\n## Subheading\n\n### Section\n\n```js\nconst answer = 42;\n```\n\nEnd";
  const { page } = await setup(text);
  await select(page, text.length);
  await page.keyboard.insertText(" edited");
  const before = await state(page);
  const defaults = await page.evaluate(() => {
    const css = selector => getComputedStyle(document.querySelector(selector));
    return { toolbar: document.querySelector("#toolbar").getBoundingClientRect().height,
      width: css(":root").getPropertyValue("--editor-reading-width").trim(),
      font: css(".cm-scroller").fontFamily, lineHeight: css(".cm-scroller").lineHeight,
      body: css(".cm-editor").fontSize, h1: css(".md-heading-1").fontSize,
      h2: css(".md-heading-2").fontSize, h3: css(".md-heading-3").fontSize,
      code: css(".md-code-line").paddingLeft, mono: css(".md-code-line").fontFamily };
  });
  assert.equal(defaults.toolbar, 44);
  assert.equal(defaults.width, "100%");
  assert.ok(defaults.font.includes("Segoe UI Variable Text"));
  assert.equal(parseFloat(defaults.lineHeight) / parseFloat(defaults.body), 1.75);
  assert.equal(parseFloat(defaults.h1) / parseFloat(defaults.body), 1.85);
  assert.equal(parseFloat(defaults.h2) / parseFloat(defaults.body), 1.5);
  assert.equal(parseFloat(defaults.h3) / parseFloat(defaults.body), 1.25);
  assert.equal(defaults.code, "16px");
  assert.ok(defaults.mono.includes("Cascadia Mono"));
  await page.evaluate(() => window.studyappEditor.setTheme({ markdown: { reading_width: 920, line_height: 195 } }));
  assert.equal((await state(page)).theme.readingWidth, "920px");
  assert.deepEqual((await state(page)).selection, before.selection);
  assert.equal((await state(page)).text, before.text);
  await page.setViewportSize({ width: 280, height: 480 });
  await page.locator("#format-toggle").focus();
  await page.keyboard.press("ArrowDown");
  assert.equal(await page.locator("#format-toggle").getAttribute("aria-expanded"), "true");
  await page.keyboard.press("End");
  assert.equal(await page.locator("#format-menu").evaluate(menu => menu.contains(document.activeElement)), true);
  await page.keyboard.press("Escape");
  assert.equal(await page.locator("#format-toggle").getAttribute("aria-expanded"), "false");
  await page.evaluate(() => window.studyappEditor.command("undo"));
  assert.equal((await state(page)).text, text);
  await page.close();
});

test("document statistics follow edits and replacement without disturbing editing state", async () => {
  const { page } = await setup("中文 hello world 😀");
  await page.waitForFunction(() => document.querySelector("#editor-statistics")?.textContent.includes("4 字词"));
  assert.match(await page.locator("#editor-statistics").innerText(), /13 字符.*约 1 分钟/);
  await select(page, 2);
  await page.keyboard.insertText("字".repeat(399));
  const before = await state(page);
  await page.waitForFunction(() => document.querySelector("#editor-statistics")?.textContent.includes("403 字词"));
  assert.match(await page.locator("#editor-statistics").innerText(), /约 2 分钟/);
  assert.deepEqual((await state(page)).selection, before.selection);
  assert.equal((await state(page)).revision, before.revision);
  await page.evaluate(() => window.studyappEditor.load({ generation: 42, text: "new", readonly: true }));
  await page.waitForFunction(() => document.querySelector("#editor-statistics")?.textContent.includes("1 字词"));
  assert.match(await page.locator("#editor-statistics").innerText(), /3 字符.*约 1 分钟/);
  assert.equal((await state(page)).revision, 0);
  await page.close();
});

test("reading statistics count Unicode words and ignore whitespace without joining chunks", () => {
  assert.deepEqual(documentStatistics([]), { words: 0, characters: 0, readingMinutes: 1 });
  assert.deepEqual(documentStatistics(["中文 hello", " world 😀\r\n"]), { words: 4, characters: 13, readingMinutes: 1 });
  assert.deepEqual(documentStatistics(["𠀀中abc文 e\u0301 don't"]), { words: 6, characters: 13, readingMinutes: 1 });
  assert.deepEqual(documentStatistics(["字".repeat(401)]), { words: 401, characters: 401, readingMinutes: 2 });
});

function statisticsLabel(text) {
  const value = documentStatistics([text]);
  return `${value.words.toLocaleString("zh-CN")} 字词 · ${value.characters.toLocaleString("zh-CN")} 字符 · 约 ${value.readingMinutes} 分钟`;
}

async function controlIdleCallbacks(page) {
  await page.evaluate(() => {
    let next = 0;
    const pending = new Map(), canceled = [];
    window.requestIdleCallback = callback => { pending.set(++next, callback); return next; };
    window.cancelIdleCallback = id => {
      if (pending.has(id)) canceled.push(pending.get(id));
      pending.delete(id);
    };
    const deadline = { didTimeout: true, timeRemaining: () => 0 };
    window.statisticsIdle = {
      step() {
        const callbacks = [...pending.values()];
        pending.clear();
        callbacks.forEach(callback => callback(deadline));
      },
      drain() {
        for (let count = 0; pending.size && count < 10000; count++) this.step();
        if (pending.size) throw new Error("Idle work did not finish");
      },
      replayCanceled() { canceled.forEach(callback => callback(deadline)); return canceled.length; }
    };
  });
}

test("large document statistics yield during loading and match synchronous Unicode counts", async () => {
  const { page, errors } = await setup("old");
  await controlIdleCallbacks(page);
  // One long line exercises bounded work even when CodeMirror yields one chunk.
  // Astral Han and emoji around the block boundary must remain whole scalars.
  const text = ("字".repeat(2047) + "𠀀😀 e\u0301 don't café 中文 ").repeat(680);
  const expected = statisticsLabel(text);
  const immediate = await page.evaluate(text => {
    window.studyappEditor.setSource(true);
    window.studyappEditor.load({ generation: 42, text, readonly: true });
    return document.querySelector("#editor-statistics").textContent;
  }, text);
  assert.match(immediate, /统计/, "loading must defer the full scan");
  const afterYield = await page.evaluate(async () => {
    window.statisticsIdle.step();
    await new Promise(resolve => setTimeout(resolve, 0));
    return document.querySelector("#editor-statistics").textContent;
  });
  assert.match(afterYield, /统计/, "the event loop must run before the full scan finishes");
  await page.evaluate(() => window.statisticsIdle.drain());
  assert.equal(await page.locator("#editor-statistics").innerText(), expected);
  assert.deepEqual(errors, []);
  await page.close();
});

for (const change of ["input", "document replacement"]) {
  test(`pending document statistics are canceled after ${change}`, async () => {
    const { page, errors } = await setup("old");
    await controlIdleCallbacks(page);
    await page.evaluate(() => {
      window.studyappEditor.setSource(true);
      window.studyappEditor.load({ generation: 42, text: "中文 hello 𠀀😀 ".repeat(20000), readonly: false });
      window.statisticsIdle.step();
    });
    const replacement = "new input 中文";
    if (change === "input") {
      await page.evaluate(text => window.studyappEditor.test({ type: "replace", from: 0, to: Number.MAX_SAFE_INTEGER, text }), replacement);
    } else {
      await page.evaluate(text => window.studyappEditor.load({ generation: 43, text, readonly: true }), replacement);
    }
    const canceled = await page.evaluate(() => window.statisticsIdle.replayCanceled());
    assert.ok(canceled > 0, "superseded work must be canceled before its next slice");
    assert.doesNotMatch(await page.locator("#editor-statistics").innerText(), /80,000 字词/);
    await page.waitForTimeout(160);
    await page.evaluate(() => window.statisticsIdle.drain());
    assert.equal(await page.locator("#editor-statistics").innerText(), statisticsLabel(replacement));
    await page.evaluate(() => window.statisticsIdle.replayCanceled());
    assert.equal(await page.locator("#editor-statistics").innerText(), statisticsLabel(replacement), "a late canceled callback cannot publish stale counts");
    assert.deepEqual(errors, []);
    await page.close();
  });
}

test("returning to a cached empty document restores its completed statistics", async () => {
  const { page } = await setup("");
  await page.waitForFunction(() => document.querySelector("#editor-statistics").textContent.includes("0 字词"));
  await controlIdleCallbacks(page);
  const restored = await page.evaluate(() => {
    window.studyappEditor.load({ generation: 42, text: "字".repeat(10000), readonly: true });
    window.studyappEditor.load({ generation: 43, text: "", readonly: true });
    return document.querySelector("#editor-statistics").textContent;
  });
  assert.equal(restored, statisticsLabel(""), "CodeMirror reuses Text.empty; cached counts must replace the pending label");
  await page.evaluate(() => { window.statisticsIdle.replayCanceled(); window.statisticsIdle.drain(); });
  assert.equal(await page.locator("#editor-statistics").innerText(), statisticsLabel(""));
  await page.close();
});

test("link and table toolbar actions insert editable Markdown with isolated undo", async () => {
  const { page } = await setup("学习[链接]");
  await select(page, 0, 6);
  await page.getByRole("button", { name: "插入链接", exact: true }).click();
  assert.equal((await state(page)).text, "[学习\\[链接\\]](https://)");
  assert.equal(await page.evaluate(async () => {
    const value = await window.studyappEditor.test({ type: "snapshot" });
    return value.text.slice(value.selection.anchor, value.selection.head);
  }), "https://");
  await page.evaluate(() => window.studyappEditor.command("undo"));
  assert.equal((await state(page)).text, "学习[链接]");
  await select(page, 6);
  await page.getByRole("button", { name: "插入表格", exact: true }).click();
  assert.match((await state(page)).text, /^学习\[链接\]\n\n\| 列 1 \| 列 2 \|\n\| --- \| --- \|/);
  await page.evaluate(() => window.studyappEditor.command("undo"));
  assert.equal((await state(page)).text, "学习[链接]");
  await page.evaluate(() => window.studyappEditor.setReadonly(true));
  assert.equal(await page.getByRole("button", { name: "插入表格", exact: true }).isDisabled(), true);
  await page.close();
});

test("saving rebases image references without losing selection scroll or undo", async () => {
  const original = "![first](old-a.png)\n\n" + Array.from({ length: 180 }, (_, i) => `A visible paragraph ${i} 🧠\n\n`).join("") + "![last](old-b.png)\n";
  const { page, errors } = await setup(original);
  const caret = original.indexOf("paragraph", 1000) + 4;
  await select(page, caret);
  await page.keyboard.type("kept-edit");
  const edited = (await state(page)).text;
  await page.evaluate(() => { document.querySelector('.cm-scroller').scrollTop = 2400; });
  await page.waitForTimeout(100);
  const before = await state(page);
  const visibleAnchor = () => {
    const viewport = document.querySelector('.cm-scroller').getBoundingClientRect();
    const line = [...document.querySelectorAll('.cm-line')].find(line => line.textContent.trim() && line.getBoundingClientRect().top >= viewport.top);
    return { text: line.textContent, top: line.getBoundingClientRect().top - viewport.top, scrollTop: document.querySelector('.cm-scroller').scrollTop };
  };
  const anchor = await page.evaluate(visibleAnchor);
  await page.evaluate(() => window.studyappEditor.flush({ request: 77 }));
  const frozenAnchor = await page.evaluate(visibleAnchor);
  const first = edited.indexOf("old-a.png"), last = edited.indexOf("old-b.png");
  const changes = [
    { from: first, to: first + "old-a.png".length, insert: "studyapp-media:group/first.png" },
    { from: last, to: last + "old-b.png".length, insert: "studyapp-media:group/last.png" }
  ];
  const target = edited.replace("old-a.png", changes[0].insert).replace("old-b.png", changes[1].insert);
  const revision = (await state(page)).revision;
  await page.evaluate(value => window.studyappEditor.rebase(value), {
    previousGeneration: 41, previousRevision: revision, generation: 42, text: target, readonly: true, changes
  });
  await page.waitForTimeout(100);
  const rebased = await state(page);
  const shift = changes[0].insert.length - "old-a.png".length;
  assert.equal(rebased.text, target);
  assert.equal(rebased.readonly, true);
  assert.deepEqual(rebased.selection, { anchor: before.selection.anchor + shift, head: before.selection.head + shift });
  const savedAnchor = await page.evaluate(visibleAnchor);
  assert.equal(savedAnchor.text, anchor.text, JSON.stringify({ anchor, savedAnchor }));
  assert.ok(Math.abs(savedAnchor.top - anchor.top) < 5, JSON.stringify({ anchor, frozenAnchor, savedAnchor }));
  await page.evaluate(() => window.studyappEditor.setReadonly(false));
  await action(page, { type: "command", name: "undo" });
  const undone = await state(page);
  assert.equal(undone.text, original.replace("old-a.png", changes[0].insert).replace("old-b.png", changes[1].insert));
  assert.equal(undone.generation, 42);
  assert.deepEqual(errors, []);
  await page.close();
});

async function pasteFiles(page, sizes = [3, 3, 3]) {
  return page.evaluate(sizes => {
    const clipboard = new DataTransfer();
    sizes.forEach((size, index) => clipboard.items.add(new File([new Uint8Array(size).fill(index + 1)], `image-${index}.png`, { type: "image/png" })));
    const event = new ClipboardEvent("paste", { clipboardData: clipboard, bubbles: true, cancelable: true });
    document.querySelector(".cm-content").dispatchEvent(event);
    return event.defaultPrevented;
  }, sizes);
}

test("image clipboard paste preserves file order while reading only one image at a time", async () => {
  const { page, errors } = await setup("keep text");
  await page.evaluate(() => {
    const read = FileReader.prototype.readAsDataURL;
    window.readerStarts = [];
    FileReader.prototype.readAsDataURL = function (file) {
      window.readerStarts.push(file.name);
      setTimeout(() => read.call(this, file), file.name === "image-0.png" ? 120 : 0);
    };
  });
  assert.equal(await pasteFiles(page), true);
  await page.waitForTimeout(30);
  assert.deepEqual(await page.evaluate(() => window.readerStarts), ["image-0.png"]);
  await page.waitForFunction(() => window.messages.filter(message => message.type === "pasteImage").length === 3);
  assert.deepEqual(await page.evaluate(() => window.messages.filter(message => message.type === "pasteImage").map(message => [message.name, message.base64, message.generation])), [
    ["image-0.png", "AQEB", 41], ["image-1.png", "AgIC", 41], ["image-2.png", "AwMD", 41]
  ]);
  assert.equal((await state(page)).text, "keep text");
  assert.deepEqual(errors, []);
  await page.close();
});

test("image clipboard paste rejects oversized files and bounded batches before reading", async () => {
  const { page, errors } = await setup("keep text");
  await page.evaluate(() => {
    window.readerStarts = 0;
    const read = FileReader.prototype.readAsDataURL;
    FileReader.prototype.readAsDataURL = function (file) { window.readerStarts++; read.call(this, file); };
  });
  for (const sizes of [[10 * 1024 * 1024 + 1], Array(21).fill(1), Array(3).fill(8 * 1024 * 1024)]) {
    assert.equal(await pasteFiles(page, sizes), true);
    assert.equal(await page.evaluate(() => window.readerStarts), 0);
  }
  assert.equal((await state(page)).text, "keep text");
  assert.deepEqual(errors, []);
  await page.close();
});

test("image clipboard paste discards late readers and queued files after a document switch", async () => {
  const { page, errors } = await setup("old document");
  await page.evaluate(() => {
    const read = FileReader.prototype.readAsDataURL;
    window.readerStarts = [];
    FileReader.prototype.readAsDataURL = function (file) {
      window.readerStarts.push(file.name);
      setTimeout(() => read.call(this, file), 100);
    };
  });
  await pasteFiles(page);
  await page.evaluate(() => window.studyappEditor.load({ generation: 42, text: "new document", readonly: false }));
  await page.waitForTimeout(180);
  assert.deepEqual(await page.evaluate(() => window.messages.filter(message => message.type === "pasteImage")), []);
  assert.deepEqual(await page.evaluate(() => window.readerStarts), ["image-0.png"]);
  assert.equal((await state(page)).text, "new document");
  assert.deepEqual(errors, []);
  await page.close();
});

test("image clipboard handler leaves normal text paste unchanged", async () => {
  const { page, errors } = await setup("before ");
  await select(page, 7);
  await page.evaluate(() => {
    const clipboard = new DataTransfer();
    clipboard.setData("text/plain", "pasted 文本");
    document.querySelector(".cm-content").dispatchEvent(new ClipboardEvent("paste", { clipboardData: clipboard, bubbles: true, cancelable: true }));
  });
  assert.equal((await state(page)).text, "before pasted 文本");
  assert.deepEqual(await page.evaluate(() => window.messages.filter(message => ["pasteImage", "requestNativeImagePaste"].includes(message.type))), []);
  assert.deepEqual(errors, []);
  await page.close();
});

test("image clipboard native fallback ignores synthetic empty paste events", async () => {
  const { page, errors } = await setup("keep text");
  await select(page, 9);
  await page.evaluate(() => {
    const clipboard = new DataTransfer();
    document.querySelector(".cm-content").dispatchEvent(new ClipboardEvent("paste", { clipboardData: clipboard, bubbles: true, cancelable: true }));
  });
  assert.deepEqual(await page.evaluate(() => window.messages.filter(message => ["pasteImage", "requestNativeImagePaste"].includes(message.type))), []);
  assert.equal((await state(page)).text, "keep text");
  assert.deepEqual(errors, []);
  await page.close();
});

test("image clipboard paste bounds overlapping paste events and resumes after a read error", async () => {
  const { page, errors } = await setup("keep text");
  await page.evaluate(() => {
    const read = FileReader.prototype.readAsDataURL;
    window.readerStarts = [];
    let first = true;
    FileReader.prototype.readAsDataURL = function (file) {
      window.readerStarts.push(file.name);
      if (first) {
        first = false;
        setTimeout(() => this.dispatchEvent(new Event("error")), 120);
      } else read.call(this, file);
    };
  });
  await pasteFiles(page, Array(20).fill(1));
  await pasteFiles(page, [1]);
  await page.waitForFunction(() => window.messages.filter(message => message.type === "pasteImage").length === 19);
  assert.equal(await page.evaluate(() => window.readerStarts.length), 20);
  assert.deepEqual(await page.evaluate(() => window.messages.filter(message => message.type === "pasteImage").map(message => message.name)), Array.from({ length: 19 }, (_, index) => `image-${index + 1}.png`));
  await pasteFiles(page, [1]);
  await page.waitForFunction(() => window.messages.filter(message => message.type === "pasteImage").length === 20);
  assert.equal((await state(page)).text, "keep text");
  assert.deepEqual(errors, []);
  await page.close();
});

test("semantic themes retain an aligned editable table and nested list geometry", async () => {
  const text = "# Reading\n\n| Name | Count |\n| :--- | ---: |\n| alpha | 123 |\n| beta | 4 |\n\n- first\n  - child\n    - grandchild\n\n```js\nconst value = 42;\n```";
  const {page, errors} = await setup(text);
  await select(page,text.indexOf("alpha")+2);
  const rows = await page.locator(".md-table-row").evaluateAll(rows => rows.map(row => [...row.querySelectorAll(".md-table-cell")].map(cell => ({left:cell.getBoundingClientRect().left,width:cell.getBoundingClientRect().width,align:getComputedStyle(cell).textAlign}))));
  assert.ok(rows.every(row => row.length === 2),"editing one row must preserve every table cell");
  for (const row of rows) { assert.ok(Math.abs(row[1].left-rows[0][1].left)<2); assert.equal(row[1].align,"right"); }
  const depth = await page.locator(".md-list-line").evaluateAll(lines => lines.map(line => ({depth:Number(line.dataset.listDepth),padding:parseFloat(getComputedStyle(line).paddingLeft)})));
  assert.deepEqual(depth.map(item => item.depth),[1,2,3]);
  assert.ok(depth[2].padding>depth[1].padding && depth[1].padding>depth[0].padding);
  await page.evaluate(() => window.studyappEditor.setTheme({fontSize:20,markdown:{code_size:18,line_height:190,reading_width:700,body_font:"serif",mono_font:"mono",colors:{heading:"#102030",code:"#102030",code_background:"#ffffff"},table_grid:true,table_stripes:true,code_border:true,code_radius:12}}));
  assert.equal(await page.locator(".md-code-line").first().evaluate(line => getComputedStyle(line).fontSize),"18px");
  assert.equal(await page.locator(".md-code-start").count(),1);
  assert.equal(await page.locator(".md-code-end").count(),1);
  assert.equal((await state(page)).text,text);
  assert.deepEqual(errors,[]);
  await page.close();
});

test("semantic reading fixtures remain readable in light dark and two custom themes", async () => {
  const text = "# 清晰阅读与自然编辑\n\n正文包含 **重点**、*强调*、[延伸阅读](https://example.test) 和 `inline code`。\n\n> 引用文字与背景保持可读。\n\n- 一级要点\n  1. 二级编号\n     - [x] 三级任务\n       延续任务内容\n\n| 名称 | 状态 | 数量 |\n| :--- | :---: | ---: |\n| alpha | 完成 | 123 |\n| beta | 进行中 | 4 |\n\n```javascript\nconst answer = 'readable'; // annotation\nfunction study() { return 42; }\n```";
  const {page,errors} = await setup(text);
  await page.setViewportSize({width:1100,height:1000});
  await select(page,text.indexOf("alpha")+2);
  const themes = [
    ["light",{mode:"light",background:"#F4F5F7",surface:"#FFFFFF",text:"#24272D",accent:"#2478D4",muted:"#AAAAAA"}],
    ["dark",{mode:"dark",background:"#1D1F23",surface:"#272A30",text:"#EFF0F3",accent:"#70ACF7",muted:"#999999"}],
    ["custom-paper",{mode:"light",background:"#FFF8E7",surface:"#F5EEDB",text:"#393229",accent:"#655436",markdown:{body_font:"serif",body_size:17,code_size:16,line_height:185,reading_width:850,code_radius:12,colors:{heading:"#5B3D23",link:"#365F60",quote_background:"#ECE5D4",code_background:"#E8E1D0",code:"#272C33",table_stripe:"#EEE7D5",list:"#59482F"}}}],
    ["custom-ink",{mode:"dark",background:"#111827",surface:"#1E293B",text:"#E2E8F0",accent:"#7DD3FC",markdown:{body_size:16,code_size:15,line_height:180,reading_width:820,table_grid:true,table_stripes:true,colors:{heading:"#BAE6FD",link:"#7DD3FC",quote:"#CBD5E1",quote_background:"#263449",code_background:"#0B1220",code:"#E2E8F0",table_stripe:"#263449",syntax_keyword:"#C4B5FD",syntax_string:"#86EFAC"}}}],
  ];
  const directory=fileURLToPath(new URL("../test-results/",import.meta.url));await mkdir(directory,{recursive:true});
  for (const [name,theme] of themes) {
    await page.evaluate(theme=>window.studyappEditor.setTheme(theme),theme);
    await page.waitForTimeout(60);
    const samples=await page.evaluate(()=>{
      const background=node=>{for(let item=node;item;item=item.parentElement){const value=getComputedStyle(item).backgroundColor;if(value!=="rgba(0, 0, 0, 0)"&&value!=="transparent")return value;}return "rgb(255, 255, 255)";};
      return [...document.querySelectorAll('.md-heading,.md-link,.md-quote,.md-inline-code,.md-table-cell,.md-bullet,.md-list-marker,.md-task[aria-checked="true"],.md-code-line,[class*="tok-"]')].map(node=>({class:node.className,foreground:getComputedStyle(node).color,background:background(node)}));
    });
    const luminance=css=>css.match(/[\d.]+/g).slice(0,3).map(Number).map(v=>v/255).map(v=>v<=.04045?v/12.92:((v+.055)/1.055)**2.4).reduce((sum,v,i)=>sum+v*[.2126,.7152,.0722][i],0);
    assert.ok(samples.length>20);
    for(const sample of samples){const a=luminance(sample.foreground),b=luminance(sample.background);assert.ok((Math.max(a,b)+.05)/(Math.min(a,b)+.05)>=4.5,JSON.stringify({name,sample}));}
    const cells=await page.locator('.md-table-row').evaluateAll(rows=>rows.map(row=>[...row.querySelectorAll('.md-table-cell')].map(cell=>({left:cell.getBoundingClientRect().left,width:cell.getBoundingClientRect().width,align:getComputedStyle(cell).textAlign}))));
    assert.ok(cells.every(row=>row.length===3));
    for(const row of cells){assert.ok(Math.abs(row[1].left-cells[0][1].left)<2);assert.equal(row[1].align,"center");assert.equal(row[2].align,"right");}
    const caretBox=await page.evaluate(() => { const rect = getSelection().getRangeAt(0).getBoundingClientRect(); return {x:rect.x,y:rect.y}; });
    const activeCell=await page.locator('.md-table-row').nth(1).locator('.md-table-cell').first().boundingBox();
    assert.ok(caretBox.x>=activeCell.x && caretBox.y>=activeCell.y && caretBox.y<activeCell.y+activeCell.height,`${name}: caret must follow changed reading geometry`);
    await page.screenshot({path:directory+`product-theme-${name}.png`});
  }
  const cell=page.locator('.md-table-row').nth(1).locator('.md-table-cell').first();
  await cell.click({position:{x:35,y:15}});
  const caret=(await state(page)).selection.anchor;
  assert.ok(caret>=text.indexOf("alpha")-1 && caret<=text.indexOf("alpha")+6,"real click must resolve within its source cell");
  await page.keyboard.insertText("X");
  assert.equal((await state(page)).text,text.slice(0,caret)+"X"+text.slice(caret));
  await page.keyboard.press("Control+z");
  assert.equal((await state(page)).text,text);
  assert.deepEqual(errors,[]);await page.close();
});

test("empty and escaped table cells preserve shared columns and source selection",async()=>{
  const text="| Left || Right |\n| :--- | :---: | ---: |\n|a||3|\n| escaped \\| pipe | `code` | 4 |\n";
  const {page,errors}=await setup(text);await select(page,text.indexOf("|a||")+2);
  const rows=await page.locator('.md-table-row').evaluateAll(rows=>rows.map(row=>[...row.querySelectorAll('.md-table-cell')].map(cell=>({text:cell.textContent,left:cell.getBoundingClientRect().left,align:getComputedStyle(cell).textAlign}))));
  assert.equal(rows[1].length,3);assert.equal(rows[1][1].text,"");assert.equal(rows[1][2].text,"3");
  assert.equal(rows[0].length,3);assert.equal(rows[0][1].text,"");
  const overflow=await page.locator('.md-table-row').evaluateAll(rows=>rows.some(row=>[...row.querySelectorAll('.md-table-cell')].some(cell=>cell.getBoundingClientRect().right>row.getBoundingClientRect().right+1)));
  assert.equal(overflow,false,"empty headers must not make the declared columns overflow the table");
  assert.equal(rows[2].length,3);assert.equal(rows[2][2].align,"right");
  for(const row of rows) assert.ok(Math.abs(row[2].left-rows[0][2].left)<2);
  assert.equal((await state(page)).text,text);assert.deepEqual(errors,[]);await page.close();
});

test("custom code surfaces keep language labels and source highlighting readable",async()=>{
  const text="# Plain paper\n\n```js\nconst value = 'text'; // comment\n```";
  const {page}=await setup(text);await select(page,0);
  await page.evaluate(()=>window.studyappEditor.setTheme({background:"#FFFFFF",surface:"#FFFFFF",text:"#24272D",markdown:{colors:{code_background:"#111827",code:"#EEEEEE"}}}));
  const ratio=(a,b)=>{const l=css=>css.match(/[\d.]+/g).slice(0,3).map(v=>Number(v)/255).map(v=>v<=.04045?v/12.92:((v+.055)/1.055)**2.4).reduce((s,v,i)=>s+v*[.2126,.7152,.0722][i],0);const x=l(a),y=l(b);return(Math.max(x,y)+.05)/(Math.min(x,y)+.05);};
  const label=await page.locator('.md-code-language').evaluate(node=>({fg:getComputedStyle(node).color,bg:getComputedStyle(node.closest('.md-code-line')).backgroundColor}));
  assert.ok(ratio(label.fg,label.bg)>=4.5);
  await page.evaluate(()=>window.studyappEditor.setSource(true));
  const colors=await page.locator('[class*="tok-"]').evaluateAll(nodes=>nodes.map(node=>getComputedStyle(node).color));
  assert.ok(colors.length>3);for(const color of colors)assert.ok(ratio(color,"rgb(255,255,255)")>=4.5,`source highlight ${color}`);
  assert.equal((await state(page)).text,text);await page.close();
});
function replayRawChanges(raw, messages, preferredEol) {
  for (const [index, message] of messages.entries()) {
    assert.ok(message.changes.length <= 4096, `emitted ${message.changes.length} patches`);
    assert.equal(message.baseRevision, index);
    assert.equal(message.revision, index + 1);
    const starts = [0];
    for (const match of raw.matchAll(/\r\n|\r|\n/g)) starts.push(match.index + match[0].length);
    const edits = message.changes.map(change => ({
      from: starts[change.fromLine - 1] + change.fromColumn - 1,
      to: starts[change.toLine - 1] + change.toColumn - 1,
      text: change.text.replace(/\n/g, preferredEol)
    }));
    for (const edit of edits.reverse()) raw = raw.slice(0, edit.from) + edit.text + raw.slice(edit.to);
    assert.ok(raw.isWellFormed(), "native revision must remain scalar-valid");
    assert.ok(Buffer.byteLength(raw) <= 10 * 1024 * 1024, "native revision exceeds the byte budget");
  }
  return raw;
}

test("fence language control follows actual code focus across title and toolbar focus", async () => {
  const text = "```js\nconst answer = 42;\n```\n\nAfter code";
  const { page, errors } = await setup(text);
  const picker = page.getByRole("combobox", { name: "代码语言" });
  const code = page.locator(".cm-line").filter({ hasText: "const answer = 42;" });
  await select(page, text.indexOf("answer"), text.indexOf("answer") + 6);
  await picker.waitFor({ state: "visible" });
  const original = await state(page);
  // A real external input models the native note-title focus boundary without
  // changing production markup or interacting with the desktop window.
  await page.evaluate(() => {
    const title = document.createElement("input");
    title.id = "test-note-title";
    title.setAttribute("aria-label", "笔记标题焦点测试");
    title.style.cssText = "position:fixed;top:0;left:600px;width:180px;height:28px;z-index:100";
    document.body.append(title);
  });
  await page.getByRole("textbox", { name: "笔记标题焦点测试" }).click();
  await page.locator(".cm-editor:not(.cm-focused)").waitFor({ state: "attached" });
  assert.equal(await picker.count(), 0, "leaving the code for the note title must hide its language control");
  assert.deepEqual((await state(page)).selection, original.selection, "title focus must not move the code selection");
  await code.click();
  await picker.waitFor({ state: "visible" });
  await picker.focus();
  await page.locator(".cm-editor:not(.cm-focused)").waitFor({ state: "attached" });
  assert.equal(await picker.count(), 1, "focusing the language input must retain its code block");
  await page.getByRole("textbox", { name: "笔记标题焦点测试" }).click();
  await picker.waitFor({ state: "detached" });
  assert.equal(await picker.count(), 0, "leaving the language input without edits must also hide its control");
  await code.click();
  await picker.waitFor({ state: "visible" });
  const beforeToolbar = await state(page);
  // Pointer formatting deliberately retains content focus. Keyboard toolbar
  // focus is a real blur and must not leave a stale code-language control.
  await page.locator("#format-toggle").focus();
  await page.locator(".cm-editor:not(.cm-focused)").waitFor({ state: "attached" });
  assert.equal(await picker.count(), 0, "moving focus to the toolbar must hide the language control");
  assert.deepEqual((await state(page)).selection, beforeToolbar.selection);
  await code.click();
  await picker.waitFor({ state: "visible" });
  assert.equal((await state(page)).text, text, "focus transitions must not edit the document");
  assert.deepEqual(errors, []);
  await page.close();
});

test("fence language control keeps its own input and candidates while code selection is retained", async () => {
  const text = "```js\nconst answer = 42;\n```\n\nAfter code";
  const { page, errors } = await setup(text);
  await select(page, text.indexOf("answer"), text.indexOf("answer") + 6);
  const picker = page.getByRole("combobox", { name: "代码语言" });
  await picker.fill("py");
  await page.locator(".cm-editor:not(.cm-focused)").waitFor({ state: "attached" });
  assert.equal(await picker.count(), 1, "the language input is part of the active code block");
  assert.equal(await picker.inputValue(), "py", "focus updates must retain the uncommitted language draft");
  assert.equal(await picker.evaluate(input => document.activeElement === input), true);
  assert.equal(await page.getByRole("option").first().textContent(), "py");
  await picker.press("ArrowDown");
  await picker.press("Enter");
  const changed = await state(page);
  assert.equal(changed.text, text.replace("```js", "```py"));
  assert.equal(changed.text.slice(changed.selection.anchor, changed.selection.head), "answer");
  assert.equal(changed.editorFocused, true);
  assert.deepEqual(errors, []);
  await page.close();
});

test("contextual fence language input ranks completions and preserves metadata, CRLF and history", async () => {
  const raw = "开头\r\n\r\n~~~~mystery  extra=yes \r\nconst answer = 42;\r\n~~~~\r\n\r\n```\r\nprint('hi')\r\n```";
  const { page, errors } = await setup(raw);
  const picker = page.getByRole("combobox", { name: "代码语言" });
  assert.equal(await picker.count(), 0);
  await select(page, raw.replaceAll("\r", "").indexOf("answer"));
  assert.equal(await picker.count(), 1);
  assert.equal(await picker.inputValue(), "mystery");
  const original = await state(page);
  await picker.fill("JaVa");
  const options = page.getByRole("option");
  assert.ok(await options.count() <= 5);
  assert.equal(await options.first().textContent(), "java");
  await picker.fill("JAVASCRIPT");
  await picker.press("Enter");
  const changed = await state(page);
  assert.equal(changed.text, original.text.replace("mystery", "javascript"));
  assert.equal(changed.text.slice(changed.selection.anchor - 6, changed.selection.anchor), original.text.slice(original.selection.anchor - 6, original.selection.anchor));
  assert.equal(await page.locator(".cm-content").evaluate(el => el === document.activeElement), true);
  const messages = await page.evaluate(() => window.messages.filter(m => m.type === "change"));
  assert.equal(replayRawChanges(raw, messages, "\r\n"), raw.replace("mystery", "javascript"));
  await page.keyboard.press("Control+z");
  assert.equal((await state(page)).text, original.text);
  await picker.fill("unsupported-language");
  await picker.press("Enter");
  assert.equal((await state(page)).text, original.text.replace("mystery", ""));
  assert.equal(await picker.inputValue(), "");
  await picker.fill("PY");
  await picker.press("Tab");
  assert.equal((await state(page)).text, original.text.replace("mystery", "py"));
  await select(page, 0);
  assert.equal(await picker.count(), 0);
  assert.deepEqual(errors, []);
  await page.close();
});

test("metadata only fences separate inserted languages while preserving whitespace CRLF and undo", async () => {
  for (const gap of ["", "  "]) {
    const raw = "```" + gap + "{.numberLines}\r\nconst answer = 42;\r\n```";
    const original = raw.replaceAll("\r", "");
    const { page, errors } = await setup(raw);
    await select(page,original.indexOf("answer"));
    const picker = page.getByRole("combobox", {name:"代码语言"});
    assert.equal(await picker.inputValue(),"");
    await picker.fill("javascript");
    await picker.press("Enter");
    const expected = "```javascript" + (gap || " ") + "{.numberLines}\nconst answer = 42;\n```";
    assert.equal((await state(page)).text,expected);
    assert.equal(await picker.inputValue(),"javascript");
    assert.ok(await page.locator(".tok-keyword").count() > 0,"inserted language must be recognized by the code parser");
    const messages = await page.evaluate(() => window.messages.filter(m => m.type === "change"));
    assert.equal(replayRawChanges(raw,messages,"\r\n"),expected.replaceAll("\n","\r\n"));
    await page.keyboard.press("Control+z");
    assert.equal((await state(page)).text,original);
    assert.deepEqual(errors,[]);
    await page.close();
  }
});

test("language draft cancellation and keys stay local, preserving code selection", async () => {
  const text = "```js\nconst answer = 42;\n```";
  const { page } = await setup(text);
  await select(page, text.indexOf("answer"), text.indexOf("answer") + 6);
  const picker = page.getByRole("combobox", { name: "代码语言" });
  await picker.fill("python");
  await picker.press("Control+s");
  await picker.press("Escape");
  assert.equal((await state(page)).text, text);
  assert.deepEqual(await page.evaluate(() => window.messages.filter(m => ["save", "shortcut", "cancelResize"].includes(m.type))), []);
  await picker.fill("ts");
  await picker.press("Enter");
  const snapshot = await state(page);
  assert.equal(snapshot.text.slice(snapshot.selection.anchor, snapshot.selection.head), "answer");
  await page.keyboard.press("Control+s");
  assert.equal(await page.evaluate(() => window.messages.filter(m => m.type === "save").length), 1);
  await page.close();
});

test("headings replace levels and paragraph clears them through configurable shortcuts", async () => {
  const { page } = await setup("## Title\n\n- [ ] item");
  await select(page, 5);
  await page.keyboard.press("Control+6");
  assert.match((await state(page)).text, /^###### Title/);
  await page.keyboard.press("Control+0");
  assert.match((await state(page)).text, /^Title/);
  await shortcuts(page, { "editor.heading-1": "Ctrl+Alt+H" });
  await page.keyboard.press("Control+1");
  assert.match((await state(page)).text, /^Title/);
  await page.keyboard.press("Control+Alt+h");
  assert.match((await state(page)).text, /^# Title/);
  await page.close();
});

function listDepth(text, content) {
  let depth = 0;
  for (let node = markdownParser.parse(text).resolveInner(text.indexOf(content), 1); node; node = node.parent) {
    if (node.name === "ListItem") depth++;
  }
  return depth;
}

test("Markdown indentation uses ordered marker width and moves selected subtrees with CRLF history", async () => {
  for (const marker of ["1.", "10."]) {
    const raw = `${marker} parent\r\n2. child\r\n   - grandchild\r\n3. sibling\r\n4. end`;
    const original = raw.replaceAll("\r", "");
    const { page } = await setup(raw);
    await select(page, original.indexOf("2. child"), original.indexOf("4. end"));
    await page.keyboard.press("Tab");
    const nested = (await state(page)).text;
    assert.equal(listDepth(nested, "parent"), 1);
    assert.equal(listDepth(nested, "child"), 2, `${marker} parent must contain child`);
    assert.equal(listDepth(nested, "grandchild"), 3);
    assert.equal(listDepth(nested, "sibling"), 2);
    assert.equal(listDepth(nested, "end"), 1);
    const messages = await page.evaluate(() => window.messages.filter(m => m.type === "change"));
    assert.equal(replayRawChanges(raw, messages, "\r\n"), nested.replaceAll("\n", "\r\n"));
    await page.keyboard.press("Shift+Tab");
    assert.equal((await state(page)).text, original);
    await page.keyboard.press("Control+z");
    assert.equal((await state(page)).text, nested);
    await page.keyboard.press("Control+z");
    assert.equal((await state(page)).text, original);
    await page.close();
  }
});

test("configurable indentation shares Markdown nesting and readonly semantics with Tab", async () => {
  const original = "10. parent\n2. child\n   - grandchild\n3. end";
  const { page } = await setup(original);
  await shortcuts(page, { "editor.indent-more": "Ctrl+Alt+I", "editor.indent-less": "Ctrl+Alt+O" });
  await select(page, original.indexOf("child"));
  await page.keyboard.press("Control+Alt+i");
  const nested = (await state(page)).text;
  assert.equal(listDepth(nested, "child"), 2);
  assert.equal(listDepth(nested, "grandchild"), 3, "cursor indentation must move the item's subtree");
  await page.keyboard.press("Control+Alt+o");
  assert.equal((await state(page)).text, original);
  await page.evaluate(() => window.studyappEditor.setReadonly(true));
  const before = await state(page);
  await page.keyboard.press("Control+Alt+i");
  await page.keyboard.press("Tab");
  assert.equal((await state(page)).text, before.text);
  assert.equal((await state(page)).revision, before.revision);
  await page.close();
});

test("Markdown indentation nests mixed list markers beneath their preceding ordered parent", async () => {
  const original = "10. parent\n- [ ] child\n  - grandchild\n- end";
  const { page } = await setup(original);
  await select(page,original.indexOf("child"));
  await page.keyboard.press("Tab");
  const nested = (await state(page)).text;
  assert.equal(listDepth(nested,"child"),2);
  assert.equal(listDepth(nested,"grandchild"),3);
  assert.equal(listDepth(nested,"end"),1);
  await page.keyboard.press("Shift+Tab");
  assert.equal((await state(page)).text,original);
  await page.close();
});

test("fenced code remains readable for resolved dark and custom surfaces", async () => {
  const { page } = await setup("```unknown\nplain text\n```\n\n```js\nconst answer = 'value'; // comment\n```\n\n```python\ndef f(x):\n    return 42\n```");
  for (const theme of [
    {mode:"dark",background:"#1D1F23",surface:"#272A30",text:"#EFF0F3",codeBackground:"#272A30",codeText:"#EFF0F3"},
    {mode:"light",background:"#FFFFFF",surface:"#FFFFFF",text:"#24272D",codeBackground:"#FFFFFF",codeText:"#24272D"},
    {mode:"dark",background:"#101010",surface:"#666666",text:"#FFFFFF",codeBackground:"#666666",codeText:"#FFFFFF"}
  ]) {
    await page.evaluate(theme => window.studyappEditor.setTheme(theme), theme);
    const samples = await page.locator(".md-code-line").evaluateAll(lines => lines.flatMap(line => {
      const background = getComputedStyle(line).backgroundColor;
      return [line, ...line.querySelectorAll('[class*="tok-"]')].map(node => ({foreground:getComputedStyle(node).color, background}));
    }));
    assert.ok(samples.length > 10, "include plain code and highlighted language tokens");
    const luminance = css => {
      const channels = css.match(/\d+/g).slice(0,3).map(value => Number(value)/255).map(value => value <= .04045 ? value/12.92 : ((value+.055)/1.055)**2.4);
      return channels[0]*.2126 + channels[1]*.7152 + channels[2]*.0722;
    };
    for (const sample of samples) {
      const a = luminance(sample.foreground), b = luminance(sample.background);
      assert.ok((Math.max(a,b)+.05)/(Math.min(a,b)+.05) >= 4.5, JSON.stringify({theme,sample}));
    }
  }
  await page.close();
});

test("Tab nests ordered unordered task lists and code, selection and undo survive", async () => {
  const text = "- first\n1. second\n- [ ] third";
  const { page } = await setup(text);
  await select(page, 0, text.length);
  await page.keyboard.press("Tab");
  assert.equal((await state(page)).text, "  - first\n  1. second\n  - [ ] third");
  await page.keyboard.press("Shift+Tab");
  assert.equal((await state(page)).text, text);
  await page.keyboard.press("Control+z");
  assert.equal((await state(page)).text, "  - first\n  1. second\n  - [ ] third");
  await page.evaluate(() => window.studyappEditor.load({generation:42,text:"```js\nconst x = 1;\n```",readonly:false}));
  await select(page, 6);
  await page.keyboard.press("Tab");
  assert.match((await state(page)).text, /\n  const/);
  await page.close();
});

test("managed readonly body click requests UTF16 caret only and theme values are safe", async () => {
  const { page } = await setup("Hello 😀 text");
  await page.evaluate(() => window.studyappEditor.load({generation:77,text:"Hello 😀 text",readonly:true,canRequestEdit:true}));
  await page.locator(".cm-line").click({position:{x:70,y:10}});
  const request = await page.evaluate(() => window.messages.find(m => m.type === "requestEdit"));
  assert.equal(request.generation, 77);
  assert.ok(Number.isInteger(request.position));
  await page.evaluate(position => {window.studyappEditor.setReadonly(false);window.studyappEditor.restoreSelection(position);}, 8);
  assert.equal((await state(page)).selection.anchor, 8);
  await page.evaluate(() => window.studyappEditor.setTheme({mode:"dark",background:"#1b2026",surface:"#252b33",text:"#edf1f5",muted:"#a1adba",border:"#3c4652",accent:"#78c9c2",selection:"#365854",codeBackground:"#242d36",fontSize:17,spacing:1.8}));
  assert.equal(await page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue("--editor-background").trim()), "#1b2026");
  await page.evaluate(() => window.studyappEditor.setTheme({background:"url(https://bad.test)",fontSize:999}));
  assert.equal(await page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue("--editor-background").trim()), "#1b2026");
  await page.screenshot({path:fileURLToPath(new URL("../test-results/adaptive-dark.png",import.meta.url))});
  await page.evaluate(() => window.studyappEditor.load({generation:78,text:"Hello",readonly:true}));
  await page.locator(".cm-line").click();
  assert.equal(await page.evaluate(() => window.messages.filter(m => m.type === "requestEdit").length), 1);
  await page.close();
});

test("readonly renders Markdown at the initial caret while editing reveals its markers", async () => {
  const text = "# Title\n\n**bold**\n\n- item\n\n- [ ] task";
  const { page, errors } = await setup(text);
  assert.match(await page.locator(".md-heading").innerText(),/^# Title$/);
  await page.evaluate(() => window.studyappEditor.setReadonly(true));
  assert.equal(await page.locator(".md-heading").innerText(),"Title");
  assert.equal(await page.locator(".md-strong").innerText(),"bold");
  assert.equal(await page.locator(".md-task").count(),1);
  assert.equal(await page.locator(".md-task").isDisabled(),true);
  assert.ok(!(await page.locator(".cm-content").innerText()).includes("- item"));
  assert.equal((await state(page)).text,text);
  assert.equal((await state(page)).revision,0);
  await page.evaluate(() => window.studyappEditor.setReadonly(false));
  assert.match(await page.locator(".md-heading").innerText(),/^# Title$/);
  assert.deepEqual(errors,[]);
  await page.close();
});

test("readonly renders the first image at the initial caret without changing Markdown", async () => {
  const text = "![first](assets/pixel.png)\n\n![missing](assets/missing.png)";
  const { page, errors, externalRequests } = await setup(text);
  const pixel = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==";
  await page.evaluate(pixel => {
    window.studyappEditor.setImages({generation:41,images:{"assets/pixel.png":pixel}});
    window.studyappEditor.setReadonly(true);
  }, pixel);
  assert.equal(await page.locator("img.md-image").count(),1);
  await page.waitForFunction(() => document.querySelector("img.md-image")?.naturalWidth === 1);
  assert.equal(await page.locator(".md-image-placeholder").count(),1);
  assert.equal((await state(page)).text,text);
  assert.equal((await state(page)).selection.anchor,0);
  assert.deepEqual(errors,[]);
  assert.deepEqual(externalRequests,[]);
  await page.close();
});

test("line wrapped inline and reference images render with bounded missing placeholders", async () => {
  const text = "![line\nwrapped](assets/pixel.png)\n\n![ref alt][my\n photo]\n\n![inline](\nassets/pixel.png\n)\n\n![" + "missing ".repeat(300) + "](assets/missing.png)\n\n[my photo]: assets/pixel.png\n\nend";
  const { page, errors, externalRequests } = await setup(text);
  const pixel = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==";
  await select(page,text.indexOf("\n\n") + 1);
  await page.evaluate(pixel => window.studyappEditor.setImages({generation:41,images:{"assets/pixel.png":pixel}}),pixel);
  assert.equal(await page.locator("img.md-image").count(),3);
  assert.equal(await page.locator(".md-image-placeholder").count(),1);
  const bounds = await page.locator(".md-image-placeholder").evaluate(node => ({width:node.getBoundingClientRect().width,height:node.getBoundingClientRect().height,parent:node.parentElement.getBoundingClientRect().width}));
  assert.ok(bounds.width <= bounds.parent && bounds.height <= 160,JSON.stringify(bounds));
  assert.equal((await state(page)).text,text);
  assert.equal((await state(page)).revision,0);
  assert.deepEqual(errors,[]);
  assert.deepEqual(externalRequests,[]);
  await page.close();
});

test("managed image map renders only validated data and insertion preserves Markdown", async () => {
  const text = "![pixel](assets/pixel.png)\n\n![blocked](file:///C:/private.png)\n\nend";
  const { page, externalRequests } = await setup(text);
  await select(page, text.length);
  const pixel = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==";
  await page.evaluate(pixel => window.studyappEditor.setImages({generation:41,images:{"assets/pixel.png":pixel,"file:///C:/private.png":"file:///C:/private.png"}}),pixel);
  assert.equal(await page.locator("img.md-image").count(), 1);
  assert.equal(await page.locator("img.md-image").getAttribute("alt"), "pixel");
  await page.evaluate(() => window.studyappEditor.setImages({generation:40,images:{}}));
  assert.equal(await page.locator("img.md-image").count(), 1);
  await page.evaluate(() => window.studyappEditor.insertText("\n![new](assets/new.png)"));
  assert.ok((await state(page)).text.endsWith("\n![new](assets/new.png)"));
  await page.evaluate(() => window.studyappEditor.command("undo"));
  assert.equal((await state(page)).text, text);
  await page.evaluate(() => window.studyappEditor.setReadonly(true));
  await page.evaluate(() => window.studyappEditor.insertText("forbidden"));
  assert.equal((await state(page)).text, text);
  assert.deepEqual(externalRequests, []);
  await page.close();
});

test("full collapsed shortcut image references resolve only allowed destinations outside code", async () => {
  const text = "![Full alt][PHOTO]\n\n![photo][]\n\n![photo]\n\n![missing][unknown]\n\n`![inline code][photo]`\n\n```md\n![fenced code][photo]\n[unknown]: assets/pixel.png\n```\n\n[photo]: <assets/pixel.png> \"title\"\n\nend";
  const { page, errors, externalRequests } = await setup(text);
  const pixel = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==";
  await select(page,text.length);
  await page.evaluate(pixel => window.studyappEditor.setImages({generation:41,images:{"assets/pixel.png":pixel}}),pixel);
  assert.equal(await page.locator("img.md-image").count(),3);
  assert.deepEqual(await page.locator("img.md-image").evaluateAll(images => images.map(image => image.alt)),["Full alt","photo","photo"]);
  assert.equal(await page.locator(".md-image-placeholder").count(),1);
  assert.match(await page.locator(".md-image-placeholder").textContent(),/missing/);
  assert.equal((await state(page)).text,text);
  await page.evaluate(() => window.studyappEditor.setTheme(null));
  assert.equal((await state(page)).text,text);
  await page.evaluate(() => window.studyappEditor.load({generation:42,text:"loaded",readonly:false}));
  assert.equal((await state(page)).text,"loaded");
  assert.deepEqual(errors,[]);
  assert.deepEqual(externalRequests,[]);
  await page.close();
});

test("reference images honor first definitions, normalized labels and escaped destinations", async () => {
  const text = "![label alt][ My   Photo ]\n\n[my photo]: assets/pixel&amp;one.png\n[my photo]: assets/duplicate.png\n\nend";
  const {page} = await setup(text);
  await select(page,text.length);
  const pixel = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==";
  await page.evaluate(pixel => window.studyappEditor.setImages({generation:41,images:{"assets/pixel&one.png":pixel}}),pixel);
  assert.equal(await page.locator("img.md-image").count(),1);
  assert.equal((await state(page)).text,text);
  await page.close();
});

test("new heading defaults migrate around existing custom digit bindings", () => {
  const resolved = resolveBindings({"editor.bold":"Ctrl+1"});
  assert.equal(resolved["editor.bold"],"Ctrl+1");
  assert.equal(resolved["editor.heading-1"],"");
  assert.equal(resolved["editor.heading-2"],"Ctrl+2");
  assert.throws(() => resolveBindings({"editor.bold":"Ctrl+1","editor.heading-1":"Ctrl+1"}));
});

test("adaptive light dark and contextual language screenshots keep controls inside the viewport", async () => {
  const text = "# 研究笔记\n\n今天整理 Markdown 编辑体验。\n\n- 资料整理\n  - [ ] 检查引用\n\n```typescript\nconst answer: number = 42;\nconsole.log(answer);\n```\n\n继续记录。";
  const {page, errors} = await setup(text);
  await select(page,text.indexOf("answer"));
  const picker = page.getByRole("combobox",{name:"代码语言"});
  await picker.fill("s");
  assert.equal(await page.getByRole("option").count(),5);
  const bounds = await picker.boundingBox();
  assert.ok(bounds.y > 200 && bounds.x + bounds.width <= 1100);
  await page.screenshot({path:fileURLToPath(new URL("../test-results/adaptive-light-language.png",import.meta.url))});
  await picker.press("Escape");
  await page.evaluate(() => window.studyappEditor.setTheme({mode:"dark",background:"#1b2026",surface:"#252b33",text:"#edf1f5",muted:"#a1adba",border:"#3c4652",accent:"#78c9c2",selection:"#365854",codeBackground:"#242d36"}));
  await page.getByRole("button",{name:"格式菜单"}).click();
  await page.screenshot({path:fileURLToPath(new URL("../test-results/adaptive-dark-menu.png",import.meta.url))});
  await page.getByRole("button",{name:"格式菜单"}).click();
  await select(page,text.indexOf("资料"));
  await page.keyboard.press("Tab");
  await page.screenshot({path:fileURLToPath(new URL("../test-results/adaptive-list-caret.png",import.meta.url))});
  assert.deepEqual(errors,[]);
  await page.close();
});

test("shortcut registry matches the native allowlist and validation rejects conflicts and platform keys", async () => {
  const native = await readFile(new URL("../../src/shortcuts.rs", import.meta.url), "utf8");
  const entries = [...native.matchAll(/\(\s*"((?:app|editor)\.[^"]+)",\s*"[^"]+",\s*"[^"]+",\s*"([^"]*)"\s*,?\s*\)/g)]
    .map(([, action, binding]) => [action, binding]);
  assert.equal(entries.length, Object.keys(shortcutDefaults).length);
  assert.deepEqual(Object.fromEntries(entries), shortcutDefaults);
  assert.deepEqual(resolveBindings({}), shortcutDefaults);
  assert.equal(normalizeBinding(" control + shift + s ; f8 "), "Ctrl+Shift+S;F8");
  assert.equal(normalizeBinding("alt + Ctrl + ["), "Ctrl+Alt+BracketLeft");
  assert.equal(normalizeBinding(" "), "");
  for (const binding of [
    "A", "Shift+A", "Ctrl", "Ctrl++S", "Ctrl+Ctrl+S", "Ctrl+S;", "Ctrl+S;control+s",
    "Win+S", "Meta+S", "Alt+F4", "Alt+Tab", "Alt+Space", "Ctrl+Space", "Ctrl+Shift+Space",
    "Ctrl+Alt+Delete", "Ctrl+Shift+Escape", "Ctrl+C", "Shift+Insert", "Ctrl+Home", "Alt+ArrowLeft", "F25", "F01"
  ]) assert.throws(() => normalizeBinding(binding), undefined, binding);
  assert.throws(() => resolveBindings({ "editor.save": "Ctrl+B" }));
  assert.throws(() => resolveBindings({ "app.arbitrary-code": "Ctrl+Q" }));
  assert.equal(resolveBindings({ "editor.save": "Ctrl+B", "editor.bold": "" })["editor.save"], "Ctrl+B");
});

test("bundled editor starts with one continuous editable Markdown surface", async () => {
  const { page, errors } = await setup("# 连续编辑\n\n正文包含 **粗体** 和 *强调*。\n\n- 第一项\n- 第二项\n\n结尾");
  assert.equal(await page.locator('[contenteditable="true"]').count(), 1);
  assert.equal(await page.locator("textarea").count(), 0);
  await select(page, 52);
  assert.ok(await page.locator(".md-heading").count());
  assert.ok(await page.locator(".md-strong").count());
  assert.ok(await page.locator(".md-emphasis").count());
  assert.ok(await page.locator(".md-bullet").count());
  const visible = await page.locator(".cm-content").innerText();
  assert.ok(!visible.includes("**粗体**"));
  assert.ok(!visible.includes("# 连续编辑"));
  await select(page, "# 连续编辑\n\n正文包含 **粗".length);
  assert.ok((await page.locator(".cm-content").innerText()).includes("**粗体**"));
  assert.deepEqual(errors, []);
  await page.close();
});

test("Chinese prose uses spacious body typography inside the single scroll viewport", async () => {
  const text = "# 连续记录，更专注\n\n直接编辑这份笔记，**重点内容** 与 *强调* 在原位置呈现。\n\n## 一个清晰的工作区\n\n- 阅读与书写在同一处\n- [x] 支持任务、列表和跨段选择\n- [ ] 支持中文输入法\n\n> 保留原始 Markdown，编辑体验更自然。\n\n| 语言 | 状态 |\n| :--- | :--- |\n| Rust / Python | 离线高亮 |\n| JavaScript / C++ | 离线高亮 |\n\n```rust\nfn main() {\n    let progress = 42;\n    println!(\"Keep learning: {progress}\");\n}\n```\n\n";
  const { page } = await setup(text);
  await select(page, text.length);
  const appearance = await page.evaluate(() => ({
    font: getComputedStyle(document.querySelector(".cm-scroller")).fontFamily,
    padding: parseFloat(getComputedStyle(document.querySelector(".cm-content")).paddingLeft),
    scrollHeight: document.querySelector(".cm-scroller").clientHeight,
    documentOverflow: document.documentElement.scrollHeight > window.innerHeight
  }));
  assert.ok(appearance.font.includes("Segoe UI"), JSON.stringify(appearance));
  assert.ok(appearance.padding >= 26, JSON.stringify(appearance));
  assert.ok(appearance.scrollHeight < 800 && appearance.scrollHeight > 700);
  assert.equal(appearance.documentOverflow, false);
  await mkdir(new URL("../test-results/", import.meta.url), { recursive: true });
  await page.screenshot({ path: fileURLToPath(new URL("../test-results/sample-editor.png", import.meta.url)) });
  await page.close();
});

test("cross-paragraph input, Unicode UTF-16 patches, undo and source preserve history", async () => {
  const { page } = await setup("# 标题\r\n\r\n甲😀乙\r\n第二段\r末段");
  const initial = "# 标题\n\n甲😀乙\n第二段\n末段";
  assert.equal((await state(page)).text, initial);
  await select(page, initial.indexOf("😀"), initial.indexOf("末段"));
  await page.keyboard.insertText("新的😀\n跨段");
  const edited = (await state(page)).text;
  const editedSelection = (await state(page)).selection;
  assert.equal(edited, "# 标题\n\n甲新的😀\n跨段末段");
  const changes = await page.evaluate(() => window.messages.filter(m => m.type === "change"));
  assert.equal(changes.length, 1);
  assert.deepEqual(changes[0], {
    type: "change", generation: 41, baseRevision: 0, revision: 1,
    changes: [{ fromLine: 3, fromColumn: 2, toLine: 5, toColumn: 1, text: "新的😀\n跨段" }]
  });
  await page.evaluate(() => window.studyappEditor.setSource(true));
  assert.equal(await page.locator(".md-heading").count(), 0);
  assert.equal((await state(page)).text, edited);
  assert.deepEqual((await state(page)).selection, editedSelection);
  await page.evaluate(() => window.studyappEditor.setSource(false));
  await page.evaluate(() => window.studyappEditor.command("undo"));
  assert.equal((await state(page)).text, initial);
  await page.evaluate(() => window.studyappEditor.command("redo"));
  assert.equal((await state(page)).text, edited);
  await page.close();
});

test("toolbar formats selections spanning paragraphs and supports task interaction", async () => {
  const { page } = await setup("第一段\n\n第二段\n\n- [ ] 待办\n\n结尾");
  await select(page, 0, 8);
  await page.getByRole("button", { name: "粗体" }).click();
  assert.equal((await state(page)).text.slice(0, 16), "**第一段**\n\n**第二段**");
  assert.equal(await page.locator(".md-strong").count(), 2);
  await page.getByRole("button", { name: "粗体" }).click();
  assert.equal((await state(page)).text, "第一段\n\n第二段\n\n- [ ] 待办\n\n结尾");
  await page.evaluate(() => window.studyappEditor.command("undo"));
  await page.evaluate(() => window.studyappEditor.command("undo"));
  await select(page, (await state(page)).text.length);
  await page.getByRole("checkbox", { name: "切换任务完成状态" }).click();
  assert.ok((await state(page)).text.includes("- [x] 待办"));
  await page.evaluate(() => window.studyappEditor.command("undo"));
  assert.ok((await state(page)).text.includes("- [ ] 待办"));
  await page.close();
});

test("split native UTF-16 input publishes one scalar-safe patch and exact byte budget", async () => {
  const { page } = await setup("甲\r\n乙");
  await action(page, { type: "replace", from: 1, text: "\uD83E" });
  assert.equal(await page.evaluate(() => window.messages.filter(m => m.type === "change").length), 0,
    "a lone high surrogate must never be sent to Rust JSON");
  await action(page, { type: "replace", from: 2, text: "\uDDE0" });
  assert.equal((await state(page)).text, "甲🧠\n乙");
  const messages = await page.evaluate(() => window.messages.filter(m => m.type === "change"));
  assert.deepEqual(messages, [{
    type: "change", generation: 41, baseRevision: 0, revision: 1,
    changes: [{ fromLine: 1, fromColumn: 2, toLine: 1, toColumn: 2, text: "🧠" }]
  }]);
  assert.equal((await state(page)).utf8Bytes, Buffer.byteLength("甲🧠\n乙"));
  assert.equal((await state(page)).rawBytes, Buffer.byteLength("甲🧠\r\n乙"));
  await page.evaluate(() => window.studyappEditor.flush({ request: 91 }));
  const flush = await page.evaluate(() => window.messages.find(m => m.type === "flush"));
  assert.equal(flush.revision, 1);
  assert.equal(flush.normalizedText, "甲🧠\n乙");
  await page.close();
});

test("flush waits for split native input and a new document drops pending old-generation text", async () => {
  const { page } = await setup("");
  await action(page, { type: "replace", from: 0, text: "\uD83E" });
  await page.evaluate(() => {
    window.flushPending = window.studyappEditor.flush({ request: 92 });
    setTimeout(() => window.studyappEditor.test({ type: "replace", from: 1, text: "\uDDE0" }), 80);
  });
  await page.evaluate(() => window.flushPending);
  assert.equal((await state(page)).text, "🧠");
  assert.equal(await page.evaluate(() => window.messages.find(m => m.type === "flush").normalizedText), "🧠");
  await page.evaluate(() => window.studyappEditor.load({ generation: 42, text: "", readonly: false }));
  await action(page, { type: "replace", from: 0, text: "\uD83E" });
  await page.evaluate(() => window.studyappEditor.load({ generation: 43, text: "新文档", readonly: false }));
  await action(page, { type: "replace", from: 3, text: "。" });
  const last = await page.evaluate(() => window.messages.filter(m => m.type === "change").at(-1));
  assert.equal(last.generation, 43);
  assert.equal(last.baseRevision, 0);
  assert.deepEqual(last.changes, [{ fromLine: 1, fromColumn: 4, toLine: 1, toColumn: 4, text: "。" }]);
  await page.close();
});

test("composed native Unicode edits stay within the bridge patch cap and preserve mixed EOLs", async () => {
  const lines = Array.from({ length: 2048 }, (_, index) => `line ${index}`);
  const separators = lines.map((_, index) => index % 5 === 0 ? "\n" : index % 7 === 0 ? "\r" : "\r\n");
  const original = "head\r\n" + lines.map((line, index) => line + separators[index]).join("") + "tail";
  const { page } = await setup(original);
  await action(page, { type: "replace", from: 1, text: "\uD83E" });
  const pending = (await state(page)).text;
  await select(page, pending.indexOf("line 0"), pending.lastIndexOf("\ntail"));
  await page.evaluate(() => window.studyappEditor.command("bold"));
  assert.equal(await page.evaluate(() => window.messages.filter(m => m.type === "change").length), 0);
  await action(page, { type: "replace", from: 2, text: "\uDDE0" });
  const messages = await page.evaluate(() => window.messages.filter(m => m.type === "change"));
  assert.ok(messages.length > 0);
  const raw = replayRawChanges(original, messages, "\r\n");
  const expected = "h🧠ead\r\n" + lines.map((line, index) => `**${line}**` + separators[index]).join("") + "tail";
  assert.equal(raw, expected);
  assert.equal((await state(page)).rawBytes, Buffer.byteLength(expected));
  await page.evaluate(() => window.studyappEditor.flush({ request: 96 }));
  const flush = await page.evaluate(() => window.messages.find(m => m.type === "flush"));
  assert.equal(flush.revision, messages.length);
  assert.equal(flush.normalizedText, expected.replace(/\r\n?|\n/g, "\n"));
  await page.close();
});

test("composed native patch batches respect the raw byte limit at every intermediate revision", async () => {
  const limit = 10 * 1024 * 1024;
  const lines = Array.from({ length: 2048 }, (_, index) => `line ${index}`);
  const separators = lines.map((_, index) => index % 5 === 0 ? "\n" : index % 7 === 0 ? "\r" : "\r\n");
  const suffix = lines.map((line, index) => line + separators[index]).join("") + "tail";
  const prefix = "x".repeat(limit - 3 - Buffer.byteLength(suffix) - 2);
  const original = prefix + "\r\n" + suffix;
  const { page } = await setup(original);
  await action(page, { type: "replace", from: (await state(page)).text.length, text: "\uD83E" });
  await action(page, { type: "replace", from: 0, to: 8193, text: "" });
  const pending = (await state(page)).text;
  await select(page, pending.indexOf("line 0"), pending.lastIndexOf("\ntail"));
  await page.evaluate(() => window.studyappEditor.command("bold"));
  await action(page, { type: "replace", from: (await state(page)).text.length, text: "\uDDE0" });
  const messages = await page.evaluate(() => window.messages.filter(m => m.type === "change"));
  const raw = replayRawChanges(original, messages, "\r\n");
  const expected = prefix.slice(8193) + "\r\n" + lines.map((line, index) => `**${line}**` + separators[index]).join("") + "tail🧠";
  assert.ok(raw === expected, "batched revisions must preserve the complete mixed-EOL document");
  assert.equal(Buffer.byteLength(raw), limit);
  assert.equal((await state(page)).rawBytes, limit);
  assert.equal((await state(page)).readonly, false);
  await page.evaluate(() => window.studyappEditor.flush({ request: 97 }));
  const flush = await page.evaluate(() => window.messages.find(m => m.type === "flush"));
  assert.equal(flush.revision, messages.length);
  assert.ok(flush.normalizedText === expected.replace(/\r\n?|\n/g, "\n"));
  await page.close();
});

test("undo and redo across separate surrogate history groups publish only complete characters", async () => {
  const { page } = await setup("甲乙");
  await action(page, { type: "replace", from: 1, text: "\uD83E" });
  await page.waitForTimeout(600);
  await action(page, { type: "replace", from: 2, text: "\uDDE0" });
  await page.evaluate(() => window.studyappEditor.command("undo"));
  assert.equal((await state(page)).text, "甲\uD83E乙");
  assert.equal(await page.evaluate(() => window.messages.filter(m => m.type === "change").length), 1);
  await page.evaluate(() => window.studyappEditor.command("undo"));
  assert.equal((await state(page)).text, "甲乙");
  await page.evaluate(() => window.studyappEditor.command("redo"));
  assert.equal((await state(page)).text, "甲\uD83E乙");
  await page.evaluate(() => window.studyappEditor.command("redo"));
  assert.equal((await state(page)).text, "甲🧠乙");
  const messages = await page.evaluate(() => window.messages.filter(m => m.type === "change"));
  assert.equal(messages.length, 3);
  assert.deepEqual(messages.map(m => m.revision), [1, 2, 3]);
  assert.deepEqual(messages[1].changes, [{ fromLine: 1, fromColumn: 2, toLine: 1, toColumn: 4, text: "" }]);
  assert.equal((await state(page)).rawBytes, Buffer.byteLength("甲🧠乙"));
  await page.close();
});

test("incomplete native Unicode input never receives a save acknowledgement", async () => {
  const { page } = await setup("保留");
  await page.evaluate(() => {
    const original = window.ipc.postMessage;
    window.ipc.postMessage = json => {
      original(json);
      if (JSON.parse(json).type === "error") setTimeout(() => window.studyappEditor.setReadonly(true), 0);
    };
  });
  await action(page, { type: "replace", from: 2, text: "\uD83E" });
  await action(page, { type: "replace", from: 3, text: "后续" });
  await page.evaluate(() => window.studyappEditor.flush({ request: 93 }));
  assert.equal((await state(page)).text, "保留后续");
  assert.equal(await page.evaluate(() => window.messages.some(m => m.type === "flush")), false);
  assert.deepEqual(await page.evaluate(() => window.messages.filter(m => m.type === "change").flatMap(m => m.changes)),
    [{ fromLine: 1, fromColumn: 3, toLine: 1, toColumn: 3, text: "后续" }]);
  assert.equal(await page.evaluate(() => window.messages.some(m => m.type === "flushRejected" && m.generation === 41 && m.request === 93)), true);
  assert.equal((await state(page)).readonly, false);
  await action(page, { type: "replace", from: 4, text: "🧠" });
  await page.evaluate(() => window.studyappEditor.flush({ request: 94 }));
  assert.equal(await page.evaluate(() => window.messages.find(m => m.type === "flush").normalizedText), "保留后续🧠");
  await page.close();
});


test("inline formatting toggles and preserves heading/list prefixes across selected lines", async () => {
  const { page } = await setup("# heading\n\n- item\n\nword");
  await select(page, 0, 17);
  await page.evaluate(() => window.studyappEditor.command("bold"));
  assert.equal((await state(page)).text, "# **heading**\n\n- **item**\n\nword");
  await page.evaluate(() => window.studyappEditor.command("undo"));
  await select(page, 19, 23);
  await page.evaluate(() => window.studyappEditor.command("italic"));
  assert.equal((await state(page)).text, "# heading\n\n- item\n\n*word*");
  await page.evaluate(() => window.studyappEditor.command("italic"));
  assert.equal((await state(page)).text, "# heading\n\n- item\n\nword");
  await page.close();
});

test("inline style toggles preserve nested emphasis and backticks remain valid code", async () => {
  const { page } = await setup("**word**");
  await select(page, 2, 6);
  await page.evaluate(() => window.studyappEditor.command("italic"));
  assert.equal((await state(page)).text, "***word***");
  await page.evaluate(() => window.studyappEditor.command("italic"));
  assert.equal((await state(page)).text, "**word**");
  await page.evaluate(() => window.studyappEditor.load({ generation: 42, text: "*word*", readonly: false }));
  await select(page, 1, 5);
  await page.evaluate(() => window.studyappEditor.command("bold"));
  assert.equal((await state(page)).text, "***word***");
  await page.evaluate(() => window.studyappEditor.command("bold"));
  assert.equal((await state(page)).text, "*word*");
  await page.evaluate(() => window.studyappEditor.load({ generation: 43, text: "a`b", readonly: false }));
  await select(page, 0, 3);
  await page.evaluate(() => window.studyappEditor.command("code"));
  assert.equal((await state(page)).text, "``a`b``");
  assert.equal(await page.locator(".md-inline-code").count(), 1);
  await page.evaluate(() => window.studyappEditor.command("code"));
  assert.equal((await state(page)).text, "a`b");
  await page.close();
});

test("inline code toggles restore text beginning or ending with backticks", async () => {
  const { page, errors } = await setup("");
  for (const text of ["`a", "a`", "``a`", "`a``"]) {
    await page.evaluate(text => window.studyappEditor.load({ generation: 42, text, readonly: false }), text);
    await select(page, 0, text.length);
    await page.evaluate(() => window.studyappEditor.command("code"));
    assert.equal(await page.locator(".md-inline-code").count(), 1);
    await page.evaluate(() => window.studyappEditor.command("code"));
    assert.equal((await state(page)).text, text);
  }
  assert.deepEqual(errors, []);
  await page.close();
});

test("formatting 140000 backtick runs does not overflow the JavaScript stack", async () => {
  const text = "a`".repeat(140000);
  const { page, errors } = await setup(text);
  await select(page, 0, text.length);
  await page.evaluate(() => window.studyappEditor.command("code"));
  const formatted = (await state(page)).text;
  assert.ok(formatted === "`` " + text + " ``", "long backtick selection must produce one valid code span");
  assert.deepEqual(errors, []);
  await page.close();
});

test("table rows with missing cells retain the header column widths", async () => {
  const text = "| A | B | C |\n|---|---|---|\n| one | two | three |\n| short |\n\nend";
  const { page } = await setup(text);
  await select(page, text.length);
  const widths = await page.locator(".md-table-row").evaluateAll(rows => rows.map(row => row.querySelector(".md-table-cell").getBoundingClientRect().width));
  assert.equal(widths.length, 3);
  assert.ok(Math.abs(widths[0] - widths[2]) < 2, JSON.stringify(widths));
  await page.close();
});

test("tables, fenced languages and unknown code remain in one editable document", async () => {
  const languages = {
    rust: "fn main() { let count = 3; }", python: "def main():\n    return True",
    javascript: "const value = true;", typescript: "const value: number = 3;",
    c: "int main() { return 0; }", cpp: "class Sample { public: int value; };",
    csharp: "class Sample { public int Value = 3; }", java: "class Sample { int value = 3; }",
    go: "package main\nfunc main() {}", html: '<div class="sample">text</div>',
    css: ".sample { color: red; }", json: '{"value": true}', yaml: "value: true",
    toml: 'name = "hello"', sql: "SELECT value FROM sample;", shell: 'echo "$HOME"'
  };
  for (const [language, code] of Object.entries(languages)) {
    const { page } = await setup("```" + language + "\n" + code + "\n```\n\nend");
    await select(page, (await state(page)).text.length);
    assert.ok(await page.locator(".tok-keyword, .tok-string, .tok-number, .tok-property, .tok-tag, .tok-type").count(), language);
    assert.ok(await page.locator(".md-code-line").count(), language);
    await page.close();
  }
  const text = "| 名称 | 数值 |\n| :--- | ---: |\n| 样本 | 42 |\n\n```unknownlang\n<b>plain code</b>\n```\n\nend";
  const { page, errors } = await setup(text);
  await select(page, text.length);
  assert.ok(await page.locator(".md-table-row").count());
  assert.equal(await page.locator(".cm-content b").count(), 0);
  assert.equal((await state(page)).text, text);
  assert.deepEqual(errors, []);
  await page.close();
});

test("Markdown never executes HTML, navigates or loads remote resources", async () => {
  const { page, errors, externalRequests } = await setup('<script>window.pwned=true</script>\n<img src="https://example.com/tracker">\n\n![image](https://example.com/image.png)\n\n[bad](javascript:alert(1))\n[link](https://example.com)\n\nend');
  await select(page, (await state(page)).text.length);
  assert.equal(await page.evaluate(() => window.pwned), undefined);
  // CM inserts inert src-less widgetBuffer images to work around browser caret bugs.
  assert.equal(await page.locator(".cm-content script, .cm-content img:not(.cm-widgetBuffer), .cm-content img[src], .cm-content iframe").count(), 0);
  assert.equal(await page.locator(".cm-content a[href]").count(), 0);
  await page.locator(".md-link").last().click({ modifiers: ["Control"] });
  assert.equal(page.url(), base);
  assert.deepEqual(externalRequests, []);
  assert.deepEqual(errors, []);
  await page.close();
});

test("multiline link titles preserve the continuous surface without invalid replacements", async () => {
  const text = '[foo](https://example.com "a\nb")\n\nend';
  const { page, errors } = await setup(text);
  await select(page, text.length);
  assert.deepEqual(errors, []);
  assert.equal((await state(page)).text, text);
  assert.equal(await page.locator('[contenteditable="true"]').count(), 1);
  await page.keyboard.insertText("!");
  assert.equal((await state(page)).text, text + "!");
  assert.deepEqual(errors, []);
  await page.close();
});

test("task keyboard activation toggles the marker without inserting text at the editor cursor", async () => {
  const { page } = await setup("- [ ] task\n\nend");
  await select(page, 14);
  const checkbox = page.getByRole("checkbox", { name: "切换任务完成状态" });
  await checkbox.focus();
  await page.keyboard.press("Enter");
  assert.equal((await state(page)).text, "- [x] task\n\nend");
  await page.keyboard.press("Space");
  assert.equal((await state(page)).text, "- [ ] task\n\nend");
  await page.close();
});

test("keyboard selection crosses paragraphs and prefix patches use the pre-transaction document", async () => {
  const { page } = await setup("one\n\ntwo\n\nthree");
  await select(page, 0);
  await page.keyboard.press("Control+Shift+End");
  const selected = (await state(page)).selection;
  assert.deepEqual(selected, { anchor: 0, head: 15 });
  await page.getByRole("button", { name: "格式菜单" }).click();
  await page.getByRole("menuitem", { name: "列表", exact: true }).click();
  assert.equal((await state(page)).text, "- one\n- \n- two\n- \n- three");
  const change = await page.evaluate(() => window.messages.filter(m => m.type === "change").at(-1));
  assert.equal(change.changes.length, 5);
  assert.deepEqual(change.changes.map(c => [c.fromLine, c.fromColumn, c.toLine, c.toColumn]), [
    [1, 1, 1, 1], [2, 1, 2, 1], [3, 1, 3, 1], [4, 1, 4, 1], [5, 1, 5, 1]
  ]);
  await page.keyboard.press("Control+z");
  assert.equal((await state(page)).text, "one\n\ntwo\n\nthree");
  await page.keyboard.insertText("替换全部\n新段落");
  assert.equal((await state(page)).text, "替换全部\n新段落");
  await page.close();
});

test("new load cancels an old flush and resets history, selection and revisions", async () => {
  const { page } = await setup("old");
  await page.evaluate(() => {
    window.studyappEditor.flush({ request: 99 });
    window.studyappEditor.load({ generation: 42, text: "next", readonly: false });
  });
  await page.waitForTimeout(100);
  assert.equal(await page.evaluate(() => window.messages.filter(m => m.type === "flush").length), 0);
  assert.equal((await state(page)).readonly, false);
  assert.equal((await state(page)).revision, 0);
  await page.evaluate(() => window.studyappEditor.command("undo"));
  assert.equal((await state(page)).text, "next");
  await action(page, { type: "replace", from: 4, text: "\r\n下一行\r末尾", request: "native-smoke" });
  assert.equal((await state(page)).text, "next\n下一行\n末尾");
  const report = await page.evaluate(() => window.messages.find(m => m.type === "test"));
  assert.equal(report.request, "native-smoke");
  assert.equal(report.generation, 42);
  assert.equal(report.result.normalizedText, "next\n下一行\n末尾");
  assert.equal(report.result.revision, 1);
  await page.close();
});

test("malformed Markdown and repeated IME updates retain literal text and flush accurately", async () => {
  const raw = "# 标题\r\n\r\n未闭合 **强调 [链接](\r\n\r\n```not-a-language\r\n<script>alert('literal')</script>";
  const { page, errors } = await setup(raw);
  await select(page, 4);
  const session = await page.context().newCDPSession(page);
  await session.send("Input.imeSetComposition", { text: "中", selectionStart: 1, selectionEnd: 1 });
  await session.send("Input.imeSetComposition", { text: "中文", selectionStart: 2, selectionEnd: 2 });
  await session.send("Input.insertText", { text: "中文输入" });
  await page.evaluate(() => window.studyappEditor.flush({ request: 73 }));
  const expected = "# 标题中文输入\n\n未闭合 **强调 [链接](\n\n```not-a-language\n<script>alert('literal')</script>";
  assert.equal((await state(page)).text, expected);
  const flush = await page.evaluate(() => window.messages.find(m => m.type === "flush" && m.request === 73));
  assert.equal(flush.normalizedText, expected);
  assert.deepEqual(errors, []);
  await page.close();
});

test("flush commits composition before freezing and acknowledges the matching revision", async () => {
  const { page } = await setup("输入：");
  await select(page, 3);
  const session = await page.context().newCDPSession(page);
  await session.send("Input.imeSetComposition", { text: "中文", selectionStart: 2, selectionEnd: 2 });
  assert.equal((await state(page)).composing, true);
  await page.evaluate(() => window.studyappEditor.flush({ request: 17 }));
  await page.waitForFunction(() => window.messages.some(m => m.type === "flush" && m.request === 17));
  const snapshot = await state(page);
  const flush = await page.evaluate(() => window.messages.find(m => m.type === "flush"));
  assert.equal(snapshot.text, "输入：中文");
  assert.equal(snapshot.readonly, true);
  assert.equal(snapshot.composing, false);
  assert.deepEqual(flush, { type: "flush", generation: 41, request: 17, revision: snapshot.revision, normalizedText: snapshot.text });
  await page.keyboard.insertText("不可插入");
  assert.equal((await state(page)).text, snapshot.text);
  await page.evaluate(() => window.studyappEditor.load({ generation: 42, text: "新文档", readonly: false }));
  assert.equal((await state(page)).revision, 0);
  assert.equal((await state(page)).generation, 42);
  await page.close();
});

test("readonly, save key, and production test isolation", async () => {
  const { page } = await setup("readonly");
  await page.evaluate(() => window.studyappEditor.setReadonly(true));
  await page.locator(".cm-content").click();
  await page.keyboard.insertText("blocked");
  assert.equal((await state(page)).text, "readonly");
  await page.keyboard.press("Control+s");
  assert.equal(await page.evaluate(() => window.messages.some(m => m.type === "save")), false);
  await page.close();
  const production = await setup("secure", false);
  assert.equal(await production.page.evaluate(() => typeof window.studyappEditor.test), "undefined");
  await production.page.close();
});

test("shortcut remap saves once, removes old bindings, updates tooltips and restores defaults", async () => {
  const { page } = await setup("editable");
  await select(page, 0, 8);
  await shortcuts(page, { "editor.save": "shift + control + s", "editor.bold": "Ctrl+Alt+B" });
  await page.keyboard.press("Control+s");
  assert.equal(await page.evaluate(() => window.messages.filter(m => m.type === "save").length), 0);
  await page.keyboard.press("Control+Shift+s");
  assert.equal(await page.evaluate(() => window.messages.filter(m => m.type === "save").length), 1);
  await page.keyboard.press("Control+b");
  assert.equal((await state(page)).text, "editable");
  await page.keyboard.press("Control+Alt+b");
  assert.equal((await state(page)).text, "**editable**");
  assert.match(await page.locator('[data-command="bold"]').getAttribute("title"), /Ctrl\+Alt\+B/);
  await shortcuts(page, { "editor.save": "", "editor.bold": "" });
  await page.keyboard.press("Control+s");
  assert.equal(await page.evaluate(() => window.messages.filter(m => m.type === "save").length), 1);
  assert.doesNotMatch(await page.locator('[data-command="bold"]').getAttribute("title"), /Ctrl/);
  await shortcuts(page, {});
  await page.keyboard.press("Control+s");
  assert.equal(await page.evaluate(() => window.messages.filter(m => m.type === "save").length), 2);
  await page.locator("#source-toggle").focus();
  await page.keyboard.press("Control+s");
  assert.equal(await page.evaluate(() => window.messages.filter(m => m.type === "save").length), 3);
  await page.close();
});

test("shifted digit shortcut keys match physical digits even when Windows reports punctuation", async () => {
  const { page } = await setup("digits");
  await select(page, 0, 6);
  await shortcuts(page, { "editor.save": "Ctrl+Shift+1", "editor.bold": "Ctrl+Shift+2" });
  await page.keyboard.press("Control+Shift+Digit1");
  assert.equal(await page.evaluate(() => window.messages.filter(m => m.type === "save").length), 1);
  await page.keyboard.press("Control+Shift+Digit2");
  assert.equal((await state(page)).text, "**digits**");
  await page.close();
});

test("shortcut remap preserves selection, revision and undo history while replacing all history aliases", async () => {
  const { page } = await setup("original");
  await select(page, 0, 8);
  await page.keyboard.insertText("edited");
  const before = await state(page);
  await shortcuts(page, { "editor.undo": "Ctrl+Alt+Z", "editor.redo": "Ctrl+Alt+Y" });
  const remapped = await state(page);
  assert.equal(remapped.text, before.text);
  assert.equal(remapped.revision, before.revision);
  assert.deepEqual(remapped.selection, before.selection);
  await page.keyboard.press("Control+z");
  assert.equal((await state(page)).text, "edited");
  await page.keyboard.press("Control+Alt+z");
  assert.equal((await state(page)).text, "original");
  await page.keyboard.press("Control+y");
  await page.keyboard.press("Control+Shift+z");
  assert.equal((await state(page)).text, "original");
  await page.keyboard.press("Control+Alt+y");
  assert.equal((await state(page)).text, "edited");
  await shortcuts(page, {});
  await page.keyboard.press("Control+z");
  await page.keyboard.press("Control+Shift+z");
  assert.equal((await state(page)).text, "edited");
  await page.close();
});

test("shortcut remap routes native actions with generation and suppresses repeats, IME and pending flush", async () => {
  const { page } = await setup("中文");
  await select(page, 0);
  await shortcuts(page, { "app.new": "Ctrl+Alt+N" });
  await page.keyboard.press("Control+Alt+n");
  assert.deepEqual(await page.evaluate(() => window.messages.filter(m => m.type === "shortcut")), [
    { type: "shortcut", action: "app.new", generation: 41 }
  ]);
  await page.evaluate(() => {
    for (const flags of [{ repeat: true }, { isComposing: true }, { keyCode: 229 }]) {
      document.querySelector(".cm-content").dispatchEvent(new KeyboardEvent("keydown", { key: "n", ctrlKey: true, altKey: true, bubbles: true, cancelable: true, ...flags }));
    }
  });
  assert.equal(await page.evaluate(() => window.messages.filter(m => m.type === "shortcut").length), 1);
  await page.evaluate(() => {
    void window.studyappEditor.flush({ request: 22 });
    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "n", ctrlKey: true, altKey: true, bubbles: true, cancelable: true }));
    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "s", ctrlKey: true, bubbles: true, cancelable: true }));
  });
  assert.equal(await page.evaluate(() => window.messages.filter(m => m.type === "shortcut").length), 1);
  assert.equal(await page.evaluate(() => window.messages.filter(m => m.type === "save").length), 0);
  await page.waitForFunction(() => window.messages.some(m => m.type === "flush" && m.request === 22));
  await page.evaluate(() => window.studyappEditor.load({ generation: 42, text: "next", readonly: false }));
  await select(page, 0);
  await page.keyboard.press("Control+Alt+n");
  assert.equal(await page.evaluate(() => window.messages.filter(m => m.type === "shortcut").at(-1).generation), 42);
  await page.close();
});

test("focused Escape forwards one generation-scoped resize cancellation before CodeMirror consumes it", async () => {
  const { page } = await setup("中文编辑保持原文");
  await select(page, 0, 3);
  const before = await state(page);
  await page.keyboard.press("Escape");
  assert.deepEqual(await page.evaluate(() => window.messages.filter(m => m.type === "cancelResize")), [
    { type: "cancelResize", generation: 41 }
  ]);
  const after = await state(page);
  assert.equal(after.text, before.text);
  assert.equal(after.revision, before.revision);
  assert.deepEqual(after.selection, { anchor: 3, head: 3 }, "normal CodeMirror Escape must still collapse the selection");
  await page.evaluate(() => {
    window.studyappEditor.load({ generation: 42, text: "next", readonly: false });
    document.querySelector(".cm-content").addEventListener("keydown", event => {
      if (event.key === "Escape") event.stopPropagation();
    });
  });
  await select(page, 0);
  await page.keyboard.press("Escape");
  assert.deepEqual(await page.evaluate(() => window.messages.filter(m => m.type === "cancelResize")), [
    { type: "cancelResize", generation: 41 }, { type: "cancelResize", generation: 42 }
  ]);
  await page.evaluate(() => {
    for (const flags of [{ isComposing: true }, { keyCode: 229 }, { repeat: true }]) {
      document.querySelector(".cm-content").dispatchEvent(new KeyboardEvent("keydown", {
        key: "Escape", bubbles: true, cancelable: true, ...flags
      }));
    }
  });
  assert.equal(await page.evaluate(() => window.messages.filter(m => m.type === "cancelResize").length), 2);
  assert.equal((await state(page)).text, "next");
  assert.equal((await state(page)).revision, 0);
  await page.close();
});

test("shortcut malformed maps are rejected atomically without changing the document or active bindings", async () => {
  const { page, errors } = await setup("kept");
  await select(page, 0);
  await shortcuts(page, { "editor.save": "Ctrl+Shift+S" });
  for (const invalid of [
    { "editor.bold": "Ctrl+Shift+S" }, { "app.unknown": "Ctrl+Q" }, { "editor.save": "Alt+F4" },
    { "editor.save": "Win+S" }, { "editor.save": "Ctrl+Space" }, { "editor.save": "A" },
    { "editor.save": "Shift+A" }, { "editor.save": "Ctrl+Shift+S;" }, { "editor.save": 42 }, null, []
  ]) {
    // A partial map starts from defaults, so force the duplicate inside the same update.
    const candidate = invalid && !Array.isArray(invalid) ? { "editor.save": "Ctrl+Shift+S", ...invalid } : invalid;
    await shortcuts(page, candidate);
  }
  await page.keyboard.press("Control+Shift+s");
  assert.equal(await page.evaluate(() => window.messages.filter(m => m.type === "save").length), 1);
  assert.equal((await state(page)).text, "kept");
  assert.equal((await state(page)).revision, 0);
  assert.deepEqual(errors, []);
  assert.match(await page.locator("#editor-status").innerText(), /快捷键/);
  await page.close();
});

test("CodeMirror command bindings can be moved or cleared while normal navigation and readonly remain intact", async () => {
  const { page } = await setup("one\ntwo");
  await select(page, 0);
  await shortcuts(page, { "editor.move-line-down": "Ctrl+Alt+J", "editor.delete-line": "", "editor.indent-more": "Ctrl+Alt+I" });
  await page.keyboard.press("Alt+ArrowDown");
  assert.equal((await state(page)).text, "one\ntwo");
  await page.keyboard.press("Control+Alt+j");
  assert.equal((await state(page)).text, "two\none");
  await page.keyboard.press("Control+Shift+k");
  assert.equal((await state(page)).text, "two\none");
  await page.keyboard.press("Control+Home");
  assert.deepEqual((await state(page)).selection, { anchor: 0, head: 0 });
  await page.keyboard.press("ArrowRight");
  assert.equal((await state(page)).selection.head, 1);
  await page.keyboard.press("Control+Alt+i");
  assert.match((await state(page)).text, /^\s+two/);
  const before = await state(page);
  await page.evaluate(() => window.studyappEditor.setReadonly(true));
  await page.keyboard.press("Control+Alt+j");
  assert.equal((await state(page)).text, before.text);
  await page.close();
});

test("large multiline UTF-8 documents load and edit beyond 10 MiB", async () => {
 const text = "中文😀\n".repeat(1000000);
 const {page}=await setup(text);
 assert.equal((await state(page)).text,text);
 await action(page,{type:"replace",from:0,text:"新增\n"});
 assert.equal((await state(page)).text,"新增\n"+text);
 await page.close();
});

test("outline parses headings and navigates duplicate Unicode headings", async () => {
 const text="# 中文\n\n```md\n# hidden\n```\n\n中文\n---\n\n# 中文";
 const {page}=await setup(text);
 await page.locator("#outline-toggle").click();
 assert.equal(await page.locator("#note-outline .outline-label").count(),3);
 await page.locator("#note-outline .outline-label").last().click();
 assert.equal((await state(page)).selection.anchor,text.lastIndexOf("# 中文"));
 await page.evaluate(() => window.studyappEditor.load({generation:42,text:"# 新笔记",readonly:false}));
 await page.waitForTimeout(400);
 assert.equal(await page.locator("#note-outline .outline-label").count(),1);
 await action(page,{type:"replace",from:0,text:"## 新标题\n\n"});
 await page.waitForTimeout(400);
 assert.equal(await page.locator("#note-outline .outline-label").count(),2);

 await page.locator("#outline-toggle").click();
 assert.equal(await page.locator("#note-outline").isVisible(),false);
 assert.equal((await state(page)).text,"## 新标题\n\n# 新笔记");
 await page.close();
});

test("a formatting transaction over 4096 patches is refused without losing the document", async () => {
  const text = Array.from({ length: 2050 }, (_, index) => `line ${index}`).join("\n");
  const { page } = await setup(text);
  await select(page, 0, text.length);
  await page.evaluate(() => window.studyappEditor.command("bold"));
  assert.ok((await state(page)).text === text, "oversized format must preserve the original document");
  assert.equal((await state(page)).revision, 0);
  assert.equal((await state(page)).readonly, false);
  assert.match(await page.locator("#editor-status").innerText(), /4096/);
  assert.equal(await page.evaluate(() => window.messages.filter(m => m.type === "change" || m.type === "error").length), 0);
  await select(page, 0);
  await page.keyboard.insertText("!");
  assert.equal((await state(page)).text, "!" + text);
  await page.close();
});

test("raw-byte budget preserves mixed EOL widths and counts preferred inserted CRLF exactly", async () => {
  const { page } = await setup("a\r\nb\nc\rd");
  assert.equal((await state(page)).rawBytes, 8);
  assert.equal((await state(page)).utf8Bytes, 7);
  await action(page, { type: "replace", from: 1, to: 4, text: "" });
  assert.equal((await state(page)).rawBytes, 4);
  assert.equal((await state(page)).text, "ac\nd");
  await action(page, { type: "replace", from: 1, text: "\n" });
  assert.equal((await state(page)).rawBytes, 6);
  assert.equal((await state(page)).utf8Bytes, 5);
  await page.close();
});

test("a CRLF note can grow beyond 10 MiB while preserving exact EOL byte accounting", async () => {
  const { page } = await setup("");
  const report = await page.evaluate(async () => {
    const limit = 10 * 1024 * 1024;
    window.studyappEditor.load({ generation: 42, text: "a".repeat(limit - 2) + "\r\n", readonly: false });
    const before = await window.studyappEditor.test({ type: "snapshot" });
    await window.studyappEditor.test({ type: "replace", from: before.text.length, text: "\n" });
    const rejected = await window.studyappEditor.test({ type: "snapshot" });
    await window.studyappEditor.test({ type: "replace", from: 0, to: 2, text: "" });
    await window.studyappEditor.test({ type: "replace", from: 0, text: "\n" });
    const after = await window.studyappEditor.test({ type: "snapshot" });
    return { beforeBytes: before.rawBytes, beforeNormalized: before.utf8Bytes, rejectedRevision: rejected.revision,
      afterBytes: after.rawBytes, afterRevision: after.revision, readonly: after.readonly };
  });
  assert.deepEqual(report, { beforeBytes: 10 * 1024 * 1024, beforeNormalized: 10 * 1024 * 1024 - 1,
    rejectedRevision: 1, afterBytes: 10 * 1024 * 1024 + 2, afterRevision: 3, readonly: false });
  await page.close();
});

test("narrow document pane keeps every toolbar action reachable", async () => {
  const { page } = await setup("# Narrow pane\n\nEditable content");
  await page.setViewportSize({ width: 280, height: 480 });
  const layout = await page.evaluate(() => {
    const toolbar = document.querySelector("#toolbar").getBoundingClientRect();
    return {
      clipped: [...document.querySelectorAll("#toolbar button")].filter(button => {
        const rect = button.getBoundingClientRect();
        return rect.left < 0 || rect.right > 280 || rect.bottom > toolbar.bottom;
      }).map(button => button.getAttribute("aria-label")),
      editorHeight: document.querySelector("#editor").getBoundingClientRect().height
    };
  });
  assert.deepEqual(layout.clipped, []);
  assert.ok(layout.editorHeight > 300);
  await page.locator("#source-toggle").click();
  assert.equal((await state(page)).source, true);
  await page.close();
});

test("250 KiB document keeps viewport DOM bounded during 200 edits", async () => {
  const text = Array.from({ length: 3500 }, (_, i) => `## 标题 ${i}\n\nContinuous Markdown paragraph ${i} has **strong text**, a [link](https://example.com), 中文与 😀.\n\n`).join("");
  assert.ok(Buffer.byteLength(text) > 250 * 1024);
  const { page } = await setup(text);
  const report = await action(page, { type: "performance", edits: 200 });
  assert.equal(report.edits, 200);
  assert.ok(report.domNodes < 2500, JSON.stringify(report));
  assert.ok(report.renderedLines < 200, JSON.stringify(report));
  assert.ok(report.p95Ms < 50, JSON.stringify(report));
  const snapshot = await state(page);
  assert.equal(snapshot.text, "x".repeat(200) + text);
  assert.equal(snapshot.revision, 200);
  await mkdir(new URL("../test-results/", import.meta.url), { recursive: true });
  await writeFile(new URL("../test-results/performance.json", import.meta.url), JSON.stringify({ bytes: Buffer.byteLength(text), ...report }, null, 2));
  console.log("PERFORMANCE", JSON.stringify(report));
  await page.screenshot({ path: fileURLToPath(new URL("../test-results/continuous-editor.png", import.meta.url)) });
  await page.close();
});

test("raw image destinations fill the reading column while prose keeps its policy", async () => {
  const uri = "studyapp-media:13826f00-5c18-470e-9e8e-cc7d70c9bb92/20261025T202943360-新笔记-8363783a58b947f19b235d492f36d588.png";
  const prose = "A normal paragraph keeps natural word boundaries and its configured reading width. ".repeat(15);
  const text = `![图片](${uri})\n\n${prose}\n\n\`inline code\`\n\n\`\`\`text\n${uri}\n\`\`\``;
  const { page, externalRequests } = await setup(text);
  const measurements = [];
  for (const source of [false, true]) {
    if (source) await page.locator("#source-toggle").click();
    for (const width of [380, 600, 770, 960, 1100, 1600]) for (const reading of [720, 920]) {
      await page.setViewportSize({ width, height: 800 });
      await page.evaluate(reading => window.studyappEditor.setTheme({ markdown: { reading_width: reading } }), reading);
      await select(page, 8);
      await page.waitForTimeout(40);
      const geometry = await page.evaluate(() => {
        const line = document.querySelector(".cm-line");
        const rect = line.getBoundingClientRect();
        const walker = document.createTreeWalker(line, NodeFilter.SHOW_TEXT);
        const rows = new Map();
        for (let node; (node = walker.nextNode());) for (let i = 0; i < node.length; i++) {
          const range = document.createRange(); range.setStart(node, i); range.setEnd(node, i+1);
          const r = range.getBoundingClientRect(); if (!r.width || !r.height) continue;
          const key = Math.round(r.top), prior = rows.get(key) || { left:r.left, right:r.right };
          prior.left = Math.min(prior.left,r.left); prior.right = Math.max(prior.right,r.right); rows.set(key,prior);
        }
        const prose = [...document.querySelectorAll(".cm-line")].find(line => line.textContent.startsWith("A normal"));
        const proseCss = getComputedStyle(prose);
        return { width:rect.width, lines:[...rows.values()].map(r=>r.right-r.left),
          overflow:document.documentElement.scrollWidth > innerWidth,
          proseWordBreak:proseCss.wordBreak, proseWidth:prose.getBoundingClientRect().width,
          hasProseUrl:!!prose.querySelector(".md-raw-url"),
          sourceUrlSpans:line.querySelectorAll(".md-raw-url").length,
          decoratedCode:document.querySelectorAll(".md-inline-code .md-raw-url, .md-code-line .md-raw-url").length };
      });
      measurements.push({ source, viewport:width, reading, ...geometry });
      const expected = Math.min(reading, width - (width <= 600 ? 36 : 52));
      assert.ok(Math.abs(geometry.width-expected) <= 3, JSON.stringify({source,width,reading,geometry,expected}));
      assert.ok(geometry.lines[0] >= geometry.width-24, `raw URL wastes reading width: ${JSON.stringify({source,width,reading,geometry})}`);
      assert.equal(geometry.overflow,false);
      assert.notEqual(geometry.proseWordBreak,"break-all");
      assert.equal(geometry.hasProseUrl,false);
      assert.equal(geometry.sourceUrlSpans,1);
      assert.equal(geometry.decoratedCode,0);
      assert.ok(Math.abs(geometry.proseWidth-geometry.width) <= 1);
      assert.equal((await state(page)).text,text);
    }
  }
  await select(page, text.indexOf(uri)+10);
  await page.keyboard.insertText("x");
  assert.equal((await state(page)).text,text.slice(0,text.indexOf(uri)+10)+"x"+text.slice(text.indexOf(uri)+10));
  await page.keyboard.press("Control+z");
  assert.equal((await state(page)).text,text);
  await mkdir(new URL("../test-results/", import.meta.url), { recursive: true });
  await writeFile(new URL("../test-results/reading-geometry.json", import.meta.url),JSON.stringify(measurements,null,2));
  assert.deepEqual(externalRequests, []);
  await page.close();
});


test("outline invalidates stale entries immediately and remains usable in readonly notes", async () => {
 const {page,errors}=await setup("# First\n\n"+"body\n".repeat(40)+"# Last");
 await page.locator("#outline-toggle").click();
 const result=await page.evaluate(async()=>{
   const old=document.querySelector("#note-outline .outline-label:last-child");
   await window.studyappEditor.load({generation:43,text:"# 短",readonly:true});
   const disabled=old.disabled;
   old.click();
   return {disabled,toggleDisabled:document.querySelector("#outline-toggle").disabled};
 });
 assert.deepEqual(result,{disabled:true,toggleDisabled:false});
 await page.waitForTimeout(400);
 await page.locator("#note-outline .outline-label").click();
 assert.equal((await state(page)).selection.anchor,0);
 assert.deepEqual(errors,[]);
 await page.close();
});

test("outline stays hidden in appearance preview and returns for notes", async () => {
 const {page}=await setup("# Note");
 await page.evaluate(()=>window.studyappEditor.load({generation:44,text:"# Theme",readonly:true,outlineEnabled:false}));
 assert.equal(await page.locator("#outline-toggle").isVisible(),false);
 assert.equal(await page.locator("#note-outline").isVisible(),false);
 await page.evaluate(()=>window.studyappEditor.load({generation:45,text:"# Note",readonly:false,outlineEnabled:true}));
 assert.equal(await page.locator("#outline-toggle").isVisible(),true);
 assert.equal(await page.locator("#note-outline").isVisible(),false);
 await page.locator("#outline-toggle").click();
 assert.equal(await page.locator("#note-outline .outline-label").innerText(),"Note");
 await page.close();
});

test("outline branches fold independently, preserve duplicate headings and support keyboard navigation", async () => {
  const text = "# 根\n\n## 子章\n\n#### 跳级\n\n## 子章\n\n# 第二章\n\n正文";
  const { page, errors } = await setup(text);
  await page.locator("#outline-toggle").click();
  const roots = page.locator('.outline-tree > [role="treeitem"]');
  assert.equal(await roots.count(), 2);
  assert.equal(await page.locator('.outline-tree [role="treeitem"]').count(), 5);
  const first = roots.first();
  await first.locator(':scope > .outline-row .outline-disclosure').click();
  assert.equal(await first.getAttribute('aria-expanded'), 'false');
  assert.equal(await page.locator('.outline-label:visible').count(), 2);
  assert.equal((await state(page)).selection.anchor, 0, 'folding never moves the document caret');
  await page.keyboard.press('ArrowRight');
  assert.equal(await first.getAttribute('aria-expanded'), 'true');
  await page.keyboard.press('ArrowRight');
  assert.equal(await page.evaluate(() => document.activeElement.getAttribute('aria-label')), '子章');
  await page.keyboard.press('ArrowRight');
  assert.equal(await page.evaluate(() => document.activeElement.getAttribute('aria-level')), '3', 'skipped levels still form a real tree');
  await page.keyboard.press('Enter');
  assert.equal((await state(page)).selection.anchor, text.indexOf('####'));
  await page.getByRole('button', { name: '全部收起', exact: true }).click();
  assert.equal(await page.locator('.outline-label:visible').count(), 2);
  await page.getByRole('button', { name: '全部展开', exact: true }).click();
  await page.locator('.outline-label').nth(3).click();
  assert.equal((await state(page)).selection.anchor, text.lastIndexOf('## 子章'));
  assert.deepEqual(errors, []);
  await page.close();
});

test("long outline has its own scroll viewport without moving the document or page", async () => {
  const text = Array.from({ length: 90 }, (_, i) => `# 章节 ${i}\n\n${'正文内容。'.repeat(12)}\n`).join('\n');
  const { page, errors } = await setup(text);
  await page.setViewportSize({ width: 460, height: 600 });
  await page.locator('#outline-toggle').click();
  const tree = page.locator('.outline-tree');
  const bounds = await tree.boundingBox();
  assert.ok(bounds.height > 50 && bounds.height < 210);
  const before = await page.locator('.cm-scroller').evaluate(node => node.scrollTop);
  await tree.hover();
  await page.mouse.wheel(0, 400);
  await page.waitForTimeout(100);
  assert.ok(await tree.evaluate(node => node.scrollTop) > 0);
  assert.equal(await page.locator('.cm-scroller').evaluate(node => node.scrollTop), before);
  await tree.evaluate(node => { node.scrollTop = node.scrollHeight; });
  await page.mouse.wheel(0, 400);
  await page.waitForTimeout(100);
  assert.equal(await page.locator('.cm-scroller').evaluate(node => node.scrollTop), before);
  assert.equal(await page.evaluate(() => document.documentElement.scrollTop), 0);
  await page.locator('.outline-label').last().click();
  await page.waitForTimeout(100);
  assert.ok(await page.locator('.cm-scroller').evaluate(node => node.scrollTop) > 100);
  assert.equal(await page.locator('[aria-current="location"] > .outline-row .outline-label').innerText(), '章节 89');
  assert.deepEqual(errors, []);
  await page.close();
});

test("outline automatically follows editor pane geometry rather than screen width", async () => {
  const { page, errors } = await setup("# 第一章\n\n正文\n\n## 第二节\n\n正文");
  await page.locator("#outline-toggle").click();
  const placement = () => page.locator("#editor-workspace").getAttribute("data-outline-position");
  assert.equal(await placement(), "left");
  const wide = await page.locator("#editor").boundingBox();
  const outline = await page.locator("#note-outline").boundingBox();
  assert.ok(outline.x + outline.width <= wide.x + 1);
  // Resize only the embedded pane, just as native splitters/sidebar changes do.
  await page.evaluate(() => { const main = document.querySelector("main"); main.style.width = "650px"; main.style.height = "680px"; });
  await page.waitForFunction(() => document.querySelector("#editor-workspace").dataset.outlinePosition === "top");
  const top = await page.locator("#note-outline").boundingBox();
  const document = await page.locator("#editor").boundingBox();
  assert.ok(top.y + top.height <= document.y + 1);
  await page.evaluate(() => { document.querySelector("main").style.height = "340px"; });
  await page.waitForFunction(() => document.querySelector("#editor-workspace").dataset.outlinePosition === "floating");
  assert.ok((await page.locator("#editor").boundingBox()).height > 220);
  assert.equal(await page.locator(".outline-label").count(), 2);
  assert.deepEqual(errors, []);
  await page.close();
});

test("outline manual placement and visibility survive reload without editing the note", async () => {
  const { page, errors } = await setup("# 我的目录\n\n## 子章节\n\n正文");
  const initial = await state(page);
  await page.locator("#outline-toggle").click();
  for (const value of ["right", "top", "left", "floating"]) {
    await page.locator("#outline-position").selectOption(value);
    assert.equal(await page.locator("#editor-workspace").getAttribute("data-outline-position"), value);
  }
  await page.locator("#outline-position").selectOption("right");
  await page.reload();
  await page.evaluate(() => window.studyappEditor.load({ generation: 42, text: "# 我的目录\n\n## 子章节\n\n正文", readonly: false }));
  assert.equal(await page.locator("#note-outline").isVisible(), true);
  assert.equal(await page.locator("#outline-position").inputValue(), "right");
  assert.equal(await page.locator("#outline-toggle svg").count(), 1);
  assert.equal(await page.locator("#outline-toggle").getAttribute("aria-label"), "隐藏目录");
  await page.getByRole("button", { name: "隐藏目录", exact: true }).last().click();
  assert.equal(await page.locator("#note-outline").isVisible(), false);
  assert.equal(await page.locator("#outline-toggle").getAttribute("aria-label"), "显示目录");
  await page.locator("#outline-toggle").click();
  await page.getByRole("treeitem").first().focus();
  await page.keyboard.press("Escape");
  assert.equal(await page.locator("#note-outline").isVisible(), false);
  assert.equal((await state(page)).text, initial.text);
  assert.equal((await state(page)).revision, 0);
  await page.reload();
  assert.equal(await page.locator("#note-outline").isVisible(), false);
  await page.locator("#outline-toggle").click();
  await page.locator("#outline-position").focus();
  await page.keyboard.press("Escape");
  assert.equal(await page.locator("#note-outline").isVisible(), false);
  assert.equal(await page.locator("#outline-toggle").evaluate(element => element === document.activeElement), true);
  assert.equal(initial.revision, 0);
  assert.deepEqual(errors, []);
  await page.close();
});

test("manual outline sidebars preserve document space in narrow panes and restore on growth", async () => {
  const { page, errors } = await setup("# Chapter\n\nText");
  await page.locator("#outline-toggle").click();
  await page.locator("#outline-position").selectOption("left");
  await page.setViewportSize({ width: 340, height: 600 });
  await page.waitForFunction(() => document.querySelector("#editor-workspace").dataset.outlinePosition === "floating");
  assert.ok((await page.locator("#editor").boundingBox()).width >= 330);
  const panel = await page.locator("#note-outline").boundingBox();
  assert.ok(panel.x >= 0 && panel.x + panel.width <= 340);
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
  await page.setViewportSize({ width: 1100, height: 800 });
  await page.waitForFunction(() => document.querySelector("#editor-workspace").dataset.outlinePosition === "left");
  assert.equal(await page.locator("#outline-position").inputValue(), "left");
  assert.deepEqual(errors, []);
  await page.close();
});

test("outline remains usable when browser preference storage is unavailable", async () => {
  const { page } = await setup("# Storage independent");
  await page.addInitScript(() => { Object.defineProperty(window, "localStorage", { get() { throw new DOMException("blocked", "SecurityError"); } }); });
  await page.reload();
  await page.evaluate(() => window.studyappEditor.load({ generation: 42, text: "# Safe", readonly: false }));
  await page.locator("#outline-toggle").click();
  await page.locator("#outline-position").selectOption("right");
  assert.equal(await page.locator(".outline-label").innerText(), "Safe");
  await page.locator(".outline-label").click();
  assert.equal((await state(page)).text, "# Safe");
  await page.close();
});

test("native primary selection covers glyphs above opaque Markdown surfaces", async () => {
  const text = "# 标题字符\n\n> 引用字符\n\n`内联字符`\n\n```text\n代码字符\n```\n\n| 表头字符 | B |\n| --- | --- |\n| 表格字符 | D |\n\n结束";
  const {page, errors} = await setup(text);
  await page.evaluate(text => window.studyappEditor.load({generation:42,text,readonly:true}), text);
  for (const word of ["标题字符", "引用字符", "内联字符", "代码字符", "表头字符", "表格字符"]) {
    const at = text.indexOf(word);
    await select(page, at);
    const before = await page.screenshot();
    await select(page, at, at + word.length);
    await page.waitForTimeout(50);
    const rect = await page.evaluate(() => {
      const range = getSelection().getRangeAt(0), r = range.getBoundingClientRect();
      return {x:r.x,y:r.y,width:r.width,height:r.height,text:getSelection().toString()};
    });
    assert.equal(rect.text, word);
    assert.ok(rect.width > 20 && rect.height > 10, `${word}: visible glyph range`);
    const after = await page.screenshot();
    const changed = await page.evaluate(async ({a,b,r}) => {
      const read = async value => {
        const bytes = Uint8Array.from(atob(value), ch => ch.charCodeAt(0));
        const image = await createImageBitmap(new Blob([bytes], {type:"image/png"}));
        const canvas = new OffscreenCanvas(image.width,image.height), context = canvas.getContext("2d");
        context.drawImage(image,0,0);image.close();
        return context.getImageData(Math.ceil(r.x)+2, Math.ceil(r.y)+2, Math.floor(r.width)-4, Math.floor(r.height)-4).data;
      };
      const [one,two] = await Promise.all([read(a),read(b)]);let changed=0;
      for(let i=0;i<one.length;i+=4) if(Math.abs(one[i]-two[i])+Math.abs(one[i+1]-two[i+1])+Math.abs(one[i+2]-two[i+2])>30) changed++;
      return changed/(one.length/4);
    }, {a:before.toString("base64"),b:after.toString("base64"),r:rect});
    assert.ok(changed > .35, `${word}: selection must paint inside the text bounds, not just the surrounding background (${changed})`);
    await select(page, at + word.length, at);
    assert.equal(await page.evaluate(() => getSelection().toString()), word, "reverse selection");
  }
  assert.equal((await state(page)).text, text);
  assert.deepEqual(errors, []);
  await page.close();
});

test("right-click copy preserves selection and uses only an explicit copy event", async () => {
  const text = "第一行 中文🙂\n第二行 copy";
  const {page, errors} = await setup(text);
  // Exercise CodeMirror's real copy handler with an in-memory DataTransfer;
  // never call the operating system clipboard during local headless tests.
  await page.evaluate(() => {
    window.copied = [];
    document.execCommand = command => {
      if(command !== "copy") throw new Error("unexpected clipboard operation");
      const data = new DataTransfer();
      document.querySelector(".cm-content").dispatchEvent(new ClipboardEvent("copy", {clipboardData:data,bubbles:true,cancelable:true}));
      window.copied.push(data.getData("text/plain"));
      return true;
    };
  });
  await select(page, 0, text.length);
  const before = await state(page);
  await page.locator(".cm-content").dispatchEvent("contextmenu", {clientX:1095,clientY:795});
  assert.equal(await page.locator("#editor-copy-menu").isVisible(), true);
  const box = await page.locator("#editor-copy-menu").boundingBox();
  assert.ok(box.x + box.width <= 1100 && box.y + box.height <= 800);
  assert.deepEqual(await page.evaluate(() => window.copied), []);
  await page.locator("#editor-copy-menu button").click();
  assert.deepEqual(await page.evaluate(() => window.copied), [text]);
  assert.deepEqual((await state(page)).selection, before.selection);
  assert.equal((await state(page)).text, text);
  await page.keyboard.press("Shift+F10");
  assert.equal(await page.locator("#editor-copy-menu").isVisible(), true);
  await page.keyboard.press("Escape");
  assert.equal(await page.locator("#editor-copy-menu").isVisible(), false);
  await select(page, 0);
  await page.keyboard.press("Shift+F10");
  assert.equal(await page.locator("#editor-copy-menu button").isDisabled(), true);
  await page.keyboard.press("Escape");
  assert.deepEqual(errors, []);
  await page.close();
});

test("selection colors are customizable, resettable and warn without replacing explicit styles", async () => {
  const {page} = await setup("文字🙂 `code`\n\n> quote\n\nEnd");
  const selectedStyle = () => {
    const node = document.querySelector(".cm-line");
    const css = getComputedStyle(node,"::selection");
    return {background:css.backgroundColor,color:css.color};
  };
  await page.evaluate(() => window.studyappEditor.setTheme({surface:"#ffffff",text:"#202020",markdown:{colors:{selection_background:"#2A5577",selection_text:"#FFFFFF"}}}));
  assert.deepEqual(await page.evaluate(selectedStyle), {background:"rgb(42, 85, 119)",color:"rgb(255, 255, 255)"});
  await select(page,0,4);
  const before = await state(page);
  await page.evaluate(() => window.studyappEditor.setTheme({surface:"#ffffff",text:"#202020",markdown:{colors:{selection_background:"#ffffff",selection_text:"#fefefe"}}}));
  assert.equal(await page.locator("#editor-theme-warning").isVisible(),true);
  assert.deepEqual(await page.evaluate(selectedStyle), {background:"rgb(255, 255, 255)",color:"rgb(254, 254, 254)"});
  assert.deepEqual((await state(page)).selection,before.selection);
  assert.equal((await state(page)).revision,before.revision);
  await page.evaluate(() => window.studyappEditor.setTheme({mode:"dark",surface:"#272A30",text:"#EFF0F3",markdown:{}}));
  assert.notEqual((await page.evaluate(selectedStyle)).background,"rgb(255, 255, 255)");
  await page.close();
});

test("copy menu keyboard activation never edits the note and closes on selection changes", async () => {
  const {page} = await setup("copy 中文🙂 unchanged");
  await page.evaluate(() => { window.copyCount=0;document.execCommand=command=>{if(command==="copy")window.copyCount++;return true;}; });
  await select(page,0,4);
  const before = await state(page);
  await page.keyboard.press("Shift+F10");
  await page.keyboard.press("Enter");
  assert.equal(await page.evaluate(() => window.copyCount),1);
  const after = await state(page);
  assert.equal(after.text,before.text);
  assert.equal(after.revision,before.revision);
  assert.deepEqual(after.selection,before.selection);
  await page.keyboard.press("Shift+F10");
  await page.keyboard.press("ArrowRight");
  assert.equal(await page.locator("#editor-copy-menu").isVisible(),false);
  await page.close();
});

test("native primary selection retains secondary CodeMirror carets and multiple edits", async () => {
  const {page} = await setup("first\nsecond\nthird");
  await shortcuts(page,{"editor.add-cursor-below":"Alt+j"});
  await select(page,2);
  await page.keyboard.press("Alt+j");
  const selected = (await state(page)).selections;
  assert.equal(selected.length,2);
  assert.ok(await page.locator(".cm-secondary-cursor").count()>=1);
  await page.keyboard.insertText("X");
  const expected = selected.slice().sort((a,b)=>b.head-a.head).reduce((text,range)=>text.slice(0,range.head)+"X"+text.slice(range.head),'first\nsecond\nthird');
  assert.equal((await state(page)).text,expected);
  await page.close();
});

test("Markdown content stays within the remaining pane with wide tables and a sidebar", async () => {
  const columns = 25;
  const text = '# Layout\n\n| ' + 'cell | '.repeat(columns) + '\n|' + ' --- |'.repeat(columns) + '\n|' + '内容 |'.repeat(columns) + '\n\nOrdinary prose remains in the configured reading column.';
  const {page, errors} = await setup(text);
  await page.evaluate(text => window.studyappEditor.load({generation:50,text,readonly:true}), text);
  await page.locator('#outline-toggle').click();
  for (const width of [1600, 1100, 770, 600, 380, 1100]) {
    await page.setViewportSize({width,height:800});
    for (const position of ['left','right','top','floating']) {
      await page.locator('#outline-position').selectOption(position);
      await page.waitForTimeout(45);
      const geometry = await page.evaluate(() => {
        const pane=document.querySelector('#editor'), content=document.querySelector('.cm-content');
        const prose=[...document.querySelectorAll('.cm-line')].find(line=>line.textContent.startsWith('Ordinary'));
        return {pane:pane.clientWidth,content:content.getBoundingClientRect().width,prose:prose.getBoundingClientRect().width};
      });
      assert.ok(Math.abs(geometry.content-geometry.pane)<=1, JSON.stringify({width,position,geometry}));
      const expected=geometry.pane-(geometry.pane<=600?36:52);
      assert.ok(Math.abs(geometry.prose-expected)<=2, JSON.stringify({width,position,geometry,expected}));
    }
  }
  assert.deepEqual(errors,[]);
  assert.equal((await state(page)).text,text);
  await page.close();
});

test("Markdown editor follows remaining width and height without reloading the document", async () => {
  const text = `# Responsive document

A paragraph that survives changes to the surrounding workspace.

`.repeat(30);
  const { page, errors, externalRequests } = await setup(text);
  await select(page, 25, 42);
  const before = await state(page);
  // Resize only the host surface: sidebar/header changes do not necessarily
  // resize the browser window or issue a corrective bridge layout command.
  for (const readonly of [false, true]) {
    await page.evaluate(readonly => window.studyappEditor.setReadonly(readonly), readonly);
    for (const position of ["auto", "left", "right", "top", "floating"]) {
      await page.evaluate(position => window.studyappEditor.setOutlinePreference({ position, shown: true }), position);
      for (const [width, height] of [[1100, 760], [680, 500], [420, 300], [280, 150], [1000, 700]]) {
        await page.evaluate(({ width, height }) => {
          const host = document.querySelector("main");
          host.style.width = `${width}px`;
          host.style.height = `${height}px`;
        }, { width, height });
        await page.waitForTimeout(50);
        const geometry = await page.evaluate(() => {
          const rect = selector => {
            const r = document.querySelector(selector).getBoundingClientRect();
            return { x: r.x, y: r.y, width: r.width, height: r.height, right: r.right, bottom: r.bottom };
          };
          return { host: rect("main"), pane: rect("#editor"), editor: rect(".cm-editor"), footer: rect("#editor-footer") };
        });
        const context = JSON.stringify({ readonly, position, width, height, geometry });
        assert.ok(Math.abs(geometry.host.width - width) <= 1, context);
        assert.ok(Math.abs(geometry.host.height - height) <= 1, context);
        for (const field of ["x", "y", "width", "height"]) {
          assert.ok(Math.abs(geometry.editor[field] - geometry.pane[field]) <= 1, context);
        }
        assert.ok(geometry.pane.x >= geometry.host.x && geometry.pane.right <= geometry.host.right + 1, context);
        assert.ok(geometry.pane.height > 0, context);
        assert.ok(Math.abs(geometry.pane.bottom - geometry.footer.y) <= 1, context);
        assert.ok(Math.abs(geometry.footer.bottom - geometry.host.bottom) <= 1, context);
      }
    }
  }
  const after = await state(page);
  for (const field of ["generation", "revision", "text", "selection"]) {
    assert.deepEqual(after[field], before[field], `geometry changed document ${field}`);
  }
  assert.deepEqual(errors, []);
  assert.deepEqual(externalRequests, []);
  await page.close();
});

test("preview caret activation survives rendered widgets, wrapping and host mode changes", async () => {
  const text = '# 标题 😀\n\n**粗体** *斜体* ~~删除~~ `inline code`\n\n- 项目 **加粗**\n- [ ] 任务\n\n> 引用\n\n---\n\n![图片](missing.png)\n\n| 表头一 | 表头二 |\n| --- | --- |\n| 数据😀 | 内容 |\n\n```js\nconst x = null;\n```\n\n末尾段落';
  const {page,errors}=await setup(text);
  const stacks=[];
  page.on('pageerror',error=>stacks.push(error.stack));
  // A missing measured line must never be consulted by readonly mousedown.
  // This deterministic fault is test-build-only and leaves DOM caret APIs real.
  await action(page,{type:"unavailablePreviewGeometry"});
  // Model the host's asynchronous requestEdit acknowledgement, including the
  // readonly compartment change and UTF-16 selection restoration.
  await page.evaluate(()=>{window.ipc={postMessage:message=>{
    const value=JSON.parse(message);window.messages.push(value);
    if(value.type==='requestEdit') setTimeout(()=>{
      window.studyappEditor.setReadonly(false);
      window.studyappEditor.restoreSelection(value.position);
    },0);
  }};});
  for(const width of [1100,570,380,900]){
    await page.setViewportSize({width,height:800});
    await page.evaluate(text=>window.studyappEditor.load({generation:51,text,readonly:true,canRequestEdit:true}),text);
    await page.waitForTimeout(30);
    const points=await page.locator('.cm-line').evaluateAll(lines=>lines.flatMap(line=>{
      const r=line.getBoundingClientRect();
      return [2,Math.min(65,r.width/2),r.width-2].map(x=>({x:r.left+x,y:r.top+r.height/2}));
    }).filter(p=>p.y>70&&p.y<740));
    for(const point of points){
      await page.evaluate(()=>window.studyappEditor.setReadonly(true));
      await page.mouse.click(point.x,point.y);
      await page.waitForTimeout(5);
      assert.deepEqual(stacks,[],JSON.stringify({width,point}));
    }
    assert.equal((await state(page)).text,text);
  }
  const requests=await page.evaluate(()=>window.messages.filter(m=>m.type==='requestEdit'));
  assert.ok(requests.length>40);
  assert.ok(requests.every(r=>r.generation===51&&Number.isInteger(r.position)&&r.position>=0&&r.position<=text.length));
  assert.deepEqual(errors,[]);
  await page.close();
});

test("readonly DOM hit testing preserves word, line, forward and reverse drag selections", async () => {
  const text="Alpha beta gamma\n\nDelta epsilon zeta\n\nLast paragraph";
  const {page,errors}=await setup(text);
  await page.evaluate(text=>window.studyappEditor.load({generation:52,text,readonly:true}),text);
  await action(page,{type:"unavailablePreviewGeometry"});
  const point=async(word,end=false)=>page.locator('.cm-content').evaluate((content,{word,end})=>{
    const walker=document.createTreeWalker(content,NodeFilter.SHOW_TEXT);
    for(let node;node=walker.nextNode();){
      const index=node.nodeValue.indexOf(word);
      if(index<0)continue;
      const range=document.createRange();range.setStart(node,index+(end?word.length:0));range.collapse(true);
      const box=range.getBoundingClientRect();return{x:box.left+.1,y:box.top+box.height/2};
    }
    throw new Error('missing rendered word');
  },{word,end});
  const selected=async()=>{const s=await state(page);return text.slice(Math.min(s.selection.anchor,s.selection.head),Math.max(s.selection.anchor,s.selection.head));};
  const beta=await point('beta');
  await page.mouse.click(beta.x+4,beta.y,{clickCount:2});
  assert.equal(await selected(),'beta');
  await page.mouse.click(beta.x+4,beta.y,{clickCount:3});
  assert.equal(await selected(),'Alpha beta gamma\n');
  const start=await point('Alpha'),end=await point('zeta',true);
  await select(page,0);
  await page.mouse.move(start.x,start.y);await page.mouse.down();
  await page.mouse.move(end.x,end.y,{steps:12});await page.mouse.up();
  const forward=await selected();assert.ok(forward.includes('beta gamma\n\nDelta epsilon'));
  await select(page,0);
  await page.mouse.move(end.x,end.y);await page.mouse.down();
  await page.mouse.move(start.x,start.y,{steps:12});await page.mouse.up();
  assert.equal(await selected(),forward);
  await select(page,0);
  await page.mouse.move(start.x,start.y);await page.mouse.down();
  await page.mouse.move(end.x,799,{steps:12});await page.mouse.up();
  assert.ok((await selected()).includes('Last paragraph'));
  assert.equal((await state(page)).text,text);
  assert.deepEqual(errors,[]);
  assert.equal(await page.evaluate(()=>window.messages.filter(m=>m.type==='requestEdit').length),0);
  await page.close();
});

test("RGBA Markdown tokens preserve alpha, use composited contrast and never change the document", async () => {
  const {page,errors,externalRequests}=await setup("# Alpha\n\n> Quote\n\n| A | B |\n| --- | --- |\n| one | two |\n\n```rust\nlet x = 1;\n```");
  const before=await state(page);
  await page.evaluate(()=>window.studyappEditor.setTheme({surface:"#ffffff",text:"#202020",markdown:{colors:{body:"#00000000",quote_background:"#FF000080",code_background:"#0000FF40",selection_background:"#33669966",selection_text:"#FFFFFFCC"}}}));
  const colors=await page.evaluate(()=>{
    const root=getComputedStyle(document.documentElement);
    const read=name=>root.getPropertyValue("--editor-"+name).trim();
    return Object.fromEntries(["text","quote-background","code-background","selection","selection-text"].map(name=>[name,read(name)]));
  });
  assert.deepEqual(colors,{text:"#00000000","quote-background":"#FF000080","code-background":"#0000FF40",selection:"#33669966","selection-text":"#FFFFFFCC"});
  assert.equal(await page.locator("#editor-theme-warning").isVisible(),true,"fully transparent black is not readable black");
  const quote=await page.locator(".md-quote").first().evaluate(node=>getComputedStyle(node).backgroundColor);
  const channels=quote.match(/[\d.]+/g).map(Number);
  assert.deepEqual(channels.slice(0,3),[255,0,0]);
  assert.ok(Math.abs(channels[3]-128/255)<1/255,"computed alpha differs by more than one byte");
  await page.evaluate(()=>window.studyappEditor.setTheme({surface:"#ffffff",text:"#202020",markdown:{colors:{body:"#000000",quote_background:"#FF000000",selection_background:"#336699FF",selection_text:"#FFFFFF"}}}));
  assert.equal(await page.locator(".md-quote").first().evaluate(node=>getComputedStyle(node).backgroundColor),"rgba(255, 0, 0, 0)");
  await page.evaluate(()=>window.studyappEditor.setTheme({markdown:{colors:{body:"#12345678;url(https://example.invalid/x)",quote_background:"#1234567"}}}));
  assert.equal(await page.evaluate(()=>getComputedStyle(document.documentElement).getPropertyValue("--editor-quote-background").includes("url(")),false);
  const after=await state(page);
  assert.equal(after.text,before.text);assert.equal(after.revision,before.revision);assert.deepEqual(after.selection,before.selection);
  assert.deepEqual(errors,[]);assert.deepEqual(externalRequests,[]);
  await page.close();
});

test("HTML inline formatting preserves Markdown text selection, and the switch restores literal source", async()=>{
  const source='before <span style="color:#123456;font-size:24px" onclick="bad()">**重点中文**</span> after';
  const {page,errors,externalRequests}=await setup(source);
  await page.evaluate(()=>window.studyappEditor.setReadonly(true));
  const span=page.locator('.cm-content span[style*="color: rgb(18, 52, 86)"], .cm-content span[style*="color:#123456"]');
  await span.first().waitFor();
  assert.equal(await span.first().evaluate(e=>getComputedStyle(e).fontSize),'24px');
  assert.equal(await page.locator('.cm-content [onclick]').count(),0);
  assert.ok(await page.locator('.cm-content .md-strong').count()>0);
  assert.equal((await state(page)).text,source);
  await page.evaluate(()=>window.studyappEditor.setTheme({renderHtml:false}));
  assert.equal(await span.count(),0);
  assert.ok((await page.locator('.cm-content').innerText()).includes('<span style='));
  await page.evaluate(()=>window.studyappEditor.setTheme({renderHtml:true}));
  await span.first().waitFor();
  assert.equal((await state(page)).text,source);
  assert.deepEqual(errors,[]);assert.deepEqual(externalRequests,[]);await page.close();
});

test("HTML blocks render across source lines without illegal plugin replacements",async()=>{
  const source='<div style="text-align:center;background-color:#123456;color:white">\n<strong>HTML 正文</strong><br>\n第二行\n</div>\n\n```html\n<span style="color:red">围栏代码</span>\n```';
  const {page,errors}=await setup(source);
  await page.evaluate(()=>window.studyappEditor.setReadonly(true));
  const block=page.locator('.md-html-fragment');await block.first().waitFor();
  assert.ok((await block.first().innerText()).includes('HTML 正文'));
  assert.equal(await block.locator('strong').innerText(),'HTML 正文');
  assert.equal(await block.locator('div').first().evaluate(e=>getComputedStyle(e).textAlign),'center');
  assert.ok((await page.locator('.cm-content').innerText()).includes('<span style="color:red">围栏代码</span>'));
  assert.equal((await state(page)).text,source);
  assert.deepEqual(errors,[]);await page.close();
});

test("active HTML, untrusted images, host attributes and layout escaping never reach the document",async()=>{
  const source='<div id="toolbar" data-action="save" onclick="window.pwned=1" style="position:fixed;z-index:99999;background-image:url(https://outside/secret);color:red"><script>window.pwned=2</script><iframe src="https://outside/frame"></iframe><svg onload="window.pwned=3"><text>hidden</text></svg><img src="https://outside/image" onerror="window.pwned=4" alt="图片"><a href="file:///private">label</a><b>safe</b></div>';
  const {page,errors,externalRequests}=await setup(source);
  await page.evaluate(()=>window.studyappEditor.setReadonly(true));
  const block=page.locator('.md-html-fragment');await block.waitFor();
  assert.equal(await block.locator('script,style,iframe,svg,math,img,[onclick],[onerror],[data-action],[id],[href],[src]').count(),0);
  assert.equal(await page.evaluate(()=>window.pwned),undefined);
  assert.ok((await block.innerText()).includes('[图片：图片]'));
  assert.equal(await block.locator('div').evaluate(e=>getComputedStyle(e).position),'static');
  assert.deepEqual(errors,[]);assert.deepEqual(externalRequests,[]);await page.close();
});

test("browser and native share the same CSS rejection and HTML safety corpus",async()=>{
  const corpus=JSON.parse(await readFile(new URL('../../../studyapp-core/tests/fixtures/html-safety.json',import.meta.url),'utf8'));
  const {page,errors,externalRequests}=await setup('corpus');
  for(const [source,expected] of corpus.styles) assert.equal((await action(page,{type:'sanitizeHtml',text:source})).style,expected,source);
  for(const source of corpus.html){
    const {html}=await action(page,{type:'sanitizeHtml',text:source});
    assert.notEqual(html,null,source);
    assert.equal((await action(page,{type:'sanitizeHtml',text:html})).html,html,source);
    for(const forbidden of [' src=',' href=',' data-',' onclick=',' onerror=','<script','<svg','<math','<iframe','<img','<form','<object']) assert.ok(!html.includes(forbidden),html);
  }
  for(const source of ['x'.repeat(65537),'<br>'.repeat(4097),'<div>'.repeat(40)+'text'+'</div>'.repeat(40)]){
    assert.equal((await action(page,{type:'sanitizeHtml',text:source})).html,null);
  }
  assert.deepEqual(errors,[]);assert.deepEqual(externalRequests,[]);await page.close();
});

test("HTML inline readonly selection and explicit copy retain exact source positions",async()=>{
  const source='before <span style="color:red">**重点中文🙂**</span> after';
  const {page,errors}=await setup(source);
  await page.evaluate(()=>{window.studyappEditor.setReadonly(true);window.studyappEditor.setCanRequestEdit(false);window.copied=[];document.execCommand=command=>{if(command!=='copy')return false;const data=new DataTransfer();document.querySelector('.cm-content').dispatchEvent(new ClipboardEvent('copy',{clipboardData:data,bubbles:true,cancelable:true}));window.copied.push(data.getData('text/plain'));return true;};});
  const start=source.indexOf('重点'),end=start+'重点中文🙂'.length;
  await select(page,start,end);
  assert.equal(await page.evaluate(()=>getSelection().toString()),'重点中文🙂');
  await page.locator('.cm-content').dispatchEvent('contextmenu',{clientX:300,clientY:150});
  await page.locator('#editor-copy-menu button').click();
  assert.deepEqual(await page.evaluate(()=>window.copied),['重点中文🙂']);
  assert.deepEqual((await state(page)).selection,{anchor:start,head:end});
  assert.equal((await state(page)).text,source);
  assert.deepEqual(errors,[]);await page.close();
});

test("HTML setting changes preserve undo history and raw source while editing",async()=>{
  const source='before <span style="color:red">text</span> after';
  const {page,errors}=await setup(source);
  await select(page,source.length);
  await page.keyboard.type('!');
  for(const renderHtml of [false,true,false,true]) await page.evaluate(renderHtml=>window.studyappEditor.setTheme({renderHtml}),renderHtml);
  assert.equal((await state(page)).text,source+'!');
  await page.keyboard.press('Control+z');
  assert.equal((await state(page)).text,source);
  await page.evaluate(()=>window.studyappEditor.setSource(true));
  assert.equal(await page.locator('.md-html-fragment').count(),0);
  assert.ok((await page.locator('.cm-content').innerText()).includes('<span style='));
  await page.evaluate(()=>{window.studyappEditor.setSource(false);window.studyappEditor.setReadonly(true);});
  assert.ok(!(await page.locator('.cm-content').innerText()).includes('<span style='));
  assert.deepEqual(errors,[]);await page.close();
});

test("HTML block native selection copies visible text without hidden source markup",async()=>{
  const source='<div style="color:red"><strong>HTML 选择中文🙂</strong><br>第二行</div>';
  const {page,errors}=await setup(source);
  await page.evaluate(()=>{window.studyappEditor.setReadonly(true);window.studyappEditor.setCanRequestEdit(true);window.copied=[];document.execCommand=command=>{if(command!=='copy')return false;const data=new DataTransfer();document.dispatchEvent(new ClipboardEvent('copy',{clipboardData:data,bubbles:true,cancelable:true}));window.copied.push(data.getData('text/plain'));return true;};});
  const text=page.locator('.md-html-fragment strong');await text.waitFor();
  await text.evaluate(element=>{const range=document.createRange();range.selectNodeContents(element);const selection=getSelection();selection.removeAllRanges();selection.addRange(range);});
  await page.waitForTimeout(80);
  assert.equal(await page.evaluate(()=>getSelection().toString()),'HTML 选择中文🙂');
  await text.dispatchEvent('contextmenu',{clientX:300,clientY:150});
  assert.equal(await page.locator('#editor-copy-menu button').isDisabled(),false);
  await page.locator('#editor-copy-menu button').click();
  assert.deepEqual(await page.evaluate(()=>window.copied),['HTML 选择中文🙂']);
  assert.equal(await page.evaluate(()=>getSelection().toString()),'HTML 选择中文🙂');
  assert.equal((await state(page)).text,source);
  assert.equal(await page.evaluate(()=>window.messages.filter(m=>m.type==='requestEdit').length),0);
  assert.deepEqual(errors,[]);await page.close();
});

test("icon toolbar keeps accessible names, tooltips, keyboard controls and source unchanged", async () => {
  const text = "# 根章节\n\n## 子章节\n\n正文";
  const { page, errors, externalRequests } = await setup(text);
  for (const width of [1100, 360]) {
    await page.setViewportSize({ width, height: 700 });
    for (const id of ["format-toggle", "source-toggle", "outline-toggle"]) {
      const control = page.locator(`#${id}`);
      assert.ok(await control.getAttribute("aria-label"));
      assert.ok(await control.getAttribute("title"));
      assert.equal((await control.innerText()).trim(), "");
      assert.equal(await control.locator('svg[aria-hidden="true"]').count(), 1);
      const bounds = await control.boundingBox();
      assert.ok(bounds.width >= 28 && bounds.height >= 28);
      assert.ok(bounds.x >= 0 && bounds.x + bounds.width <= width);
    }
  }
  await page.locator("#format-toggle").focus();
  await page.keyboard.press("Enter");
  assert.equal(await page.locator("#format-toggle").getAttribute("aria-expanded"), "true");
  await page.keyboard.press("Escape");
  await page.locator("#outline-toggle").click();
  for (const name of ["全部收起", "全部展开"]) {
    const control = page.getByRole("button", { name, exact: true });
    assert.equal((await control.innerText()).trim(), "");
    assert.equal(await control.getAttribute("title"), name);
    await control.click();
  }
  await page.locator("#source-toggle").click();
  assert.equal(await page.locator("#source-toggle").getAttribute("aria-pressed"), "true");
  await page.locator("#source-toggle").click();
  assert.equal((await state(page)).text, text);
  assert.deepEqual(errors, []);
  assert.deepEqual(externalRequests, []);
  await page.close();
});

test("safe ordered start attributes keep the same bounded integer grammar", async () => {
  const {page, errors}=await setup("integer attributes");
  for (const [raw, expected] of [["0","0"],["0003","3"],["1000000","1000000"],["+3",null],["-1",null],[" 3",null],["3 ",null],["3.0",null],["1e3",null],["1000001",null],["",null]]) {
    const result=await action(page,{type:"sanitizeHtml",text:`<ol start="${raw}"><li>entry</li></ol>`});
    assert.equal(result.html, `<ol${expected===null?"":` start="${expected}"`}><li>entry</li></ol>`,raw);
  }
  assert.deepEqual(errors,[]); await page.close();
});

const tableSource = "# Table\n\n| Name | Explanation |\n| :--- | ---: |\n| A | A much longer explanation for automatic widths |\n| B | short |\n";
async function tableSetup(text=tableSource, overrides={}, readonly=true) {
  const result=await setup(text);
  await result.page.evaluate(({text,overrides,readonly})=>window.studyappEditor.load({generation:42,text,readonly,tableViews:overrides,tableConfigurable:true}),{text,overrides,readonly});
  await result.page.locator('.md-table-actions').waitFor({state:'visible'});
  return result;
}
async function tableWidths(page,index=0) {
  return page.locator('.md-table-header').nth(index).locator('.md-table-cell').evaluateAll(cells=>cells.map(c=>c.getBoundingClientRect().width));
}
async function tableMessages(page) {return page.evaluate(()=>window.messages.filter(m=>m.type==='tableView'));}

test('table display alignment and adaptive width never modify readonly source or revision',async()=>{
  const {page,errors}=await tableSetup();const before=await state(page), equal=await tableWidths(page);
  assert.ok(Math.abs(equal[0]-equal[1])<2);
  await page.getByRole('button',{name:'按内容自适应列宽',exact:true}).click();
  await page.waitForFunction(()=>document.querySelector('.md-table-cell').getBoundingClientRect().width<200);
  const fitted=await tableWidths(page);assert.ok(fitted[1]>fitted[0]*2);
  await page.getByLabel('选择表格列',{exact:true}).selectOption('1');
  await page.getByRole('button',{name:'居中对齐',exact:true}).click();
  await page.waitForFunction(()=>getComputedStyle(document.querySelectorAll('.md-table-header .md-table-cell')[1]).textAlign==='center');
  const after=await state(page);assert.equal(after.text,before.text);assert.equal(after.revision,before.revision);
  assert.equal((await tableMessages(page)).length,2);
  assert.equal(await page.evaluate(()=>window.messages.some(m=>m.type==='change'||m.type==='requestEdit')),false);
  assert.deepEqual(errors,[]);await page.close();
});

test('table drag previews live, commits once and cancel restores widths',async()=>{
  const {page,errors}=await tableSetup();const original=await tableWidths(page);
  let box=await page.getByRole('separator',{name:'调整第 1 列宽度'}).boundingBox();
  await page.mouse.move(box.x+5,box.y+8);await page.mouse.down();await page.mouse.move(box.x+85,box.y+8,{steps:8});
  await page.waitForTimeout(80);assert.equal((await tableMessages(page)).length,0);
  assert.ok((await tableWidths(page))[0]>original[0]+50);await page.mouse.up();
  assert.equal((await tableMessages(page)).length,1);const committed=await tableWidths(page);
  box=await page.getByRole('separator',{name:'调整第 1 列宽度'}).boundingBox();
  await page.mouse.move(box.x+5,box.y+8);await page.mouse.down();await page.mouse.move(box.x-60,box.y+8,{steps:4});
  await page.getByRole('separator',{name:'调整第 1 列宽度'}).dispatchEvent('pointercancel',{pointerId:1});await page.mouse.up();await page.waitForTimeout(80);
  assert.ok(Math.abs((await tableWidths(page))[0]-committed[0])<2);assert.equal((await tableMessages(page)).length,1);
  assert.deepEqual(errors,[]);await page.close();
});

test('table overrides survive reload, prefix edits and identical headers without sharing settings',async()=>{
  const second=tableSource.replace('A much longer explanation for automatic widths','different content');
  const text=tableSource+'\n'+second;
  const {page,errors}=await tableSetup(text,{},false);
  await page.getByRole('button',{name:'居中对齐',exact:true}).click();
  const saved=(await tableMessages(page)).at(-1);assert.ok(saved);
  const headers=page.locator('.md-table-header');assert.equal(await headers.count(),2);
  assert.notEqual(await headers.nth(0).getAttribute('data-table-id'),await headers.nth(1).getAttribute('data-table-id'));
  const first=text.indexOf('Name');await action(page,{type:'replace',from:first,to:first+4,text:'Renamed'});
  await action(page,{type:'replace',from:0,text:'前插中文🙂\n\n'});
  await page.evaluate(()=>window.studyappEditor.flush({request:701}));
  const latest=(await tableMessages(page)).at(-1);assert.equal(latest.table,saved.table);
  const current=(await state(page)).text;
  await page.evaluate(({text,saved})=>window.studyappEditor.load({generation:43,text,readonly:true,tableConfigurable:true,tableViews:{[saved.table]:saved.value}}),{text:current,saved:latest});
  await page.waitForFunction(()=>getComputedStyle(document.querySelector('.md-table-cell')).textAlign==='center');
  const all=await headers.evaluateAll(rows=>rows.map(r=>getComputedStyle(r.querySelector('.md-table-cell')).textAlign));
  assert.deepEqual(all,['center','left']);
  await headers.nth(1).hover();
  await page.waitForFunction(()=>document.querySelector('[data-alignment=source]').getAttribute('aria-pressed')==='true');
  assert.equal(await page.getByRole('button',{name:'居中对齐',exact:true}).getAttribute('aria-pressed'),'false');
  assert.deepEqual(errors,[]);await page.close();
});

test('table keyboard widths, global defaults and reset remain independent from source and undo',async()=>{
  const {page,errors}=await tableSetup();
  await page.getByRole('separator',{name:'调整第 1 列宽度'}).focus();await page.keyboard.press('ArrowRight');
  assert.equal((await tableMessages(page)).length,1);
  const saved=(await tableMessages(page)).at(-1);assert.equal(saved.value.widths.reduce((a,b)=>a+b,0),10000);
  await page.evaluate(()=>window.studyappEditor.setTheme({tableDefaults:{sizing:'adaptive',alignment:'center'}}));
  await page.getByRole('button',{name:'恢复全局默认',exact:true}).click();
  await page.waitForFunction(()=>getComputedStyle(document.querySelector('.md-table-cell')).textAlign==='center');
  // Reset schedules the presentation redraw on the next animation frame.
  // Alignment may already reflect the earlier theme update while widths still
  // belong to the explicit keyboard override. Await the actual reset geometry.
  await page.waitForFunction(()=>{
    const cells=[...document.querySelectorAll('.md-table-header .md-table-cell')];
    return cells.length===2&&cells[1].getBoundingClientRect().width>cells[0].getBoundingClientRect().width*2;
  });
  const widths=await tableWidths(page);assert.ok(widths[1]>widths[0]*2);
  assert.equal((await tableMessages(page)).at(-1).value,null);assert.equal((await state(page)).revision,0);
  await page.setViewportSize({width:350,height:410});await page.waitForTimeout(100);
  const bounds=await page.locator('.md-table-actions').boundingBox();assert.ok(bounds.x>=0&&bounds.x+bounds.width<=350);
  const header=await page.locator('.md-table-header').boundingBox();
  assert.ok(bounds.height<=40);assert.ok(bounds.y+bounds.height<=header.y||bounds.y>=header.y+header.height,'floating controls must not cover the active table header');
  await page.screenshot({path:fileURLToPath(new URL('../test-results/table-controls-narrow.png',import.meta.url))});
  await page.evaluate(()=>window.studyappEditor.setSource(true));await page.locator('.md-table-controls').waitFor({state:'hidden'});
  assert.equal((await state(page)).text,tableSource);assert.deepEqual(errors,[]);await page.close();
});

test('table Escape cancels a drag while content retains focus; new generation cannot receive old drag',async()=>{
  const {page,errors}=await tableSetup();const before=await tableWidths(page);
  const handle=page.getByRole('separator',{name:'调整第 1 列宽度'});
  let b=await handle.boundingBox();await page.mouse.move(b.x+4,b.y+9);await page.mouse.down();await page.mouse.move(b.x+65,b.y+9);
  await page.keyboard.press('Escape');await page.mouse.up();await page.waitForTimeout(80);
  assert.ok(Math.abs((await tableWidths(page))[0]-before[0])<2);assert.equal((await tableMessages(page)).length,0);
  b=await handle.boundingBox();await page.mouse.move(b.x+4,b.y+9);await page.mouse.down();await page.mouse.move(b.x+65,b.y+9);
  await page.evaluate(text=>window.studyappEditor.load({generation:99,text,readonly:true,tableConfigurable:true}),tableSource.replaceAll('Name','Other'));
  await page.mouse.up();await page.waitForTimeout(80);
  assert.equal((await tableMessages(page)).length,0);assert.ok(Math.abs((await tableWidths(page))[0]-(await tableWidths(page))[1])<2);
  assert.deepEqual(errors,[]);await page.close();
});

test('table empty cells retain widths across inline source selection and text undo',async()=>{
  const text='| A | B | C |\n| --- | :---: | ---: |\n| | value | |\n| a | | c |\n';
  const {page,errors}=await tableSetup(text,{},false);
  const initial=await tableWidths(page);
  await page.getByRole('separator',{name:'调整第 1 列宽度'}).focus();await page.keyboard.press('Shift+ArrowRight');
  const saved=(await tableMessages(page)).at(-1);assert.ok(saved?.value.widths);
  // Keyboard resizing saves synchronously but redraws on the next animation
  // frame. Establish the baseline from the committed geometry, not the old
  // equal-width DOM or CodeMirror's transient measurement elements.
  await page.waitForFunction(({table,widths})=>{
    const header=document.querySelector(`.md-table-header[data-table-id="${table}"]`);
    if(!header)return false;
    const cells=[...header.querySelectorAll('.md-table-cell')];
    return cells.length===widths.length&&cells.every((cell,i)=>
      Math.abs(cell.getBoundingClientRect().width-header.clientWidth*widths[i]/10000)<2);
  },{table:saved.table,widths:saved.value.widths});
  const before=await tableWidths(page);assert.ok(before[0]>initial[0]+20);
  await select(page,text.indexOf('value'),text.indexOf('value')+5);
  await page.keyboard.type('changed');await action(page,{type:'command',name:'undo'});await page.waitForTimeout(100);
  assert.equal((await state(page)).text,text);
  const widths=await tableWidths(page);assert.ok(Math.abs(widths[0]-before[0])<2);
  const rows=await page.locator('.md-table-row').evaluateAll(rows=>rows.map(row=>[...row.querySelectorAll('.md-table-cell')].map(cell=>cell.getBoundingClientRect().width)));
  for(const row of rows) {assert.equal(row.length,3);row.forEach((w,i)=>assert.ok(Math.abs(w-widths[i])<2));}
  assert.deepEqual(errors,[]);await page.close();
});

test("unordered markers follow real mixed AST depth and leave tasks and code unchanged", async () => {
  const text = "1. ordered\n   - child\n     - grandchild\n       - fourth\n\n- [ ] task\n  - nested task child\n\n```md\n- code, not a list\n```\n\n`- inline code`";
  const {page,errors} = await setup(text);
  await page.evaluate(()=>window.studyappEditor.setReadonly(true));
  const rows = await page.locator('.md-list-line').evaluateAll(rows=>rows.map(row=>({depth:Number(row.dataset.listDepth),bullet:row.querySelector('.md-bullet')?.textContent,task:!!row.querySelector('.md-task')})));
  assert.deepEqual(rows.filter(r=>r.bullet).map(r=>[r.depth,r.bullet]),[[2,'▪'],[3,'▸'],[4,'◆'],[2,'▪']]);
  assert.equal(rows.filter(r=>r.task).length,1);
  assert.ok((await page.locator('.md-code-line').allTextContents()).some(line=>line.includes('- code, not a list')));
  assert.equal((await state(page)).text,text);
  assert.deepEqual(errors,[]);
  await page.close();
});

test("list marker appearance updates preserve readonly source revision and explicit source copy", async () => {
  const text = '- 中文🙂 parent\n  - child\n    - grandchild\n\n- [x] done';
  const {page,errors} = await setup(text);
  await page.evaluate(()=>{window.studyappEditor.setReadonly(true);window.messages=[];});
  const before = await state(page);
  const markers = ['star','dash','plus','arrow','square','circle','diamond','triangle'];
  await page.evaluate(listMarkers=>window.studyappEditor.setTheme({listMarkers}),markers);
  assert.deepEqual(await page.locator('.md-bullet').allTextContents(),['✦','–','+']);
  assert.equal(await page.locator('.md-task[aria-checked="true"]').count(),1);
  await select(page,0,text.length);
  const copied = await page.evaluate(()=>{const data=new DataTransfer();document.querySelector('.cm-content').dispatchEvent(new ClipboardEvent('copy',{clipboardData:data,bubbles:true,cancelable:true}));return data.getData('text/plain');});
  assert.equal(copied,text);
  const after=await state(page);
  assert.equal(after.text,text); assert.equal(after.revision,before.revision);
  assert.equal(await page.evaluate(()=>window.messages.filter(m=>['change','requestEdit'].includes(m.type)).length),0);
  assert.deepEqual(errors,[]);
  await page.close();
});

test("deep list markers cycle eight configured levels and safely reject foreign markup", async () => {
  const text=Array.from({length:11},(_,i)=>'  '.repeat(i)+'- level '+(i+1)).join('\n');
  const {page,errors,externalRequests}=await setup(text);
  await page.setViewportSize({width:350,height:800});
  await page.evaluate(()=>window.studyappEditor.setReadonly(true));
  await page.evaluate(()=>window.studyappEditor.setTheme({listMarkers:['star','dash','plus','arrow','square','circle','diamond','triangle']}));
  assert.deepEqual(await page.locator('.md-bullet').allTextContents(),['✦','–','+','➤','▪','◦','◆','▸','✦','–','+']);
  for (const invalid of [null, ['star'],Array(8).fill('<img src=https://bad.test>'),Array(8).fill('__proto__')]) {
    await page.evaluate(listMarkers=>window.studyappEditor.setTheme({listMarkers}),invalid);
    assert.deepEqual((await page.locator('.md-bullet').allTextContents()).slice(0,3),['•','▪','▸']);
  }
  assert.equal(await page.locator('.md-bullet img,.md-bullet script').count(),0);
  assert.deepEqual(externalRequests,[]); assert.deepEqual(errors,[]);
  await page.screenshot({path:'test-results/list-markers-narrow.png'});
  await page.close();
});

test("list marker preview never enters text undo history and editing retains source markers", async () => {
  const text='- parent\n  - child\n    - grandchild\n\nEnd';
  const {page,errors}=await setup(text);
  await select(page,text.length);
  await page.keyboard.type(' edited');
  await page.evaluate(()=>window.studyappEditor.setTheme({listMarkers:Array(8).fill('star')}));
  assert.deepEqual(await page.locator('.md-bullet').allTextContents(),['✦','✦','✦']);
  await page.evaluate(()=>window.studyappEditor.command('undo'));
  assert.equal((await state(page)).text,text);
  await select(page,2);
  assert.equal(await page.locator('.md-list-line').first().locator('.md-bullet').count(),0);
  assert.ok((await page.locator('.md-list-line').first().textContent()).startsWith('- parent'));
  assert.equal((await state(page)).text,text);
  assert.deepEqual(errors,[]); await page.close();
});

test("quoted loose lists and continuation paragraphs keep source markers and alignment", async () => {
  const text='> + parent\n>\n>   continuation paragraph 中文🙂\n>\n>   * child\n>\n>     - grandchild\n\n7. ordered start\n8. next';
  const {page,errors}=await setup(text);
  await page.evaluate(()=>window.studyappEditor.setReadonly(true));
  assert.deepEqual(await page.locator('.md-bullet').allTextContents(),['•','▪','▸']);
  const lines=await page.locator('.md-list-line').evaluateAll(lines=>lines.map(line=>({depth:Number(line.dataset.listDepth),text:line.textContent,left:parseFloat(getComputedStyle(line).paddingLeft)})));
  const continuation=lines.find(r=>r.text.includes('continuation paragraph'));
  assert.equal(continuation.depth,1);
  const child=lines.find(r=>r.text.includes('child')&&!r.text.includes('grandchild'));
  assert.ok(child.left>continuation.left);
  assert.ok((await page.locator('.cm-content').textContent()).includes('7. ordered start'));
  await page.evaluate(()=>window.studyappEditor.setTheme({listMarkers:Array(8).fill('diamond')}));
  assert.deepEqual(await page.locator('.md-bullet').allTextContents(),['◆','◆','◆']);
  assert.equal((await state(page)).text,text);
  assert.deepEqual(errors,[]); await page.close();
});


test("native outline preferences survive a fresh browser context with storage blocked", async () => {
  const {page, errors} = await setup(`# Native outline

Body`);
  await page.addInitScript(() => { Object.defineProperty(window, "localStorage", {get() {throw new DOMException("blocked", "SecurityError");}}); });
  await page.reload();
  await page.evaluate(() => window.studyappEditor.load({generation:42,text:`# Native outline

Body`,readonly:true,
    outlinePreference:{position:"right",shown:true}}));
  assert.deepEqual((await state(page)).outline,{position:"right",shown:true});
  assert.equal(await page.locator("#note-outline").isVisible(),true);
  assert.equal((await state(page)).editorFocused,false);
  assert.equal(await page.evaluate(() => window.messages.filter(m=>m.type==="outlinePreference").length),0);
  const before=await state(page);
  await page.locator("#outline-position").selectOption("top");
  await page.getByRole("button",{name:"隐藏目录",exact:true}).last().click();
  const saved=await page.evaluate(() => window.messages.filter(m=>m.type==="outlinePreference").at(-1));
  assert.equal(saved.generation,42);
  assert.deepEqual(saved.value,{position:"top",shown:false});
  assert.equal((await state(page)).text,before.text);
  assert.equal((await state(page)).revision,before.revision);
  const context=await browser.newContext();
  const fresh=await context.newPage();
  await fresh.addInitScript(() => {
    Object.defineProperty(window,"localStorage",{get(){throw new DOMException("blocked","SecurityError");}});
    window.__STUDYAPP_EDITOR_TEST__=true;window.messages=[];
    window.ipc={postMessage:message=>window.messages.push(JSON.parse(message))};
  });
  await fresh.goto(base);
  await fresh.evaluate(preference=>window.studyappEditor.load({generation:43,text:`# Native outline

Body`,readonly:true,outlinePreference:preference}),saved.value);
  assert.deepEqual((await state(fresh)).outline,saved.value);
  await fresh.evaluate(()=>window.studyappEditor.setOutlinePreference({position:"left",shown:true}));
  assert.deepEqual((await state(fresh)).outline,{position:"left",shown:true});
  assert.equal((await state(fresh)).revision,0);
  assert.equal((await state(fresh)).editorFocused,false);
  assert.equal(await fresh.evaluate(()=>window.messages.filter(m=>m.type==="outlinePreference").length),0);
  for(const bad of [null,[],{position:"external",shown:true},{position:"right",shown:"true"}]) {
    await fresh.evaluate(bad=>window.studyappEditor.setOutlinePreference(bad),bad);
    assert.deepEqual((await state(fresh)).outline,{position:"left",shown:true});
  }
  assert.deepEqual(errors,[]);
  await context.close();await page.close();
});


test("migration outline teardown leaves the narrow final-draft body click unobscured", async () => {
  const {page,errors}=await setup("");
  await page.setViewportSize({width:280,height:550});
  const original=(await state(page)).outline;
  await action(page,{type:"outlinePreference",name:"right",shown:true});
  await page.waitForFunction(()=>document.querySelector("#editor-workspace").dataset.outlinePosition==="floating");
  assert.equal(await page.evaluate(()=>!!document.elementFromPoint(75,105)?.closest(".cm-content")),false);
  assert.equal((await state(page)).text,"");
  const restored=await action(page,{type:"outlinePreference",name:original.position,shown:original.shown});
  assert.deepEqual(restored.outline,original);
  assert.equal(restored.revision,0);
  assert.equal(await page.evaluate(()=>!!document.elementFromPoint(75,105)?.closest(".cm-content")),true);
  await page.mouse.click(75,105);
  assert.equal((await state(page)).editorFocused,true);
  await page.keyboard.insertText("关闭窗口前的末次输入 🧠");
  assert.equal((await state(page)).text,"关闭窗口前的末次输入 🧠");
  assert.deepEqual(errors,[]);
  await page.close();
});

test("default Markdown prose fills the remaining pane while explicit width remains optional", async () => {
  const text = "A long ordinary paragraph should use the space left by the workspace. ".repeat(90);
  const { page, errors } = await setup(text);
  for (const reading of [undefined, 0, 480, 920, 720, 0]) {
    await page.evaluate(reading => window.studyappEditor.setTheme({ markdown: { reading_width: reading } }), reading);
    for (const width of [2200, 1600, 800, 600, 380, 1800]) {
      await page.setViewportSize({ width, height: 800 });
      await page.waitForTimeout(50);
      const geometry = await page.evaluate(() => {
        const pane = document.querySelector("#editor").getBoundingClientRect();
        const prose = document.querySelector(".cm-line").getBoundingClientRect();
        return { pane: pane.width, prose: prose.width };
      });
      const available = geometry.pane - (geometry.pane <= 600 ? 36 : 52);
      const expected = reading ? Math.min(reading, available) : available;
      assert.ok(Math.abs(geometry.prose - expected) <= 2, JSON.stringify({ reading, width, geometry, expected }));
    }
  }
  assert.equal((await state(page)).text, text);
  assert.deepEqual(errors, []);
  await page.close();
});

test("modal notifications retain rendered prose and block background input without changing document state", async () => {
  for (const readonly of [false, true]) {
    const text = Array.from({length:180}, (_, i) => `Paragraph ${i}: ` + "ordinary Markdown prose ".repeat(15)).join("\n\n");
    const {page, errors} = await setup(text);
    await select(page, 11, 24);
    if (!readonly) await page.evaluate(() => window.studyappEditor.insertText("preserved edit"));
    await page.evaluate(readonly => {
      window.studyappEditor.setReadonly(readonly);
      document.querySelector(".cm-scroller").scrollTop = 420;
    }, readonly);
    await page.waitForTimeout(80);
    const before = await state(page);
    for (const opacity of [0.48, 0.64, 0.48]) {
      await page.evaluate(opacity => window.studyappEditor.setModalOverlay(true, opacity), opacity);
      await page.waitForTimeout(30);
      const geometry = await page.evaluate(() => ({
        inert:document.querySelector("main").inert,
        line:document.querySelector(".cm-line").getBoundingClientRect().width,
        editor:document.querySelector("#editor").getBoundingClientRect().height,
        shade:getComputedStyle(document.querySelector("#native-modal-shade")).display,
        command:window.studyappEditor.command("bold"),
        insert:window.studyappEditor.insertText("must not enter the background")
      }));
      assert.equal(geometry.inert, true);
      assert.ok(geometry.line > 300 && geometry.editor > 300);
      assert.notEqual(geometry.shade, "none");
      assert.equal(geometry.command, false); assert.equal(geometry.insert, false);
      await page.mouse.click(150, 180);
      await page.keyboard.type("blocked input");
      await page.keyboard.press("Control+z");
      await page.mouse.wheel(0, 500);
      await page.waitForTimeout(60);
      const blocked = await state(page);
      for (const key of ["text","generation","revision","selection","selections","scrollTop","scrollLeft"])
        assert.deepEqual(blocked[key], before[key], key);
      assert.equal(blocked.modalBlocked, true);
    }
    await page.evaluate(() => window.studyappEditor.setModalOverlay(false, 0));
    const after = await state(page);
    for (const key of ["text","generation","revision","selection","scrollTop","scrollLeft"])
      assert.deepEqual(after[key], before[key], key);
    assert.equal(after.modalBlocked, false);
    if (!readonly) {
      await page.evaluate(() => window.studyappEditor.command("undo"));
      assert.equal((await state(page)).text, text, "modal must not replace undo history");
    }
    assert.deepEqual(errors, []);
    await page.close();
  }
});


test("modal restores the focused editor even if the native parent already took focus", async () => {
  const {page, errors} = await setup("A focused draft keeps its caret");
  await select(page, 4, 11);
  await page.locator(".cm-content").focus();
  const before = await state(page);
  assert.equal(before.editorFocused, true);
  await page.evaluate(() => {
    // The native host has returned keyboard ownership to its parent HWND,
    // while WebKit/Chromium still retains the previously active DOM element.
    Object.defineProperty(document, "hasFocus", {configurable:true, value:() => false});
    try { window.studyappEditor.setModalOverlay(true, 0.48); }
    finally { delete document.hasFocus; }
  });
  assert.equal((await state(page)).modalBlocked, true);
  await page.evaluate(() => window.studyappEditor.setModalOverlay(false, 0));
  const after = await state(page);
  assert.equal(after.editorFocused, true);
  for (const key of ["text","generation","revision","selection","selections","scrollTop"])
    assert.deepEqual(after[key], before[key], key);
  assert.deepEqual(errors, []);
  await page.close();
});
