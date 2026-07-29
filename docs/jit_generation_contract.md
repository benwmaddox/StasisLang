# Direct-Call JIT Generation Contract

Status: architecture contract for the JIT generation migration

This document is the canonical implementation contract for replacing per-function JIT dispatch
with complete, immutable, direct-call generations. `docs/live-compilation-prd.md` owns product
requirements, `docs/spec.md` owns observable language behavior, and this document owns the
compiler/runtime ABI, lifecycle, and verification details.

## Mapping, rationale, and extension point

- Mapping: one source revision becomes one complete reachable program, one state layout, one host
  export set, and one executable-memory owner. A tick and its render observe that package through
  one `ActiveGeneration` reference.
- Rationale: direct internal calls are safe only while the caller, callee, state bindings, and
  executable memory have the same lifetime. Publishing individual function pointers would allow a
  mixed generation and is therefore forbidden.
- Extension point: a future debugger may retain immutable metadata or state snapshots by
  generation number. It must not retain executable entry pointers or keep code memory alive after
  the execution window ends.

The nearest tempting alternative is to keep `FnId -> code_ptr` for unchanged functions and use
direct calls only for changed code. That creates cross-generation edges, preserves the hot-path
lookup mechanism, and makes retirement depend on an unbounded graph of code ownership. It is not a
compatibility mode.

## Non-negotiable invariants

1. A development build finalizes the complete reachable Stasis program as one generation.
2. Calls between Stasis functions are direct Cranelift calls within that generation.
3. Only explicit host exports cross the generation boundary.
4. The host publishes all exports, state bindings, and executable-memory ownership by replacing
   one `ActiveGeneration` reference. Independent pointer stores are forbidden.
5. An execution window snapshots one `ActiveGeneration`; `tick`, its following `render`, and every
   synchronous internal call use only that snapshot.
6. No Stasis frame or raw compiled pointer may outlive its execution window.
7. Parse, index, semantic analysis, lowering, code generation, and finalization never run on the
   runtime thread.
8. A failed or superseded build never becomes visible and never mutates active state.
9. Publication occurs only between complete execution windows.
10. JIT and AOT use the same internal direct-call lowering and host-export ABI.

`FnId` may remain as a compiler identity for diagnostics, call graphs, semantic caches, and edit
summaries. It is not a runtime dispatch key and does not authorize retaining a body pointer.

## Program and generation contents

Reachability starts from:

- lifecycle entries present in the program: `main`, `tick`, `render`, and `on_code_swap`;
- entries required by the selected versioned host-set manifest; and
- any other source declaration explicitly exported through that manifest.

The compiler resolves this complete root set before lowering. A generation contains every
reachable function body and reachable struct/type metadata, and excludes unreachable definitions.
The host export manifest records each exported symbol's stable name, signature hash, phase policy,
and code address inside the generation.

Conceptual ownership is:

```text
ActiveGeneration
  generation_number       successful-publication sequence
  source_revision         immutable input revision
  program_hash            complete reachable semantic identity
  target                  host target and feature profile
  host_exports            complete immutable name -> typed entry map
  state_layout            canonical compiler-owned layout
  state_bindings          storage used by this generation
  executable_owner        keeps all generated code alive
  diagnostics_metadata    symbols, source spans, and cost/data-flow summaries
```

`PendingGeneration` has the same code, metadata, and ownership fields plus a build request ID,
compatibility report, and candidate state plan. It is not callable by ordinary runtime work. The
commit transaction may invoke only its validated `on_code_swap` export against isolated candidate
state.

The active reference is immutable after publication. Mutable gameplay data lives behind the state
bindings owned by that generation, but the identity and addresses of those bindings cannot change
inside an execution window.

## Call and host-export ABI

### Internal calls

