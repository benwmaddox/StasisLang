## Asset Hot Reload (Long-Term Plan)

### Goal
Make asset hot reload (sprites, data files) smooth and deterministic without per-frame filesystem polling.

### Current State
- Games commonly call `gfx_poll_reload(handle)` every tick.
- The runtime implements `gfx_poll_reload` by checking file `mtime` for each handle.
- On Windows this can be expensive (AV, NTFS metadata, path resolution), causing frame-time spikes.

### Proposed Long-Term Architecture (Hybrid)
Reads (events) are host-driven; rendering stays command-based.

1. Event-driven file watching
   - The host/runtime watches asset directories using native file notifications.
   - When a file changes, the runtime marks the corresponding sprite entries as dirty (`needs_reraster=1`).
   - The next draw of that sprite triggers re-bake/re-upload.

2. No per-frame polling
   - `gfx_poll_reload` becomes optional (debug/legacy) and is not required for correctness.
   - Hot reload latency is driven by the OS notification (typically < 1 frame).

3. Deterministic point of application
   - Changed assets are applied at a predictable point:
     - either at frame boundaries (after event pump, before draw),
     - or at the next draw call for that handle.
   - This keeps behavior deterministic and avoids mid-frame partial updates.

### Implementation Notes
- Windows: directory notifications via `FindFirstChangeNotification` (simple) or `ReadDirectoryChangesW` (precise file names).
- Linux/macOS: inotify / FSEvents equivalents.
- WASM: the watcher becomes a browser host responsibility; changes arrive via a host API and the runtime marks dirty.

### ABI Surface (minimal)
- A “mark dirty” hook is enough for correctness:
  - `stasis_gfx_notify_file_changed(path)` (path-based)
  - or `stasis_gfx_mark_sprite_dirty(handle)` (handle-based)

### Why this is better
- Eliminates per-frame filesystem stats.
- Avoids frame-time spikes and scales to many assets.
- Fits future WASM hosting (events come from JS, not local disk polling).

