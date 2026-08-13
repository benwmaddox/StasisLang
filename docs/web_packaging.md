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

## Current compiler lane

The first production web lane supports i32-compatible scalars (`i32`, `bool`, `u8`, `u16`, and
`u32`), scalar globals, local variables, calls, arithmetic, conditions, `if`, and `for`. It emits a
real WebAssembly module from the same compiler HIR used by JIT and AOT.

Structured state, indexed collections, floating-point expressions, strings, `foreach`, and Stasis
conversion statements currently fail with a deterministic `web scalar lane does not yet support`
diagnostic. They are not substituted or interpreted in JavaScript. Expand this backend lane before
packaging games that use those shapes.

`samples/web_export_smoke` is the end-to-end acceptance project. It verifies compiled movement,
keyboard and pointer input, Canvas rendering, a Stasis-triggered WebAudio tone, the performance HUD,
and the static plus standalone package layouts.
