# Browser compiler core

The first browser compiler slice compiles one fixed, in-memory Stasis program in a module Worker:

```stasis
function main(): i32 { return 720; }
```

The returned bytes are a standalone `game.wasm`; instantiating it and calling `main` returns `720`.
The fixed-sample request remains a compatibility smoke. The browser playground also accepts
caller-provided virtual projects, runs them in the production Web host, and exports static packages.

## Dependency seam

`stasis_compiler::backend::wasm::WasmProcess` already lowers the shared checked HIR directly to a
WebAssembly binary. The browser obstacle was crate coupling rather than code generation:

| Dependency area | Browser-core decision |
| --- | --- |
| Parser, checker, HIR, Wasm encoder, semantic snapshots | Retained; these operate on in-memory data. |
| Cranelift JIT/object/native ISA crates | `native` feature only. |
| `stasis_jit` and `stasis_dynload` runtime publication | `native` feature only; browser compilation has no process-global JIT publication. |
| AOT linking, patching, migration, native state query | `native` feature only. |
| Workshop atomic filesystem editing | `native` feature only. |
| Stable global/string identity hashes | Moved to a target-neutral module shared by native and Web emitters. |

The `stasis_browser_compiler` cdylib depends on `stasis_compiler` with default features disabled and
only `browser-core` enabled. Its versioned C ABI owns the compiled bytes until the next compile call.
`compiler_worker.js` copies those bytes out of compiler memory before transferring the buffer.

## Worker contract

The Worker loads `compiler_core.wasm` before sending:

```js
{ type: "ready", abiVersion: 1 }
```

It accepts `{ type: "compile-fixed", requestId }` and replies with either:

```js
{ type: "compiled", requestId, gameWasm: ArrayBuffer }
{ type: "compile-error", requestId, message }
```

The core and fixed-sample game import no host capabilities. Playground games import only the
supported production Web-host functions described below.
The Worker performs no request after its initial compiler-core fetch. A restrictive test CSP permits
same-origin scripts, the Worker, the initial Wasm fetch, and WebAssembly compilation; it permits no
external origin.

## Reproducible smoke

Install the Rust target once, then build through the repository Cargo cache and run Chromium:

```powershell
rustup target add wasm32-unknown-unknown
python tools/cargo_cache.py run -- cargo build -p stasis_browser_compiler --target wasm32-unknown-unknown --release
node tools/stage_browser_compiler.mjs build/codex-cargo-target/wasm32-unknown-unknown/release/stasis_browser_compiler.wasm target/browser-compiler-bundle
node tools/run_browser_compiler_smoke.mjs target/browser-compiler-bundle
```

The staging command copies the Cargo artifact to the exact `compiler_core.wasm` sibling URL requested
by `compiler_worker.js`, alongside the Worker and smoke assets. The smoke serves only that staged
layout, waits until the Worker has loaded the compiler, records the application server request count,
runs two compilations, verifies byte-for-byte determinism, instantiates `game.wasm`, and calls `main`.
It fails if any application-server request occurs after the ready boundary and writes a JSON receipt
under `target/browser-compiler-smoke/`.

## Browser playground

Open `playground.html` from the staged bundle on a static HTTP server. The editor loads its compiler
Worker, Web runtime, and library sources once. Editing files, compiling, importing images, running,
hot swapping, and creating the downloadable ZIP then operate entirely in browser memory. No local
toolchain or application server is involved in those operations. Library imports use paths such as
`vendor/stasis/stdlib/graphics.stasis`; the complete imported source tree is part of each virtual compile request.
The nightly browser lane publishes the ready-to-host `browser-playground-bundle` artifact, so a
playground user does not need Rust or the staging tools.

The Worker accepts `{ type: "compile-project", requestId, entry, files }`, where files are
`{ path, source }` records. Paths are confined relative `.stasis` paths. Each request builds a fresh
compiler process, so deleted files and failed compiles cannot leave stale definitions or artifacts.
The core owns a bounded input buffer; the Worker copies UTF-8 JSON into it and copies both the
generated Wasm and metadata out before transferring the result. The browser compiler imports no
host capabilities and has no filesystem project root. Missing imports are diagnostics.

Projects must define `main`, `tick`, and `render` as `(): i32`. An optional `on_code_swap` must be
`(): void`. Compile metadata includes exact host imports, entrypoint signatures, memory and string
tables, state layout, canonical snapshot support, and provenance. Unsupported source constructs
fail through the shared compiler pipeline.

### Editor assistance

The source editor highlights Stasis tokens using the compiler's shared lexer and requests
completion from an isolated compiler-core analysis ABI. Analysis runs in the existing Worker
and leaves compiled modules and diagnostics intact. It accepts incomplete drafts and combines
current declarations with the last successful project catalog; deleted files are excluded.
Suggestions cover keywords, types, functions, locals, globals, imported namespaces, and members.
Function suggestions show signatures. Type a prefix or a dot to open suggestions, or press
**Ctrl+Space**. Use the arrow keys and **Enter** or **Tab** to accept, **Escape** to dismiss,
and **Ctrl+Enter** to run. Highlight spans and replacement ranges use UTF-16 editor offsets.
Both highlighting and completion work locally after the ready boundary.

### Transactional publication

The playground uses the existing Canvas/WebGL2, input, audio, and image preparation host. The
opt-in in-memory bootstrap avoids a game-Wasm fetch. A successful first run calls `main` once.
Later swaps instantiate an isolated candidate and commit between animation frames:

1. Validate the candidate's entries, imports, and canonical snapshot support.
2. Require the same compiler state-layout digest and snapshot size as the running generation.
3. Copy canonical user-state bytes into the candidate. Host/presentation buffers are not migrated.
4. Run candidate `on_code_swap` without calling `main`. Hook imports cannot produce live host
   effects; `reject_code_swap`, a trap, or any failed restore aborts the transaction.
5. Publish the candidate instance and its configuration only after all checks succeed.

The old instance and user state remain active on every failed candidate. Changes to state types or
capacities require an explicit new session. Existing asset handles retain the resources initialized
by `main`; use **New session** to initialize newly selected assets. Opaque/unsupported snapshot shapes cannot hot swap;
there is no raw-memory migration fallback. Network, portal, external-URL, clipboard, and storage
imports are unavailable in the playground. A static exported game uses the packaged Web runtime
independently of the browser editor and compiler.

### Images and export

Uploaded PNG files are signature/dimension checked and decoded before use. SVG uses a restricted
element/attribute policy that rejects scripts, event handlers, external resources, entities, CSS,
and foreign content, then rasterizes the accepted image to PNG. Previews use image objects rather
than inserting uploaded XML into the document. Both formats have file, dimension, and pixel limits.
Asset manifests record dimensions, sizes, and hashes of the bytes used by the game. Missing assets
cannot fall back to application-server requests in playground mode.

Export downloads a stored ZIP with `index.html`, `game.js`, `game.wasm`, image assets, and provenance.
Serve the extracted package as ordinary static files; it does not ship the editor or compiler core.

### Acceptance

After the fresh compiler build and staging above:

```powershell
node tools/run_browser_playground_acceptance.mjs target/browser-compiler-bundle
```

The acceptance uses a real Chromium page and production renderer. It checks edited multi-file
compilation, canonical state preservation, changed code and hook execution, candidate rollback,
host-effect rejection, image import safety, exported package execution, and zero application-server
requests after the editor's ready boundary. PNG states and a screencast frame sequence accompany
the JSON receipt. The fixed-sample smoke continues to verify ABI compatibility separately.