- The compiler declares all reachable functions in one Cranelift module before defining bodies.
- Each resolved Stasis-to-Stasis call lowers to the module-local callee `FuncRef`.
- Recursive calls and strongly connected components use the same declarations and direct calls.
- No internal call imports an arity wrapper, `FnId` lookup, dispatch mutex, or body pointer from a
  previous generation.
- Shared lowering selects the same direct-call ABI for JIT and AOT. Backend policy differs only in
  target symbol binding, finalization, and artifact ownership.

Semantic hashes still avoid unnecessary frontend analysis and may cache target-independent HIR,
call-graph facts, or pre-finalization lowering inputs. They must not reuse a live machine-code body
or relocation whose lifetime belongs to another generation.

### Host exports

The compiler, not the runtime, discovers exports from lifecycle rules and the selected host-set
manifest. Before finalization it rejects duplicate names, missing required entries, unsupported
signatures, and exports not reachable from the declared root set.

The host resolves an export only through its execution-window generation snapshot. It may cache a
typed entry address on the stack for that window; it may not store the address in a process-global,
callback, component, or per-function table. A missing optional export is represented in the
generation metadata, never by consulting another generation.

Host-export signatures are compatibility boundaries. An update that changes a required export's
ABI is rejected unless the host-set version changes through an explicit restart or negotiated
upgrade path. Ordinary internal functions may be added, removed, renamed, or have signatures
changed because all reachable callers are rebuilt together.

## Thread ownership and messages

The file watcher and editor produce immutable source revisions. The coordinator may coalesce them,
but the compiler service owns its own source snapshot and compiler caches. Runtime state, compiler
AST/HIR, Cranelift module state, and executable allocators are never shared mutably across the
compiler and runtime threads.

The versioned message boundary is:

```text
FileChangeEvent(path, revision, text_source, change_kind)
BuildGeneration(request_id, revision, source_snapshot_id, target, host_set, active_contract)
BuildFinished(request_id, revision, status, diagnostics[], pending_generation?)
CommitGeneration(request_id, pending_generation)
CommitFinished(request_id, status, active_generation_number?, diagnostic?)
CancelBuild(request_id, superseded_by_request_id)
```

The watcher sends `FileChangeEvent` to the coordinator. The coordinator coalesces events, creates an
immutable source snapshot, and sends its ID in `BuildGeneration`; the compiler never reconstructs a
snapshot from mutable watcher state. `active_contract` contains immutable host-export ABI and
state-layout metadata, not runtime values or pointers. `BuildFinished` transfers ownership of a
finalized `PendingGeneration`; it never installs code or storage. Cancellation is a message/control
flag only and does not expose mutable compiler state. The coordinator records the newest requested
revision. Results for older revisions are discarded even if cancellation arrived too late to stop
code generation.

The runtime thread may validate metadata, prepare bounded candidate state, run the swap hook, and
replace the active reference. It must not parse, check, lower, generate, link, or finalize code.

## One generation state machine

There is one state machine per game process. `N` is the active successful generation number and
`R` is a build request ID.

