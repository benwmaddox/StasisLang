# Live workspace protocol

`stasis live [ENTRY] --live-stdio` runs the normal graphical hot-swap session with a
versioned JSON-line child-process protocol for editor integrations. Rendering stays on
the main thread; requests are observed or committed between ticks. `--live-script PATH`
runs a deterministic command file and `--live-json` emits full response envelopes.
The manifest supplies the default entry and title, and the entry parent is watched.
The project must provide `main`, `tick`, and `render`; `on_code_swap` is optional.

The compiler-backed semantic edit, inspection, completion, validation, and scratch
operations below remain available to clients. The former interactive TUI and AI
frontends have been removed.

## Commands

```text
:help
:status
:pause
:resume
:step 1
:cancel 42
:symbols tick --page 0 --limit 50
:read tick function --file src/main.stasis --owner Game --signature "tick(): i32"
:complete ti
:palette hrohp
:preview
:apply
:inspect
:inspect score
:inspect state.enemies[2].hp
:inspect state.enemies[?hp >= 10]
:watch score + state.enemies[2].hp
:set score 10
:print score * 2
:changes
:undo
:redo
:quit
```

Code-aware add and update commands use an inline multiline buffer ending with `:end`:

```text
:update function tick src/main.stasis
function tick(): i32 {
    score += 4;
    return 0;
}
:end
```

The TUI provides session command history and a compiler-backed command and symbol palette. Typing
filters the persistent pane; Ctrl+Space or Ctrl+P explicitly arms it. Up/Down or PageUp/PageDown
select, Tab inserts the highlighted candidate, and Esc disarms without changing the buffer.
Inserted commands still require Enter, so a completion selection never mutates the session by
itself.

The palette includes functions, structs, enum variants, globals, state paths, parameters,
explicitly typed locals, fields, and receiver-qualified members such as `hero.hp` or
`hero.damage`. Root search omits static `Type.field` catalog entries that cannot be evaluated
without an instance; definition editing retains them as useful type context. Scoped candidates
carry compiler-owned file, semantic-owner, visibility-span, and type metadata, so locals from
another function or an out-of-scope block are excluded. Each row stays concise with kind and
type/signature/source context. `:palette QUERY [--page N --limit N]
[--owner OWNER --file FILE --signature SIGNATURE --offset N --expected-type TYPE]` exposes the same deterministic,
bounded ranking to scripts and future desktop clients. Press Ctrl-C or enter `:abort` to discard a
multiline buffer without submitting it. Symbol results are paged; selectors accept `--file`,
`--owner`, and `--signature` for same-name overloads and receiver methods.

Add, update, delete, read, list, and palette completion operate on compiler-owned symbol, scope,
and type indexes. Successful edits refresh the palette atomically with the new runtime. Edits use
the same semantic selectors, expected source hashes, import reconciliation,
atomic source writes, test gate, and content-addressed receipts as `stasis symbol`. Successful
edits are parsed, planned, compiled, and tested on a bounded background preparation worker. The
graphics thread remains responsive and performs only the hash guard, atomic source/receipt write,
bounded state snapshot, hook invocation, and pointer commit between ticks. `:undo` and `:redo` use
the recorded semantic plan; they do not reverse arbitrary text ranges.

Preparation records every `src/` and `tests/` input hash, stages JIT literals without publishing
them, and runs tests in a cancellable helper process whose output cannot block the worker. A queued
cancel or quit is observed before a ready candidate can commit, and session shutdown joins the
worker.

Add `--preview` to an inline `:add`, `:update`, or `:delete` to compile and retain a validated
plan without writing. `:preview` displays that staged plan and `:apply` commits it only if its
source hashes are still current.

Layout-affecting edits always stop at a versioned swap preview, even when the original edit did
not pass `--preview`. The preview lists candidate dispatch-patch functions, source and target state-layout versions,
struct-scoped or whole-state migration steps, compatibility, capacity-shrink data-loss warnings,
and a deterministic commit-cost estimate. `:apply` regenerates the preview and refuses the commit
if it differs from the validated version.

