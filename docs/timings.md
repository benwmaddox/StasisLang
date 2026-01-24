# Timings

This file records one-off performance measurements taken during development.

## Windows hot-swap timing (non-jit / AOT, Cranelift)

Date: 2026-01-24

Repo:
- Branch: feat/wsl-dev-brickout
- Commit: 7c5e7aa

Command:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\hotswap_timing_brickout_v1.ps1 -Mode aot -Iterations 8 -SwapTimeoutMs 60000 -SleepAfterEditMs 2500
```

Metric:
- Parse `HOTSWAP ok: ... load=...us` from `build/hotstate/brickout_revenge_v1.brick.runner.err.log` (DLL load time only).

Samples (ms):
- 782.724, 921.693, 898.394, 1208.455, 889.309, 878.257, 874.238, 703.424

Summary:
- count=8 min=703.424ms avg=894.562ms max=1208.455ms

Notes:
- Runner log reported `stasis_load_font: failed to open docs/assets/fonts/dejavu-sans-mono.ttf` during this run.

## WSL hot-swap timing (non-jit / AOT, Cranelift)

Date: 2026-01-24

Repo:
- Branch: feat/wsl-dev-brickout
- Commit: 2e0e561

Command:

```bash
STASIS_DISABLE_AUDIO=1 SDL_AUDIODRIVER=dummy scripts/hotswap_timing_brickout_v1.sh
```

Metric:
- Parse `HOTSWAP load(us): ...` from `build/hotswap_brickout_v1.out.log` (DLL load time only, same as Windows).

Samples (ms):
- 0.367, 0.362, 0.362

Summary:
- count=3 min=0.362ms avg=0.364ms max=0.367ms

Notes:
- This run stalled after edit 4 (no new HOTSWAP load line). Re-run if a full 8-sample set is required.
