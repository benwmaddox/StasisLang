# Game Dev Workflow (Fast Iteration + Publishing)

This doc captures a practical workflow for making fun games quickly in Stasis, using:

- `main()` for one-time initialization
- `tick()` for one frame of work (logic + draw)
- a host-controlled FPS loop (so we can hot-swap code between ticks)
- SVG sprites baked at runtime (with cheap asset hot reload)

If you are new to graphics in Stasis, read `docs/flappy-birds-tutorial.md` and `docs/brickout-revenge-assets.md` first.

## Core Design Rules (for iteration speed)

### 1) One global state

Prefer a single top-level global:

- `global state: GameState;`

Put *everything persistent* inside `state`:

- simulation data
- RNG state/seed
- UI/debug metrics
- graphics handles (sprite ids, etc.)

Reason: the hot-swap workflow copies `state` from the old module into the new module, so keeping everything in one place makes swapping reliable.

### 2) Split `main()` and `tick()`

#### `main(): i32`

- Create the window and load long-lived resources.
- Initialize state once (guarded by a sentinel, see below).
- Return `0` if init succeeded (do not run the game loop here).

#### `tick(): i32`

- Run exactly one frame: input -> update -> draw.
- Return `0` to continue.
- Return `1` to exit cleanly (treated as success by the runner).
- Return any other non-zero value to signal an error.

### 3) Use a sentinel to avoid clobbering restored state

In the tick hot-swap workflow, `main()` is called once at process start, and hot swaps do not call `main()` again.
That means swaps between ticks will not re-run your init code.

However, if you use a workflow that restores `state` and then calls `main()` (eg. restart-based restore), `main()` can clobber restored data.

Use a sentinel like:

- `state.initialized: i32`

Pattern:

- In `main()`: if `state.initialized == 0`, do first-time initialization and set it to `1`.

### 4) Host controls pacing (no `sleep_ms` in-game)

With `tick()` hosting, the runner targets `--fps` and sleeps between ticks.

- Do not call `sleep_ms` inside `tick()` (it will stall hot swaps and distort frame timings).
- If you need slow-motion, implement it as a simulation multiplier in code.

### 5) SVG assets and hot reload

- Store source art in `assets_src/<game-name>/.../*.svg`.
- Load with `gfx_load_sprite(...)` once and store the returned handle in `state`.
- For rapid art iteration, run in dev watch mode and edit the SVGs; sprites reload automatically (no explicit polling).

The runtime bakes SVG -> RGBA -> atlas and can update atlas regions on change.
Keep SVGs within the supported subset described in `docs/svg-migration-plan.md`.

## Day-to-Day Loop (Fast Iteration)

### The short iteration loop (5-10 minutes)

Goal: change something, feel it immediately, keep the game running.

- Keep `.\stasis.bat run ... --fps ...` running.
- Make a small code change (movement, cooldown, damage, camera), save, and keep playing.
- Change SVGs and the running game will pick them up automatically in dev watch mode.
- When a change makes the game less fun, revert immediately and try a different direction.

### The medium loop (30-60 minutes)

Goal: converge on one "fun slice" and validate it is stable.

- Pick one measurable goal (eg. "enemies feel readable", "ball speed is controllable", "one good combo").
- Add temporary debug toggles (hitboxes, collisions, AI state, RNG seed display).
- Add 1-3 cheap stress tests (high entity counts, worst-case overlaps, spam shots).
- Keep timings visible (logic ms) so you notice regressions early.

### Recommended commands

Run with hot reload (Windows + Cranelift + `tick()`):

`.\stasis.bat run .\samples\your_game.stasis --fps 60`

Tips:

- Use `--fps 10` while debugging input/logic.
- Keep the game running; save the `.stasis` file to hot-swap code between ticks.
- Edit SVGs; sprites reload automatically in dev watch mode.

### What hot-swap actually does

On each code edit:

1) CLI compiles to CLIF and AOTs to `.obj`.
2) CLI links a new `.dll`.
3) Runner hot-swaps between ticks:
   - copies `state` out of the old DLL
   - `LoadLibraryA()` the new DLL
   - restores `state` into the new DLL
   - continues calling `tick()` (does not call `main()` on swap)

This is a same-process swap: your game keeps running, and state stays in memory.

### Reading the timing logs

The CLI prints a per-phase breakdown for each hot reload:

