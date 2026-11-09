import { build } from "esbuild";
import { readFile, writeFile, mkdir, readdir } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";
import { licenseNotices } from "./license-notices.mjs";

const root = dirname(fileURLToPath(import.meta.url));
const result = await build({
  absWorkingDir: root, entryPoints: ["src/editor.ts"], bundle: true,
  outfile: "dist/editor.js", format: "iife", platform: "browser", target: "es2022",
  minify: true, charset: "utf8", legalComments: "none", metafile: true, write: false
});
const notices = new Map();
async function notice(packagePath, closure=false) {
  packagePath=resolve(root,packagePath);
  if (notices.has(packagePath)) return;
  const pkg = JSON.parse(await readFile(join(packagePath, "package.json"), "utf8"));
  let license;
  const names=await readdir(packagePath);
  for (const name of ["LICENSE", "LICENSE.txt", "LICENSE.md", "LICENSE.markdown", "license", "license.txt"]) {
    const actual=names.find(value=>value.toLowerCase()===name.toLowerCase());
    if(actual){license=await readFile(join(packagePath,actual),"utf8");break;}
  }
  if (!license) {
    try {
      const readme=await readFile(join(packagePath,"README.md"),"utf8");
      const section=readme.match(/^##? License\s*\n([\s\S]+)$/mi);
      if(section) license=section[1];
    } catch {}
  }
  if (!license) throw new Error(`Missing license text for bundled package ${pkg.name}`);
  notices.set(packagePath, `${pkg.name}@${pkg.version}
License: ${pkg.license}

${license.trim()}
`);
  // JSZip's browser distribution already bundles its helpers. Conservatively
  // include its locked dependency closure, which esbuild cannot see inside dist.
  if (closure || pkg.name === "jszip") {
    const require=createRequire(join(packagePath,"package.json"));
    for (const name of Object.keys(pkg.dependencies || {})) {
      const directory=dirname(require.resolve(`${name}/package.json`));
      await notice(directory, true);
    }
  }
}
for (const input of Object.keys(result.metafile.inputs)) {
  if (!input.startsWith("node_modules/")) continue;
  const parts = input.split("/");
  await notice(parts[1].startsWith("@") ? parts.slice(0, 3).join("/") : parts.slice(0, 2).join("/"));
}
const files = new Map([
  // Remove insignificant trailing spaces from generated standalone CSS rules.
  ["editor.js", result.outputFiles[0].text.replace(/(^\.[^\n]*\{[^\n]*\})[ \t]+$/gm,"$1")],
  ["editor.css", await readFile(join(root, "src/editor.css"), "utf8")],
  ["index.html", await readFile(join(root, "src/index.html"), "utf8")],
  ["THIRD_PARTY_LICENSES.txt", licenseNotices(notices.values())]
]);
await mkdir(join(root, "dist"), { recursive: true });
for (const [name, text] of files) {
  const path = resolve(root, "dist", name);
  if (process.argv.includes("--check")) {
    if (await readFile(path, "utf8") !== text) throw new Error(`Stale generated asset: ${name}; run npm run build`);
  } else await writeFile(path, text);
}
console.log(`${process.argv.includes("--check") ? "Verified" : "Built"} offline editor: ${Buffer.byteLength(files.get("editor.js"))} JS bytes; ${notices.size} bundled license notices.`);
