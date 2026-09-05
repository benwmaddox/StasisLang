# WebAssembly Game Requirements

## Purpose

This document reviews the current Stasis game/runtime approach and describes what is required to make Stasis games run in a web browser as WebAssembly.

This is based on the current `main` codebase, not on a hypothetical redesign.

## Executive Summary

Implementation status (2026-08-13): `stasis package --target web` emits a real Wasm guest and a
static browser-host package. The web backend now covers scalar and
structured-global fields, fixed collection memory, integer/float expressions, stable string
handles, and the existing graphics command-buffer ABI. The browser host packages PNG/SVG/font and
WAV/MP3 assets and implements Canvas and WebAudio. See `docs/web_packaging.md` for remaining compiler
limits and acceptance fixtures.

Stasis game code is already structurally close to a browser-friendly model:

- gameplay state is explicit
- the runtime writes input/window state into host-owned snapshot globals
- game code writes rendering into command buffers instead of issuing host calls on the hot path
- games expose a simple `main() -> tick() -> render()` loop

That part is good news.

The main blockers are below:

1. The current dev/runtime path is native and Windows-centric.
2. The current AOT path emits native object files and native executables, not `.wasm`.
3. The current runtime is `stasis_graphics` built as a native SDL_Renderer shared library, not a browser host.
4. Asset loading, audio, timing, and file watching assume native process capabilities.

The shortest credible path is not "port the Windows runner to the browser." The right path is:

- keep the host-frame + command-buffer game ABI
- add a real `wasm-web` target as a compiled/exported target, like AOT or mobile
- add a browser host/runtime that drives the same ABI from JavaScript
- add web packaging for assets, HTML, JS bootstrap, and a `.wasm` payload

## Current Architecture Review

### What already maps well to the web

These parts are already aligned with a browser-hosted model:

- `README.md` describes the intended game loop as `main()`, then `tick()`, then `render()`.
- `src/stdlib/internal/host_frame_raw.stasis` defines a host-owned frame snapshot for time, window size, pointers, keyboard state, and quit state.
- `src/stdlib/internal/host_window_request.stasis` defines guest-to-host window requests as globals.
- `src/stdlib/internal/gfx_cmd.stasis` defines the single fixed graphics command buffer ABI.
- `src/stdlib/graphics.stasis` uses host snapshots for reads and command buffers for render output.
- `src/stdlib/testing/input_testkit.stasis` provides test-only mutation of the host snapshot.

This matters because browser-hosted games also want:

- one host-produced input/time snapshot per frame
- one guest-produced render command stream per frame
- minimal direct host calls in gameplay logic

That is already the direction of the Stasis architecture.

### What is native-only today

The current runtime and execution path are explicitly native:

- `crates/stasis_dynload/src/lib.rs` loads `stasis_graphics.dll` with `LoadLibraryW` and resolves symbols with `GetProcAddress`.
- many `stasis_dynload` host calls explicitly return "only supported on windows" outside Windows.
- `runtime/CMakeLists.txt` builds `stasis_graphics` as a native SDL3 renderer library.
- `runtime/README.md` documents a native SDL3 renderer runtime.
- `apps/stasis/src/lib.rs` implements `play` as a native loop that:
  - loads the graphics runtime
  - JIT-compiles game code
  - calls `main()`
  - each frame calls `host_get_frame`, `tick`, `render`, and `gfx_submit_u8`
  - uses `sleep_ms`
- `apps/stasis/src/watch.rs` and the surrounding `play` logic rely on filesystem watch services for hot reload.

### What the compiler produces today

The current compiler backends target native execution:

- JIT uses `cranelift_jit`
- AOT uses `cranelift_object::ObjectModule`
- `crates/stasis_jit/src/lib.rs` links native DLLs and native executables with `lld-link.exe` or `cc`

This is a critical point:

- native object emission is not the same thing as emitting a WebAssembly module
- the current AOT path does not produce `.wasm`
- the current linker path does not package browser-consumable output

## What Is Required For WebAssembly

## 1. Define the product target precisely

Before implementation, Stasis needs to define what "web game" means in v1.

Recommended v1 definition:

