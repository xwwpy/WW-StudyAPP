import { build } from "esbuild";
import { readFile, writeFile, mkdir } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(fileURLToPath(import.meta.url));
const result = await build({
  absWorkingDir: root, entryPoints: ["src/editor.ts"], bundle: true,
  outfile: "dist/editor.js", format: "iife", platform: "browser", target: "es2022",
  minify: true, charset: "utf8", legalComments: "none", metafile: true, write: false
});
const notices = new Map();
for (const input of Object.keys(result.metafile.inputs)) {
  if (!input.startsWith("node_modules/")) continue;
  const parts = input.split("/");
  const packagePath = parts[1].startsWith("@") ? parts.slice(0, 3).join("/") : parts.slice(0, 2).join("/");
  if (notices.has(packagePath)) continue;
  const pkg = JSON.parse(await readFile(join(root, packagePath, "package.json"), "utf8"));
  let license;
  for (const name of ["LICENSE", "LICENSE.txt", "LICENSE.md", "license", "license.txt"]) {
    try { license = await readFile(join(root, packagePath, name), "utf8"); break; } catch {}
  }
  if (!license) throw new Error(`Missing license text for bundled package ${pkg.name}`);
  notices.set(packagePath, `${pkg.name}@${pkg.version}\nLicense: ${pkg.license}\n\n${license.trim()}\n`);
}
const files = new Map([
  ["editor.js", result.outputFiles[0].text],
  ["editor.css", await readFile(join(root, "src/editor.css"), "utf8")],
  ["index.html", await readFile(join(root, "src/index.html"), "utf8")],
  ["THIRD_PARTY_LICENSES.txt", [...notices.entries()].sort(([a], [b]) => a.localeCompare(b, "en")).map(([, text]) => text).join("\n--------------------\n\n")]
]);
await mkdir(join(root, "dist"), { recursive: true });
for (const [name, text] of files) {
  const path = resolve(root, "dist", name);
  if (process.argv.includes("--check")) {
    if (await readFile(path, "utf8") !== text) throw new Error(`Stale generated asset: ${name}; run npm run build`);
  } else await writeFile(path, text);
}
console.log(`${process.argv.includes("--check") ? "Verified" : "Built"} offline editor: ${Buffer.byteLength(files.get("editor.js"))} JS bytes; ${notices.size} bundled license notices.`);