| State | Accepted event | Guard and action | Next state |
| --- | --- | --- | --- |
| `Running(N)` | source revision queued | Allocate monotonically increasing `R`; keep running `N`. | `Building(N,R)` |
| `Building(N,R)` | newer revision queued | Send cancellation for `R`, allocate newer request `R2`; `N` remains active. | `Building(N,R2)` |
| `Building(N,R)` | build fails | Publish diagnostics only; release all request-owned artifacts. | `Running(N)` |
| `Building(N,R)` | stale or cancelled build finishes | Release the result without commit or hook execution. | Current state for the newest request |
| `Building(N,R)` | current build finishes successfully | Transfer one finalized `PendingGeneration`; keep running `N`. | `Ready(N,R)` |
| `Ready(N,R)` | newer revision queued | Discard the candidate, allocate `R2`; `N` remains active. | `Building(N,R2)` |
| `Ready(N,R)` | execution window is active | Defer; do not block or interrupt the window. | `Ready(N,R)` |
| `Ready(N,R)` | between-window safe point | Revalidate request freshness, export ABI, target, layout plan, and resource bounds; create isolated candidate state. | `Preparing(N,R)` |
| `Preparing(N,R)` | newer revision queued or supersession observed | Mark `R` stale, destroy any candidate state, and start the newest request `R2`; no hook runs. | `Building(N,R2)` |
| `Preparing(N,R)` | validation or migration fails | Destroy candidate state and generation; active code/state are unchanged. | `Running(N)` |
| `Preparing(N,R)` | no hook is present | Preflight the infallible publication record. | `Publishing(N,R)` |
| `Preparing(N,R)` | valid hook is present | Invoke the candidate hook once against isolated candidate state. | `Hook(N,R)` |
| `Hook(N,R)` | newer revision queued while the hook is running | Let the synchronous hook return, mark `R` stale, destroy its isolated effects, and start newest `R2`; never publish `R`. | `Building(N,R2)` |
| `Hook(N,R)` | hook rejects or traps | Destroy candidate state and generation; active code/state are unchanged. | `Running(N)` |
| `Hook(N,R)` | hook succeeds | Preflight the infallible publication record. | `Publishing(N,R)` |
| `Publishing(N,R)` | current-request compare-and-exchange fails | `R` was superseded after preflight; destroy it and start the newest request without visibility. | `Building(N,R2)` |
| `Publishing(N,R)` | current-request compare-and-exchange succeeds | Linearize request freshness and the owning-reference exchange, assign `N+1`, then report success. | `Running(N+1)` |

`Publishing` has no fallible candidate work. Allocation, migration, hook execution, export lookup,
and diagnostic construction finish before it. Its one compare-and-exchange verifies both that `R`
is still the coordinator's current request and that `N` is still active while replacing the owning
generation reference. A revision ordered before that linearization makes the exchange fail and can
never publish `R`; a revision ordered after it is a new request based on `N+1`. If the platform
cannot provide this atomic current-request + owning-reference publication boundary, that target
must reject development hot swap at startup.

Generation numbers start at one for the first successfully published program and increase only on
successful publication. Failed, cancelled, and superseded request IDs do not consume generation
numbers. Request IDs and source revisions are separate monotonic domains.

## Safe point and execution window

An execution window begins when the runtime acquires the active owning reference and ends after
all synchronous Stasis calls for that frame return. For the graphical loop it contains `tick` and
the following `render`; headless and tool entrypoints define the same acquire/call/release shape.

A safe point exists only when:

- the previous window has released its generation reference;
- no Stasis stack frame is active;
- no swap transaction is active;
- no host callback can re-enter using a pointer captured from the previous window; and
- state bindings are not being replaced by another host operation.

Compilation may span any number of ticks. The runtime continues complete windows on `N` while
`R` builds. Once a current candidate is ready, publication waits for the next safe point. It never
interrupts a frame and never publishes between `tick` and `render`.

## State migration and `on_code_swap`

The compiler emits the canonical candidate layout and a deterministic migration plan. At the safe
point the runtime creates candidate storage without rebinding active storage, copies compatible
values, initializes new values, applies bounded collection rules, and verifies the result.

The optional candidate `on_code_swap(): void` runs exactly once after migration and before
publication. Its calls are direct calls inside the pending generation and use candidate state
bindings. It may mutate candidate Stasis state or call `reject_code_swap()`. It cannot call
`main`, `tick`, `render`, non-transactional host effects, or an extern that can retain a callback or
pointer. Phase-policy validation rejects those call paths before execution.

Because candidate state is isolated, a hook rejection or trap requires destruction, not restoration
of already-mutated active state. The active generation continues with the exact code and values it
held before the attempt.

## Long-lived execution and callbacks

- Host calls from Stasis are synchronous unless their ABI explicitly copies bounded value data and
  returns before the Stasis frame exits.
- An asynchronous service may later request a new host-export invocation by symbolic export name;
  the runtime resolves that name from the generation captured by the future execution window.
