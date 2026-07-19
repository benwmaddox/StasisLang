# Stasis

![Stasis Lang](opengraph.jpg)

Stasis is an experimental programming language and toolchain focused on deterministic, game-style programs:

- Static global memory (no hidden allocations)
- Predictable layouts (stable field offsets and array layouts)
- A simple game loop model (`main` once, then `tick` + `render` each frame)
- Fast edit-compile-run loops via in-process JIT + hot swap in development

## Status

Fast-moving. Expect breaking changes.

## Philosophy and Influences

Stasis is built around a few pragmatic ideas that work well for games and simulations:

- Deterministic simulation: the same inputs produce the same outputs (tick-based, fixed-step thinking).
- Make state explicit: prefer a single `global state` struct over scattered globals and hidden runtime state.
- No hidden work: avoid implicit allocations and unpredictable background activity on the tick path.
- Fast iteration: compile and hot-swap between ticks, with an explicit `on_code_swap()` hook for invariants.

Direct influences:

- **Age of Empires II**: deterministic, tick-style simulation mindset (good for replay/debug/lockstep-style thinking).
- **Handmade Hero**: "simple and debuggable" game code, data-oriented structures, and skepticism of hidden complexity.

## Start Here

Most users will:

1. Install one release archive and put its `stasis` executable on `PATH`.
2. Run `stasis new my_game`, then work from the project root or any subdirectory.
3. Use `stasis fmt`, `stasis check`, `stasis test`, and `stasis run` during development.
4. Use `stasis build --mode release`, `stasis package --target desktop`, or
   `stasis package-mobile --target android-arm64|ios-arm64` to ship.

The integrated CLI, `stasis.json` workspace contract, JSON output, offline behavior, and
installation layout are documented in `docs/toolchain_cli.md`.

Lean Android/iOS app packaging is documented in `docs/mobile_packaging.md`;
the lower-level AOT artifact contract is in `docs/mobile_aot_artifacts.md`.

Nightly releases are published from `main`:

- Releases: https://github.com/benwmaddox/StasisLang/releases
- Workflow: `.github/workflows/nightly-release.yml`

Windows release zip layout:

- `stasis.exe` at the archive root
- `stasis_graphics.dll` at the archive root
- `lld-link.exe`, `clang-cl.exe`, `stasis_dynload.dll`, and `stasis_dynload.dll.lib` for offline AOT builds
- `stasis_runner.exe` and `stasis_graphics.dll` for packaged desktop games
- `src/`, `samples/`, `mobile/shells/`, and `runtime/` at the archive root

That keeps the common Windows command simple:

```powershell
.\stasis.exe play samples\bucket_catcher.stasis
```

On Windows, SmartScreen may warn on unsigned binaries.

## Hello, World

Create `hello.stasis`:

```stasis
import "../src/stdlib/stdlib.stasis";

function main(): i32 {
    print_string("hello from stasis\n");
    return 0;
}
```

Note: import paths are project-relative. In this repo, the samples typically use `../src/stdlib/...` or `../../src/stdlib/...`.

## Minimal Game Skeleton

Stasis gameplay code is usually:

- One global `state` struct (your entire simulation state).
- `main()` initializes state and requests the window.
- `tick()` updates state using host snapshots (input/window info).
- `render()` emits drawing commands (no direct rendering calls on hot path).

Example:

```stasis
import "../src/stdlib/stdlib.stasis";
import "../src/stdlib/graphics.stasis";
import "../src/stdlib/sdl_scancodes.stasis";

struct GameState {
    x: f32;
}

global state: GameState;

function main(): i32 {
    // Host reads the request and creates/updates the window.
    init_window(800, 600, "Stasis Game");
    state.x = 120.0;
    return 0;
}

function tick(): i32 {
    if (should_quit()) { return 1; }

    if (is_key_down(Scancode.Left)) { state.x = state.x - 3.0; }
    if (is_key_down(Scancode.Right)) { state.x = state.x + 3.0; }

    return 0;
}

function render(): i32 {
    begin_frame();
    clear(0.05, 0.05, 0.10, 1.0);
    draw_line(state.x, 60.0, state.x + 120.0, 60.0, 1.0, 1.0, 1.0, 1.0);
    end_frame();
    return 0;
}

// Optional: runs after a successful hot swap.
function on_code_swap(): void { return; }
```

