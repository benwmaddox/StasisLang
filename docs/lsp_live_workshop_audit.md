# LSP, Live Workshop, TUI, and DAP Completion Audit

This audit maps the requested editor experience to the implementation and executable evidence. The
compiler and `stasis_language_service` remain the semantic authorities; LSP, VS Code, the TUI, and
DAP are transports or presentation hosts.

## Architecture and latency

- `stasis lsp --stdio` is a long-lived LSP 3.17 server. It owns one persistent, revisioned
  `LanguageService` and incremental UTF-16 document overlays.
- `vscode-stasis` uses `vscode-languageclient`. It registers no VS Code language providers and
  starts no per-request compiler commands. Its only child processes are the persistent LSP and DAP
  transports selected for a workspace or debug session.
- Live Workshop process ownership, request correlation, backpressure, and the bounded latest-value
  cache live in `stasis_lsp`. Static requests never wait for a game tick. Runtime values enrich a
  result only when accepted source hashes and semantic identities match the current index.
- The TUI holds the same Rust language-service and live-observation state in process; it does not
  loop language queries through JSON-RPC or maintain another parser.
- `warm_intelligence_queries_meet_local_latency_contract` covers warm completion, hover, signature,
  symbol, diagnostic, and rename budgets. LSP worker tests cover asynchronous live dispatch and
  backpressure.

## Requirement matrix

| # | Requirement | Implementation evidence | Behavioral evidence |
| ---: | --- | --- | --- |
| 1 | Continuous diagnostics | Incremental overlays publish compiler diagnostics through standard `textDocument/publishDiagnostics`. | `diagnostics_follow_dirty_overlay_revisions_and_clear_on_close`; `did_open_publishes_diagnostic_and_full_change_clears_it`; packaged VS Code E2E adds, observes, repairs, and clears an unsaved compiler error. |
| 2 | Hover: type, owner, signature, docs, live value | `LanguageService::hover` joins compiler identities with compatible cached runtime observations; standard `textDocument/hover` projects Markdown. | `hover_reports_inferred_type_owner_signature_and_documentation`; `hover_uses_only_hash_and_type_compatible_live_observations`; standard LSP and packaged live-hover tests. |
| 3 | Signature help | Compiler-owned callable signatures drive standard `textDocument/signatureHelp`, including the active parameter. | `signature_help_tracks_active_parameter`; `standard_requests_return_completion_hover_and_signature_help`. |
| 4 | Compiler-validated rename | Prepare/rename uses semantic bindings, exact snapshot revisions, collision checks, candidate compilation, and versioned workspace edits. | `rename_is_revisioned_and_compiler_validated`; standard LSP prepare/rename assertions; TUI rename-preview test. |
| 5 | Document/workspace symbols | Compiler symbol spans project through standard document and workspace symbol methods. | `navigation_and_symbols_share_compiler_owned_spans`; standard LSP symbol assertions; packaged Outline/workspace-symbol requests. |
| 6 | Code actions | Structured compiler diagnostics carry quick-fix edits; organize imports uses the compiler import graph and validates the candidate workspace. Rename supplies the safe-refactor path. | `structured_import_quick_fixes_are_compiler_validated`; `duplicate_import_quick_fix_preserves_the_compiling_import`; `organize_imports_code_action_is_current_compiler_validated_edit`; standard and packaged code-action tests. |
| 7 | Semantic highlighting | Standard semantic tokens use compiler bindings to distinguish type, function, global, field, parameter, and local identities. Broken edits retain only coordinate-safe unchanged regions. | `semantic_tokens_recover_only_unchanged_regions_of_broken_source`; `standard_semantic_tokens_distinguish_compiler_bound_symbols`; packaged semantic-token request. |
| 8 | Inlay hints | Standard hints project compiler-inferred local types, resolved parameter names, and compatible cached live values. | `inlay_hints_publish_inferred_types_parameters_and_last_good_recovery`; standard and packaged inlay-hint tests. |
| 9 | Detailed completion | Completion/resolve supplies signatures, deferred documentation, expected-type ranking, bracket-aware snippets, indexed fields, and revision-safe auto-import edits. | `completion_uses_typed_dirty_snapshot_scope_and_replacement_range`; `completion_adds_import_for_unreachable_workspace_symbol`; `compatible_runtime_layout_completes_indexed_collection_fields`; standard resolve and packaged completion tests. |
| 10 | Call/type hierarchy | Standard call hierarchy projects compiler call edges. Type hierarchy explicitly models Stasis struct composition as containing/contained components, not inheritance. | `hierarchy_recovers_from_incomplete_function_without_stale_call_ranges`; `type_hierarchy_exposes_struct_composition`; standard hierarchy protocol and TUI hierarchy tests. |
| 11 | DAP debugging | `stasis dap --stdio` runs the actual instrumented Cranelift JIT on a worker thread and exposes source breakpoints, pause/continue, step in/over/out, accepted-source stack frames, current lexical scalar values, typed globals, and watch evaluation. Normal JIT and AOT builds remain uninstrumented. | `jit_debugger_blocks_on_breakpoints_and_preserves_real_nested_frames`; `instrumented_jit_stops_with_nested_frames_and_current_lexical_values`; `instrumented_jit_exposes_scalar_foreach_item_and_index`; `dap_session_stops_in_real_jit_with_stack_locals_globals_and_stepping`; packaged VS Code DAP E2E. |
| 12 | Editing polish | Standard folding, selection, linked editing, document/range/on-type formatting, and bracket-aware snippets use the canonical lexer, formatter, and semantic scopes. | `folding_and_selection_use_current_incomplete_overlay`; `linked_edits_use_current_compiler_scopes`; `document_range_and_on_type_formatting_share_canonical_formatter`; corresponding standard LSP and packaged tests. |

