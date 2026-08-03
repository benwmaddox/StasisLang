# Stasis LSP and Live Workshop Plan

## Outcome

Stasis will provide a standard Language Server Protocol (LSP) endpoint for any editor, a thin
VS Code LSP client packaged in the VSIX, and a Live Workshop experience that enriches indexed
language facts with compatible facts from a running game. The terminal UI (TUI) will call the
same Rust language-service operations directly instead of maintaining editor-specific analysis.

The target experience is continuous diagnostics, compiler-aware navigation and refactoring, and
low-latency type and live-value inspection without starting a compiler process for every request.

## Baseline before implementation

The useful substrate already exists, but it is split across hosts:

- `vscode-stasis/src/extension.ts` registers VS Code formatting and completion providers directly.
  Offline completion starts `stasis --json symbol list` for each request; live completion uses the
  graphical runner's `--live-stdio` JSON channel.
- `vscode-stasis/src/liveSession.ts` owns the running process and the Live Values tree. This is a
  Stasis-specific protocol client, not an LSP client.
- `crates/stasis_runner/src/live.rs` owns completion ranking and the live request/response schema.
- `apps/stasis/src/live_workspace.rs` builds compiler-owned symbol, scope, type, reference, and
  completion data for a running workspace.
- `apps/stasis/src/toolchain_cli/live_tui.rs` consumes parts of those indexes in process, but its
  language behavior is coupled to the live graphical host.
- compiler diagnostics can be formatted for builds and live swaps, but there is no persistent
  document-overlay service publishing structured ranges as a user types.

The existing live JSON protocol remains valuable for deterministic automation. LSP does not
replace it; both protocols will adapt the same service and runtime operations.

## Architectural boundaries

### 1. Compiler-owned language service

Add a `stasis_language_service` Rust crate. It owns long-lived workspace state and exposes typed,
transport-independent operations:

- open, change, save, and close document overlays;
- diagnostics;
- completion and completion resolve;
- hover and signature help;
- definition, references, and rename preview;
- document and workspace symbols;
- code actions, semantic tokens, inlay hints, and hierarchy queries;
- formatting operations delegated to the canonical formatter.

The service consumes compiler parser, semantic, symbol, type, scope, call, and reference indexes.
Those indexes must move behind compiler-owned APIs where they are currently assembled in an app
host. The service must not introduce a second parser, token-offset detector, or fake semantic
fallback.

Each response is computed from an immutable `WorkspaceSnapshot` identified by a monotonically
increasing revision. Open-document text shadows disk text. A successful analysis atomically
publishes a new snapshot; readers never observe a half-updated index. File invalidation is the
correctness boundary and existing per-function semantic hashes may gate backend work, but they do
not permit partial semantic analysis of a changed file.

The TUI, CLI symbol commands, Android Workshop semantic edits, and LSP server should progressively
consume these operations directly. Host-specific rendering, key handling, JSON/LSP conversion,
and runtime control stay outside the language-service crate.

### 2. Standard LSP server

Add `stasis lsp --stdio` as a long-lived JSON-RPC/LSP 3.17 server. It translates protocol objects
to language-service calls and advertises only implemented capabilities. Initial synchronization
uses incremental text changes with UTF-16 LSP positions converted once at the transport boundary;
compiler and live-service offsets remain UTF-8 bytes.

Standard methods provide the reusable editor surface. Stasis-specific `stasis/*` requests and
notifications provide Live Workshop control and observations without weakening compatibility for
editors that only implement standard LSP:

- `stasis/live/start`, `stop`, `pause`, `resume`, and `step`;
- `stasis/live/inspect`, `watch`, and `unwatch`;
- `stasis/live/status` and change notifications;
- tested semantic edit preview/apply operations already supported by the live workspace.

The LSP process owns the language service. It also owns a `LiveSessionBroker` that can launch or
attach to the existing live runner protocol. The broker translates runtime events into bounded,
revision-tagged observations; the compiler index remains the authority for symbol identity and
static types.

### 3. Live/indexed result composition

