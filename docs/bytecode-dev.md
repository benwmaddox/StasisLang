# Bytecode Interpreter Backend (Dev Hot Swap)

Goal: make dev iteration much faster than Cranelift AOT (and avoid Windows `LoadLibrary` cost) by running Stasis in a bytecode VM and hot-swapping by replacing bytecode (no native link, no DLL load).

## Why bytecode (vs Cranelift AOT)

- Windows AOT hot swap is dominated by DLL load time (often ~400-700ms).
- A bytecode VM can swap in milliseconds: write a small `.stbc` file (or in-memory), validate, then swap code pointers.
- Runtime cost is higher than native, but acceptable for many dev loops.

## Design constraints (Stasis-specific)

- Stasis uses static global memory and deterministic layout rules.
- Hot swap should preserve global state when structs/layout did not change.
- AoS-to-SoA lowering must still happen (VM should operate on the lowered storage model).

## Proposed architecture

Build graph (dev):

- parse + sema + layout -> bytecode emit -> runner interprets -> hot swap replaces bytecode

Key property: the "runner" stays stable; only bytecode changes.

## Swap strategy

1) Compile new module to bytecode
2) Validate compatibility:
   - module ABI version
   - global layout hash (or per-global size/type signature)
3) Migrate state:
   - preserve globals by name/type when compatible
   - if incompatible, either reset incompatible globals or force restart
4) Swap:
   - atomically replace function table / bytecode buffers

## Instruction set (initial)

Start with a simple, typed, stack VM (i32 first; f32 and memory ops next).

- `ConstI32 <imm>`
- `LoadLocalI32 <idx>` / `StoreLocalI32 <idx>`
- `LoadGlobalI32 <idx>` / `StoreGlobalI32 <idx>`
- `AddI32/SubI32/MulI32/DivI32`
- `Jump <ip>` / `JumpIfZeroI32 <ip>`
- `ReturnI32` / `ReturnVoid`

Planned extensions:

- f32 ops (and i32<->f32 conversions)
- comparisons + structured control flow
- arrays/struct field addressing with explicit bounds rules
- calls (direct call by function index; host imports for gfx/audio/sys)

## Integration plan (incremental)

Phase 0 (this branch):
- implement VM + module model + hot-swap global migration (i32/f32)
- implement minimal compiler emitter for a headless subset (locals/globals, if/for, + - * /, comparisons, assignment)
- tests for VM execution and hot swap

Phase 1:
- add compiler backend to emit bytecode for a small subset (no graphics)
- `stasisc run --backend bytecode` for headless programs

Phase 2:
- move VM into the native runner (or add a stable host-import layer) so games can run with SDL/graphics and still hot swap in milliseconds.

## Current status (prototype)

- CLI supports `--backend bytecode` for `--emit-ir` (prints disassembly) and a dev-only `run` path that requires a top-level `function tick()`.
- `--watch` + `tick()` runs an in-process C# VM and hot-swaps by recompiling + `vm.HotSwap(...)`.
- No structs/arrays/member access, no externs, no function calls. This is intentionally minimal to measure "edit -> swap" overhead.

## Benchmarking hot swap

Run the headless timing harness:

- `powershell -ExecutionPolicy Bypass -File scripts/hotswap_timing_bytecode_counter.ps1`

It edits `examples/bytecode_counter.stasis` in a loop and summarizes:

- `HOTSWAP(ms): total=... latency=... load=...`

`load` is the in-memory swap time (no `LoadLibrary`), and should be near-zero.

### Example results (Windows, Feb 4 2026)

Command:

- `powershell -ExecutionPolicy Bypass -File scripts/hotswap_timing_bytecode_counter.ps1 -Iterations 100 -SleepAfterEditMs 100`

Observed:

- Initial compile: ~24.57ms
- HOTSWAP total (edit->recompile+swap): min~0.164ms avg~0.354ms max~4.365ms
- HOTSWAP load (vm swap only): min~0.001ms avg~0.004ms max~0.313ms
- HOTSWAP latency: ~75-90ms (dominated by the debounce window and filesystem notification timing, not compilation)

## Windows Defender / Smart App Control notes

- The bytecode watch loop avoids generating/loading new DLLs, so it should be far less sensitive to Windows Defender file scanning than the Cranelift hot-swap path.
- If Smart App Control blocks execution of build outputs, prefer running via `dotnet` or ensure your build artifacts live in a trusted/excluded directory.
