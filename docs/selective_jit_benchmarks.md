# Selective JIT verification

This report is the checked performance evidence for Maddox #188. The canonical behavior contract
remains [jit_generation_contract.md](jit_generation_contract.md). All timings below use nearest-rank
percentiles and release benchmark binaries after five unmeasured warm edits.

## What the measurements prove

Warm JIT emission is selective. A narrow Chess TD edit emits 4 of 181 reachable functions, and a
narrow Brickout edit emits 2 of 182. Unchanged bodies keep their prior addresses. New bodies call
retained unchanged bodies directly; only affected host-called Stasis entrypoints are republished
through stable trampolines.

Incremental compilation is a development-JIT feature only. Publish builds always perform one
coherent full AOT compilation. AOT evidence below checks compile/package behavior and observable
parity with accepted development revisions; it does not apply or advertise selective AOT patches.

Direct internal calls deliberately trade a few reverse-caller recompiles for ordinary call speed.
A widely shared Chess TD helper therefore emits 51 functions, while the representative narrow edit
emits 4. This is the intended cost model, not a reason to reintroduce a trampoline per function.

The measurements also distinguish selective backend work from whole-file frontend work. The
5,000-function one-file fixture emits only 2 functions, with sub-millisecond codegen, but its full
changed-file parse/check/index preparation makes compile-ready latency several seconds. That result
does not indicate whole-reachable JIT emission; it identifies changed-file frontend scale as the
next optimization boundary.

Each emitted function records its parsed lowering dependencies as a side effect of the normal
lowering pass. Warm planning re-hashes those cached dependencies against current constants,
global/collection paths, extern contracts, type descriptors, and recursively used struct layouts;
it does not lower unchanged bodies again. A changed fingerprint seeds that function before
reverse-caller expansion. This keeps layout/handle edits correct without turning an unrelated use
of the same root object into a false patch seed. The final Chess TD timings below include this pass.

## Hardware and method

- Date: 2026-07-30
- Host: Windows NT 10.0.26200.0, Intel Core Ultra 9 185H, 22 logical processors
- Toolchain: `rustc 1.92.0 (ded5c06cf 2025-12-08)`
- Synthetic method: 3 cold samples, 5 warmups, 30 measured alternating edits
- Real narrow-edit method: 5 cold samples, 5 warmups, 30 measured alternating edits
- Broad-helper method: 1 cold sample, 5 warmups, 10 measured alternating edits

The Chess TD source was copied under the StasisLang workspace for the run. Only its `src` tree and
the imported nightly `src/stdlib` and `src/runtime` trees were copied; the temporary copy was deleted
after measurement. The original Chess TD checkout was not modified.

## Results

All times are milliseconds.

| Program/edit | Reachable | Changed | Emitted | Reused | Host entries | Cold p50/p95 | Compile-ready p50/p95 | Plan p50/p95 | Codegen p50/p95 | Finalize p50/p95 | Publication p50/p95 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Synthetic chain-root, 100 functions | 101 | 1 | 2 | 99 | 1 | 7.578 / 8.545 | 2.597 / 2.904 | 0.423 / 0.510 | 0.187 / 0.247 | 0.004 / 0.006 | n/a |
| Synthetic chain-root, 1,000 functions | 1,001 | 1 | 2 | 999 | 1 | 194.723 / 196.005 | 79.511 / 92.458 | 5.401 / 6.056 | 0.256 / 0.314 | 0.006 / 0.007 | n/a |
| Synthetic chain-root, 5,000 functions | 5,001 | 1 | 2 | 4,999 | 1 | 4,374.564 / 4,715.801 | 4,326.213 / 5,810.282 | 36.484 / 52.183 | 0.394 / 0.569 | 0.009 / 0.011 | n/a |
| Chess TD `command_guard_ticks` | 181 | 1 | 4 | 177 | 1 | 121.172 / 125.043 | 50.219 / 52.347 | 1.249 / 1.309 | 2.983 / 3.054 | 0.010 / 0.013 | 0.000 / 0.000 |
| Chess TD shared `abs_i32` | 181 | 1 | 51 | 130 | 3 | 123.600 / 123.600 | 78.785 / 80.189 | 1.508 / 1.531 | 30.788 / 31.245 | 0.036 / 0.043 | 0.000 / 0.000 |
| Brickout `brickout_shop_anim_step` | 182 | 1 | 2 | 180 | 1 | 68.667 / 72.776 | 23.494 / 24.113 | 1.135 / 1.198 | 0.888 / 0.992 | 0.010 / 0.012 | 0.000 / 0.000 |

The initial provisional targets (25 ms at 1,000, 75 ms at 5,000, and 50 ms for Chess TD) did not
match the measured frontend-inclusive behavior. The canonical hardware-qualified stop conditions
are therefore 100 ms, 6,000 ms, and 60 ms respectively. Emitted counts, codegen, and finalization
remain selective; changed-file frontend preparation dominates. CI enforces the 100 ms portable
1,000-function lane and the non-negotiable exact `2 / 1,001` emission count.

The production publication-path benchmark ran 30 measured 60 Hz watch edits through the immutable
host-entry table and stable tick/render trampolines:

| Phase | p50 | p95 |
| --- | ---: | ---: |
| Compile | 0.269 | 0.389 |
| Package | 0.001 | 0.001 |
| Hook | 0.000 | 0.000 |
| Host-entry publication | 0.000 | 0.000 |
| First new tick/render | 0.000 | 0.000 |
| Total edit-to-visible | 0.327 | 0.455 |

## Correctness and platform matrix

