import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { access, copyFile, mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import path from "node:path";

const repository = path.resolve(import.meta.dirname, "..");
const compilerWasm = path.resolve(process.argv[2] || path.join(
  repository,
  "build/codex-cargo-target/wasm32-unknown-unknown/release/stasis_browser_compiler.wasm",
));
const output = path.resolve(process.argv[3] || path.join(repository, "target/browser-compiler-bundle"));
const webRoot = path.join(repository, "crates/stasis_browser_compiler/web");

await access(compilerWasm);
await mkdir(output, { recursive: true });
for (const file of (await readdir(webRoot)).filter(file =>
  /\.(?:m?js|html|css)$/.test(file) && !/\.test\.m?js$/.test(file))) {
  await copyFile(path.join(webRoot, file), path.join(output, file));
}
await copyFile(path.join(repository, "runtime/web/game.js"), path.join(output, "game.js"));
// Bundle caller-visible library sources before the editor becomes ready. The
// compiler still receives a complete virtual project and never reads disk.
const libraryRoot = path.join(repository, "src/stdlib");
const libraryFiles = [];
async function collectLibrary(directory) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const sourcePath = path.join(directory, entry.name);
    if (entry.isDirectory()) await collectLibrary(sourcePath);
    else if (entry.isFile() && entry.name.endsWith(".stasis")) {
      libraryFiles.push({
        path: `vendor/stasis/stdlib/${path.relative(libraryRoot, sourcePath).split(path.sep).join("/")}`,
        source: await readFile(sourcePath, "utf8"),
      });
    }
  }
}
await collectLibrary(libraryRoot);
libraryFiles.sort((left, right) => left.path.localeCompare(right.path, "en"));
await writeFile(path.join(output, "stdlib.json"), `${JSON.stringify(libraryFiles)}\n`);

const stagedCompiler = path.join(output, "compiler_core.wasm");
await copyFile(compilerWasm, stagedCompiler);
const sourceBytes = await readFile(compilerWasm);
const stagedBytes = await readFile(stagedCompiler);
assert.deepEqual(stagedBytes, sourceBytes, "staged compiler_core.wasm differs from the Cargo artifact");

console.log(JSON.stringify({
  schema: "stasis.browser_compiler_bundle.v1",
  output,
  compiler: {
    cargoFile: path.basename(compilerWasm),
    stagedFile: path.basename(stagedCompiler),
    byteLength: stagedBytes.length,
    sha256: createHash("sha256").update(stagedBytes).digest("hex"),
  },
}, null, 2));
