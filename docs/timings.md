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
- Commit: 8738bf1

Command:

```bash
STASIS_DISABLE_AUDIO=1 SDL_AUDIODRIVER=dummy scripts/hotswap_timing_brickout_v1.sh
```

Metric:
- Parse `HOTSWAP load(ms): ...` from `build/hotswap_brickout_v1.out.log` (DLL load time only, same as Windows).

Samples (ms):
- 0.321, 0.340, 0.336, 0.368, 0.368, 0.368, 0.368, 0.368

Summary:
- count=8 min=0.321ms avg=0.355ms max=0.368ms

Notes:
- Script defaults `STASIS_HOTSWAP_DELAY_MS=500` and retries when the runner restarts.

## WSL data-binding reload timing (brickout_revenge_v1 config.json)

Date: 2026-01-26

Repo:
- Branch: fix/wsl-llvmsharp-restore
- Commit: eccb2af

Command:

```bash
STASIS_DISABLE_AUDIO=1 SDL_AUDIODRIVER=dummy ./stasis.sh run samples/brickout_revenge/brickout_revenge_v1.stasis --watch --backend cranelift --graphics --module brick --fps 60
```

Metric:
- Parse `DATABIND: reloaded ... apply_ms=...` from `build/hotstate/brickout_revenge_v1.brick.runner.err.log` after editing `samples/brickout_revenge/data/config.json`.

Samples (ms):
- 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1

Summary:
- count=10 min=0.100ms avg=0.100ms max=0.100ms
