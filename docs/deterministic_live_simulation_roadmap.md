# Deterministic Live Simulation Roadmap

This is the canonical product roadmap for making a Stasis simulation inspectable,
replaceable, replayable, and safe to modify while it runs. It is a product contract
and dependency map, not a mutable task-status inventory.

## Product promise: questions the product must answer

A developer working on a live simulation should be able to answer these questions
with bounded, reviewable evidence:

- How much memory is the simulation using, and which declared state owns it?
- What was the complete simulation state at tick N?
- Which normalized inputs were applied at tick N, and in what order?
- Which code and data changes were accepted, rejected, or deferred at a safe point?
- Can the same initial state, code identity, and input record reproduce the result?
- If a change is rejected or a replay diverges, can the prior state be preserved and
  the cause be identified without guessing?

The promise is deterministic, statically bounded simulation with an inspectable
state model and explicit evidence. "Live" means changes can be prepared while the
simulation runs; publication still occurs at a defined tick safe point.

## Product boundaries and ownership

The simulation owns declared state, tick order, bounded collections, deterministic
input snapshots, state hashes, replay receipts, and safe-point commit decisions.
The host owns windowing, rendering, audio, filesystem, network, wall-clock services,
and platform lifecycle. Host effects are sampled or queued at the simulation boundary;
they do not silently become simulation state or simulation time.

The language and runtime remain statically bounded: capacities, layouts, and per-tick
work budgets are explicit and reject overflow or incompatible changes. Inspection and
recording must use the same state boundary as gameplay, excluding presentation buffers
and incidental host state. The hot-swap architecture remains specified by
[`live-compilation-prd.md`](live-compilation-prd.md); this document states the
cross-cutting product outcome it must support.

## Determinism profiles

The product names its reproducibility contract instead of treating all execution
environments as equivalent.

### Strict profile

Strict mode fixes the target profile, compiler/runtime build, language and schema
identity, initial snapshot, normalized input stream, tick rate, and host capability
set. Strict cross-target simulation state uses integer and Q16.16 deterministic
operations. Those operations must produce identical state hashes and receipts for
the same profile. Native float disqualifies cross-architecture strict claims; native
floating-point behavior is only promised within a declared target profile.

### Replay profile

Replay mode records the strict-profile identity, initial state snapshot, ordered input
events, accepted code/data identities, layout and capacity metadata, and per-tick
state hashes. Replay may include native float only under a recorded same-target/toolchain
profile. A verifier re-runs the record under that profile and reports the first
divergent tick, expected hash, observed hash, and relevant receipt. A replay is
evidence of reproducibility, not a claim of cross-architecture native-float equivalence.

### Local profile

Local is the weakest profile. It preserves deterministic tick ordering and bounded
state within one running development process while allowing explicitly marked host
conveniences such as live rendering, wall-clock diagnostics, or unavailable external
services. Local results are not advertised as replayable until the inputs, host
capabilities, code identity, and profile requirements of strict mode are captured.

## Capability gates and dependencies

Capabilities ship in the following dependency order. The issue IDs identify the
outcome slice that owns each capability; they do not describe current task status.

| Gate | Issue | Capability outcome | Depends on |
| --- | --- | --- | --- |
| G1 | #146 | Tick safe point and deterministic commit foundation | -- |
| G2 | #155 | Numeric, type, JIT, and AOT fidelity foundation | #146 |
| G3 | #147 | Bounded collections with explicit capacity and overflow behavior | #146, #155 |
| G4 | #148 | State hashing, normalized input, and replay records | #146, #147, #155 |
| G5 | #156 | Bounded headless scenarios and reproducible failures | #146, #148 |
| G6 | #149 | Memory accounting and typed live inspection | #146, #147, #156 |
| G7 | #150 | Rewind, branch, and first-divergence reporting | #148, #149, #156 |
| G8 | #151 | Data-flow and state ownership inspection | #149 |
| G9 | #152 | Validated semantic edits with safe rollback | #146, #151 |
| G10 | #153 | Layout-aware hot reload and state migration | #147, #149, #152 |
| G11 | #154 | Bounded cost reports and layout evidence | #147, #149, #153 |
| G12 | #157 | Mobile snapshot and lifecycle parity | #149, #156 |
| G13 | #158 | Workshop inspection and edit workbench | #152, #153, #156 |
| G14 | #159 | Inspectable deterministic showcase experience | #148, #156, #158 |
| G15 | #160 | Deterministic simulation CLI | #148, #156 |
| G16 | #161 | Live CLI workspace with safe edits and inspection | #153, #160 |
| G17 | #273 | Deterministic headless video recording for dev builds | #148, #156 |
| G18 | #275 | Captures deterministic game audio in headless recordings | #273 |
| G19 | #276 | Adds deterministic pre-tick recording hooks and MP3 export | #273, #275 |

The order is a gate order, not a promise that every gate is implemented in one
release. A later surface may consume an earlier capability only through its published
contract and evidence.

## Issue-to-outcome traceability

The capability table is the complete roadmap traceability map: each child issue maps
to one observable product outcome and its prerequisite contracts. Implementation
details, mutable status, and temporary sequencing stay in
[`build_checklist.md`](build_checklist.md), while normative language rules stay in
[`spec.md`](spec.md).

The map deliberately groups the recording extensions (#273, #275, and #276) after
the base input/replay gate. This makes recording an extension of deterministic
evidence rather than a second simulation model.

## Completion gates and evidence

The roadmap is complete when the following evidence exists for every applicable gate:

1. A deterministic automated test proves the capability's success and bounded failure
   paths, with explicit seeds, ticks, capacities, or profile identifiers.
2. A representative JIT/live path and the corresponding AOT or headless path agree on
   state, layout, inputs, and rejection behavior where the capability spans both.
3. A replay or inspection artifact identifies its code identity, schema/layout identity,
   initial snapshot, input record, tick, and state hash; artifacts are bounded and
   reproducible from repository inputs.
4. A negative test proves safe preservation: rejected edits, capacity overflow, invalid
   records, host-boundary violations, and first divergence do not silently mutate the
   previously accepted simulation state.
5. User-facing documentation answers the six product questions above and names the
   applicable determinism profile, including the limits of native floating point.
6. CI runs the documentation contract checker and focused tests, while implementation
   slices retain their own bounded validation in the build checklist.

Evidence may be JSON, text, snapshots, hashes, or test output. A successful command
alone is not evidence when the artifact is not inspected or its identity is missing.

## Exclusions and non-promises

This roadmap does not promise cross-architecture native-float equivalence, hidden
unbounded allocation, unbounded collection growth, wall-clock gameplay semantics,
automatic repair of arbitrary semantic edits, or a full debugger. It does not define
the compiler grammar, replace the hot-compilation PRD, or carry a live inventory of
task status. Rendering pixels may vary by backend even when simulation state and
inputs match; visual parity is a separate evidence question.

## Document ownership

- [`spec.md`](spec.md) is normative language semantics and invariants.
- [`live-compilation-prd.md`](live-compilation-prd.md) owns hot-swap architecture,
  lifecycle, and product requirements for code publication.
- [`build_checklist.md`](build_checklist.md) owns implementation slices, status, and
  temporary sequencing.
- Focused subsystem documents own detailed contracts and evidence for their subsystem.
- This roadmap owns the cross-cutting product promise, boundaries, determinism
  profiles, capability dependencies, traceability, and completion gates.
