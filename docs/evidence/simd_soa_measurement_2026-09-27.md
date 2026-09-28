# SIMD payoff for SoA game loops (2026-09-27)

## Decision

Do not open a SIMD/vector-lowering implementation task from this evidence. Production Android ARM64 output remains scalar for all measured shapes. A scratch explicit-NEON position kernel proves that packed lowering is mechanically possible, but its static body is larger and no qualified ARM64 runtime was available to establish a speedup. Wall bounce and Brickout-shaped control flow add masks, branches, calls, alias questions, and inactive lanes that the branchless prototype does not answer.

Runtime payoff is unresolved, not negative. Reconsider only after the same checked-in harness can run on a qualified ARM64 device and show a repeatable whole-tick or isolated-loop improvement large enough to cover tail and control-flow overhead.

## Scope and method

The characterization is based on `ab96b3e02d294fa6f2b48b08703a64b4d0387b05`, which includes the scalar value-reuse work from Maddox task #734, implemented by PR #864. It does not duplicate that work: there is no compiler lowering, field-pointer, ABI, layout, language, or dispatch change here.

The focused test generates four fixed-SoA fixtures at capacities 4, 64, and 900:

- `position`: branchless `x += vx; y += vy`, with every lane active.
- `wall_sparse`: the same update with one boundary-crossing lane and scalar wall bounce branches.
- `wall_dense`: every lane crosses an x/y boundary on the first tick.
- `brickout_active`: the representative Brickout `active` gate, scaled movement, radius clamps, bounce streak, and top deactivation. Distributed active slots exercise 2/4, 32/64, and the production cap of 90/900; helper/random jiggle calls are deliberately excluded so the measured body remains the vectorizable portion.

For each case, the real JIT executes five ticks and compares every field after every tick against a separate indexed-array reference. This checks ordinary updates, boundary collisions, and interleaved active/inactive holes, including 810 inactive lanes in the 900/90 Brickout case. The test then captures production AOT CLIF and emits `aarch64-linux-android` ELF objects with Cranelift `speed_and_size`. The analyzer disassembles the actual `tick` object with NDK `llvm-objdump` and records packed-lane work, including `qN` vector loads and stores. Vector absence is a dated observation in this snapshot, not a compiler invariant.

Compile samples are whole `AotProcess` time (frontend plus Cranelift codegen), not isolated backend time: one warmup and five measured repetitions on an AZW GTi14, Intel Core Ultra 9 185H, 68,179,521,536 bytes RAM, Windows 11 build 26200, Rust/Cargo 1.92.0, and NDK 27.0.12077973. Raw samples and median absolute deviations are in [the JSON snapshot](simd_soa_measurement_2026-09-27.json).

The snapshot's `git_revision` is the clean parent used for the measurement. `git_dirty: true` records the expected uncommitted characterization harness and report present during the run; there were no unrelated checkout changes.

## Production scalar results

| Shape | Capacity | Active | Text bytes | Instructions | Loads | Stores | Branches | Packed lanes | Compile median us | Range us |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| position | 4 | 4 | 100 | 25 | 9 | 3 | 2 | 0 | 34,076 | 30,796-43,770 |
| position | 64 | 64 | 100 | 25 | 9 | 3 | 2 | 0 | 36,262 | 33,093-37,900 |
| position | 900 | 900 | 100 | 25 | 9 | 3 | 2 | 0 | 37,932 | 25,080-39,848 |
| wall sparse | 4 | 4 | 212 | 53 | 9 | 5 | 6 | 0 | 44,010 | 36,920-44,363 |
| wall sparse | 64 | 64 | 212 | 53 | 9 | 5 | 6 | 0 | 44,832 | 33,540-54,617 |
| wall sparse | 900 | 900 | 212 | 53 | 9 | 5 | 6 | 0 | 38,896 | 34,561-48,857 |
| wall dense | 4 | 4 | 212 | 53 | 9 | 5 | 6 | 0 | 47,282 | 39,702-51,360 |
| wall dense | 64 | 64 | 212 | 53 | 9 | 5 | 6 | 0 | 46,174 | 37,856-51,194 |
| wall dense | 900 | 900 | 212 | 53 | 9 | 5 | 6 | 0 | 49,130 | 38,142-56,385 |
| Brickout active | 4 | 2 | 444 | 111 | 34 | 15 | 9 | 0 | 68,923 | 66,089-77,981 |
| Brickout active | 64 | 32 | 444 | 111 | 34 | 15 | 9 | 0 | 75,628 | 65,273-77,758 |
| Brickout active | 900 | 90 | 444 | 111 | 34 | 15 | 9 | 0 | 69,630 | 56,320-73,133 |