Every accepted runtime build publishes an identity containing the project root, source input
hashes, semantic symbol identity version, and live generation. A live value may enrich hover or an
inlay hint only when the queried symbol/path resolves in the current language snapshot and the
runtime's accepted hash for the owning file matches that snapshot. Otherwise the result is marked
stale or omitted; it never replaces indexed type or definition information.

Static and live work have separate budgets. Static hover, completion, navigation, and diagnostics
must complete even if the runner is stopped, busy, or backpressured. Live enrichment is optional,
cancellable, tick-tagged, and served from the latest bounded cache rather than synchronously
waiting on a game tick.

Lexical locals and parameters receive static information only until a future debugger exposes a
real paused stack. The service must not fake lexical runtime values by instrumenting later ticks.

### 4. VS Code client and Live Workshop

Replace direct VS Code language providers with `vscode-languageclient`. Extension activation starts
one `stasis lsp --stdio` process per Stasis workspace folder and lets VS Code negotiate standard
capabilities. Multi-root workspaces keep isolated project snapshots and live sessions.

The VSIX retains the Stasis activity view, status item, and play controls as a thin client of the
custom `stasis/*` methods. The first Live Workshop surface includes:

- start/stop/pause/resume/step;
- current tick, build revision, and stale/fresh state;
- watches and ad hoc inspection;
- compiler diagnostics linked to source;
- hover that combines signature/type/docs with a compatible cached live value;
- tested edit preview/apply status and rollback errors.

No VS Code provider may independently parse Stasis or spawn a per-request `stasis` command after
its corresponding LSP capability is available. TextMate grammar remains a lexical fallback until
semantic tokens ship.

### 5. TUI reuse

Refactor the TUI to hold a `LanguageService` handle and a live-observation handle. Its completion
pane, selected-item details, definition/reference commands, diagnostics, and rename previews call
the same operations as LSP. The TUI does not route those calls through JSON-RPC; it uses the Rust
API in process and retains its deterministic input, rendering, and live command queues.

## Latency and scheduling contract

These are product constraints, not aspirational benchmarks:

- keep the server and compiler indexes warm for the workspace lifetime;
- publish an initial disk-backed index before background enrichment;
- apply document changes in memory and debounce only expensive diagnostics, not keystroke-local
  completion or signature context;
- coalesce superseded diagnostics and completion work by document revision;
- honor LSP cancellation and discard results whose snapshot revision is no longer current;
- prioritize visible-document completion, hover, signature help, and diagnostics over workspace
  symbol and hierarchy work;
- never run JIT/AOT code generation for ordinary language-service queries;
- keep runtime inspection off the tick thread and serve UI reads from a bounded latest-value cache;
- persist an optional content-addressed disk index only after the in-memory path is correct; validate
  compiler version, manifest, file hashes, and target configuration before reuse.

Initial performance gates on the representative sample workspace:

| Operation | Warm p95 target | Notes |
| --- | ---: | --- |
| completion | 20 ms | local process, indexed, no runtime wait |
| hover/signature help | 30 ms | static result; cached live enrichment may be included |
| document symbols | 30 ms | current immutable snapshot |
| changed-file diagnostics | 150 ms | coalesced after edit burst |
| rename preview | 250 ms | workspace validation, no writes |
| live cache read | 5 ms | never waits for a tick |

Benchmark fixtures and machines must report actual distributions; a target is not a reason to
skip semantic correctness or return incomplete results.

## Delivery slices

Each slice is independently testable and removes the host-specific path it replaces.

### Slice 1: persistent diagnostics vertical

- [x] Add `stasis_language_service` with versioned document overlays and structured diagnostics.
- [x] Add `stasis lsp --stdio` with initialize/shutdown, incremental synchronization, cancellation,
   and diagnostic publication.
- [x] Convert the VSIX to `vscode-languageclient` and display diagnostics through the standard client.
- [x] Add protocol tests, overlay revision tests, UTF-8/UTF-16 range tests, malformed-file tests, and
   a VSIX end-to-end test that observes a Problems entry and its removal after a fix.
- [x] Run a representative `.stasis` program through compiler-to-Cranelift executable verification so
   the diagnostic path is proven to share the real compiler pipeline.

