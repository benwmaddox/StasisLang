# StasisLang architecture complexity and simplification

## Purpose

This document records where StasisLang implementation complexity currently
lives and a conservative sequence for reducing maintenance cost. It is an
architecture baseline and planning aid, not a claim that a particular line
count is good or bad. Lines of code (LOC) are directional: they help identify
large ownership surfaces, but they do not measure quality, difficulty, risk, or
the value of a component.

The central conclusion is that the compiler is the largest single reusable
subsystem, but the surrounding development hosts and runtime integration add
at least as much coordination complexity. The LSP and VS Code extension are
small by LOC and large by coupling. Simplification should therefore focus on
contracts, transaction boundaries, and ownership before attempting broad
rewrites.

## Snapshot and method

The inventory is a snapshot of commit `865168ac` on the analyzed branch. It was
counted from tracked, owned production source and rounded to the nearest useful
precision. Embedded Rust tests were separated at `mod tests` where practical.
The inventory excludes generated output, vendored dependencies, build output,
samples, documentation, and tests. Some buckets represent shared code and some
represent product or platform code; the boundary is stated below so that the
numbers are not read as a claim that every repository line belongs to one
single executable.

The counts are approximate physical LOC. They are useful for asking which
surfaces deserve a smaller ownership boundary, but they are not a quality
metric. A small ABI adapter can be more consequential than a large UI file.

## Where the production code is

The following buckets are a directional inventory of the core implementation.
They use primary ownership boundaries: shared code is counted once in its
shared bucket, and host rows do not repeat those shared files. The rows are
therefore useful as a comparative map, not as an exact accounting of every
repository file or a measure of product value.

| Primary area | Approx. production LOC | Share of core inventory | Boundary and interpretation |
| --- | ---: | ---: | --- |
| Compiler | 38.4k | 25% | Frontend, semantic analysis, native lowering, Wasm lowering, and compiler-side JIT support |
| Android Workshop host | 28.4k | 19% | Java, JNI C, Rust bridge, and Codex-native workshop integration |
| CLI, TUI, DAP, Gauntlet, and related tooling | 21.8k | 14% | Development workflows around the desktop runner |
| Shared native graphics and runtime | 17.6k | 12% | Shared C/native runtime and graphics implementation used by hosts |
| Desktop live host core | 14.1k | 9% | Live workspace, host loop, input, rendering/session integration, and desktop orchestration |
| JIT, publication, and runtime substrate | 10.0k | 7% | Shared dispatch, publication, guest state, snapshots, and runtime ABI support outside the compiler bucket |
| Language service, LSP, and VS Code | 7.7k | 5% | Editor-neutral language service, protocol server, and extension presentation |
| Asset, network, and AI services | 5.2k | 3% | Shared service and integration surfaces |
| Stasis standard library | 3.8k | 2.5% | Authored language-level library modules |
| Web browser host | 2.45k | 1.6% | Browser JavaScript and HTML host code; it also consumes the separate Wasm compiler backend |
| Published Android/iOS shells | 2.2k | 1.5% | Thin release adapters, common mobile entry code, and platform packaging glue |

The rounded percentages sum to approximately the whole inventory. The rows
should not be added to other repository measurements without accounting for
shared runtime files, generated bindings, test code, and excluded content.

## Compiler complexity

The compiler is approximately 38.4k production LOC, with about 15k lines of
inline tests. Its production split is approximately:

| Compiler portion | Approx. production LOC | What makes it costly |
| --- | ---: | --- |
| Frontend, parser, and editor operations | 10.8k | Syntax, source spans, editing operations, and compiler-owned Workshop queries |
| Semantics and orchestration/data flow | 5.7k | Type/effect analysis, reachability, SCC propagation, and diagnostics |
| Backend and lowering | 21.8k | Native Cranelift lowering, Wasm encoding/lowering, ABI details, and incremental/JIT emission |

Backend work is roughly 57% of compiler production code. Parsing is not the
dominant cost; lowering, target parity, incremental state, and publication
contracts are.

