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

1. Write a `.stasis` game with `main()`, `tick()`, `render()`.
2. Run it in dev with `play` (watch + hot swap).
3. Write `.stasis` tests and run them with `test`.
4. Later: build production artifacts with AOT (WIP).

Nightly releases are published from `main`:

- Releases: https://github.com/benwmaddox/StasisLang/releases
- Workflow: `.github/workflows/nightly-release.yml`

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
cargo run -p stasis --release -- play samples\brickout_revenge\brickout_revenge_v1.stasis --watch-dir samples\brickout_revenge
```

Edit and save any `.stasis` file in the current import/dependency graph. You should see output like:

```text
[watch] change detected: ...
[swap] swapped ok total=29ms (compile=...ms package=...ms hook=...ms deps=...ms)
```

Notes:

- `play` is currently Windows-focused (graphics runtime integration).
- You can cap runtime for smoke testing with `--ticks N`.

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