- Host code may not cache a Stasis body pointer, export pointer, state-binding address, or borrowed
  reference after the call returns.
- Fibers, suspended Stasis frames, coroutines, guest-created threads, and callbacks that re-enter a
  captured generation are unsupported. The compiler or host-set validator emits
  `GENERATION_LONG_LIVED_EXECUTION_UNSUPPORTED` rather than silently extending code lifetime.
- Foreign libraries that require stable callbacks must use a host-owned trampoline that carries no
  guest code pointer and schedules symbolic re-entry at a future safe execution window.

## Retirement and executable memory

The execution window holds an owning reference to `ActiveGeneration`. After publication the old
generation becomes retiring. Its code, export map, state bindings, and loader/module handle are
released together when the last owning window reference is dropped. A fixed tick delay is not a
proof of safety and is removed with the pointer-table retirement window.

Under the no-long-lived-frame rule, at most these complete code owners exist in steady operation:
the active generation, one current pending generation, and one transient retiring generation. A
new build supersedes and releases any older pending generation. Runtime diagnostics report active,
pending, and retiring generation numbers and executable bytes separately. At a quiescent safe point
after publication, retiring generation count must be zero.

## Failure table

| Failure | Detection owner | Active visibility | Required result |
| --- | --- | --- | --- |
| Parse, semantic, lowering, or finalization error | Compiler service | No change | Discard request artifacts; report source diagnostics. |
| Missing/duplicate host export or invalid export ABI | Compiler service | No change | Reject build with symbol and expected ABI. |
| Internal unresolved call or cross-generation import | Compiler service | No change | Reject build; never emit a fallback dispatch call. |
| Target/feature mismatch | Compiler service/runtime preflight | No change | Reject with required and actual target profile. |
| Cancelled or superseded request finishes before preparation | Coordinator | No change | Release it without migration or hook execution. |
| Request is superseded during migration or hook execution | Coordinator/runtime pre-publication check | No change | Finish only the synchronous isolated work needed to unwind, destroy its effects, and start the newest request. |
| Current-request compare-and-exchange fails | Runtime publication boundary | No change | Destroy the stale candidate; never retry it. |
| No safe point yet | Runtime | No change | Continue complete old-generation windows and retry. |
| Resource or executable-memory preflight fails | Runtime | No change | Release candidate; report requested and allowed bytes. |
| Host-export signature incompatibility | Runtime preflight | No change | Reject with export name and old/new signatures. |
| State-layout migration incompatibility | Runtime preflight | No change | Reject with exact state path and reason. |
| Candidate allocation or migration fails | Runtime transaction | No change | Destroy candidate state and generation. |
| `on_code_swap` phase violation, trap, or explicit rejection | Runtime transaction | No change | Destroy candidate state and generation; report hook diagnostic. |
| Atomic owning-reference publication unavailable | Runtime startup/preflight | No change | Disable JIT hot swap with deterministic target diagnostic. |
| Attempted retained pointer, callback, fiber, or guest thread | Compiler/host-set validation | No change | Reject as unsupported before execution. |

There is no partial-publication recovery row: all fallible work precedes the one owning-reference
exchange.

## Target and platform matrix

`Required` means the implementation children must provide the named deterministic CI or device
evidence. A supported macOS development host is an unsigned/local developer process that can obtain
Cranelift executable memory; a hardened process without the required entitlement is explicitly
unsupported for JIT and must use AOT.

| Target | Development JIT | Production AOT | Required G4 evidence |
| --- | --- | --- | --- |
| Windows x86_64 | Required, native host JIT | Required, native PE/COFF | Windows x86_64 PR CI plus the pinned performance runner; complete generation publication test. |
| Linux x86_64 | Required, native host JIT | Required, native ELF | Linux x86_64 PR CI; complete generation publication test. |
| macOS x86_64 | Required for unsigned/local developer processes | Required, native Mach-O | Native x86_64 macOS CI runner; explicit hardened-process exclusion diagnostic test. |
| macOS arm64 | Required for unsigned/local developer processes | Required, native Mach-O | Native arm64 macOS CI runner; no translated JIT; explicit hardened-process exclusion diagnostic test. |
| Android arm64 | Required in Workshop | Required in published package | Named physical arm64 device for Workshop JIT, plus published AOT package/link/launch evidence on arm64. |