Important hotspots include:

- `crates/stasis_compiler/src/backend/emit.rs` is about 9.1k production lines.
  It is the central Cranelift lowering engine and has the largest individual
  blast radius.
- `crates/stasis_compiler/src/frontend/workshop.rs` is about 4.5k lines. It
  contains compiler-owned semantic editing and Android/editor operations, not
  only ordinary parsing.
- `crates/stasis_compiler/src/backend/wasm.rs` is about 3.8k lines. It is a
  separate hand-written Wasm encoder, so semantic parity with native lowering
  is an ongoing risk.
- `crates/stasis_compiler/src/backend/jit.rs` is about 3.4k production lines
  and about 5.1k test lines. It manages transactional incremental compilation,
  staged activation, rollback, and retained code.
- `crates/stasis_compiler/src/data_flow.rs` is about 3.1k lines. It handles
  type/effect analysis, SCC propagation, and bounded-loop reasoning.

The compiler's complexity is concentrated in backend behavior and stateful
boundaries, rather than in the basic parser. Any compiler refactor should keep
the reachability-first and one-pass lowering constraints in `AGENTS.md` and
should preserve native, AOT, JIT, and Wasm behavior with explicit fixtures.

## Host and runtime complexity

### Desktop development host

The desktop live host core is approximately 14.1k lines. It is not merely a
window and event loop: it owns a live workspace, compiler backend orchestration,
input/render integration, publication, and session state. The desktop
development product also includes approximately 21.8k lines of CLI, TUI, DAP,
packaging, recording, and Gauntlet tooling. Those surfaces are counted in the
tooling row rather than again in the desktop core row.

Representative desktop hotspots are:

- `apps/stasis/src/compiler_backend.rs` is about 3.95k production lines for
  compilation-mode selection, JIT/AOT staging, rollback, linking, and
  publication.
- `apps/stasis/src/live_workspace.rs` is about 3.3k production lines for the
  desktop live session.

The desktop executable is therefore the development toolchain, not only a
runtime host. Its main maintainability risk is orchestration spread across
watching, compilation, publication, rendering, CLI, and diagnostics.

### Android Workshop

Android Workshop is approximately 28.4k platform-specific production lines
across Java, JNI C, Rust bridge code, and Codex-native integration. It is the
largest individual host implementation. The workshop is a full editing,
compilation, live-preview, AI, asset-management, and source-control environment
rather than a thin release shell.

`mobile/android/app/src/workshop/java/com/stasislang/workshop/MainActivity.java`
alone is roughly 12k lines. The activity should eventually become lifecycle
wiring and view ownership, with focused components for the project editor,
preview session, assets/audio, AI, GitHub synchronization, persistence/recovery,
and acceptance diagnostics.

### Published mobile shells

The published Android shell is about 1.7k platform-specific lines, plus about
460 lines of common mobile entry code and the shared native runtime. The
published iOS shell has about 70 lines of native adapter code, plus project
metadata, common mobile entry code, and the same shared runtime. Together the
published shells are approximately 2.2k lines in the inventory.

This contrast matters: a conventional packaged host can be relatively cheap
when it reuses the native AOT runtime. A full Workshop product is a separate
product-sized host and should not be treated as an incremental shell.

### Web browser host

The Web host is approximately 2.45k JavaScript/HTML lines. It uses browser-native
Canvas2D, WebAudio, storage, and networking behavior. It also depends on the
compiler's separate approximately 3.8k-line Wasm backend, so the host's small
surface does not remove the target-parity obligation.

### Shared runtime and graphics

Shared host machinery is substantial and has high fan-out:

- `runtime/stasis_graphics.c` is roughly 8k lines.
- `crates/stasis_dynload/src/lib.rs` is roughly 7.1k production lines. Despite
  its name, it covers dynamic loading, JIT trampolines, guest state, snapshots,
  rendering, audio, storage, and network ABI glue.
