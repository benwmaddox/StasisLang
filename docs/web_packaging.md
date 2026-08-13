# Web packaging

Stasis packages a browser game with the same `package` command used for desktop and mobile:

```text
stasis package --target web --development-build
```

Release toolchains omit `--development-build`. The default output is
`dist/<project-name>-web/` and contains:

- `index.html`, `game.js`, and `game.wasm`: an inspectable static-hosting bundle;
- `assets/`: the same reachable, prepared release assets selected for Android/desktop packaging;
- `stasis_provenance.json`: the normal package provenance receipt.

Release packaging runs Binaryen's `wasm-opt -Oz` when `wasm-opt` is on `PATH` and also accepts the
unhyphenated `wasmopt` executable name used by some Windows tool layouts. Set
`STASIS_WASM_OPT` to an explicit executable path for pinned toolchains or CI. A configured
optimizer that fails aborts packaging; when no optimizer is discoverable, packaging succeeds with
the original module and reports `wasm_optimized: false` in JSON output. Development packages skip
optimization so their full diagnostic names remain available. The optimized bytes are written to
`game.wasm`.

The package must be served over HTTP. From the package root, for example, run
`python -m http.server 8000` and open `http://localhost:8000/`.
Self-contained single-file HTML output is intentionally deferred; web packages keep Wasm and
assets external so hosted output remains compact and follows the same asset preparation path as
Android and desktop packages.

## Runtime contract

The browser owns `requestAnimationFrame`, input collection, Canvas 2D command execution, WebAudio,
and the performance HUD. The compiled Wasm guest owns `main`, `tick`, `render`, and game state.
The browser populates the canonical HostFrame arrays and consumes the existing graphics command
buffers, matching Android/Windows. Browser policy (fullscreen gestures, Clipboard API, local
storage, and WebAudio unlocking) remains in JavaScript.

Development packages show a HUD with current and worst observed `tick` and `render` time; both must
remain below 16 ms. Release packages omit the performance HUD. Browser audio is unlocked by the
**Enable sound** user gesture; subsequent
`web_play_tone` calls originate in Stasis game logic.

## Current compiler and host lane

The web backend emits a real WebAssembly module from the same reachable HIR used by JIT and AOT. It
supports integer, boolean, `f32`, and `f64` values; scalar and structured-global fields; fixed
primitive/SoA collection storage; native-shape three-word struct views; internal collection views;
strings as stable compiler handles; conversions; calls; arithmetic; short-circuit conditions;
`if`; `for`; and `foreach`. Every indexed access receives a bounded Wasm trap check.

The browser reads the existing `gfx_cmd_i32`, `gfx_cmd_f32`, and `gfx_cmd_u8` command buffers. This
keeps clear, line, rectangle, ordered sprite, and cached/dynamic text rendering on the same guest ABI
as desktop/mobile. Release asset validation and preparation retain only reachable manifest assets;
PNG, SVG, TTF, WAV, and MP3 files are placed under `assets/` and loaded as external package files.
WebAudio playback requested during `main()` is queued until the required user
gesture unlocks the audio context.

Release Wasm exports lifecycle/host-access functions and memory, but keeps Stasis globals private.
Development packages additionally export globals and full reachable function names for diagnostics;
release export names remain in the Wasm export section while `wasm-opt` may discard the optional
custom name section. Called overload families that cannot be selected from HIR identity still fail
with deterministic diagnostics.

`samples/web_export_smoke` is the end-to-end acceptance project. It verifies compiled movement,
keyboard and pointer input, Canvas rendering, a Stasis-triggered WebAudio tone, the performance HUD,
and the static package layout.

The existing `samples/windows_launch_smoke` and `samples/audio_asset_playback` fixtures are also web
package gates. Together they cover the shared graphics command buffers, PNG/SVG/font assets, and
decoded WAV/MP3 WebAudio playback.

## Slice reflection (2026-08-13)

- Good: compiling the unchanged Windows rendering and audio fixtures exposed the exact storage,
  qualified-call, asset, and browser-policy seams needed for a credible export target.
- Bad: the first scalar slice treated parser assignment labels as semantic storage and emitted all
  indexed functions, which hid global resolution and overload/reachability constraints.
- Adjustment: every web lowering addition is now gated through reachable compiler HIR, semantic
  global/collection tables, bounded memory accesses, and at least one existing-game package.

Theory gained: the durable cross-platform boundary is a reachable Wasm guest that owns state and
writes the existing command buffers; JavaScript owns only browser policy and host resources. The
same mapping predicts that another bounded renderer command can be added in the browser without
changing gameplay code or introducing a second compiler path.

Release optimization reflection:

- Good: optimizing the already-linked Wasm module preserves a single compiler/runtime path while
  reducing the hosted payload.
- Bad: relying only on process `PATH` makes long-running build agents miss newly installed tools.
- Adjustment: release packaging accepts an explicit `STASIS_WASM_OPT` path and reports whether
  optimization was actually applied, including before/after byte counts.

Theory gained: Wasm size optimization belongs after semantic lowering and linking but before the
static package is assembled. A browser acceptance pass validates the preserved host ABI.

Emitter cleanup reflection:

- Good: replacing paired signed bounds traps with one unsigned comparison preserved negative and
  upper-bound rejection while materially shrinking a large unchanged game module.
- Bad: the initial encoder represented every local as a separate declaration group and appended a
  fallback result even when every terminal branch already returned.
- Adjustment: keep local Wasm cleanup limited to directly provable encodings; leave inlining,
  stackification, and cross-function optimization to Binaryen.

Theory gained: for a nonnegative collection length, `index >=u length` is exactly the union of
`index < 0` and `index >=s length`. Together with grouped local declarations and proven terminal
returns, this reduces the guest before Binaryen without creating a second semantic pipeline.