### Slice 2: shared completion, hover, and signature help

- [x] Move accepted completion, dirty-overlay scope inference, and ranking behind the language
  service's immutable completion snapshot operation.
- [x] Add documentation, signatures, inferred/declared types, expected-type ranking, snippets, and
  auto-import edits.
- [x] Add standard LSP completion, hover, and signature help from the same typed snapshot.
- [x] Switch the TUI completion/details query to the shared snapshot operation and make the LSP the
  default VSIX completion provider. Keep the newly landed live-session provider temporarily for
  runtime-only indexed collection fields until Slice 5 composes live data into LSP responses.
- [x] Measure warm local p95 latency and cover the standard operations in the packaged VSIX.

### Slice 3: navigation and symbols

- [x] Publish definition and reference locations from compiler-owned reference records, including
  cursor-sensitive global receivers, indexed field paths, and exact global declaration spans.
- [x] Publish document and workspace symbols from canonical compiler symbol records for Outline,
  breadcrumbs, and workspace symbol search.
- [x] Expose standard LSP definition, references, document-symbol, and workspace-symbol requests.
- [x] Remove the VSIX's per-request CLI Definition/References providers and switch the TUI
  references command to the shared language-service navigation snapshot.
- [x] Cover functions, indexed fields, global receivers, document symbols, and workspace symbols in
  the packaged VSIX.

### Slice 4: compiler-validated rename

- [x] Implement prepare-rename and rename preview for locals, parameters, fields, globals,
  functions, structs, and other supported types from compiler-owned identity and scope records.
- [x] Produce versioned workspace edits, reject collisions or stale documents, and compile the
  complete proposed overlay before returning edits.
- [x] Expose standard LSP prepare-rename/rename and the same non-mutating validated preview in the
  live TUI protocol.
- [x] Preserve the last known good semantic index for read-only completion, hover, signature, and
  navigation while the current dirty source is malformed. Current-source diagnostics continue,
  but edit-producing refactors pause until the current revision indexes safely.
- [x] Cover every supported rename identity, collisions, executable behavior, standard VS Code
  rename, TUI no-write preview, and incomplete `a(state.` receiver completion.

### Slice 5: Live Workshop composition

- [x] Publish an accepted runtime identity on live responses: session, generation, source hashes,
  and indexed collection layout.
- [x] Add a bounded `LiveSessionBroker` cache and custom LSP observation notification. Compose live
  hover values only when owning-file hashes and static types match the current semantic snapshot.
- [x] Route runtime-only indexed collection completion through the standard LSP and remove the
  VSIX's final direct completion provider/CLI fallback.
- [x] Move LSP-launched session lifecycle, pause/resume/step, watches, inspection, event streaming,
  and runtime cache ownership behind bounded asynchronous custom LSP methods. Delete the VSIX
  child-process/JSONL compatibility bridge.
- [ ] Add external-process attach after the runtime exposes an authenticated attachable IPC
  endpoint; the existing live-stdio protocol is child-process-only and cannot safely attach.
- [x] Route TUI diagnostics, hover/type/live values, definitions, completion, and rename preview
  through one persistent in-process `LanguageService` and its live-observation broker, retaining
  deterministic host-owned command queues and presentation.
- [x] Move tested edit preview/apply and rollback status behind the broker and retain live JSON
  automation as a versioned adapter. The packaged VSIX verifies preview is non-mutating, apply
  swaps executable behavior and disk source, and undo restores both.

### Slice 6: code intelligence depth

- [x] Add a standard compiler-validated Organize Imports code action that sorts, deduplicates, and
  prunes unused modules; expose the same no-write preview through the TUI.
- [ ] Add structured compiler diagnostic quick fixes and safe refactor actions without matching
  human diagnostic strings or introducing a second parser.
- [x] Add semantic tokens from compiler-owned identities, including globals, fields, parameters,
  locals, types, functions, methods, enums, and constants. During broken edits, publish only tokens
  whose source spans remain byte-identical in unchanged last-good regions.
