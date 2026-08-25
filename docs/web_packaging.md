# Web packaging

Stasis packages a browser game with the same `package` command used for desktop and mobile:

```text
stasis package --target web --development-build
```

Release toolchains omit `--development-build`. The default output is
`dist/<project-name>-web/` and contains:

- `index.html`, `game.js`, and `game.wasm`: an inspectable static-hosting bundle;
- `assets/`: the same reachable, prepared release assets selected for Android/desktop packaging;
- network-enabled packages additionally contain `network_guest.bundle`, a bounded archive of the
  three core web files plus every reachable prepared asset (including nested font/audio/image
  paths) used by the native LAN host;
- `stasis_provenance.json`: the normal package provenance receipt.

Release packaging runs Binaryen's `wasm-opt -Oz` when `wasm-opt` is on `PATH` and also accepts the
unhyphenated `wasmopt` executable name used by some Windows tool layouts. Set
`STASIS_WASM_OPT` to an explicit executable path for pinned toolchains or CI. A configured
optimizer that fails aborts packaging; when no optimizer is discoverable, packaging succeeds with
the original module and reports `wasm_optimized: false` in JSON output. Development packages skip
optimization so their full diagnostic names remain available. The optimized bytes are written to
`game.wasm`.

## State storage

The Wasm backend keeps persistent fields flattened from named structs in one
linear-memory allocation. Fixed collections retain their structure-of-arrays
field planes in that allocation, and scalar struct fields plus collection
metadata receive compiler-reported offsets alongside them. True top-level
scalar globals may remain Wasm globals; function parameters and temporary
values remain Wasm locals.

The exported `__stasis_global_get_*` and `__stasis_global_set_*` reflection
helpers preserve path-hash access for both physical storage lanes. This keeps
host inspection independent of whether a path is backed by linear memory or a
Wasm global.

The package must be served over HTTP. From the package root, for example, run
`python -m http.server 8000` and open `http://localhost:8000/`.
Self-contained single-file HTML output is intentionally deferred; web packages keep Wasm and
assets external so hosted output remains compact and follows the same asset preparation path as
Android and desktop packages.

The browser shell owns page fitting and the guest owns its logical canvas. The shared `index.html`
shell opts into `viewport-fit=cover`, reserves the CSS safe-area insets, and uses `svh`/`dvh`
fallbacks. Its inline fitter uses `visualViewport.width`/`height` when available (with the layout
viewport as a fallback), refitting on window resize, orientation changes, and visual-viewport
resize/scroll. The shell keeps the canvas aspect ratio centered inside the currently visible safe
area; the body clip box moves with a nonzero visual-viewport origin so its overflow clip and canvas
remain together. It changes CSS dimensions only and never rewrites the canvas `width`/`height`
backing resolution. A `MutationObserver` refits after an intentional intrinsic backing-size change without
observing the fitter's own style writes, so it cannot form a resize loop. Pointer coordinates remain
guest-logical through `getBoundingClientRect()` in the runtime, including after a toolbar or
orientation change. The runtime's private synchronous refit hook runs before an intentional
backing resize is reported in HostFrame; extent events are coalesced into one generation, while
origin-only scroll remains quiet. Consumers should not add post-processing resize or fullscreen controls.

Web packages do not render an audio-enable control. The runtime requests audio immediately and
automatically retries on the first pointer or keyboard gesture when browser autoplay policy starts
the audio context suspended.

## Loading shell font

The optional `web.loading_font` manifest field selects a project font for the static loading title:

```json
{
  "web": { "loading_font": "/assets/fonts/display.ttf" }
}
```

The value may be rooted (`/assets/...`) or project-relative (`assets/...`), but must name an
existing `.ttf`, `.otf`, `.woff`, or `.woff2` file under `assets/`. `stasis check` and packaging
validate the path before producing output. Web packages retain the configured font even when the
game does not load it through Stasis code, preload it in the HTML shell, and use it for the loading
title. Projects without this field keep the Georgia fallback and the same loading DOM contract.

## Runtime contract