## Recovery and navigation invariants

- Dirty text is always the coordinate authority. Successful analysis publishes a new immutable
  semantic snapshot atomically.
- When a function is temporarily unparseable, diagnostics come from the current text while
  read-only queries may use last-good identities only where offsets can be recovered safely.
  Edit-producing operations such as rename are rejected until the semantic snapshot is current.
- `incomplete_call_keeps_global_receiver_field_completion` covers both requested forms:
  `function b() { a(state.` and `function b() { a(state. }`. Both retain `state.x` completion.
- `global_receiver_and_owned_field_navigate_to_distinct_definitions` proves that invoking Go to
  Definition on `state` in `state.x` reaches the global declaration, while invoking it on `x`
  reaches the owning struct field.

## TUI reuse

The live TUI uses the persistent service directly for completion and exposes shared commands for
references, diagnostics, hover/type inspection, definition, compiler-authored quick fixes,
organize imports, inlay hints, call hierarchy, type hierarchy, and rename preview. Tests exercise
reference and rename responses, structured quick fixes, persistent service reuse, and live-hover
composition. Runtime control and terminal rendering remain host-specific.

## Windows executable policy

Generated AOT test programs can be blocked when launched from the system temp directory. The
repository supports a stable `.stasis_cache/tmp` execution path and `STASIS_AOT_SIGN_TOOL`, invoked
as `<tool> <artifact>`, for environments that require signing. On the audited Windows machine, all
50 AOT backend tests passed from the stable path without signing; this distinguishes path policy
from compiler or linker failure.

## Final validation evidence

The completion audit used bounded commands (all below five minutes):

- `cargo test -p stasis_compiler -- --test-threads=1`: 462 passed, including real JIT, linked AOT,
  JIT/AOT parity, debugger frames, and scalar `foreach` scope values. Generated executables used
  `.stasis_cache/tmp` on Windows.
- `cargo test --workspace --all-targets --exclude stasis_compiler --exclude stasis --
  --test-threads=1`: all remaining library and target suites passed.
- `cargo test -p stasis --all-targets -- --test-threads=1`: 245 library, 108 binary, and 21 CLI
  integration tests passed under normal temp semantics. The Windows game-launch test passed
  separately under `.stasis_cache/tmp` after system-temp execution was blocked with error 4551.
