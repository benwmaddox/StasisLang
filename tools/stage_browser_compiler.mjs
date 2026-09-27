import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { access, copyFile, mkdir, readFile } from "node:fs/promises";
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
for (const file of ["compiler_worker.js", "smoke.html", "smoke.js"]) {
  await copyFile(path.join(webRoot, file), path.join(output, file));
}

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
