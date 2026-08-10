# Selective Direct-Call JIT Patch Contract

Status: canonical architecture contract for development JIT updates

This document replaces the superseded complete-reachable-generation contract from Maddox
#173-#178. `docs/live-compilation-prd.md` owns product requirements, `docs/spec.md` owns observable
language behavior, and this document owns selective invalidation, code binding, publication, and
verification. Production AOT remains a coherent full-program build.

## Mapping, rationale, and extension point

- Mapping: one accepted source revision produces one validated patch containing the changed
  function/SCC and the minimum reverse direct-call closure required to reach stable host-entry
  trampolines.
- Rationale: ordinary Stasis calls should remain direct in development. Moving a callee therefore
  requires rebuilding its direct callers, then their callers, until the changed address is hidden
  behind a stable host-entry trampoline. Unrelated compiled bodies remain useful and are reused.
- Extension point: automatic executable-code reclamation may later trace current entry targets and
  retained code dependencies. Priority-1 work intentionally retains superseded JIT arenas until a
  manual process restart.

The intended tradeoff is explicit: compared with a trampoline on every function, a compatible
body edit may compile a few additional reverse callers; compared with rebuilding every reachable
function, it avoids unrelated backend work. A highly shared helper may still produce a broad
closure. Patch size is a measured graph property, not a fixed promise.

## Non-negotiable invariants

1. Full semantic analysis runs for every changed file before backend invalidation is accepted.
2. Calls between Stasis functions are direct Cranelift calls. No internal hash lookup, mutex,
   ABI-by-arity wrapper, or stable per-function trampoline is allowed.
3. Stable trampolines exist only for host-to-Stasis entries: `main`, `tick`, `render`,
   `on_code_swap`, manifest exports, and Stasis callbacks whose address explicitly escapes to the
   host.
4. Outbound runtime/foreign imports use the normal native import ABI and are not Stasis
   publication trampolines.
5. A warm edit emits only the exact affected reverse-caller closure. Unaffected reachable bodies
   keep their addresses and may be called directly by newly emitted code.
6. Recursive and mutually recursive functions invalidate and emit as strongly connected
   components.
7. All affected host-entry targets publish as one safe-point transaction between complete
   execution windows. Partial root publication is forbidden.
8. Parse, index, semantic analysis, invalidation planning, lowering, code generation, relocation,
   and finalization never run on the runtime thread.
9. A failed or superseded patch never becomes visible and never mutates active state.
10. Superseded executable code may remain allocated until process restart. Automatic retirement
    and compaction are not priority-1 correctness requirements.
11. JIT and AOT share semantic analysis and per-function lowering. JIT selects a patch closure;
    AOT emits the complete reachable program.

An eligible `@inline` call is still represented by the ordinary caller-to-callee dependency edge.
The shared lowering path may embed the callee expression in both AOT and JIT machine code while a
real callee body remains emitted. A body or annotation change therefore seeds the normal reverse
caller closure; selective JIT must never retain a caller containing stale embedded code.

`FnId` remains a stable compiler identity for call graphs, hashes, diagnostics, patch plans, and
metadata. It is not an internal runtime dispatch key.

## Host-entry boundary and reachability

Reachability starts from lifecycle entries present in the program (`main`, `tick`, `render`, and
`on_code_swap`), entries required by the selected host-set manifest, and explicit host callbacks.
An internal function whose address escapes becomes an explicit host-entry boundary and receives a
stable trampoline; accidental pointer escape is a compile-time error.

Each stable trampoline resolves its typed target through one immutable `ActiveEntryTable`. The
runtime may replace that table only at a safe point. Internal compiled code never consults the
table. The table contains host-entry names, signatures, phase policies, target body addresses, the
accepted revision, and patch number. Gameplay state remains runtime-owned and is not stored in the
entry table.

## Patch contents and exact invalidation

The compiler retains the accepted revision's canonical function identities, signatures, semantic
hashes, reachability, forward call graph, reverse caller graph, layout facts, and active body
addresses. After whole-file checking it builds an immutable `PatchPlan`.

The seed set contains reachable functions whose body, signature, or lowered contract changed;
affected SCC peers; newly reachable functions; and functions whose embedded layout/global access
facts changed. The planner then adds reverse direct callers whenever the callee address, ABI, or
lowered call contract requires a new caller body. A host-entry trampoline ends propagation only for
the external host caller, which is not a Stasis call-graph node. If another Stasis function directly
calls a host-entry function, that real reverse edge continues through the normal closure.

The planner does not add unchanged callees merely because patched code calls them. It records their
accepted addresses as retained dependencies. An unreachable body edit produces no JIT code until a
later graph change makes it reachable. Deleted or renamed functions require every remaining caller
to resolve successfully; otherwise the patch fails before emission.

