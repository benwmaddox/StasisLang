# Tasks

This file is a lightweight, persistent checklist of remaining work. It complements the more detailed design docs in `docs/`.

## Active

### Runtime / Host

- [x] HostFrame vNext: add `version`, `flags`, `tick_index`, and optional `tick_hz` so `tick()` can be snapshot-only.
- [x] HostFrame: add keyboard state + quit/focus flags so programs can avoid `is_key_down`/`should_quit` imports.
- [x] Host ABI cleanup: route hot-path reads through `host_*` globals; remove per-tick query calls from samples.

### Graphics Command Buffers

- [ ] Command buffers: keep persistent prebuilt streams (only rewrite dirty ranges; avoid rebuilding every tick).
- [ ] Command buffers: optimize runtime submit fast paths (avoid per-sprite overhead when debug hashing is off; build VBOs directly from streams).
- [ ] Command buffers: evolve text output toward cached glyph runs (avoid per-frame UTF-8 copies + parsing).

### Sys / Stdlib

- [x] Sys/memory: bulk move + safe wrappers (`mem_copy_*`, `mem_set_*`) exist.
- [x] Sys/memory: keep bulk clears (`memset`) as a compiler/runtime detail (avoid exposing `sys_memset_*` to user code).
- [x] Stdlib/platform externs: support `@extern` no-body declarations (and optional link name) so APIs are visible in source.
- [ ] Stdlib/platform externs: wire up per-platform stdlib selection (so extern-backed APIs can vary by platform).

### Game Dev Readiness

- [ ] P0: stdlib modules (`game_math`, `game_draw`, `game_collision`) + canonical UTF-8 buffer helpers (remove samples writing headers directly).
- [ ] P1: input helpers (went_down/up, mapping), viewport/camera helpers, draw batching helpers.
- [ ] P2: audio mixer layer (one-shots + loops) and more templates/examples.

### Follow-Through (Docs -> Code)

- [ ] Execute `docs/audio-plan.md` (desktop SDL2 audio MVP first).
- [ ] Execute `docs/input-plan.md` (pointer snapshot, mouse + touch/taps).
- [ ] Execute `docs/aquarium-sample-plan.md` (add `samples/aquarium.stasis`).
- [ ] Execute `docs/data-hot-reload-plan.md` (end-to-end dev workflow + tests).
- [ ] Execute `docs/cranelift-backend-plan.md` (close remaining backend gaps).
- [ ] Execute `docs/host-snapshot-command-buffer.md` (HostFrame snapshot + per-tick command buffers).
- [ ] Execute `docs/android-plan.md` (Android runtime build + host proof-of-concept).
- [ ] Execute `docs/brickout-android-debug-plan.md` (debug APK + adb asset push workflow).
- [ ] Execute `docs/svg-migration-plan.md` (finish SVG pipeline + validation).

### Tooling / DX

- [x] Support compiling Markdown code blocks: allow `stasis build/test` on `.md` by extracting ```stasis fenced blocks so docs stay valid.
- [ ] Maintenance: regularly scan open PRs for conflicts and keep them mergeable (merge `main` into branches or rebase).
