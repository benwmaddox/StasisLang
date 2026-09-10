# Task 524 implementation validation

Native window commands run on the live runtime thread between ticks. The editor
submits and polls bounded requests without waiting on its UI thread. Tiling and
focus do not submit pause, semantic edit, or input-override commands. Normal
placement is saved atomically per workspace; disconnected-monitor coordinates
are clamped, and sizing is reapplied after native DPI changes.

Validation completed during implementation:

- `python tools/cargo_cache.py run -- cargo test -p stasis --bin stasis desktop_ -- --test-threads=1`: 89 passed. The three opt-in native/live/review evidence tests were inactive in this hermetic run; their execution is recorded separately.
- `python tools/cargo_cache.py run -- cargo test -p stasis_runner --lib live:: -- --test-threads=1`: 34 passed, including signed coordinates, default monitor queries, and rejection of empty window extents.
- `python tools/cargo_cache.py run -- cargo test -p stasis_ai --lib -- --test-threads=1`: 90 passed, including multimodal requests, model capability checks, bounded provider errors, and cancellation.
- Fresh MSVC runtime build in `target/runtime-task-524`; the native window
  placement contract opens an SDL window, reads monitor bounds, applies outer
  geometry, verifies the result, requests focus, and shuts down.
- `ctest --test-dir target/runtime-task-524 -R stasis_window_placement_contract --output-on-failure`: passed.
- Cross-monitor placement first moves and synchronizes the native window, then
  re-reads destination-monitor decoration metrics before setting the exact outer
  extent. The focused native placement contract was rebuilt and passed after
  this correction. The available Windows acceptance host exposes one physical
  monitor, so the monitor transition itself remains covered by the deterministic
  ordering and the single-monitor native geometry oracle rather than a live
  mixed-DPI pair.
- Runtime ABI and host-runtime contract checks passed with additive optional
  ABI-3 exports. Existing runtimes report unavailable window controls explicitly.
- `python tools/cargo_cache.py run -- cargo test -p stasis --lib scalar_transactions_preview_and_commit_atomically -- --test-threads=1`: passed.

OpenRouter array-size grammar constraints are enforced locally at admission,
including semantic batches (64 edits), symbol queries (16 files), and PNG
requests (512 shapes). Multi-turn provider usage is summed before presentation;
live evidence also retains individual provider timing and cost events.

The full `tools/validate_repo.sh` entrypoint was retried after installing the
missing VS Code package dependencies. Its architecture characterization,
Python, and web checks passed, then its no-ignored-tests audit stopped on two
pre-existing timing-report tests in `crates/stasis_dynload/src/lib.rs` and
`crates/stasis_compiler/src/backend/program_snapshot.rs`. Both ignore attributes
were confirmed in HEAD. They were not changed as part of this task.

Full native CTest also reports the existing mixed-quad planner assertion at line
64. The test and planner files are unchanged from HEAD. The new native placement
test passes independently.

Optional Windows signing reports no matching certificate; test executables run
successfully. This is not an implementation blocker.

Visual evidence: native fixture PNGs are recorded separately in
`layout-validation.md`; they demonstrate rendering and layout, not live AI
execution. Live task evidence must be assessed separately.

Theory gained: SDL Windows positions identify the client origin while tiling
must use outer rectangles. Normalizing decorations at the host boundary keeps
the editor's layout math independent of SDL and predicts correct edge placement
when native title-bar dimensions change.

## Native desktop acceptance environment

GlazeWM 3.10.1 initially overrode application geometry. Only the two exact
windows of the disposable acceptance process were set to uncentered floating;
no global configuration or unrelated window changed. Closing that process
removes those containers and restores the external precondition.

UI Automation discovered and invoked the named **Tile Editor + Game** button.
Physical Win32 rectangles were editor `[0,0,956,1080]` and game
`[964,0,956,1080]`, an 8-pixel gap without overlap. The game was deliberately
minimized and the product's Focus game command restored it. Foreground focus
could not be observed because Windows was locked. Desktop GDI capture showed
the lock screen and was discarded; it is not visual acceptance evidence.
The retained fallback captures only the two verified native HWND surfaces with
`PrintWindow` and composes them under explicit editor/game labels. The inspected
`../task-524/native-window-flow.png` shows the graphical editor beside the
healthy live Asset Breakout game. The inspected 15.25-second
`../task-524/native-window-flow.mp4` contains 61 H.264 frames at 4 fps and shows
the image-capable second request progressing into its applied proposal while
the game continues. `../task-524/native-window-flow.json` records PID `29096`,
editor `[0,0,956,1080]`, game `[964,0,956,1080]`, the 8-pixel gap, and a fresh
`report.json` stop condition. No desktop or lock-screen pixels appear in the
retained PNG or MP4.

## Live validation isolation

The initial real acceptance exposed a state-corruption bug: desktop validation
ran JIT tests inside the game process. Asset Breakout's final test deliberately
sets the loader failure globals, which then appeared in the live game.
Desktop Apply now validates a staged candidate in a bounded child before source
publication, rechecks the original fingerprint, and retains actual test receipts.
Focused tests use the same process boundary. Staging includes scenario and data
fixtures; resolved data inputs participate in the validation fingerprint.

The regression verifies a parent JIT sentinel survives both Apply and focused
tests, scenario/test counts are positive, and a failing candidate never changes
live sources. A deterministic real-game render edit also passed with unchanged
session, paused tick, paddle, bricks, and loader health after generation advance.
Its watcher measured compile 84 ms, package 0 ms, and commit 176 ms.

Theory gained: JIT test activation owns process-global state. Validation must
run outside the live process and before source publication; this predicts that
future test fixture types must join both staged inputs and their fingerprint.

## Reproduce the live acceptance

Build the CLI and native runtime fresh. Supply `OPENROUTER_API_KEY` securely in
the process environment, or set `STASIS_EDITOR_LIVE_ENV` to an existing ignored
credential file; never copy a key into evidence.

```powershell
$env:STASIS_RUNTIME_LIBRARY_PATH = (Resolve-Path target/runtime-task-524/bin/stasis_graphics.dll).Path
$env:STASIS_EDITOR_LIVE_SOURCE = (Resolve-Path samples/asset_breakout).Path
$env:STASIS_EDITOR_LIVE_ACCEPTANCE_DIR = 'docs/evidence/ai-editor/task-524'
$env:STASIS_EDITOR_OPENROUTER_MODEL = 'google/gemini-2.5-flash'
python tools/cargo_cache.py run -- cargo build -p stasis --bin stasis
python tools/cargo_cache.py run -- cargo test -p stasis --bin stasis two_openrouter_tasks_share_one_live_game -- --nocapture --test-threads=1
```

The opt-in harness copies the sample to `target/task-524-live-workspace`, boots
the real runtime, waits for playable assets, runs two task-scoped provider
requests, explicitly accepts and applies the reviewed semantic plans, and
captures native editor/SDL renders. It never modifies the source sample.
`STASIS_EDITOR_LIVE_LOCAL_APPLY_ONLY=1` runs the deterministic isolation preflight
without inference. Clear that variable for provider acceptance.