- [x] Add standard LSP inlay hints from compiler-owned inferred local types and resolved call
  signatures, expose the same read-only query as `:inlay-hints FILE` in the TUI, and recover only
  byte-identical last-good hints outside broken edits.
- [ ] Add completion resolve/detail, expected-type refinements, and remaining import insertion.
- [ ] Add call hierarchy and type hierarchy from compiler-owned graphs and symbol IDs.

### Slice 7: debugging and editing polish

Add a Debug Adapter Protocol implementation backed by real runtime pause/stack/scope support, then
folding, selection ranges, linked editing, range/on-type formatting, and bracket-aware snippets.
Debugger work must expose real lexical frames before live locals or watches are claimed.

## Validation

For every implementation slice:

- unit-test transport-independent operations before protocol mapping;
- test both disk documents and dirty overlays, including same-line declarations and imports that
  are reachability roots;
- test stale-result cancellation and snapshot atomicity under rapid edits;
- verify positions with ASCII, multibyte UTF-8, and UTF-16 surrogate pairs;
- run LSP transcript tests against the compiled `stasis` executable;
- run VSIX unit/e2e tests for the capabilities changed;
- run focused Rust tests and `tools/validate_repo.sh`, each bounded to five minutes;
- check for and terminate lingering test processes after every test step;
- run one representative sample through compiler, Cranelift IR, executable build, execution, and
  behavioral assertions when compiler behavior changes;
- record measured latency distributions and regress the service when the warm targets are missed.

## Migration and compatibility

Existing semantic VS Code providers remain only until their LSP equivalents pass end-to-end tests;
they must not answer in parallel because competing semantic sources create nondeterminism. The
temporary live completion overlay may add runtime-only indexed fields until Slice 5 moves that
enrichment behind the LSP broker.
The live schema-v1 JSON protocol and CLI commands remain supported adapters. New service types are
not serialized directly: LSP, live JSON, Android JNI, and terminal presentation each own explicit
versioned mappings.

The migration is complete when VS Code uses a standard LSP client for all language features, the
Live Workshop is a thin custom-protocol surface, the TUI consumes the same language-service API,
and no editor path reparses source or launches a compiler process per request.

## Theory gained

The compiler index and running game describe the same program at different times: semantic symbol
identity plus source hashes are the join key. The current completion path demonstrates that warm,
compiler-owned indexes can serve both the TUI and editor, while the VSIX's per-request process path
shows why transport ownership prevents reuse and adds latency. Therefore a compatible live hover
should be a cheap cache join onto an immutable semantic snapshot; the adjacent prediction is that
rename, references, and semantic highlighting can reuse the same identity/index publication path
without consulting the runner.

## Slice reflection

- Good: tracing one completion request from VS Code through both offline and live paths exposed the
  reusable compiler-owned ranking and identity data already present.
- Bad: language analysis is assembled inside runner/app hosts, so editor and TUI features cannot yet
  share one revisioned snapshot or cancellation model.
- Adjustment: extract transport-independent snapshot operations first, and delete each host-local
  analysis path as soon as its service-backed replacement passes end-to-end verification.

### Persistent diagnostics implementation

- Good: the executable transcript and packaged VSIX test proved that dirty UTF-16 editor changes
  reach the real compiler and that repaired buffers clear standard diagnostics without disturbing
  the existing live Workshop path.
- Bad: Windows file URIs and workspace roots arrived with different drive-letter casing, which the
  compiler correctly treated as distinct case-sensitive source identities.
- Adjustment: canonicalize real filesystem paths at the LSP transport boundary and keep canonical
  compiler identity independent of editor URI normalization.

Theory gained: editor paths are transport identifiers, while compiler paths are semantic identity.
The packaged test exposed the distinction through Windows drive normalization, and canonicalizing
only at the LSP boundary restored one stable source identity. This predicts that every future LSP
workspace edit and navigation location must pass through the same boundary conversion rather than
constructing compiler paths from URI strings directly.

### Shared completion, hover, and signature implementation

- Good: one immutable completion-snapshot query now handles dirty full documents and TUI definition
  overlays while allowing the live host to merge runtime commands, state paths, and scratch cells.
