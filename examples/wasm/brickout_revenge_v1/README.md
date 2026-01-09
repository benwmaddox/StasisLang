# Brickout Revenge v1 - WASM prototype

This is a minimal prototype to compile and run `samples/brickout_revenge/brickout_revenge_v1.stasis` in a browser via `wasm32`.

Status: proof-of-concept host shims (Canvas2D, no input, no audio).

## Build

From repo root:

- `powershell -ExecutionPolicy Bypass -File scripts/build-wasm-brickout.ps1`

Outputs `examples/wasm/brickout_revenge_v1/brickout_revenge_v1.wasm`.

## Run

You need a local HTTP server (file:// won't load WASM).

- `powershell -ExecutionPolicy Bypass -File scripts/serve-wasm.ps1 -Root examples/wasm/brickout_revenge_v1`

Then open `http://127.0.0.1:5173/` in a browser.