The browser owns `requestAnimationFrame`, input collection, Canvas 2D command execution, WebAudio,
and the performance HUD. The compiled Wasm guest owns `main`, `tick`, `render`, and game state.
The browser populates the canonical HostFrame arrays and consumes the existing graphics command
buffers, matching Android/Windows. Browser policy (fullscreen gestures, Clipboard API, local
storage, and WebAudio unlocking) remains in JavaScript.

Before the Wasm module and game assets are ready, `index.html` displays a static, title-aware
loading shell. The package generator substitutes the manifest game name into
`#stasis-loading-title`; the separate `#stasis-loading-status` line is the live status target
updated by both browser runtimes. The shell uses only inline HTML/CSS so it remains available on
slow or offline starts. `setLoading` keeps the title and decorative structure intact while it
updates that status, hides the shell after readiness, and leaves it visible with a readable error
status when startup fails. The `stasis-loading` element retains `role="status"` and
`aria-live="polite"` for assistive technology.

Development packages show a HUD with current and warmup-excluded worst observed `tick`, `wasm
render`, `browser replay`, and total `frame work` time. Tick is guest simulation, wasm render is
guest command-buffer generation, browser replay is host execution/compositing, and frame work is
their sum. The 16 ms verdict uses worst total frame work. Body datasets expose each phase and
`worst*Ms`; `renderMs` and `worstRenderMs` remain combined-render compatibility aliases. Release
packages omit the performance HUD. Browser audio is requested
immediately and retried on the first pointer or keyboard gesture when autoplay policy initially
suspends it; subsequent `web_play_tone` calls originate in Stasis game logic.

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
WebAudio playback requested during `main()` is queued until the audio context starts or a user
gesture unlocks it.

Large consecutive rectangle runs use a lazy WebGL2 instanced batcher in the full host, rendered to
a transparent offscreen canvas and composited once at the run position. Non-rectangle commands
flush runs to preserve source-over order. Small runs, unavailable WebGL2, and shader/context/resize
failures use the Canvas 2D fallback; the command ABI is unchanged.

Release Wasm exports lifecycle/host-access functions and memory, but keeps Stasis globals private.
Development packages additionally export globals and full reachable function names for diagnostics;
release export names remain in the Wasm export section while `wasm-opt` may discard the optional
custom name section. Called overload families that cannot be selected from HIR identity still fail
with deterministic diagnostics.

Web packaging carries the reachable Wasm import set into host linking. Scalar games that use only
the direct canvas/math/print bridge receive a small generated JavaScript host containing exactly
those imported functions. Games using the shared graphics buffers retain the full renderer, while
audio declarations, helpers, imports, and the sound-unlock UI are removed unless a reachable
function imports audio. JIT and AOT continue to use the same function-reachability closure before
backend emission, so unreachable Stasis functions are excluded consistently across targets.

`samples/web_export_smoke` is the end-to-end acceptance project. It verifies compiled movement,
keyboard and pointer input, Canvas rendering, a Stasis-triggered WebAudio tone, the performance HUD,
and the static package layout.

`samples/pong_web_minimal` is the dependency-linking acceptance project. It is an autonomous Pong
game drawn only with canvas rectangles and text. It intentionally declares unreachable audio and
keyboard helpers; package tests require those helpers and host functions to be absent from both
`game.wasm` and `game.js`, and require the sound UI to be absent from `index.html`.

`samples/pong_web_standard` is an idiomatic, plain HTML and JavaScript implementation of the same
gameplay and presentation. It provides a direct size and behavior comparison for the Stasis web
package without adding audio, input, image, font, or framework dependencies.

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

Function-level host-linking reflection (2026-08-15):

- Good: reusing the Wasm emitter's reachable import set made browser host selection a direct linker
  decision instead of a second source scanner.
- Bad: the original static browser host mixed optional audio policy with mandatory frame/render
  policy, so dead guest functions were removed while their JavaScript support remained.
- Adjustment: optional browser capability blocks must be keyed from reachable imports, with an
  end-to-end sample that contains deliberately unreachable feature calls.

Theory gained: a target package is only as dependency-aware as both sides of its ABI. Reachable HIR
selects guest functions and imports; carrying that same import set into host linking removes unused
browser policy without inventing target-specific reachability. An adjacent prediction is that
storage, clipboard, and sprite/font support can move behind the same import-keyed host blocks.