- `cargo test -p stasis_language_service` and `cargo test -p stasis_lsp`: 29 and 16 passed.
- `cargo test -p stasis --lib live_workspace::tests`: 50 shared TUI/live-workspace tests passed.
- `npm test` and `npm run test:e2e` in `vscode-stasis`: unit/type checks passed; the packaged VSIX
  installed into isolated VS Code 1.96 and completed the LSP, DAP, and Live Workshop acceptance
  flow. The live gate starts and renders the game, applies an observable function hot-swap, and
  applies a compiler-previewed struct migration that preserves existing fields and initializes a
  new field to its type default.
- Every substantive gate in `tools/validate_repo.sh` passed when invoked directly from PowerShell.
  The wrapper itself could not start because this environment does not expose `bash`, `dirname`, or
  `python3` on its executable path; the Python commands, ignore audit, and partitioned Cargo command
  were run directly instead.

## Slice reflection

- Good: the requirement-by-requirement audit found a real scope omission that feature-level DAP
  coverage had missed, and a compiled breakpoint regression now proves the corrected behavior.
- Bad: one broad test invocation initially reported policy and fixture-location failures together,
  obscuring whether the compiler or the Windows launch environment was responsible.
- Adjustment: keep executable-launch tests on the stable cache path, keep standalone project-root
  tests on the system temp path, and report the two partitions explicitly.

Theory gained: a debugger scope is truthful only when compiler metadata and values emitted at the
same executable statement describe the same lexical bindings. The scalar `foreach` regression
proves item and index identities now meet at the JIT rendezvous; this predicts composite `foreach`
items should become expandable DAP variables by extending this value projection, without changing
parsing, stepping, or frame ownership.

### Active-toolchain dependency and nested play-root recovery

- Good: reproducing the failure against ChessTD separated the LSP play-root bug from the project's
  dated stdlib dependency and exposed the stdlib's matching runtime-module requirement.
- Bad: the first regression fixture used a synthetic vendored module and therefore did not exercise
  the real `stdlib -> runtime` import edge.
- Adjustment: full editor play fixtures opt into `"stdlib": "toolchain"` and import a real bundled
  graphics module from `.stasis_cache/toolchain/src`, while focused tests cover live project-root choice,
  transactional dependency synchronization, and generated filename aliases.

Theory gained: the LSP workspace root is the source-identity and dependency boundary, while the
entry parent is only a default watch location. A successful ChessTD graphical live session proves
that the active executable can materialize its exact stdlib/runtime pair under that boundary and
compile an entry nested under `src`; this predicts any editor using the same executable and manifest
will observe identical compiler, stdlib, runtime, diagnostics, and live-play behavior.

### VS Code live-edit acceptance

- Good: extending the packaged editor flow from function-only hot swap to a layout edit proves the
  VSIX carries the compiler's migration preview and transactional apply contract end to end.
- Bad: the prior framebuffer and hot-swap checks could pass without exercising state-layout changes,
  leaving the most consequential live-edit workflow covered only below the editor boundary.
- Adjustment: packaged VSIX acceptance must always gate running/rendering, an observable function
  edit, and a compatible struct edit with value-preservation and default-initialization assertions.

Theory gained: VS Code does not migrate state itself; it brokers a compiler-owned preview and apply
transaction against the running generation. Preserved `hp`/`speed` values and zero-initialized
`armor` in the next generation prove that source publication, code activation, and state migration
share one safe-point commit; this predicts a rejected field type change will leave all three on the
previous generation.

### Warm declaration navigation

The language index now retains compiler-derived declaration spans, root types, and struct-field type
edges for its workspace revision. The LSP warms that navigation cache at startup. Definition requests
walk the cached type edges instead of rebuilding source items and scanning every token in every file.
After ordinary edits, unchanged declaration spans are mapped onto the current text and remain usable
without a workspace rebuild; an edit that touches the declaration fails that mapping and rebuilds the
index before returning a location.