JIT is native-host-only. A requested JIT triple different from the running process fails with
`JIT_TARGET_MUST_MATCH_HOST`; it never falls back to AOT or an interpreter. Production AOT packages
do not perform source compilation or live JIT swaps. If an AOT watch/loader tool replaces code, it
must load and publish a complete module through this same owning-reference contract.

iOS arm64 remains AOT-only because the product does not ship a JIT/executable-memory entitlement
path there. Android x86/x86_64, Windows arm64, Linux arm64, and WebAssembly are outside this task's
required matrix and must be excluded by target selection rather than silently counted as covered.
The repository's standard `Stasis_API_35` AVD is x86_64 and may provide a separate Workshop smoke
check, but it cannot satisfy any Android arm64 G4 gate.

## Performance and memory budgets

These are initial gates, not claims about real games. Every report separates frontend, codegen,
finalization/link, safe-point wait, publication, old-generation ticks, and executable bytes.

Existing cold trivial-function observations are 11.275 ms for 100 functions, 110.160 ms for 1,000,
and a historical 1,738.212 ms for 5,000. Their original machine profile was not recorded, so they
are planning baselines rather than independently reproducible proof. G4 must check in a versioned
`tests/perf/generation_reference_profile.json` before enforcing the gates. It records runner name,
CPU model/core count, RAM, OS/build, power profile, target triple, Rust/Cranelift versions, tick
rate, fixture hashes, parent commit, and benchmark command. A runner profile change requires a
recorded parent-baseline rerun and contract review; results from another profile are not compared to
these absolute gates.

The complete-generation implementation must meet these bounded p95 gates on that pinned Windows
x86_64 performance runner:

| Fixture | Background complete-generation p95 | Edit-to-visible p95 |
| --- | ---: | ---: |
| 100 trivial reachable functions | 25 ms | compile time plus at most 2 tick intervals |
| 1,000 trivial reachable functions | 150 ms | compile time plus at most 2 tick intervals |
| 5,000 trivial reachable functions | 2,500 ms | compile time plus at most 2 tick intervals |
| Brickout-scale representative game | Baseline and gate recorded by child G4 | compile time plus at most 2 tick intervals |

Measurement protocol:

1. Build release benchmark binaries before timing; compilation of the benchmark harness is excluded.
2. Run with the pinned power profile and no concurrent repository compiler/test process.
3. For cold-generation timing, start from an empty compiler cache and fresh process for each sample.
4. Run five unmeasured warmups, then 30 measured samples for 100/1,000 functions and 10 measured
   samples for 5,000 functions and Brickout-scale.
5. Compute p50 and nearest-rank p95 from raw monotonic-clock nanoseconds. Store every raw sample,
   phase breakdown, command, profile, and commit in the CI artifact.
6. Measure edit-to-visible in a persistent runtime after one accepted baseline generation, using 30
   deterministic one-function edits. Count old-generation ticks from source-revision enqueue through
   successful publication.
7. Run the parent commit and candidate commit on the same profile in the same job. The absolute gate
   controls acceptance; the paired parent result explains environmental drift and is not a waiver.

Publication itself has a p95 budget of 0.25 ms on the pinned Windows performance runner and 1.0 ms
on the named Android arm64 target, excluding bounded state migration and hook time, and a hard
requirement below one 60 Hz tick. Other desktop matrix rows record p50/p95 and must remain below one
tick until a platform-specific tighter gate is established. Old-generation
tick count must agree with `ceil(background_compile_ms / tick_period_ms)` within two ticks; a long
compile is update latency, never a frame stall.

