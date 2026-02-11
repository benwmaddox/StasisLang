# Editor Buffer Overlay Workflow

This document describes the current unsaved-buffer workflow for Stasis watch mode.

## Goal

Compile/swap from editor buffers without saving source files to disk.

## Supported watch paths

- `WatchCraneliftTickJitSwap` (JIT runner path): supported
- `WatchCraneliftTickInProcessSwap` (in-process path): supported
- `WatchCraneliftTickHotSwap` (AOT runner path): overlay input is ignored

## 1) Start watch mode with overlay pipe

Set a pipe name/path before running watch:

- Windows example:
  - `set STASIS_BUFFER_OVERLAY_PIPE=stasis-watch-overlay`
- Optional structured events:
  - `set STASIS_WATCH_EVENT_JSON=1`

Run watch:

- `.\stasis.bat run <file>.stasis --watch --backend cranelift --module hot --fps 60`

## 2) Configure VS Code extension

Set:

- `stasis.watchOverlayPipe`: `stasis-watch-overlay`

Behavior:

- on open/change: extension sends `{"kind":"set","path":...,"text":...}`
- on close: extension sends `{"kind":"clear","path":...}`

## 3) Overlay command protocol

JSON-line commands accepted by watch process:

- `set` / `overlay.set`
  - fields: `path` (string), `text` (string)
- `clear` / `overlay.clear`
  - fields: `path` (string)
- `clear_all` / `overlay.clear_all`
  - no extra fields

Paths can be absolute file paths or `file://` URIs.

## 4) Machine-readable event output

When `STASIS_WATCH_EVENT_JSON=1` is set, watch prints:

- `WATCH_EVENT {"type":"swap_state", ...}`
- `WATCH_EVENT {"type":"diagnostic", ...}`

This is intended for editor/tooling consumers to avoid parsing human-formatted logs.