- The remainder of the shared native graphics/runtime bucket includes related
  runtime and bridge code. The JIT/publication/runtime substrate bucket tracks
  the separately owned publication and dispatch surface; the files are not to
  be counted twice when doing a detailed inventory.

The risk here is not just file size. A change to a shared layout, symbol, or
resource lifecycle can affect desktop, Workshop, release mobile, and Web
behavior through different adapters.

## Language service, LSP, and editor tooling

The complete language-tooling surface is approximately 7.7k production lines:

| Tooling piece | Approx. production LOC | Responsibility |
| --- | ---: | --- |
| Language service | 3.15k | Workspace revisions, caches, compiler-backed queries, and editor-neutral results |
| LSP protocol server | 2.45k | Transport, text synchronization, feature handlers, and protocol conversion |
| VS Code extension/configuration | 2.1k | Activation, workspace client, live UI, DAP integration, and presentation |

This is only about 5% of the core inventory. It is nevertheless dense in
coupling:

- Revision caches must provide useful last-good results while a source edit is
  temporarily broken.
- Diagnostics compile workspace snapshots and must agree with the compiler's
  inclusion and exclusion rules.
- Live values travel through a long custom path:
  runtime -> live workspace/TUI -> LSP JSON broker -> VS Code TypeScript.
- Completion types and ranking are split among runtime live helpers, compiler
  semantic facts, language-service aggregation, and VS Code presentation.
- `crates/stasis_language_service/src/lib.rs` and
  `crates/stasis_lsp/src/lib.rs` are each concentrated in a large file.

The LSP is not primarily a quantity-of-code problem. Its maintainability risk
is synchronization across compiler, live runtime, JSON protocol, and
TypeScript representations.

## Ownership map: what should be shared

The useful consolidation boundary is an explicit contract, not a universal
host implementation.

| Invariant or concern | Canonical owner | Share across | Keep platform-specific |
| --- | --- | --- | --- |
| Symbols, scopes, types, references, spans | Compiler semantic layer | Language service and all editor clients | UI presentation and protocol range conversion |
| Workspace revisions, last-good recovery, query caches | Language service | LSP and future editor clients | Transport and editor widgets |
| HostFrame, render commands, resource lifecycle, guest entrypoints | Versioned host contract/runtime ABI | Rust, C, Java, JavaScript adapters | Windowing, renderer, event loop, and lifecycle APIs |
| Candidate validation and safe publication | Narrow runner/session transaction | Desktop live host and Android Workshop | Watchers, JNI, activity lifecycle, and AOT packaging |
| Diagnostics envelope and stable error identity | Shared typed protocol fixtures | Compiler, language service, LSP, and VS Code | Presentation and retry UX |
| Asset-package manifest | Shared manifest/validation contract | Desktop, Workshop, mobile, and Web tooling | Filesystem, Android, and browser storage adapters |

Desktop, Android Workshop, release mobile, and Web should keep their rendering,
event-loop, lifecycle, and UI implementations separate. They should share
layouts, manifests, command fixtures, validation policy, and safe publication
semantics where those invariants are genuinely common.

## Complexity ranking

The current highest-complexity surfaces, considering both size and coupling,
are:

1. Native lowering in `emit.rs`, together with maintaining native/Wasm parity.
2. Desktop development-toolchain orchestration across the live workspace,
   compiler backend, CLI, packaging, and Gauntlet.
3. Android Workshop, especially its monolithic activity and custom
   compiler/JIT/JNI session boundary.
4. JIT runtime state and ABI machinery in `stasis_dynload` and compiler JIT
   support.
5. Semantic and effect analysis.
6. Language-service/LSP live integration.
7. Published mobile and browser presentation shells themselves.

This ranking explains why reducing parser LOC alone would not address the main
maintenance burden.

## Conservative simplification sequence

The sequence below intentionally begins with evidence and file boundaries. It
does not require a language rewrite, a cross-platform UI rewrite, or a new
universal host abstraction.

### P1 slice 1: establish behavioral gates before restructuring