- Bad: completion models and deterministic ranking originated in `stasis_runner`, so extracting the
  shared operation exposed a historical ownership inversion even though no second ranking path was
  added.
- Adjustment: keep transport-neutral query orchestration in `stasis_language_service`; move the
  remaining generic completion models/ranker out of the live protocol when the custom live broker
  replaces that protocol's editor-facing completion command.

Theory gained: the stable reusable unit is not merely a symbol catalog; it is catalog plus lexical
scope, accepted source spans, dirty overlay, expected type, and deterministic ranking at one
workspace revision. The TUI overlay tests and VSIX requests now exercise that same unit. This
predicts that rename preview and navigation should consume a similarly immutable symbol/reference
snapshot while host-only runtime values remain optional enrichment.

### Shared navigation and symbols implementation

- Good: main's temporary CLI-backed Definition/References providers supplied concrete packaged
  behavior that could be migrated request-for-request onto the standard LSP without weakening
  indexed-field or cross-file semantics.
- Bad: grouped global source items classified declarations as writes and exposed declaration-wide
  spans, so a global receiver initially had no precise definition target.
- Adjustment: compiler reference indexes must publish explicit exact definition records for every
  declaration kind even when edit-oriented source items intentionally group multiple declarations.

Theory gained: edit grouping and navigation identity are separate views of the same source. A
grouped globals item is useful for transactional replacement, but navigation requires the exact
identifier token plus its canonical symbol identity. The global receiver/owned-field regression
proves the distinction and predicts that rename should plan edits from exact reference identities,
then validate/apply them through the grouped transactional edit model.

### Compiler-validated rename and error recovery implementation

- Good: completion scopes and exact reference aliases supplied enough compiler-owned identity to
  plan one validated rename transaction for globals, fields, structs, functions, parameters, and
  shadowed locals across both LSP and TUI surfaces.
- Bad: rebuilding the semantic index eagerly meant one half-written function could temporarily
  remove read-only intelligence for otherwise valid code, and grouped edit records alone were too
  coarse for identifier-sized workspace edits.
- Adjustment: retain a revision-tagged last known good index for read-only queries, require a
  current index for any edit-producing operation, and publish durable exact binding identities in
  future compiler index work instead of reconstructing them from completion scopes.

Theory gained: editor recovery needs two simultaneous truths: the current dirty text owns the
cursor and diagnostics, while the last successfully indexed snapshot can safely answer read-only
semantic questions. The incomplete `a(state.` regressions show that current lexical receiver text
can query last-good global type data, while the stale-index rename rejection shows why edits must
not cross that boundary. This predicts that future code actions and formatting may use recoverable
current syntax, but semantic refactors must remain revision-exact and transactionally validated.

### Live observation composition implementation

- Good: adding accepted source hashes and runtime layout to every successful live response made
  hover and indexed completion cheap cache joins, and allowed deletion of the VSIX's competing
  completion provider without losing `state.enemies[0].speed` behavior.
- Bad: the first compatibility check required every indexed file to appear in the runtime build;
  test-only files are intentionally absent from runtime reachability, so the valid live layout was
  initially rejected.
- Adjustment: validate every accepted runtime input against the current semantic snapshot, but do
  not require editor-only/test-only files to be runtime inputs. Continue requiring the exact owner
  file hash for a displayed live value.

Theory gained: runtime compatibility is a directed subset relation, not equality between project
indexes. Every file that produced the accepted executable must still match, while additional test
or editor-only files do not make that executable stale. The packaged indexed-completion and live
hover tests support this mapping; it predicts that attach/debug handshakes must publish the same
accepted-input set rather than a generic workspace revision number.

### Persistent TUI language-service implementation

- Good: one persistent in-process service added diagnostics, hover with compatible live values,
  definition, and rename preview without introducing a TUI parser or a JSON-RPC loopback.
- Bad: the TUI previously reused shared completion snapshots but recreated the language service
  for rename and had no shared diagnostics, hover, or definition command surface.
- Adjustment: host surfaces should own one long-lived language-service handle, synchronize only
  changed accepted files, and keep transport queues and human presentation outside the service.

