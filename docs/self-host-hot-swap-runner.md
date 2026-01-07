# Self-host hot swap runner (DLL preload + shared runtime)

This note captures the current hot-swap/hot-reload execution model for the self-hosted compiler, and the runner changes intended to keep the "edit -> swap" loop tight.

## Summary

- A dedicated runner that already loads graphics/runtime DLLs: mostly the same effect as above; the real win still comes from not re-linking big static libs per swap.
- The self-host CLI (`stasis watch run`) defaults to hot reload + hot swap when a `tick()` entrypoint exists.
- The runner now:
  - Keeps the program directory as the process working directory (so relative file IO works).
  - Adds both the program DLL directory and a discoverable runtime DLL directory to the Windows DLL search path.
  - Optionally preloads `stasis_graphics.dll` (when present) so it stays resident across swaps.

## Why this exists

Hot swap latency has two main costs:

1) Relinking the native artifact after a change (clang link time).
2) Loading the new DLL inside the already-running runner (Windows loader time, plus any AV scanning).

For graphics-heavy builds, (1) can be dominated by repeatedly linking big static libs. Linking against a shared runtime import lib avoids embedding those dependencies into each swapped program DLL.

## Linking strategy

When the compiler detects graphics runtime usage, it now prefers linking against:

- `runtime/build/bin/Release/stasis_graphics.lib` (import library for `stasis_graphics.dll`)

and falls back to the previous static bundle:

- `runtime/build/Release/stasis_graphics_static.lib` (+ copied dependency libs)

This selection is implemented in `src/stasis/cli_linking.stasis` via `append_graphics_link_inputs()`.

## Runner runtime DLL discovery

`runtime/stasis_runner.c` now tries to find `stasis_graphics.dll` in:

- Next to the runner executable (legacy copy-to-root/build layout).
- `runtime/build/bin/Release/` relative to the runner directory (repo root or `build/` copies).

If found, it:

- Adds that directory to DLL search (Windows).
- Preloads `stasis_graphics.dll` once at startup (best-effort).

## Current timings (self-host)

On `samples/hotstate_tick_watch.stasis` (module `hot`), one edit-triggered swap produced:

- `HOTRELOAD phases(ms): emit=0 link=109 total=109`
- `HOTRELOAD phases(ms): emit=0 link=125 swapWrite=0 total=704`
- `HOTSWAP latency(ms): 579`
- Runner log: `HOTSWAP ok: load=575204us tick=0us` (from `build/hotstate/hotstate_tick_watch.hot.runner.err.log`)

These numbers will vary significantly by machine and by whether the OS/AV aggressively scans newly-written DLLs.

## How to benchmark locally

- Build the runtime (runner + graphics DLL/import lib): `runtime/build.bat`
- Run hot swap watch loop: `build/stasis_release.exe watch run --backend llvm --module hot samples/hotstate_tick_watch.stasis`
- Edit the file and watch for:
  - `HOTRELOAD phases(ms): ...`
  - `HOTSWAP latency(ms): ...`
  - `HOTSWAP ok: ...` in `build/hotstate/*.runner.err.log`

