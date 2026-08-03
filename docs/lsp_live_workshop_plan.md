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
- [x] Switch the TUI completion/details query to the shared snapshot operation and delete the VSIX
  per-request CLI/live completion provider.
- [x] Measure warm local p95 latency and cover the standard operations in the packaged VSIX.

### Slice 3: navigation and symbols

Ship definition, references, document symbols, and workspace symbols. Extract canonical symbol
identity and reference data from the current workshop/live host into compiler/service APIs. Wire
Outline, breadcrumbs, and workspace symbol search; update TUI read/reference commands.

### Slice 4: compiler-validated rename

Implement prepare-rename and rename preview for locals, parameters, fields, globals, functions,
structs, and other supported types. Resolve identities semantically, produce versioned workspace
edits, reject collisions or stale documents, and compile the complete proposed overlay before
returning edits. The TUI shows the same preview; applying remains host-controlled and atomic.

### Slice 5: Live Workshop composition

Move launch/attach and live cache ownership behind `LiveSessionBroker`. Add revision handshakes,
custom LSP methods, type/live hover composition, live watches, and tested edit preview/apply. Make
the VS Code Live Workshop view a protocol client and preserve live JSON automation compatibility.

### Slice 6: code intelligence depth

Add code actions, organize imports, semantic tokens, inlay hints, improved completion resolve,
call hierarchy, and type hierarchy. Every capability consumes the same snapshot and symbol IDs.

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

The existing VS Code providers remain only until their LSP equivalents pass end-to-end tests; they
must not answer in parallel because competing completion or diagnostics sources create nondeterminism.
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