- `HOTRELOAD phases(ms): read=... parse=... sema=... layout=... lower=... aotCompile=... link=... total=...`

The runner prints per-swap timings:

- `HOTSWAP ok: save=...us load=...us restore=...us bytes=... symbols=...`

Use these to decide what to optimize. In practice:

- `link=...` tends to dominate.
- `load=...us` is usually small once outputs and dependencies are stable and on a fast path.

## Animation and SVG Guidance (current best practice)

The runtime SVG baking is intended for a conservative, deterministic SVG subset.
Treat SVG as a clean source format and drive most animation from code:

- Prefer layered sprites (base/turret/effects) and animate by time in `tick()` (bob/pulse/flash/rotate).
- Avoid SMIL `<animate*>`, filters, and other features that tend to be inconsistent across renderers.
- Keep a consistent `viewBox` contract per asset family so sizing stays predictable.
- If an SVG looks good in a browser but fails to bake, reduce it to simple paths/rects/circles with solid fills/strokes.

## Approaches That Work Well in Stasis (today)

Stasis is opinionated: static global memory, deterministic layouts, and predictable performance. Lean into that.

### Best fit genres and patterns

- Arcade games with fixed entity budgets (brickout, flappy, shmups, top-down action).
- Bullet-hell / particle-heavy games where "lots of simple things" is the core.
- Deterministic simulations (replays, lockstep multiplayer later) driven by a stored RNG seed.
- Grid/tile based games (tilemap + entity layer) with fixed-size arrays.
- Data-oriented entity updates: arrays of positions, velocities, flags; update in tight loops.

### Architecture patterns that scale

- "One world struct": `state.world` contains all gameplay data; other sub-structs are systems (input, ui, metrics, audio).
- Indices, not pointers: store `entity_id` indices and look up into arrays.
- Fixed pools with free lists:
  - `alive: i32[Max]`
  - `next_free: i32[Max]` + `free_head: i32`
  - This keeps allocation deterministic and fast.
- Separation of concerns by update passes:
  - gather input
  - integrate movement
  - resolve collisions
  - apply damage/effects
  - render
- "Debug build overlay": keep a HUD that can show timing, counts, and a few key state values (toggleable).

### Asset and rendering patterns that hold up

- Make sprite sizing explicit in code:
  - define logical widths/heights for gameplay (collision)
  - define separate visual scaling (render)
- Treat SVG as source, not a runtime scene graph:
  - bake to atlas once, then render quads every frame
  - do animation in code by choosing layers or applying offsets/rotations
- Keep draw order and layers deterministic so issues are easy to reason about.

## Approaches That Usually Fight Stasis (today)

These are not impossible forever, but they are high friction given the current memory model, toolchain, and hot-swap workflow.

### Avoid (or postpone) these patterns

- Dynamic heap-style object graphs (linked lists, trees of nodes, "each entity is a heap object").
- Variable-sized collections that grow/shrink constantly (lists/vectors/maps) for core gameplay.
- String-heavy logic in the hot path (building lots of strings each tick, parsing text each frame).
- Doing file IO, asset baking, or network calls in `tick()` (it will stall hot swap and destroy timings).
- Large "engine inside the game" abstractions:
  - deep inheritance-style OOP patterns
  - dynamic dispatch per entity per frame
  - reflection-like systems

### Things to be cautious with

- Frequent `state` layout changes during hot-swap sessions:
  - reordering fields can break compatibility
  - changing array sizes changes the snapshot size
  - prefer adding new fields at the end and guarding with versioning
- Complex SVG features:
  - filters, masks, and SMIL animation often fail deterministic baking
  - keep assets within the supported subset and animate via code
- Over-reliance on "real time" deltas:
  - prefer a fixed or clamped timestep for stable gameplay
  - use frame timers only for metrics, not for core simulation behavior

## Coding Checklist for "Fun Fast"

### State and structure

- [ ] One `global state: GameState;`
- [ ] `state.initialized` sentinel
- [ ] No per-frame allocation patterns (prefer fixed arrays and indices)
- [ ] Deterministic RNG seed stored in `state` (easy repro)
- [ ] Keep a "reset" path that can reinitialize gameplay without rebuilding

### Tick health

- [ ] `tick()` does exactly one frame
- [ ] `tick()` returns `1` for clean exit
- [ ] No `sleep_ms` inside `tick()`
- [ ] Inputs are sampled once per tick and stored in state (helps debugging/replay later)

