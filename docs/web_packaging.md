# Web packaging

Stasis packages a browser game with the same `package` command used for desktop and mobile:

```text
stasis package --target web --development-build
```

Release toolchains omit `--development-build`. The default output is
`dist/<project-name>-web/` and contains:

- `<project-name>.html`: one self-contained package file that embeds the Wasm guest and browser runtime;
- `index.html`, `game.js`, and `game.wasm`: an inspectable static-hosting bundle;
- `stasis_provenance.json`: the normal package provenance receipt.

The static bundle must be served over HTTP. The self-contained HTML package does not fetch local
files and can also be opened directly.

## Runtime contract

The browser owns `requestAnimationFrame`, input collection, Canvas 2D command execution, WebAudio,
and the performance HUD. The compiled Wasm guest owns `main`, `tick`, `render`, and game state.
Browser calls are explicit `@extern` functions in the `web_*` namespace. Draw imports enqueue
commands and the host executes the queue after `render()` returns.

The HUD reports current and worst observed `tick` and `render` time separately. Both must remain
below 16 ms. Browser audio is unlocked by the **Enable sound** user gesture; subsequent
`web_play_tone` calls originate in Stasis game logic.

## Current compiler and host lane

The web backend emits a real WebAssembly module from the same reachable HIR used by JIT and AOT. It
supports integer, boolean, `f32`, and `f64` values; scalar and structured-global fields; fixed
primitive/SoA collection storage in exported Wasm memory; strings as stable compiler handles;
conversions; calls; arithmetic; conditions; `if`; and `for`. Every indexed access receives a bounded
Wasm trap check.

The browser reads the existing `gfx_cmd_i32`, `gfx_cmd_f32`, and `gfx_cmd_u8` command buffers. This
keeps clear, line, rectangle, ordered sprite, and cached/dynamic text rendering on the same guest ABI
as desktop/mobile. PNG, SVG, TTF, WAV, and MP3 assets are copied into the static bundle and embedded
as data URLs in the standalone file. WebAudio playback requested during `main()` is queued until the
required user gesture unlocks the audio context.

`foreach`, general array-view parameters, native system helpers, and called overload families that
cannot yet be selected from HIR identity still fail with deterministic web-backend diagnostics. They
are not substituted or interpreted in JavaScript.

`samples/web_export_smoke` is the end-to-end acceptance project. It verifies compiled movement,
keyboard and pointer input, Canvas rendering, a Stasis-triggered WebAudio tone, the performance HUD,
and the static plus standalone package layouts.

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