Conceptually:

```text
PatchPlan
  request_id             monotonically ordered build request
  source_revision        immutable checked source snapshot
  changed_ids            semantic change seeds with reasons
  affected_sccs          recursive compilation units
  re_jit_ids             exact reverse-caller closure
  reused_dependencies    unchanged FnId -> accepted body address
  affected_entries       host-entry targets replaced at publication
  signature_delta        internal and host ABI facts
  layout_delta           storage facts and migration plan
  reason_chains          source change -> every scheduled FnId
```

### Worked call-graph examples

In these diagrams `H` is a host-entry trampoline target and arrows point from caller to callee.

Leaf chain: `H -> A -> B -> C`. Editing `C` emits `{C,B,A,H}`. An unrelated `H2 -> U` remains
untouched. Editing `A` emits `{A,H}` and reuses `B` and `C` directly.

Diamond: `H -> A -> S` and `H -> B -> S`. Editing shared `S` emits `{S,A,B,H}`. Editing `A`
emits `{A,H}` and reuses `S`.

Multiple roots: `tick -> S` and `render -> S`. Editing `S` emits `{S,tick,render}` and publishes
the two entry targets together. Neither root may switch independently.

SCC: `H -> A`, `A -> B`, `B -> A`. Editing either `A` or `B` emits `{A,B,H}`.

Signature change: `H -> A -> B`. Changing `B`'s signature is accepted only when `A` is updated to
type-check against it; the emitted closure is `{B,A,H}`. A host-entry signature change is rejected
unless the host-set ABI changes through an explicit restart/upgrade path.

Unchanged callee reuse: `H -> A -> U`. Editing `A` emits `{A,H}`. New `A` binds a direct native call
to the retained accepted address of `U`; `U` is not re-JITed.

## Direct-call patch ABI

The JIT creates one staged module for the affected closure. It predeclares every re-JITed function
so calls within the patch, including SCC edges, resolve to new bodies. Calls to unchanged retained
functions bind directly to their accepted native addresses. Such cross-patch direct calls are
required behavior, not a compatibility fallback.

No internal call may import `stasis_jit_lookup_code_ptr`, `stasis_jit_call_*`, a dispatch mutex, or
a host-entry trampoline. Backend-specific code may bind retained addresses and host/runtime
symbols, but shared lowering owns call signatures, argument lowering, and direct-call semantics.

Cold JIT compilation emits every reachable function because no accepted bodies exist. Warm JIT
compilation emits only `PatchPlan.re_jit_ids`. AOT always emits every reachable function into its
production artifact and never consumes live JIT addresses.

## Thread ownership and messages

The watcher/editor, coordinator, compiler service, and runtime communicate with immutable messages:

```text
FileChangeEvent(path, revision, text_source, change_kind)
BuildPatch(request_id, revision, source_snapshot_id, target, host_set, active_contract)
BuildFinished(request_id, revision, status, diagnostics[], pending_patch?)
CommitPatch(request_id, pending_patch)
CommitFinished(request_id, status, active_patch_number?, diagnostic?)
CancelBuild(request_id, superseded_by_request_id)
```

`active_contract` contains accepted function identities, hashes, graph/layout/host ABI metadata,
and retained body addresses in a compiler-service snapshot. The runtime does not expose mutable
entry tables or gameplay state to the compiler thread. `BuildFinished` transfers ownership of one
finalized `PendingPatch`; it installs nothing. Results older than the newest requested revision are
discarded even if code generation finished before cancellation was observed.

## Selective patch state machine

`N` is the accepted patch number and `R` is a build request ID.

| State | Accepted event | Guard and action | Next state |
| --- | --- | --- | --- |
| `Running(N)` | source revision queued | Allocate `R`; snapshot source and accepted compiler contract; keep running `N`. | `Building(N,R)` |
| `Building(N,R)` | newer revision queued | Cancel/supersede `R`, allocate `R2`; active entries remain unchanged. | `Building(N,R2)` |
| `Building(N,R)` | build fails | Publish diagnostics only; retain active entries/state. | `Running(N)` |
| `Building(N,R)` | stale build finishes | Retain no callable reference to its patch; never run its hook. | Current newest-request state |
| `Building(N,R)` | current build succeeds | Transfer one validated `PendingPatch`; keep running `N`. | `Ready(N,R)` |
| `Ready(N,R)` | newer revision queued | Mark patch stale and start `R2`. | `Building(N,R2)` |
| `Ready(N,R)` | execution window active | Continue the complete old window. | `Ready(N,R)` |
| `Ready(N,R)` | between-window safe point | Revalidate freshness, host ABI, retained dependencies, and layout; create isolated candidate state when required. | `Preparing(N,R)` |
| `Preparing(N,R)` | superseded or validation/migration fails | Discard isolated effects; active entries/state remain unchanged. | `Building(N,R2)` or `Running(N)` |
| `Preparing(N,R)` | valid hook present | Invoke staged `on_code_swap` against isolated candidate state. | `Hook(N,R)` |
| `Preparing(N,R)` | no hook | Finish all fallible preflight. | `Publishing(N,R)` |
| `Hook(N,R)` | superseded, rejects, or traps | Let synchronous work unwind, discard candidate effects, never publish. | `Building(N,R2)` or `Running(N)` |
| `Hook(N,R)` | succeeds and remains current | Finish all fallible preflight. | `Publishing(N,R)` |
| `Publishing(N,R)` | freshness compare-and-exchange fails | Discard stale publication record; never retry it. | `Building(N,R2)` |
| `Publishing(N,R)` | freshness compare-and-exchange succeeds | Exchange one immutable `ActiveEntryTable`, assign `N+1`, retain all referenced code arenas. | `Running(N+1)` |