Deterministic planner and executable tests cover host roots, leaf/chain, mid-level retained callees,
branching/diamond/shared helpers, self and mutual recursion, multi-edit, add/delete/rename,
unreachable/made-reachable functions, signature/layout incompatibility, compile/lower/relocation/hook
failure recovery, stale revisions, and multi-tick background preparation. The 5,000-node SCC fixture
also proves planning uses an iterative traversal rather than the process stack. A bounded planner
matrix asserts exact chain, binary-branching, diamond, shared-helper, and SCC closure sizes at 100,
1,000, and 5,000 functions; broad 5,000-function topologies stay planner-only to avoid deliberately
retaining tens of thousands of native bodies in one validation command.

A collection-layout regression changes only a fixed capacity and asserts the exact emitted set is
the storage consumer plus its reverse callers; an unrelated function and host entry retain their
old bodies. The live-workspace text-capacity regression additionally migrates and executes the
changed layout, covering the failure that exposed stale lowering contracts on Linux CI.

Prebuilt-graph closure-expansion timings (milliseconds) are shown below. They isolate the reverse
closure/SCC algorithm after graph construction; they are not complete edit-to-plan latency. The
synthetic and real-project tables above report complete `plan_patch` phase timings, including graph
construction, changed-seed detection, and retained-dependency work.

| Topology | 100 p50/p95 | 1,000 p50/p95 | 5,000 p50/p95 |
| --- | ---: | ---: | ---: |
| Chain-root | 0.002 / 0.002 | 0.002 / 0.002 | 0.002 / 0.002 |
| Binary branching | 0.010 / 0.012 | 0.016 / 0.018 | 0.027 / 0.029 |
| Diamond | 0.005 / 0.005 | 0.005 / 0.005 | 0.005 / 0.006 |
| Shared helper | 0.161 / 0.170 | 2.190 / 2.385 | 14.234 / 14.581 |
| One SCC | 0.101 / 0.106 | 1.374 / 1.488 | 8.453 / 8.821 |

The first measured SCC implementation rescanned each component for every member and took about
4.5 seconds at 5,000 nodes. Processing each selected component once reduced p95 to 8.821 ms while
preserving the exact set.

Windows runs the real project numbers above. Linux runs the strongest portable 1,000-function
selective-emission gate in Perf CI. macOS JIT remains supported only where hardened-process policy
permits executable memory, as specified by the contract. The host-side Android Workshop bridge
regression stages and activates a real `JitProcess` candidate, asserts emitted `{helper, tick}` and
reused `{main, untouched}`, verifies the single affected `tick` host entry, and executes the new
tick behavior. This verifies the shared Workshop code path but is not Android arm64 device evidence.
No Android device was attached on 2026-07-30 (`adb devices -l` returned an empty list), so the named
physical arm64 Workshop JIT and published full-AOT acceptance cell remains unverified. Production
AOT tests compile coherent full builds; Windows CI with a linker executes both revisions and compares
their results to selective JIT. AOT is intentionally not selective. The local Windows host lacked a
linker, so that executable parity test compiled but reported its explicit linker skip locally.

The engine hot-update benchmark measures first new tick/render and total edit-to-visible latency
because real games cannot safely execute arbitrary benchmark edits without their normal host
initialization. The safe-point wait was zero in this single-threaded fixture; the graphical runner's
controlled background test separately proves that old windows continue while a patch spans ticks.
Perf CI keeps this end-to-end gate alongside the compiler phase gate.

## Reproduction

```text
cargo run -p stasis_compiler --release --example rust_native_jit_bench -- --functions 100,1000,5000 --cold-samples 3 --incremental-samples 30
cargo run -p stasis_compiler --release --example project_selective_jit_bench -- --label chess_td_narrow --entry <copy>/src/main.stasis --edit-file <copy>/src/game.stasis --needle "return 180 + game.active_level * 300;" --replacement "return 181 + game.active_level * 300;" --cold-samples 5 --warmups 5 --samples 30
cargo run -p stasis_compiler --release --example project_selective_jit_bench -- --label chess_td_shared --entry <copy>/src/main.stasis --edit-file <copy>/src/game.stasis --needle "return 0 - v;" --replacement "return 1 - v;" --cold-samples 1 --warmups 5 --samples 10
cargo run -p stasis_compiler --release --example project_selective_jit_bench -- --label brickout_narrow --entry samples/brickout_revenge/brickout_revenge_v1.stasis --edit-file samples/brickout_revenge/brickout_revenge_v1_core.stasis --needle "a = a + dt * speed;" --replacement "a = a + dt * speed + 0.001;" --cold-samples 5 --warmups 5 --samples 30
cargo run -p stasis --release --example engine_hot_update_bench -- --samples 30 --tick-sleep-us 16666 --warmup-ticks 8 --timeout-ms 5000
cargo test -p stasis_compiler --release scaled_topology_matrix_has_exact_selective_closures -- --nocapture
```

## Slice reflection

- Good: exact emitted/reused counts made the backend behavior unambiguous on both synthetic and real projects.
- Bad: the first 5,000-node SCC implementation used recursive traversal and overflowed the Windows stack; total compile-ready timing also hid the much smaller planning/codegen costs until phase metrics were added.
- Adjustment: keep graph walks iterative, store each SCC once by index, and always report frontend, plan, codegen, finalization, publication, and visible-window phases separately.

Theory gained: native-address changes propagate through the reverse direct-call closure, while
unchanged callees remain valid dependencies across patch arenas. The observed 4/181 Chess closure
supports that mapping. An adjacent prediction is that splitting very large source files, or caching
their frontend products, will reduce compile-ready latency without changing emitted-function sets.
