# Demo Day (Windows): runnable Stasis programs

This is a quick "what can I run live?" guide for demo day. Commands assume a `cmd.exe` prompt from the repo root (`F:\StasisLang`).

## One-time setup

Build everything (compiler, runtime, runner, Cranelift AOT tool):

```bat
cd /d F:\StasisLang
build.bat
```

Run the full test suite (C# unit tests + Stasis end-to-end tests):

```bat
cd /d F:\StasisLang
test.bat
```

## Cranelift note (runner vs EXE)

For `--backend cranelift`, `run`/`test` default to producing a DLL and invoking it via `stasis_runner.exe` (the "runner"). This is the path used for hot-swap between `tick()` calls and is also the fastest warm iteration loop.

If you want Cranelift to behave more like LLVM (produce and run an EXE), pass `--no-cranelift-runner`:

```bat
.\stasis.bat run .\samples\basic.stasis --backend cranelift --no-cranelift-runner
```

Cranelift `run`/`test` also caches linked artifacts under `.stasis_cache\run` and `.stasis_cache\test` so repeated runs of an unchanged file should not relink. Set `STASIS_DISABLE_ARTIFACT_CACHE=1` to disable this cache.

## Interactive demos (graphics/input/audio/text)

### 1) Interactive showcase (graphics + pointer input + audio + font text)

Features:
- Immediate-mode rendering via `draw_line`
- Pointer input (`input_pointer_*`)
- Procedural audio (`audio_push_f32_interleaved`)
- Font loading + text measurement/drawing (`load_font`, `measure_text`, `draw_text`)

Run (LLVM):
```bat
.\stasis.bat run .\samples\interactive_showcase.stasis --backend llvm --graphics
```

Run (Cranelift):
```bat
.\stasis.bat run .\samples\interactive_showcase.stasis --backend cranelift --graphics
```

Notes:
- Uses `C:\Windows\Fonts\consola.ttf` by default; edit `FONT_PATH` in `samples/interactive_showcase.stasis` if needed.

### 2) Asteroids (vector graphics + keyboard input + gameplay)

Features:
- Rendering via `draw_line`
- Keyboard input via `is_key_down` (W/A/D/Arrows/Space/Esc)
- Classic update loop + deterministic-ish gameplay math helpers

Run:
```bat
.\stasis.bat run .\samples\asteroids.stasis --backend llvm --graphics
```

### 3) Underwater automation (graphics prototype + simulation)

Features:
- Rendering via `draw_line`
- Time-based simulation (`get_time_ms`)
- Global state + arrays-heavy gameplay-ish logic

Run:
```bat
.\stasis.bat run .\samples\underwater_automation.stasis --backend llvm --graphics
```

### 4) Flappy Birds (sprites + asset hot reload + keyboard input)

Features:
- Sprite pipeline (`gfx_load_sprite`, `gfx_draw_sprite`)
- Asset hot reload: edit the SVGs under `assets_src/flappy-birds/` while running in dev watch mode
- Keyboard input (`is_key_down` Space)

Run:
```bat
.\stasis.bat run .\examples\flappy_birds.stasis --backend llvm --graphics
```

## Focused feature demos

### Audio output (procedural sine)

Features:
- Audio device discovery + streaming samples

Run:
```bat
.\stasis.bat run .\samples\audio_sine.stasis --backend llvm
```

Expected output (example):
```text
audio: sample_rate=48000 channels=2
audio: queued_frames=...
```

### Pointer input visualizer (mouse/touch)

Features:
- Pointer input snapshot APIs
- Simple immediate-mode rendering

Run:
```bat
.\stasis.bat run .\samples\input_pointers.stasis --backend llvm --graphics
```

### Render submission benchmark (per-call vs batched vs command buffer)

Features:
- Compares many host calls (`draw_line`) vs one batched call (`draw_lines_f32`) vs one command-buffer submit (`gfx_submit_*`)
- Uses debug hash to validate all paths submit identical draw streams
- See `docs/graphics-command-buffer-v1.md` for the command buffer layout

Run:
```bat
.\stasis.bat run .\samples\render_command_buffer_bench_submit.stasis --backend cranelift --graphics
```

## Console / IO demos

### Sudoku (console IO + deterministic RNG + tests)

Run:
```bat
.\stasis.bat run .\samples\sudoku.stasis --backend llvm
```

Run tests:
```bat
.\stasis.bat test .\samples\sudoku.stasis --backend llvm
```

### Guess (console IO + RNG)

Run:
```bat
.\stasis.bat run .\samples\sudoku.stasis --backend llvm
```

## Compiler / language feature samples (quick)

### Basic program (exit code)

```bat
.\stasis.bat run .\samples\basic.stasis --backend llvm
echo exit=%ERRORLEVEL%
```

### Test harness output (PASS/FAIL summary)

```bat
.\stasis.bat test .\samples\tests.stasis --backend llvm
```

### Operators, loops, enums, strings

```bat
.\stasis.bat test .\samples\operators.stasis --backend llvm
.\stasis.bat test .\samples\forloop_tests.stasis --backend llvm
.\stasis.bat test .\samples\test_enums.stasis --backend llvm
.\stasis.bat test .\samples\strings.stasis --backend llvm
```

## Hot reload / iteration workflow demos

If a program defines `tick()`, `run` defaults into a dev loop (watch + hot swap between ticks). See `docs/game-dev-workflow.md` for details.

### Tick hot-swap (edit code while it runs)

Run (Cranelift runner + hot swap between ticks):
```bat
.\stasis.bat run .\samples\hotstate_tick_watch.stasis --backend cranelift --fps 60
```

While it is running, edit/save `samples\hotstate_tick_watch.stasis` and watch the output update.

### Restart-based hot state (persist `global state` across process runs)

Run this command multiple times and observe `counter` increment:
```bat
.\stasis.bat run .\samples\hotstate_counter.stasis --backend cranelift --hot-state
```

## Data hot reload (JSON -> global state)

These samples use `data\...` JSON files that the runner applies to globals before `main()` and/or between ticks.

### Smoke: apply JSON before main()

Edit `data\data_hotreload_smoke\balance.json` and run:
```bat
.\stasis.bat run .\samples\data_hotreload_smoke.stasis --backend cranelift
```

Expected output (example):
```text
health=123
```

### Latency: edit JSON while running (between ticks)

Run:
```bat
.\stasis.bat run .\samples\data_hotreload_latency.stasis --backend cranelift
```

While it runs, edit `data\data_hotreload_latency\balance.json` and watch it print `health changed=...`.