Theory gained: language-operation reuse does not require transport reuse. The LSP and TUI can call
the same revisioned Rust operations and live-observation broker while each retains its natural
queue and response format. The persistent-service test proves that static identity and runtime
values compose in-process; this predicts that live edit preview and rollback can share broker
state without routing TUI commands through JSON-RPC.

### LSP-owned Live Workshop process implementation

- Good: the packaged VSIX test exercised launch, pause, indexed completion, inspection, live hover,
  watches, stepping, semantic edit preview/apply/undo, ordinary saved-file hot swap, resume,
  framebuffer capture, and stop after the extension's direct child-process and JSONL decoder were
  deleted.
- Bad: the old observation-forwarding bridge duplicated runtime values in TypeScript and made the
  language server a passive cache recipient even though it owned the semantic side of the join.
- Adjustment: keep long-running custom operations on bounded worker threads, keep runtime protocol
  correlation and cache publication in one Rust broker, and leave the main LSP loop available for
  ordinary language requests.

Theory gained: process ownership and semantic cache ownership must meet on the server side of the
LSP boundary. The end-to-end test proves that VS Code needs only custom requests and notifications
while standard completion and hover consume the same server-owned cache. This predicts that tested
edit preview/apply can move behind the broker without adding another extension-side compiler or
runtime client. External attach remains a distinct runtime transport problem because stdio has no
discoverable or authenticated endpoint.

### Compiler-validated organize-imports implementation

- Good: reusing the compiler's import graph made sorting, duplicate removal, and unused-module
  pruning one deterministic operation shared by LSP and TUI, and candidate compilation prevented
  unsafe workspace edits.
- Bad: the existing semantic-edit cleanup compared normalized import sets, so it intentionally did
  not expose textual normalization as an editor operation.
- Adjustment: expose narrow compiler-owned transformation plans from the canonical parser/index,
  then map those plans to versioned LSP edits rather than reconstructing source structure in the
  editor or language-service transport.

Theory gained: a safe source action is a compiler transformation plan plus candidate validation,
not a diagnostic-message heuristic. The compiler and language-service tests prove that import
organization can repair duplicate/unused imports while broken unrelated source yields no action.
This predicts that quick fixes should originate as structured compiler diagnostics with explicit
edit plans instead of matching rendered error text.

### Compiler-bound semantic highlighting implementation

- Good: the validated-rename binding resolver already distinguished structs, functions, globals,
  fields, parameters, and shadowed locals, so semantic highlighting gained exact ownership without
  another parser or editor-side classifier.
- Bad: a last-good semantic index contains correct identities but obsolete byte offsets inside the
  active edit, so publishing the whole stale token stream would visibly miscolor unrelated text.
- Adjustment: retain last-good read-only intelligence, but remap only the byte-identical prefix and
  suffix around a broken edit and discard every token that intersects the changed region.

Theory gained: a stale semantic identity can remain valid while its source coordinate is invalid.
The incomplete-function recovery test proves that exact unchanged regions can safely retain
compiler-aware coloring while the edited region falls back to lexical highlighting. This predicts
that inlay hints and hierarchy selection ranges can use the same unchanged-region recovery rule,
while edit-producing actions must still require a current compiler snapshot.

### Compiler-owned inlay hints implementation

- Good: recording inferred locals inside the existing data-flow walk exposed the compiler's real
  type result without a literal guesser, while the existing resolved completion signatures supplied
  parameter labels for direct and receiver-form calls.
- Bad: the first implementation repeated a small control-flow traversal solely to collect inferred
  locals, creating pressure for a second analysis path even though it called the same expression
  typer.
- Adjustment: instrumentation and editor metadata must be collected by the canonical semantic walk
  itself; protocol layers may attach source spans but must not replay semantic control flow.

Theory gained: an inlay hint is a projection of an accepted semantic fact onto a recoverable source
anchor. Compiler tests prove inferred `let` types originate in the same data-flow walk used for
checking, and packaged VSIX/TUI tests prove type and parameter projections share one cached service.
This predicts that future live-value hints should join onto these static anchors only when runtime
identity and source hashes match, exactly as live hover already does.