Create a fast, cross-target characterization suite before moving major code.
Capture parser, semantic, diagnostic, reachability, patch-plan, JIT/AOT/Wasm,
and host/runtime behavior. Include:

- Failed hot swaps preserving old code and state.
- HostFrame and render-command binary layouts.
- Asset, audio, storage, and network contract behavior.
- LSP and VS Code live-protocol fixtures.
- ABI symbols, layout hashes, representative CLIF/Wasm output, and package
  manifests.

The suite is the safety net for every later slice. A refactor may change
implementation boundaries, but it must not silently change an ABI, rollback
result, target output, or editor contract.

Acceptance gates:

- Fixtures run deterministically within the normal bounded test budget.
- Native, AOT, JIT, Wasm, desktop, Workshop, and editor-facing contracts have
  at least one representative case where applicable.
- A failed publication test proves that the previous code/data remain active.
- Contract snapshots are reviewable and changes are intentional.

### P1 slice 2: mechanically split the largest files

Change file boundaries before changing behavior. Keep APIs, symbols, output,
and ownership unchanged while extracting coherent modules.

Compiler extraction targets:

- Extract analysis collection from `backend/emit.rs`.
- Extract native ABI declarations and storage/addressing helpers.
- Keep mutually recursive statement, expression, and condition lowering
  together.
- Split `data_flow.rs` into semantic validation, direct effects, SCC
  aggregation, and effect-contract diagnostics.
- Extract binary-format mechanics from `backend/wasm.rs` while keeping Wasm
  semantic lowering together.

Runtime and tooling extraction targets:

- Split `stasis_dynload` internally into dynamic loading, JIT dispatch, guest
  memory, snapshots, rendering, audio, storage, and network modules. Preserve
  one crate and the exported ABI initially.
- Split `runtime/stasis_graphics.c` into command validation, resources,
  text/fonts, display transforms, and renderer implementation while retaining
  one build target.
- Split language service into documents, indexes, completion, navigation,
  editing, formatting, and live enrichment.
- Split LSP into transport/server, synchronization, feature handlers,
  conversion, and live-process handling.
- Split VS Code activation, workspace clients, testing, live UI, and DAP out
  of the extension monolith.

Acceptance gates:

- The diff is movement and visibility changes, with no intentional behavior
  change.
- Characterization fixtures from slice 1 remain unchanged.
- Each extracted module has one clear owner and no new cyclic dependency.
- Exported ABI symbols and package outputs remain stable.

### P1 slice 3: establish one canonical host contract

Create a small, versioned contract definition for data shared across hosts:

- HostFrame fields and offsets.
- Render-command and resource-lifecycle layouts.
- Guest lifecycle entrypoints.
- Diagnostic envelopes and stable error codes.
- Compile-candidate and activation results.
- Asset-package manifest shape.

The canonical registry lives at `contracts/v1/host_runtime.json`. Host
implementations remain handwritten; `tools/ci/check_host_runtime_contract.py`
checks their Stasis, C, Rust, Java, and JavaScript copies against the registry.
Platform-only lifecycle extensions are recorded explicitly instead of being
forced into hosts that do not implement them. Runner diagnostic versioning and
asset-package identity publication land as separate checked checkpoints, so
registry edits cannot silently change runtime behavior.

Generate or validate constants and DTOs for Rust, C, Java, and JavaScript; do
not generate implementations. Keep byte-for-byte contract fixtures consumed by
each language. This removes layout and schema drift without pretending that
windowing, rendering, or lifecycle code is portable.

Acceptance gates:

- Rust, C, Java, and JavaScript agree on fixture bytes, offsets, tags, and
  version handling.
- Unknown or incompatible versions fail deterministically.
- Contract tests cover both valid and malformed messages.
- Existing desktop, Workshop, release mobile, and Web behavior remains
  unchanged.

### P1 slice 4: consolidate the development hot-swap transaction

Desktop and Android Workshop duplicate correctness-critical lifecycle and
rollback orchestration. Share the narrow transition below in a runner/session
module:

```text
source revision
-> compile candidate
-> validate signature/layout
-> stage state and resources
-> snapshot accepted runtime
-> invoke on_code_swap
-> atomically activate or restore snapshot
```

The shared transaction owns validation, snapshot/restore, activation, and
all-or-nothing failure behavior. It does not own file watchers, background
compilation scheduling, AOT linking/packaging, JNI, Android lifecycle, window
loops, render replay, or diagnostic presentation.

`apps/stasis/src/compiler_backend.rs` should continue to construct compiler
artifacts. The Android bridge should continue to marshal JNI and frame data.
Both should call the same validation/activation transaction and consume the
same fixtures. If their lifecycle differences are too large for one
controller, share pure validation and transition functions rather than forcing
an artificial common host.

Acceptance gates:

- Candidate compilation is distinct from publication.
- Signature/layout incompatibility rejects without partial commit.
- `on_code_swap` failure restores the prior accepted runtime and state.
- Desktop and Workshop produce equivalent receipts for equivalent candidates.
- Stop/shutdown and child-exit paths release or reject pending candidates.

Delivered boundary: `crates/stasis_compiler/src/backend/development_swap.rs`
now owns the JIT-specific synchronous transition, not a universal host
controller. A fully compiled candidate enters with a sorted/deduplicated
change descriptor. The module plans and finalizes state migration, stages a
narrow host publication participant, activates through the bounded runtime
snapshot/restore path, publishes host resources, runs the optional hook, and
accepts the candidate only after every step succeeds. Every failure carries
the same versioned accepted/rejected receipt shape.

Desktop supplies only a JIT host-entry participant; its prior entry table is
restored if publication or the hook rejects. Android Workshop supplies only an
embedded-resource participant; its prior catalog is restored with the old JIT
runtime and state. Candidate compilation, watch scheduling, JNI, lifecycle,
windowing, rendering, and diagnostics remain in their existing owners. The
runner pipeline also exposes explicit shutdown that joins its compiler worker,
drains queued values, and emits deterministic failed commit results for
unaccepted request IDs; `Drop` uses the same cleanup path.

Focused evidence covers accepted publication, incompatible layout, staging
failure, partial publication failure, hook mutation plus rejection, desktop
between-tick acceptance/rejection, Workshop hook rollback, and pending pipeline
shutdown. The architecture gate continues to exercise the existing rollback
fixture, so the new owner replaces duplicated transition code without adding a
parallel behavior path.

These four P1 slices are the highest-value implementation units and should be
tracked independently even when delivered under one architecture program.

### Follow-on slice 5: decompose the product monoliths

After the first four slices stabilize contracts, split orchestration without
creating needless binaries or crates.

For the desktop toolchain, separate play/window/input from compilation
orchestration, packaging, CLI commands, TUI, DAP, tests, recording, and
Gauntlet. Keep one executable initially; let dependency boundaries demonstrate
when a new crate or binary is justified.

For Android Workshop, extract focused Java services/controllers for activity
lifecycle/navigation, project/source editing, native preview, assets/audio, AI,
GitHub synchronization, persistence/recovery, and acceptance diagnostics. The
activity should own lifecycle and views, not every product concern. Split the
Rust bridge by compiler session, frame marshaling, state access, resources, and
C ABI exports.

Acceptance should show that each controller can be tested at its boundary and
that lifecycle failures do not corrupt the shared live session.

### Follow-on slice 6: clarify editor-tooling ownership

Use this dependency direction:

```text
compiler semantic facts
-> language-service revision/query facade
-> LSP conversion and transport
-> VS Code presentation
```

Compiler owns symbols, references, scopes, inferred types, and source spans.
The language service owns workspace revisions, caching, last-good recovery, and
editor-neutral query results. LSP owns URI/range/protocol translation only.
VS Code owns presentation. Completion ranking should move out of
`stasis_runner::live` and into editor-neutral language queries.

Replace any secondary hand-built definition scanner only after compiler-level
parity fixtures cover its existing behavior. Define one shared workspace
inclusion/exclusion fixture, including the existing `.stasis-cache` versus
`.stasis_cache` naming discrepancy.