`Publishing` performs no parsing, allocation, migration, hook execution, symbol resolution, or
diagnostic construction. A revision ordered after the successful exchange is a new request based
on `N+1`.

## Safe point and execution window

An execution window contains `tick`, its following `render`, and every synchronous Stasis call they
make. Headless/tool hosts define an equivalent complete window. Publication occurs only after the
window returns, when no Stasis frame or synchronous re-entry is active. Compilation may span any
number of ticks; the old entry table continues serving complete windows.

The host snapshots one entry-table reference for a window. Stable entry trampolines resolve through
that snapshot, so `tick`, `render`, and callbacks scheduled within the window cannot observe
different patch numbers. Asynchronous host work may schedule symbolic entry invocation in a future
window; it may not retain an internal body pointer.

## State migration and `on_code_swap`

Body-only patches with unchanged state layout reuse active storage. A compatible layout change uses
the compiler-owned bounded migration plan and isolated candidate storage. The staged
`on_code_swap(): void` target runs after migration and before publication against candidate state.
It may reject the patch. Failure destroys candidate state; active code/state were never modified.

Layout invalidation is precise where compiler facts prove which bodies embed changed offsets or
storage contracts. If precision is unavailable, the planner conservatively seeds every reachable
body that may access the changed storage, then applies the normal reverse-caller closure. It does
not silently fall back to whole-program warm emission without reporting the reason and affected set.

## Code lifetime and restart reclamation

New patch modules may call unchanged bodies in older JIT arenas, so an accepted program may span
multiple arenas. Priority-1 development keeps every successfully finalized arena that may be
referenced by accepted or later code for the process lifetime. Superseded and replaced bodies are
not invoked after publication, but their storage need not be reclaimed.

Diagnostics should report patch count and retained executable bytes when available. Unbounded
editing is not promised: a developer may restart the process to reclaim all JIT code. Automatic
graph tracing, arena retirement, compaction, and code movement are deferred and must not complicate
the selective compile/publication path.

## Failure table

| Failure | Detection owner | Active visibility | Required result |
| --- | --- | --- | --- |
| Parse, semantic, graph, lowering, relocation, or finalization error | Compiler service | No change | Discard request artifacts and report deterministic source/phase diagnostics. |
| Invalid or non-minimal affected closure | Compiler planner/tests | No change | Reject the plan; never substitute silent whole-generation emission. |
| Missing retained callee address or retained ABI mismatch | Compiler service | No change | Reject with caller, callee, expected signature, and accepted revision. |
| Unsupported internal pointer escape | Compiler/host-set validation | No change | Require an explicit host-entry callback boundary or reject. |
| Missing/duplicate host entry or invalid host ABI | Compiler/runtime preflight | No change | Reject with entry and expected ABI. |
| Cancelled or superseded request completes | Coordinator | No change | Discard it without hook/publication. |
| No safe point yet | Runtime | No change | Continue complete old windows and retry. |
| Layout migration incompatibility or allocation failure | Runtime transaction | No change | Destroy candidate state; active state remains unchanged. |
| `on_code_swap` phase violation, trap, rejection, or mid-hook supersession | Runtime transaction | No change | Unwind synchronous work, destroy isolated effects, never publish. |
| Atomic entry-table publication unavailable | Runtime startup/preflight | No change | Disable development hot swap with a deterministic target diagnostic. |
| Executable-memory growth during a long dev session | Developer/runtime diagnostics | Existing patches remain valid | Report retained bytes; restart the process to reclaim code. |

There is no partial-publication recovery row because every fallible step precedes one entry-table
exchange.

## Target and platform matrix

