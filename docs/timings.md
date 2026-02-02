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
- Parse `HOTSWAP ok: ... load=...ms` from `build/hotstate/brickout_revenge_v1.brick.runner.err.log` (DLL load time only).

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
- Parse `load=...` from `HOTSWAP(ms): total=... latency=... load=...` in `build/hotswap_brickout_v1.out.log` (DLL load time only, same as Windows).

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

## Brickout Revenge v1 global memory size (layout)

Date: 2026-02-02

Repo:
- Branch: main
- Commit: bed0a92

Command:

```bat
.\stasis.bat run .\samples\brickout_revenge\brickout_revenge_v1.stasis --backend cranelift --module brick --emit-ir --out .\.stasis_cache\tmp_ir.txt
```

Result:
- total globals: 626180 bytes (611.50 KiB)
- state: 44562 bytes (43.52 KiB)

Top 30 globals by size (bytes):
- 369168 gfx_cmd_f32
- 139392 gfx_cmd_i32
- 65536 gfx_cmd_u8
- 44562 state
- 3072 host_i32
- 2048 audio_buf
- 768 brickout_level_event_preset
- 768 brickout_level_event_tick
- 288 sfx_voices
- 256 host_f32
- 68 brickout_level_name_tmp
- 40 brickout_level_name_0
- 40 brickout_level_name_1
- 40 brickout_level_name_2
- 12 brickout_digits_tmp
- 12 brickout_level_event_count
- 12 brickout_level_event_offset
- 12 brickout_level_initial_power_cap
- 12 brickout_level_initial_scraps
- 4 audio_sr
- 4 bench_count
- 4 bench_frame
- 4 bench_sum_ms
- 4 bench_sum_us
- 4 brickout_level_done
- 4 brickout_level_next_event_index
- 4 brickout_level_tick
- 4 brickout_levels_magic
- 4 brickout_ui_font
- 4 desktop_size_h

Note:
- `total globals` includes graphics/audio/input buffers and other globals (e.g. `gfx_cmd_*`, `host_*`) in addition to the game `state` struct.
