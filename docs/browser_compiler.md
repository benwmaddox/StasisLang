# Browser compiler core

The first browser compiler slice compiles one fixed, in-memory Stasis program in a module Worker:

```stasis
function main(): i32 { return 720; }
```

The returned bytes are a standalone `game.wasm`; instantiating it and calling `main` returns `720`.
This slice deliberately excludes editor integration, arbitrary source input, assets, upload, state
migration, hot swap, and a public playground UI.

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

The core and generated game are deterministic and do not import browser or application-server APIs.
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