The opt-in `chess_td_warm_definition_reports_service_component_latency` benchmark loads the local
ChessTD `src/` and `tests/` trees. On the audited Windows machine, the former per-request reference
scan took 88 ms inside the language service, the one-time debug index warmup took 345 ms, and 50
cached `game.progression_dirty` definition requests measured 225 us at p95. The acceptance budget is
not this component measurement: the packaged VSIX test times 50 complete
`vscode.executeDefinitionProvider` calls and requires the p95 VS Code -> LSP -> VS Code round trip to
remain below 100 ms. `STASIS_E2E_SOURCE_PROJECT` runs that same installed-VSIX gate against a copied
local project. With a copied ChessTD workspace, 50 complete definition requests measured 13.87 ms
at p95 through the isolated installed VSIX.

- Good: measuring the real ChessTD graph separated one-time index construction from the repeated
  navigation path and exposed the redundant workspace scan.
- Bad: definition previously reused the references operation, paying to classify every matching use
  even though it only needed one declaration.
- Adjustment: keep read-only navigation indexes alive across revisions when unchanged-region mapping
  proves their target spans are still current; rebuild only when that proof fails.

Theory gained: definition latency is a cached identity-and-type-edge lookup plus coordinate mapping,
not a reference search. The `game -> ChessGame -> progression_dirty` benchmark proves that this path
stays valid across unrelated edits; this predicts nested state-field navigation cost grows with path
depth rather than ChessTD workspace size.

### Global-first Live Values tree

Starting a VS Code play session now requests a typed snapshot of all globals without requiring users
to add watches. Dotted global and struct paths are grouped into an expandable tree. Global arrays of
structs include a bounded, same-tick row snapshot and expose a native view-item action that toggles
that collection between field-by-field tree rows and compact table rows. Refresh updates both the
default global snapshot and user-added watches. Automatic refresh follows accepted game ticks, is
configurable from every tick to every N ticks, and defaults to 30. The shared runtime caps one
snapshot at 4,096 scalar values/cells and marks partial collection rows explicitly. Collection
values use a compact column-described row matrix: field names and static types occur once in shape
metadata, while rows contain raw scalar cells. The runtime distributes its cell budget across
collections so a large render buffer cannot starve later gameplay arrays from the snapshot.
Arrays of structs with a boolean `active`/`Active` field hide false rows by default in both layouts;
`stasis.live.filterInactiveCollectionRows` exposes all captured rows without another runtime query.
VS Code subscribes to automatic snapshots and runtime watches only while the Live Values view is
visible. It explicitly unsubscribes both when hidden; the play session and hot-swap loop continue
without inspection work, and locally remembered watches are restored when the view reopens.

- Good: extending the existing compiler-owned `inspect_all` response kept VS Code and the TUI on the
  same state-inspection operation and made collection rows internally consistent at one tick.
- Bad: the previous response returned collection shape metadata without element values, so an editor
  could describe an array but could not render its contents without one request per cell.
- Adjustment: bulk inspection contracts should carry bounded values together with shape metadata;
  clients may choose tree or table presentation without creating a second runtime query path.

Theory gained: live-value presentation is a projection of one typed runtime snapshot, not a watch
list. Scalar globals plus the `Enemy[]` row regression prove that hierarchy and table layout can share
the same identities and tick; this predicts nested collection presentation can be added by extending
snapshot shape metadata without changing the LSP transport or watch semantics.

### Navigation cache review hardening

Warm navigation is now best-effort during LSP startup, so an invalid file produces diagnostics while
the server remains available. Definition cache entries retain all overload declarations. Dotted field
lookups also retain the compiler source spans that establish each root-type and field-type edge; stale
reuse is allowed only while those spans map unchanged into the current documents.

- Good: review scenarios became narrow regressions for invalid startup, overload multiplicity, and a
  same-name field reached through a changed global owner type.
- Bad: declaration-name remapping alone proved only that the destination still existed; it did not
  prove that the cached type path still selected that destination.
- Adjustment: warm semantic caches must retain and validate every source fact used to derive an
  answer, while purely lexical declaration lookups may continue using target-span remapping alone.

Theory gained: a cached field definition is a path proof, not merely a location. Preserving the root
type declaration and each struct-field declaration span proves the path remains valid across unrelated
edits; this predicts call hierarchy caching will need the same dependency-edge validation when it is
moved onto a persistent index.