At a safe point the runtime may own no more than active + current pending + one transient retiring
generation. After the next quiescent safe point, retired executable bytes must be zero aside from
allocator pages explicitly reported as reusable slack. A 100-swap stress test must show no upward
trend in quiescent executable bytes after normalizing for the active generation size.

Real-game desktop and Android p50/p95 are mandatory before the migration is declared complete. A
budget miss blocks the relevant child or requires an evidence-backed contract revision; it does not
authorize partial dispatch, mixed generations, or runtime-thread code generation.

## Implementation sequence and bounded verification gates

### G0 - Contract lock (Maddox #174)

- Replace pointer-table and patch-set requirements in the PRD, specification, checklist, and shared
  backend architecture notes.
- Record this state machine, failure table, matrix, budgets, and obsolete-path deletion list.
- Gate: documentation consistency checks plus repository validation; no runtime behavior claim.

### G1 - Complete direct-call modules (Maddox #175)

- Predeclare and define every reachable function in one JIT module using the shared JIT/AOT
  direct-call lowering path.
- Retain semantic caches only before final machine-code ownership.
- Delete internal arity dispatch imports and per-function machine-code patch assembly.
- Gates, each under 300 seconds: compiler unit tests; CLIF assertions for leaf, mid-level, shared,
  recursive/SCC, host-root, added/deleted/renamed, and unreachable functions; representative
  executable JIT and AOT sample with no internal dispatch imports.

### G2 - Atomic publication and retirement (Maddox #176)

- Add owning `PendingGeneration`/`ActiveGeneration`, versioned messages, single-reference
  publication, safe-point snapshots, isolated migration/hook execution, supersession, and
  reference-counted retirement.
- Delete independent host-entry stores, pointer-table generation commits, and tick-delay retirement.
- Gates: delayed build runs old code for multiple complete windows; the next window is purely new;
  A -> B supersession publishes only B; compile/migration/hook failures preserve the exact active
  reference and state; 100-swap lifetime test reaches zero retired owners.

### G3 - Transition fixture matrix (Maddox #177)

- Cover host roots, ordinary leaf and mid-level callers, shared utilities, recursive/SCC calls,
  render, `on_code_swap`, multiple edits, add/delete/rename, unreachable changes, compatible body
  edits, incompatible host ABI/layout edits, syntax/lowering failures, and recovery.
- Gate: deterministic observable generation/state assertions across JIT and AOT; no fixture may
  inspect an implementation-only per-function pointer.

### G4 - Platform and performance matrix (Maddox #178)

- Run every required target row and record explicit exclusions.
- Measure cold generation, edit-to-visible p50/p95, affected function count, frontend/codegen/link/
  publication time, ticks on old code, and executable-memory retirement for 100/1,000/5,000 and
  Brickout-scale fixtures.
- Gates: bounded repository validation, Android Workshop JIT and published AOT acceptance on the
  named physical arm64 device, separate optional x86_64 AVD smoke, native JIT/AOT CI for supported
  desktop rows, and all budgets above.

Every child ends with a touched-file cruft pass, a representative compiled Stasis executable, and
the AGENTS.md Good/Bad/Adjustment and Theory gained summary.

## Obsolete paths to remove

The migration is incomplete while any production path retains:

- internal `FnId -> code_ptr` lookup or per-arity dispatch wrappers;
- per-function live machine-code reuse, patches, staged pointer overrides, or independent export
  stores;
- patch-set-based compile/commit messages or `swapped_fn_ids` as publication truth;
- hook lookup through a staged per-function pointer preview;
- fixed tick-count retirement of executable regions;
- separate JIT indirect-call and AOT direct-call lowering implementations; or
- fallback behavior that emits placeholder code when complete generation compilation fails.

Compatibility shims may exist only in tests during the child that deletes them and must be removed
before that child is complete.