| Target | Development selective JIT | Production AOT | Required evidence |
| --- | --- | --- | --- |
| Windows x86_64 | Required | Required PE/COFF | Native PR CI and pinned edit-shape benchmark. |
| Linux x86_64 | Required | Required ELF | Native CI selective patch execution. |
| macOS x86_64 | Required for permitted local processes | Required Mach-O | Native x86_64 runner and hardened-process exclusion. |
| macOS arm64 | Required for permitted local processes | Required Mach-O | Native arm64 runner; no translated JIT. |
| Android arm64 | Required in Workshop | Required published package | Named physical arm64 Workshop JIT plus AOT package evidence. |

JIT must match the running host target; mismatches fail with `JIT_TARGET_MUST_MATCH_HOST`. The
standard `Stasis_API_35` AVD is x86_64 and is useful smoke evidence but does not satisfy Android
arm64. iOS remains AOT-only.

Selective compilation and publication apply only to the running development JIT. Every production
publish performs a coherent full AOT compile/package; AOT lanes verify diagnostics and behavioral
parity for accepted source revisions, never selective patch reuse.

## Performance and memory budgets

Reports separate whole-file frontend time, invalidation planning, codegen/finalization, safe-point
wait, publication, first-new-window, changed count, re-JITed count, reused count, and retained code
bytes. Release benchmark binaries are built before timing. Use five unmeasured warmups, then 30
measured samples for 100/1,000-function and real-game narrow edits, 10 measured samples for 5,000
functions and broad shared-helper cases, and nearest-rank p95. Run parent and candidate commits on
the same hardware profile.

Initial pinned-runner gates are:

| Case | Compile-ready p95 | Required emission behavior |
| --- | ---: | --- |
| 100-function narrow closure | 25 ms | Exact closure only. |
| 1,000-function narrow closure | 100 ms | Exact closure only; changed-file frontend dominates. |
| 5,000-function narrow closure | 6,000 ms | Exact closure only; one-file stress case, not a frame budget. |
| Chess TD narrow body edits | 60 ms | Commonly fewer than ten functions; report actual graph result. |
| Broad shared-helper edit | Evidence-based cold-relative gate | Report the honest reverse closure; never hide it. |

Edit-to-visible is compile-ready time plus safe-point wait and at most two 60 Hz tick intervals on
the pinned desktop runner. Publication p95 is 0.25 ms on that runner and 1.0 ms on the named Android
arm64 target, excluding explicit migration/hook work. A gate miss requires evidence-backed revision;
it does not authorize internal trampolines or whole-reachable warm emission.

Executable-memory retirement has no priority-1 budget. Retained bytes and patch count are diagnostic
evidence only; process restart is the reclamation operation.

## Implementation sequence and bounded verification gates

### P0 - Correct contract (Maddox #184)

Replace the merged whole-generation/no-cross-patch requirements with this contract, worked graphs,
failure table, checker, and negative mutations.

### P1 - Exact planner (Maddox #185)

Build deterministic changed/SCC/reverse-caller plans with reason chains, retained dependencies, and
exact set tests across multi-file/signature/layout/reachability edits.

### P2 - Selective patch emission (Maddox #186)

Emit only planned bodies; bind patched-to-patched and patched-to-retained direct calls; preserve
unchanged addresses; remove warm complete-generation emission.

### P3 - Host-entry publication (Maddox #187)

Build in background, validate one patch, publish one immutable entry table between windows, preserve
old code/state on failure/supersession, and retain code until restart.

### P4 - Edit-shape/performance matrix (Maddox #188)

Enforce exact function sets, behavior, JIT/AOT parity, platform lanes, Chess TD/Brickout evidence,
and regression gates that detect accidental whole-reachable warm emission.

Implementation and portable verification are complete. Checked results, hardware qualifications,
reproduction commands, and the direct-call tradeoff are recorded in
[selective_jit_benchmarks.md](selective_jit_benchmarks.md). Named physical Android arm64 acceptance
remains a release-evidence gate and cannot be inferred from host-side Workshop tests.

Each child uses commands bounded to 900 seconds, includes a representative executable Stasis path,
performs a touched-code cruft review, and records Good/Bad/Adjustment plus Theory gained.

## Superseded architecture to remove

Current requirements and production paths must not depend on:

- rebuilding every reachable function for each warm edit;
- one executable-memory owner per accepted source revision;
- prohibiting direct calls from new code to retained unchanged bodies;
- automatic quiescent retirement or zero-retired-byte gates;
- internal `FnId -> code_ptr` hash/mutex lookup or ABI-by-arity wrappers;
- stable trampolines for ordinary internal Stasis functions; or
- fallback behavior that emits placeholder semantics when selective planning/emission fails.

Historical references may describe #173-#178 only when explicitly marked superseded and must not be
read as an active compatibility path.