### Follow-on slice 7: version the live protocol

The live path currently crosses runtime DTOs, TUI JSON, LSP JSON, and
TypeScript interfaces. Define one typed, versioned wire contract with golden
fixtures for start/stop/status, inspection and watches, collections, runtime
identity, pending/final responses, errors, and process termination.

Use the fixtures in Rust and TypeScript. Migrate one command family at a time
and delete the old decoder after each migration. Share decoding and stable
error identity, but retain separate projections: LSP needs compact observations
for hover/completion, while VS Code may need richer watch and UI state.

### Follow-on slice 8: simplify tooling concurrency

Once protocol and ownership boundaries are clear:

- Use one bounded, latest-revision-wins diagnostics worker instead of compiling
  synchronously on every edit.
- Replace one blocking thread per live request with a coordinator or bounded
  worker pool.
- Prioritize stop/shutdown and promptly fail pending requests after child exit.
- Centralize revision handling while keeping feature indexes lazy.

Acceptance should cover bounded thread count, rapid-edit coalescing, no stale
diagnostics, deterministic shutdown, and no leaked pending requests.

### Follow-on slice 9: keep release hosts deliberately thin

Enforce dependency and size budgets for published shells. Android should retain
its Activity, verified extraction cache, and small adapters. iOS should retain
its bundle entry and platform permission/network UI. Both should reuse the
native AOT runtime and should not gain compiler, JIT, watcher, editor, or
dynamic-loader dependencies. Web should retain browser-native Canvas2D,
WebAudio, storage, and networking behavior.

Share commands, layouts, fixtures, and policies; keep renderers and event loops
platform-specific.

## Consolidations to avoid

The following changes would increase coupling or migration risk without
addressing the measured complexity:

- A universal `Host` trait spanning desktop, Workshop, release mobile, and Web.
- A generic backend abstraction that forces Cranelift and Wasm lowering into
  one implementation before parity behavior is characterized.
- Rewriting the hardcoded parser with a parser generator.
- Replacing Android Java UI with a cross-platform UI framework as part of this
  maintainability effort.
- Forcing Java, Canvas2D, and native C renderers or event loops to share an
  implementation.
- Moving compiler-specific packaging into `stasis_runner`.
- Splitting `stasis_dynload` into multiple crates while also changing its ABI.
- Creating many tiny modules solely to reduce file length.
- Keeping old and new protocol or publication paths indefinitely during a
  migration.

## Acceptance gates for the overall program

The architecture work is successful only if all of the following remain true:

- Native, JIT, AOT, Wasm, desktop, Workshop, mobile, and Web characterization
  behavior remains covered at the relevant boundary.
- Hot-swap changes are all-or-nothing and preserve prior state on rejection.
- ABI layouts, symbols, manifests, and protocol versions are explicit and
  fixture-tested.
- Editor results remain deterministic across transient broken edits and use
  one workspace inclusion policy.
- Module ownership and dependency direction are visible in the source tree.
- Release shells remain free of development-only compiler and watcher paths.
- Tests and end-to-end checks stay within the repository's bounded execution
  budgets, with lingering processes cleaned up after checks.

## Success metrics

Measure the refactor by engineering outcomes rather than total LOC:

- Fewer owners per invariant and fewer duplicated rollback/layout/schema paths.
- Smaller review and test blast radius for a host, compiler, or protocol change.
- Unchanged cross-target behavior under characterization fixtures.
- Shorter and more inspectable hot-swap transaction code.
- Deterministic failure and shutdown behavior.
- Faster diagnosis of whether a regression belongs to compiler semantics,
  lowering, runtime ABI, host orchestration, or editor presentation.
- Deletion of obsolete paths after each migration instead of permanent dual
  implementations.

The first four P1 slices should be completed and reviewed as a group, then the
dependency graph and defect patterns should be reassessed before committing to
the follow-on slices. This keeps the work evidence-driven and avoids turning a
complexity inventory into a large speculative rewrite.