- target: modern desktop/mobile browsers
- execution: browser-hosted WebAssembly
- build class: compiled/export target, not `play`/watch-mode iteration
- render host: browser runtime, not native SDL DLL
- packaging: static web bundle
- supported game model: `main()`, `tick()`, `render()`
- hot reload: not required for v1
- JIT in browser: not required for v1
- shipping output: one `.wasm` plus JS/HTML/assets

Without this scope lock, the project will mix too many different problems:

- browser runtime
- wasm code generation
- browser development tooling

The first version should be treated like a mobile/export target, not like a browser-native replacement for `play`.

## 2. Add a real WebAssembly code generation target

This is the largest technical requirement.

### Current state

Today the compiler emits:

- JIT machine code in-process
- native object files for AOT
- native linked artifacts such as `.dll` or `.exe`

### Required change

Add a new target mode, for example:

- `TargetMode::WasmWeb`
- CLI surface such as `stasis build --target wasm-web`

This target should be modeled as an export/build artifact path alongside AOT/mobile-style targets, not as a live dev runner.

### What that target must produce

At minimum:

- `game.wasm`
- metadata describing exported entrypoints and ABI bindings

### Why the current AOT path is insufficient

`cranelift_object::ObjectModule` is for native object output. A browser wants:

- a valid WebAssembly module
- browser-compatible imports/exports
- browser-visible memory model

So Stasis needs one of these:

1. a dedicated wasm backend for Stasis lowering
2. a new emit stage that converts the lowered program into `.wasm`
3. a different backend architecture specifically for web builds

This is not just a linker swap.

### Compiler requirements for wasm

The wasm target must define:

- module memory layout
- entrypoint export names for `main`, `tick`, `render`, and optional `on_code_swap`
- how globals such as `host_i32`, `host_f32`, `gfx_cmd_i32`, `gfx_cmd_f32`, `gfx_cmd_u8`, and `host_req_*` are surfaced to the host
- how imported host functions are represented for startup/asset/audio calls
- calling convention rules for browser host interop

Recommended rule:

- keep the Stasis guest ABI explicit
- export either:
  - named globals/pointers, or
  - explicit getter functions returning offsets into linear memory

The browser host must not need to guess symbol layout.

## 3. Formalize a browser host ABI

The current host ABI is conceptually strong but not yet formalized for wasm.

### Reusable pieces

These should be preserved with minimal semantic change:

- `host_frame`
- `host_window_request`
- `gfx_cmd`
- input snapshot structures

### What must be specified for wasm

For each ABI block, the web target needs exact rules for:

- ownership
- location in memory
- alignment
- reset semantics per frame
- who writes it
- who reads it
- whether the browser host reads raw linear memory or accesses exported helpers

### Required web ABI blocks

At minimum:

- host frame snapshot
- host window request block
- graphics command buffers
- optional audio command/data ring buffer if audio remains pull/push based
- optional asset request channel if startup asset APIs are no longer direct imports

### Recommended approach

Keep the current mental model:

- JS host writes frame snapshot into wasm memory
- wasm game runs `tick()` and `render()`
- JS host reads command buffers from wasm memory
- JS host executes rendering/audio/window behavior

That is much more compatible with the existing Stasis design than replacing everything with host callbacks.

## 4. Replace the native graphics runtime with a browser host runtime

Today `stasis_graphics` is a native SDL_Renderer library.

For the web, Stasis needs a browser runtime layer that replaces these responsibilities:

- window/canvas setup
- frame timing
- input collection
- sprite/font asset loading
- rendering
- audio
- optional resize/fullscreen handling

### Rendering options

Stasis needs to choose one browser rendering backend for v1:

1. Canvas 2D
2. WebGL
3. WebGPU

Recommendation:

- use Canvas 2D or WebGL for v1
- do not make WebGPU a requirement for first ship

### Mapping from current graphics model

Current commands already fit browser rendering reasonably well:

- clear
- line draws
- sprite draws
- text draws
- present flag

The browser runtime must implement those command types faithfully.

### Text rendering

Text is a non-trivial requirement.

Current runtime behavior includes:

- `load_font(path, size)`
- `measure_text`
- cached text handles
- rendering text through the native graphics runtime

For the web target, Stasis must define whether:

1. text is rendered through Canvas text APIs
2. text is rasterized into atlases
3. text remains a host-only feature with browser-managed font loading

Recommendation:

- keep text as a host responsibility in v1
- expose numeric font handles and cached text handles similarly to native
- do not attempt to fully replicate native font internals inside wasm first

## 5. Replace native input/time/window plumbing with browser equivalents

### Input

Current games use:

- SDL scancodes
- pointer snapshots
- host frame state

For web support, the browser host must map:

- DOM keyboard events to Stasis key state
- pointer/mouse/touch events to the current host frame layout

### Keyboard mapping requirement

This must be explicitly defined:

- whether Stasis keeps SDL scancode semantics on web
- whether web keyboard codes are translated into the existing SDL-based values

Recommendation:

- keep the Stasis-visible key codes stable
- translate browser input into the existing scancode values in the JS host

This avoids rewriting sample/game code.

### Time

Current code already trends in the right direction because game code uses host-provided time snapshots.

For web support:

- the browser host should populate the frame snapshot lanes surfaced as
  `HostFrame.time_ms`, `HostFrame.time_us`, and `HostFrame.tick_index`
- use `performance.now()` or equivalent
- do not rely on blocking sleep semantics for gameplay

### Window/canvas

`host_window_request` currently assumes native window requests.

For web support, define how these requests map to browser behavior:

- initial canvas size
- resize requests
- fullscreen requests

Not every browser request can be honored synchronously. The spec must state:

- which requests are advisory
- which require user gesture
- what happens on rejection

## 6. Remove blocking/native loop assumptions

The browser frame loop is not a normal blocking while-loop.

### Current native assumptions

Current native play/runtime code assumes:

- blocking loop under host control
- optional `sleep_ms`
- filesystem watch events can be polled
- dynamic library swapping can happen between ticks

### Browser requirement

The browser host must drive the loop from:

- `requestAnimationFrame` for visual updates
- browser input callbacks
- browser lifecycle events

This means:

- no blocking sleep-based pacing in the browser runtime
- no native watch thread in the browser runtime
- no direct DLL swap mechanism

For v1 web builds, hot swap should be considered out of scope unless a dedicated browser-dev design is added later.

## 7. Redesign asset loading for the web

This is another major requirement.

### Current native model

Current startup/asset APIs assume file-path access:

- `gfx_load_sprite(path, ...)`
- `load_font(path, size)`
- runtime-side file watching and reload

### Browser requirement

Browsers do not have normal local filesystem access to arbitrary paths in shipped builds.

Web support therefore requires:

- asset packaging into the web bundle
- URL or manifest-based asset lookup
- browser-friendly async fetch/load behavior
- a way to preserve Stasis-facing path strings or replace them with asset IDs

### Recommended v1 approach

Add a build-time asset manifest:

- Stasis source can still refer to logical asset paths
- the web build step resolves them into packaged asset URLs
- the JS host owns actual fetch/decode/cache
- Stasis still receives numeric handles

### Required decisions

The project needs to define:

- whether asset loading remains synchronous from Stasis's perspective
- whether `main()` is allowed to fail until assets are ready
- whether a preload phase runs before calling `main()`

Recommendation:

- use a preload phase before `main()`
- keep `main()` semantics simple
- do not introduce async gameplay semantics in phase 1

## 8. Add a browser audio backend

Guest audio uses caller-owned `AudioStream`, `AudioAsset`, and `AudioVoice`
values linked through `stasis_graphics`.

For web support, that host implementation needs to be replaced with browser audio, likely via WebAudio.

Required work:

- implement `AudioStream.open()`, `refresh()`, `push()`, and `close()` over
  browser audio, including its availability, format, queue, and underrun fields
- implement `AudioAsset.load_audio()` and `AudioVoice`/`play_once()` playback
  over browser decoding and voice ownership
- define buffering policy
- define browser autoplay/unlock behavior
- decide whether audio starts only after user interaction

This last point is not optional on the web. Browser audio policies are stricter than native.

## 9. Add web packaging and CLI support

The current release/build model is native-binary-oriented.

The web target needs a new output format.

### Required output shape

A reasonable v1 package is:

- `index.html`
- `game.js`
- `game.wasm`
- `assets/...`
- `manifest.json` or equivalent metadata

### Required CLI work

Stasis should add a command path for web builds, for example:

```text
stasis build --target wasm-web --entry samples/bucket_catcher.stasis --out dist/web
```

That flow needs to:

- compile Stasis to wasm
- collect and package assets
- generate or copy the browser host shell
- emit a runnable output directory

### Optional but secondary

- a lightweight static file server for browser testing
- optional source map / debug metadata output

The important point is that serving/testing is not the core target definition. The core requirement is a build/export pipeline that emits a runnable web bundle.

## 10. Treat web as an export target, not a dev runner

This should be explicit, because the native dev loop is one of Stasis's core selling points and the browser target should not try to inherit it by default.

### Native dev loop today

Native `play` currently provides:

- watch mode
- background compile
- hot swap between ticks
- `on_code_swap()`

### Required web assumption

- `wasm-web` is a compiled output like AOT/mobile packaging
- it is not a browser equivalent of `play`
- it does not need watch mode, JIT, or in-browser hot swap for v1
- full rebuild + refresh is acceptable for initial browser validation

### Practical v1 developer flow

1. Run `stasis build --target wasm-web ...`
2. Host the emitted directory with any static web server
3. Refresh the page to test the new build

Anything more advanced than that should be treated as follow-on tooling, not as part of the core web target definition.

## 11. Add web-focused tests and acceptance criteria

Native tests are not enough.

The web target needs its own quality gates.

### Minimum required tests

- compiler test: a simple game builds to `.wasm`
- ABI test: browser host can locate and use exported ABI buffers
- runtime smoke: `main`, `tick`, and `render` execute in order
- render smoke: clear + line + sprite + text commands render visibly
- input smoke: keyboard and pointer input reach the guest
- asset smoke: sprite/font assets load from packaged bundle
- audio smoke: audio initializes and a short buffer plays after user gesture
- resize/fullscreen behavior tests

### Good sample gates

Use existing game-style samples as milestones:

- minimal line sample
- Bucket Catcher
- one heavier sample such as Brickout Revenge

Bucket Catcher is a good early web milestone because it exercises:

- window setup
- keyboard input
- pointer input
- sprite loading
- text rendering
- fixed-step tick/render loop

## 12. Recommended implementation order

### Phase 1: Formalize the ABI

- lock the web product scope
- define wasm-visible exports/imports
- define memory access contract for host frame, requests, and gfx command buffers

### Phase 2: Produce runnable wasm

- add `wasm-web` compiler target
- emit a minimal module for `main`, `tick`, and `render`
- prove a trivial sample runs in browser host shell

### Phase 3: Browser runtime shell

- JS bootstrap
- canvas creation
- frame pump
- host frame writes
- gfx command buffer reads

### Phase 4: Assets and text

- sprite manifest and preload
- font loading
- text measurement and draw support

### Phase 5: Audio and input polish

- WebAudio backend
- stable keyboard mapping
- touch/mobile pointer polish

### Phase 6: Packaging and docs

- `stasis build --target wasm-web`
- output layout
- basic static hosting guidance
- release documentation

## 13. Recommended non-goals for v1

These should not block first browser support:

- in-browser JIT
- native-equivalent hot swap
- `play` parity in the browser
- asset file watching in browser
- exact native SDL and WebGL2 semantic parity
- browser-side AOT relinking of native artifacts
- full offline/PWA story
- multiplayer/networking

## 14. Concrete delta between current state and web-ready state

Below is the shortest honest summary.

Already reusable:

- Stasis game loop contract
- explicit global state style
- host frame snapshot concept
- command-buffer rendering ABI
- snapshot-based input direction

Must be built:

- wasm output backend
- browser host shell
- browser renderer
- browser audio backend
- web asset packaging
- web build/serve CLI
- web test matrix

Must be redesigned or explicitly scoped out:

- JIT dev loop in browser
- browser `play`-style iteration loop
- DLL-based runtime loading
- filesystem watch/reload model
- native linker/exe packaging assumptions

## 15. Bottom line

Stasis does not need a new gameplay model for the web. The existing host-snapshot plus command-buffer design is already the right foundation.

What it does need is a new target/runtime stack:

- compiler output that is actually WebAssembly
- a browser host that speaks the existing ABI
- a web packaging pipeline for assets and startup

If the project keeps the guest ABI stable and treats the browser as "just another host," web support is realistic.

If it tries to preserve the current Windows-native dev/runtime path as-is, it will fight the browser at every layer.