### Assets

- [ ] Assets live under `assets_src/<game>/`
- [ ] Only supported SVG features (no filters/SMIL; animate via layering + transforms in code)
- [ ] Sprite handles live under `state` and are not recomputed every tick
- [ ] Run in dev watch mode to get live sprite updates

### Performance instrumentation

- [ ] Track per-tick work time using `get_time_ms()` deltas
- [ ] Use `frame_timer_*` helpers for stable rolling stats
- [ ] Add a small debug HUD that can be toggled on/off

## Publishing Workflow (Checklist)

Stasis games are currently easiest to ship as a single `.exe` plus runtime DLLs plus SVG assets.

### Pre-release preparation

- [ ] Freeze your `state` layout (avoid field reorder/add/remove late in the cycle)
- [ ] Add a `state.version` integer if you expect to migrate save data later
- [ ] Ensure `main()` is idempotent with `state.initialized`
- [ ] Ensure asset paths are relative and work from a clean working directory
- [ ] Ensure FPS/vsync behavior is correct for shipping (no accidental uncapped loop)
- [ ] Run `dotnet test -c Release`

### Build

Recommended release build command:

- `.\stasis.bat release .\samples\your_game.stasis --graphics`

Notes:

- `release` defaults to optimized settings (and generally targets the LLVM path today).
- If you need an explicit output path, use `--out`.

### Package contents (Windows)

Ship a folder/zip containing:

- [ ] `your_game.exe`
- [ ] `SDL2.dll`
- [ ] `glew32.dll`
- [ ] `stasis_graphics.dll` (if dynamically linked)
- [ ] `assets_src/<game>/.../*.svg` (current runtime bakes from source SVGs)
- [ ] `README.txt` (controls, requirements, known issues)

If your assets must not be editable, plan for a future "baked assets" pipeline (see ideas in `docs/brickout-revenge-assets.md`).

### Smoke test (release artifact)

On a clean machine/user profile:

- [ ] Run from the packaged folder (not from repo root)
- [ ] Verify window opens, inputs work, game runs for 60s, exits cleanly
- [ ] Verify all sprites load (no missing bake logs)
- [ ] Verify performance is acceptable at the chosen FPS

### Gameplay / UX checklist (easy wins)

- [ ] One screen explaining controls (or a minimal "Press X to start" + controls overlay)
- [ ] Restart works and does not require restarting the process
- [ ] Window sizing/resolution is reasonable and consistent
- [ ] Inputs are debounced (no accidental double-press menus)
- [ ] Audio off/mute toggle (when audio exists)

### Quality checks

- [ ] No debug-only overlays on by default
- [ ] No hot-reload logs in normal play (only in `--watch` workflows)
- [ ] No reliance on undeclared environment variables (like `STASIS_ASSET_ROOT`)
- [ ] Deterministic seed behavior documented (how to reproduce a run)
- [ ] Licenses for third-party dependencies documented (SDL2, etc.)

### Post-release operations

- [ ] Tag the repo and record build command + toolchain versions
- [ ] Keep a "repro build" note (exact command line, asset revision, any env vars)
- [ ] Track crash reports / repro seeds / problematic SVGs

## Recommended Directory Layout (for a game in this repo)

This keeps examples and docs aligned with the spec and assets:

- `samples/your_game.stasis` (prototype, iteration)
- `assets_src/your-game/` (SVG sources)
- `docs/your-game-assets.md` (asset contract, sizes, supported SVG subset)
- `examples/your-game/` (once it is stable and you want it as a curated example)

## Notes on Reliability

- Hot swapping assumes compatible `state` layout between swaps.
- If you change struct layout often, expect occasional "restart required" moments; design your `state` so core gameplay stays stable and experimental data stays in a separate sub-struct.
- When in doubt: keep the hot-swap loop for *gameplay iteration*, and occasionally do a full restart when you change fundamentals.

## Flags You Should (and Should Not) Care About

For day-to-day game iteration, you should not need any special "state file" arguments.
The CLI and runner handle the mechanics internally.

- Prefer `stasisc run <file>` / `.\stasis.bat run <file>` for hot reload.
- Use `--fps` to set the host pacing.
- Use `--hot-state` only if you are experimenting with restart-based snapshot/restore across separate process runs.
