# `src/` Layout Policy

Stasis source modules in this repo live under `src/`. The `src/` folder is treated as a user-owned
workspace with two framework-owned subtrees.

## Folder Contract

- `src/stdlib/`: framework/library modules. Default home for reusable code.
- `src/runtime/`: host/runtime bridge modules. Only for ABI/layout glue between the Stasis program
  and the host runtime (hot reload, frame/input snapshots, gfx command buffers, etc.).
- `src/*.stasis` (root): user project code only. Framework-owned modules must not live directly
  under `src/`.

## Runtime Allowlist / Ownership

Only the following framework-owned modules belong in `src/runtime/` today:

- `gfx_cmd.stasis`: graphics command buffer ABI (guest writes, host reads).
- `host_frame.stasis`: host frame snapshot layout (window/input/time).
- `host_input_snapshot.stasis`: input snapshot helpers derived from the host frame.
- `host_window_request.stasis`: guest->host window request ABI.
- `input_testkit.stasis`: test-only helpers for seeding host/runtime state.

If a new module needs to live in `src/runtime/`, it must be explicitly justified as host/runtime
specific (not general-purpose stdlib).

## Migration Rules

- One-shot moves only: when relocating a module, move the file and update imports everywhere.
- No compatibility shim files (no stub `src/foo.stasis` that re-exports `src/runtime/foo.stasis`).

## CI Guardrails

PR CI enforces the policy:

- No `.stasis` files are allowed directly under `src/`.
- Imports that target the old `src/<module>.stasis` locations for runtime modules are rejected.

See `tools/ci/check_stasis_src_layout.py` and `.github/workflows/pr-ci.yml`.