Capacity and collision density do not change static loop size because they are runtime trip count/data. Sparse and dense bounce therefore have identical machine shape; they may have different branch behavior at runtime, which this host cannot qualify. The AOT CLIF contains scalar `f32` loads, stores, adds, multiplies, comparisons, and branches with no vector types. Final bounce and Brickout objects use a few `movi vN.2s` encodings to materialize scalar zero, but no packed-lane load, arithmetic, or store; the scalar consumers use `sN` registers.

## Scratch explicit SIMD

The analyzer generates, but does not ship, matched C kernels for the branchless position update. Both are cross-compiled with NDK Clang 18 at `-O3` with automatic loop and SLP vectorization disabled. The explicit version performs two `fadd vN.4s` operations per vector iteration and then a scalar remainder loop for 1-3 lanes.

| Kernel | Text bytes | Instructions | Loads | Stores | Branches | Packed operations |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| scalar C | 48 | 12 | 4 | 2 | 2 | 0 |
| explicit NEON | 160 | 40 | 8 | 4 | 5 | 8 |

This is a static feasibility result, not a performance comparison. The explicit kernel is 112 bytes and 28 static instructions larger because it carries vector-trip and scalar-tail paths. A long run could amortize that overhead, but neither the break-even count nor actual memory throughput is known without ARM64 execution. An explicit wall/Brickout prototype was not considered trustworthy without device execution: lane masks, per-lane state publication, deactivation, helper effects, and sparse/dense divergence are the material questions.

## Runtime and semantic limits

`adb devices -l` exposed only `emulator-5554`, model `sdk_gphone64_x86_64`, ABI `x86_64`. It is not an ARM64 performance environment. No warmed per-tick ARM64 timing, noise distribution, speedup, or regression claim is made. Desktop timing was intentionally omitted because it would not answer the target question.

The shipped scalar semantics remain the source of truth:

- Fixed `foreach` walks the declared capacity; the Brickout active gate preserves inactive lanes exactly.
- The scratch vector loop has an explicit scalar remainder, but its runtime parity remains unresolved until it executes on ARM64.
- Indexed reference accesses retain current fatal bounds checks. No check was removed or moved.
- The measured production loop uses direct fixed SoA storage. Views and aliases are intentionally not treated as vector-safe; any future slice would need canonical-owner/no-overlap proof and invalidation across calls, copies, views, and unknown host effects.
- The full Brickout collision path remains a poor initial candidate: nested loops, calls, field mutation, random/audio helpers, and spawning create control/effect dependencies beyond this active-ball update.

## Reproduce

From the repository root, choose a new output directory:

```powershell
python tools/measure_simd_soa.py --output build/simd-soa-evidence-rerun --warmups 1 --repetitions 5
```

The command runs the focused semantic/AOT test through `tools/cargo_cache.py`, writes source, CLIF, objects, disassembly, raw samples, device inventory, and `analysis.json`, then compiles the scratch-only scalar/NEON pair. Generated artifacts stay under the chosen ignored build directory. The checked-in JSON is the final 2026-09-27 run.

The Rust characterization test is ignored by the normal compiler test suite because it performs repeated measurement compiles and records incidental code shape. The reproduction tool invokes it explicitly with `--ignored`; future vector lowering should be reported in new evidence rather than treated as a regression.

Validation on the measured branch:

- `cargo fmt --all -- --check`: passed through `tools/cargo_cache.py`.
- Focused ignored characterization: 1/1 passed through the reproduction tool.
- `cargo check --workspace --all-targets --locked`: passed.
- `cargo test -p stasis_compiler --lib -- --test-threads=1`: 904/904 passed.
- `tools/validate_repo.sh`: every preceding policy, contract, architecture, Python, Node, render-parity, compiler, app library/main, and asset-stress lane passed; the final workspace matrix stopped at the Windows `desktop_display_metrics_seam` because native display-event injection returned 0 instead of 1. The exact isolated rerun failed identically. This is the pre-existing host/display seam also recorded by #734 and does not execute or depend on the test-only compiler characterization.

## Theory and reflection

Theory gained: SoA makes the branchless memory streams structurally vector-friendly, but capacity alone does not predict payoff. The current emitted loop body is capacity-independent, and useful vectorization depends on enough active contiguous work to amortize a larger vector-plus-tail body without violating per-lane branches or alias/effect rules. A qualified ARM64 run should therefore show the clearest benefit first for the 64/900 branchless cases and little or none for capacity 4; failure of that prediction would point to memory/address-generation or measurement overhead rather than a need for broader vectorization.

Good: one generated fixture family connects exact state, CLIF, final AArch64, size, and compile noise without altering production lowering. Bad: static disassembly cannot decide sparse/dense branch payoff or the NEON break-even count. Adjustment: do not propose SIMD from IR capability or instruction-count intuition; require exact-state ARM64 timings for the narrow branchless case before widening to masks, aliases, or Brickout control flow.

Visual evidence: not applicable (compiler measurement with no user-visible behavior).