Compatible scalar and fixed-collection fields are copied by compiler-owned path/type metadata.
New fields and expanded collection capacity are explicitly initialized to the type default before
`on_code_swap`; removed fields are reported, and collection lengths are clamped when capacity
shrinks. Growth is preflighted against runtime storage ownership and the bounded live-state budget.
UTF-8 shrink retains only the largest valid code-point prefix and recomputes byte and character
counts. Type changes at an existing state path and function ABI changes reject deterministically.
Compiler, migration, test, stale-hash, receipt, or `on_code_swap` failure restores the prior disk
sources, dispatch table, fixed-capacity headers, and bounded typed runtime state with no partial
commit. `on_code_swap(): void` can call the stdlib `reject_code_swap()` helper to reject after
validating or adjusting migrated state; the runtime then restores the prior disk, code, and state.

## Scratch and state transactions

Bare `:inspect` returns a bounded state tree containing scalar values, collection shapes,
capacities/active counts, struct fields, and current memory totals. Explicit `:inspect`, `:print`,
and `:watch` use compiler-indexed state queries. Queries support scalar paths, fixed collection
indexes (`state.enemies[2].hp`), bounded predicates (`state.enemies[?hp >= 10]`), parentheses,
and scalar arithmetic/comparison with normal precedence. Predicate scans stop after 4096 elements
and return at most 64 matches with explicit truncation fields. Watches re-evaluate the same query
between ticks and publish only changes. Predicate watches share one 4096-element scan budget per
tick, and a watch that becomes invalid publishes one `watch_error` until it recovers or its error
changes.

Queries do not expose arbitrary addresses, lexical stacks, calls, or runtime reflection. Missing
paths/fields, invalid indexes/types/operators, divide-by-zero, and unsupported syntax return stable
diagnostics from the compiler metadata path. `:set` and `:do` remain intentionally narrower:
they accept compiler-indexed scalar paths (`i32`, `f32`, `f64`, or `bool`) and literal/path values.
`:do` is a semicolon-separated assignment transaction:

```text
:do --preview
score = 20;
ready = true;
:end
```

All paths and values are validated before any assignment is written. Preview performs no write.
Calls, arbitrary addresses, and unsupported mutation expressions fail clearly instead of using a
second compiler or runtime-reflection path.

Named cells retain code and results only for the current development session:

```text
:cell put reset_score
score = 0;
:end
:cell run reset_score --preview
:cell run reset_score
:cell list
:cell clear reset_score
```

`:cell persist NAME KIND SYMBOL [FILE]` explicitly promotes a cell through the normal semantic
edit path. Scratch text never silently becomes project source.

## Automation protocol

Gauntlet adds two schema-v1 JSON commands without changing the human TUI:

```json
{"schema_version":1,"request_id":70,"type":"set_input_state","pointers":[{"id":0,"x":480,"y":270,"is_down":true,"went_down":true}]}
{"schema_version":1,"request_id":71,"type":"capture_frame","artifact":"candidate-0001"}
```

`set_input_state` accepts at most eight logical pointers and overrides physical
pointer data until replaced (an empty array clears the simulated pointers).
Edge flags clear after one deterministic tick. `capture_frame` accepts only a
bounded artifact identity and captures a PNG from the next presented frame;
the runtime chooses the path under the configured project output. Its final
`capture_completed` response is deferred until the PNG decodes successfully,
with path, dimensions, byte length, SHA-256, scheduling/completion ticks, and
runtime identity. A scheduled capture alone is not success. Verification has a
five-second deadline, supports cancellation, and abandons disconnected callers.
Rendering continues while paused, so capture does not require a gameplay step.
Gauntlet
combines these commands with pause, step, validation snapshot/restore, and
state inspection to run repeatable scenarios. Callers cannot supply a capture
filesystem path.

Use `--live-json` to print schema-v1 response envelopes as JSON lines. A command file can mix the
terminal spelling above with request JSON:

```json
{"schema_version":1,"request_id":42,"type":"inspect","path":"score"}
```

Every response includes `schema_version`, `request_id`, `tick`, `ok`, `kind`, and either `data` or
`error`. An edit first returns `edit_preparing`; its final response reuses the same request ID after
background validation and the between-tick commit. Watch events use request ID `0`, and dropped
watch notifications are followed by a `watch_backpressure` count. Queue length, serialized request
and response bytes, multiline/cell size, symbol/completion pages, transaction assignments, and
runtime snapshot bytes are bounded; overload is reported as backpressure, rejection, or explicit
truncation.

`--live-json` changes only presentation. The normal terminal and JSON-lines clients use the same
request queue, compiler indexes, edit plans, tick boundaries, and response objects.

Run a repeatable session without Cargo or repository-only tools:

```text
stasis live src/main.stasis --live-script live.commands --live-json
```