## Game Dev Workflow (Watch + Hot Swap)

Development runs in one process:

- Stasis compiles to machine code via Cranelift JIT.
- File changes are compiled in the background.
- Swap commit happens between ticks.
- On success the runner prints swap timing.

Run Brickout Revenge v1 (Windows in-process dev runner):

```powershell
cargo run -p stasis --release -- play samples\brickout_revenge\brickout_revenge_v1.stasis
```

Edit and save any `.stasis` file in the current import/dependency graph. You should see output like:

```text
[watch] change detected: ...
[swap] swapped ok total=29ms (compile=...ms package=...ms hook=...ms deps=...ms)
```

Notes:

- `play` is currently Windows-focused (graphics runtime integration).
- If `--watch-dir` is omitted, `play` watches the entry file's parent directory by default.
- You can cap runtime for smoke testing with `--ticks N`.
- Capture the rendered framebuffer with `--screenshot artifacts\frame.png`. PNG is
  selected by the `.png` extension; other extensions preserve the existing BMP output.
  `--screenshot-frame N` selects a 1-based frame (default `1`). The capture happens
  after queued drawing and post-effects, immediately before present. PNG bytes are
  deterministic for identical input pixels, but rasterization can differ between
  graphics backends, drivers, and platforms.
- The CLI creates missing parent directories and replaces an existing output file.
  With `--exit-after-screenshot`, a write failure also stops the game and returns a
  nonzero exit code instead of leaving screenshot automation running indefinitely.

For example:

```powershell
cargo run -p stasis --release -- play samples\brickout_revenge\brickout_revenge_v1.stasis --screenshot artifacts\frame-12.png --screenshot-frame 12 --exit-after-screenshot
```

The equivalent runtime environment variables are `STASIS_SCREENSHOT_ONCE`,
`STASIS_SCREENSHOT_FRAME`, and `STASIS_EXIT_AFTER_SCREENSHOT=1`.

## Tests (In Stasis, Run via JIT)

Create a test file like `math.test.stasis`:

```stasis
import "../src/stdlib/stdlib.stasis";

test `adds`(): bool {
    return 2 + 3 == 5;
}
```

Run tests in a directory:

```powershell
cargo run -p stasis --release -- test --dir tests/stasis
```

Watch mode:

```powershell
cargo run -p stasis --release -- test --dir tests/stasis --watch --watch-settle-ms 50
```

## Current Constraints

Current intentional language/runtime constraints:

| Constraint | Notes |
|------------|-------|
| `for` header requires all 3 clauses (`init; condition; step`) | `for (; cond; step)` is intentionally rejected as a compile-time error. |

## Build From Source

```bash
cargo build
cargo test
```

On Windows, `play` also needs the native graphics runtime DLL:

```powershell
runtime\build.bat
cargo build -p stasis --release
```

After the runtime exists under the repo (`runtime/build/...` or `runtime/build_ci/...`), the
`stasis` build automatically stages `stasis_graphics.dll` next to `stasis.exe` so you can run
`play` from the built output without manually copying the DLL.

## Where Things Live

- `apps/stasis`: main app/CLI (includes `play`, `test`, `aot-cli`).
- `crates/stasis_compiler`: Rust-native frontend + Cranelift lowering (JIT/AOT).
- `crates/stasis_jit`: JIT/AOT support + function pointer table.
- `crates/stasis_runner`: swap pipeline contracts + sequencing.
- `runtime/`: graphics/audio host runtime (used by `play`).
- `src/stdlib/`: standard library.
- `samples/brickout_revenge/`: end-to-end game sample.

## Specs / PRD

- `docs/spec.md`: canonical language spec
- `docs/live-compilation-prd.md`: hot swap + product/architecture requirements
- `docs/build_checklist.md`: execution plan and slice ordering
- `docs/mobile_packaging_abi.md`: v1 Android/iOS AOT packaging ABI
- `docs/mobile_packaging.md`: one-command lean Android/iOS app packaging
