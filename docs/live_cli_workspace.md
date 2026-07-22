# Interactive live workspace

`stasis run --interactive` starts the manifest entry in the normal in-process graphical runner
and opens a desktop-focused local terminal prompt. Rendering remains on the main thread. Terminal
input enters the bounded `stasis_runner::live` protocol queue and every request is observed or
committed at a normalized between-tick boundary. Android Workshop shares compiler-owned semantic
edit and receipt contracts, but intentionally keeps its own mobile interaction model.

The default terminal is a human workspace view, not a protocol dump. It prints concise scalar,
symbol, edit, scratch, status, and diagnostic lines; large semantic plans are summarized by changed
symbols/files and reload class. Routine human responses omit protocol request and tick metadata. Add
`--live-json` only for clients that need complete schema-v1 response envelopes.

The project must provide the graphical lifecycle entry points `main`, `tick`, and `render`.
`on_code_swap` is optional. This mode is local-only and does not open a network listener.

## Persistent TUI

Status: the first desktop feel slice is implemented. Run `stasis run --interactive` in a Stasis
project to open it. The deterministic `--live-script` and `--live-json` clients remain available
without the TUI.

### Subscription-backed AI

`stasis ai "PROMPT"` starts the live runtime, executes one AI change, and exits after the atomic
result. It is the simplest form for automation and repeatable evaluations.

`:ai PROMPT` runs a bounded coding turn through the locally installed `codex` executable and its
existing ChatGPT/Codex subscription sign-in. Stasis does not request, store, or infer an API key.
On Windows, the provider prefers the current npm-installed native Codex binary over older desktop
shims on `PATH`. It defaults to `gpt-5.6-sol` with medium reasoning. Set `STASIS_CODEX_EXE`,
`STASIS_AI_MODEL`, or `STASIS_AI_REASONING_EFFORT` to override those selections. `:ai status`,
`:ai cancel`, and Ctrl+C expose status and cancellation without stopping the live runtime.
Deterministic `--live-script` files may also contain `:ai PROMPT`; each line runs through the same
provider, live tools, atomic transaction boundary, and trace policy as the interactive TUI.

Each model turn runs ephemerally in an empty read-only temporary directory. The project is not
mounted into that directory. Codex receives only the user request, bounded Stasis tool catalog,
and bounded observations returned by the live workspace. Symbol reads, runtime inspection, and
deterministic ticks pass through `stasis_runner::live`; model write batches become one
`WorkshopSemanticEditBatch`, compile and test on the normal preparation worker, and commit at a
between-tick boundary. A layout-changing edit remains a validated preview and requires explicit
user `:apply` approval.

Before changing a behavior-bearing symbol, the AI must use compiler-backed `find_references` to
locate its definition, reads, writes, and calls without receiving unrelated source bodies. For an
observable game change it uses AI-only `validate_runtime_state` acceptance checks: the requested
condition must fail before the edit (red), the atomic edit must compile and pass project tests, and
the same condition must pass afterward (green). The default `live` baseline pauses and restores the
same bounded runtime snapshot, so normal game progression cannot manufacture a pass. The optional
`fresh` baseline boots `main`, advances bounded `tick` frames, renders, and inspects state in a
separate Stasis child process for integration-style red/green checks without changing the running
game. Projects with host adapters can select game-level `setup`, `tick`, and `render` entrypoints so
the isolated check exercises logical gameplay without opening a second window or duplicating host
resource side effects. These acceptance checks are ephemeral and do not create or replace project `.test.stasis`
files; durable regression tests remain the appropriate place for behavior that should be protected
after the AI turn.

Human commands intentionally cover every useful live AI capability:

| AI tool | CLI / TUI equivalent |
| --- | --- |
| `list_symbols` | `stasis symbol list` / `:symbols` |
| `find_references` | `stasis symbol references SYMBOL` / `:references SYMBOL` |
| `read_symbol` | `stasis symbol read SYMBOL` / `:read SYMBOL` |
| `write_symbol`, `delete_symbol` | `stasis symbol add|update|delete` / `:add`, `:update`, `:delete` |
| `inspect_runtime_state` | `:inspect`; `stasis validate` exposes fresh-run scalar evidence |
| `validate_runtime_state` | `stasis validate PATH OP VALUE` / `:validate PATH OP VALUE` |
| `run_frame` | `:step`; `stasis validate --frames N` in an isolated CLI run |
| `run_tests` | `stasis test`; live `:apply`, `:undo`, `:redo`, and definition apply run tests automatically |

`stasis validate` uses the isolated `fresh` baseline and accepts `--frames`, `--setup`, `--tick`,
and `--render`. TUI `:validate` uses a snapshot of the live game, runs the requested frames, reports
PASS/FAIL with expected and actual values, and restores the snapshot before returning.

Every run creates `build/ai-traces/tui-ai-<timestamp>.jsonl`. The trace records the user request,
turns, user-visible working notes, and the exact tool calls and bounded tool observations exchanged
with the agent. Tool source arguments and results therefore appear verbatim when they were sent to
or received from the agent. The trace does not store the complete Codex transport request, response
schema, tool catalog, repeated conversation envelope, or CLI transport output.

Provider-reported token usage is written separately to
`build/ai-traces/tui-ai-<timestamp>.usage.jsonl`. Each line is the exact `usage` object from a
Codex `turn.completed` event; Stasis does not estimate or add missing token categories. The Codex
JSON event stream is consumed in memory and all non-usage transport events are discarded.

AI `list_symbols` calls search only the project entry file by default. The agent can pass `files` as
an array of up to 16 project-relative paths to widen that starting scope, and can further narrow it
with `query`, `kind`, `owner`, `page`, and `limit`. Human `stasis symbol list` and TUI `:symbols`
accept repeated `--file` options and use the same entry-file default. Listings return at most 32
entries by default and omit imports, empty global groups, source bodies, and source hashes. Each
item contains only its name, kind, signature, file, and owner when applicable. `read_symbol` returns
the selected source and its hash so a later write can use that hash solely as a stale-write guard.
An AI request may use up to 15 provider turns. The agent may batch up to 50 deliberate tool calls
in each turn, such as reading a related set of functions after targeted discovery. Combined
observations are bounded to 1 MiB; this supports substantial explicit source reads without making
whole-project enumeration the default behavior.

Provider requests use JSONL: one immutable request-header record followed by append-only
`turn_result` records. Each model response and its tool observations appear exactly once, including
completion-gate feedback. Every later payload is the complete previous payload byte-for-byte plus
one newline and one new record, so provider prefix caching can reuse the stable instruction,
context, tools, contract, and prior interactions.

The current key map is:

```text
type / :              progressively filter the always-visible completion pane
Up / Down             choose a completion when armed; otherwise move/history
Tab                   accept the selected safe completion; otherwise no-op
Ctrl+Space or Ctrl+P  explicitly arm completion
Ctrl+K                open the command bar without leaving a definition
Enter                 run a prompt command or add an auto-indented editor line
Ctrl+Enter             validate, test, and apply the current definition snapshot
Ctrl+W                 close a definition (twice when discarding dirty text)
Alt+D / Alt+P / Alt+I Do / Print / Inspect the selection or current token
Ctrl+Z / Ctrl+Y       undo / redo text edits only
```

Pane-focus cycling with F6/Shift+F6 is intentionally still an open product question. This slice
keeps keyboard focus in the input/editor and makes completion selection contextual instead.

Implemented now are the persistent/adaptive panes, asynchronous coalesced completion, safe Tab and
ghost insertion, multiline definition editing/apply, text undo/redo, command bar, concise default
and pinned inspection, syntax colors, and guaranteed terminal restoration.
Transcript scrollback with unread counts, visible selection styling, matching-delimiter feedback,
diagnostic-span underlines, and a shared declarative command grammar remain planned polish; this
slice does not claim them yet.

The interactive human view becomes one long-lived terminal application rather than a line editor
that temporarily opens a palette. The left side contains the transcript and an adaptive input
area. The right side is always present and is split between ranked candidates and details or
diagnostics for the highlighted item. When the terminal is too narrow for useful side-by-side
panes, the right pane moves below the editor instead of squeezing or refusing to run. Normal REPL
input keeps a short editor and a larger transcript; opening a function expands the editor and
compresses the transcript.

The right pane is useful even when completion is not armed. It shows commands and symbols ranked
for the current semantic context, with signature, type, owner, and source details for the
highlighted row. Typing an identifier, receiver dot, or a leading `:` query arms completion;
Up/Down then changes the candidate and Tab accepts exactly that row. Esc disarms completion without
changing text. Otherwise Up/Down retain normal meaning: history in the short REPL input and
vertical cursor movement in the multiline editor. Ctrl+Space explicitly arms completion on a blank
or indented line. Tab is reserved for completion and is a no-op when no safe candidate is armed; it
never inserts indentation or changes focus.

The selected candidate and inline ghost text are one model. The ghost renders only the untyped
suffix of a genuine prefix match in dim gray. Fuzzy matches may remain visible in the pane, but do
not produce misleading inline text. Async refresh retains selection by stable symbol or command
identity and discards stale query results. Accepting a function or method inserts only its symbol;
the signature remains visible in the details pane, and there is no snippet or Tab-stop mode.

Completion insertion is permitted only when everything from the cursor to the end of the entire
input buffer is whitespace or newlines. When that safety rule is not satisfied, the pane may still
show context, but no candidate is armed, no ghost is drawn, and Tab is a no-op. Multiline dirty
buffers need a bounded, tolerant compiler-owned overlay analysis so parameters, completed typed
`let` bindings, receiver fields and methods, and expected types remain available before the edited
definition is balanced or accepted. Overlay analysis is read-only and must never compile, write,
or publish the dirty buffer. It augments the accepted project catalog rather than introducing a
second language model.

Enter inserts a newline and applies brace-aware auto-indentation. Ctrl+Enter snapshots the complete
buffer, validates it, runs the normal test gate, and atomically hot-swaps it. Editing remains
available while that immutable snapshot is prepared. A successful current snapshot closes the
editor and returns to the prompt. If the user typed after submission, the accepted snapshot becomes
the new base without erasing those later keystrokes, and the editor stays open with them dirty. Failed validation
or apply keeps the editor open and reports diagnostics in the status/transcript; structured
right-pane diagnostics with adaptive height remain planned polish. Stale results never overwrite a
newer submitted snapshot. `:end` remains a compatibility escape hatch for the script and legacy
line-input paths.

`:edit SYMBOL` opens the complete current definition with its semantic selector and expected source
hash. Overloads are disambiguated in the right pane using owner, file, and signature metadata. The
user-facing command remains concise; storage-level selectors are generated internally. After a
successful clean apply the transcript records the swap and the editor closes. Ctrl+W closes an
editor manually, requiring confirmation before a dirty buffer is discarded.

A leading `:` in the normal REPL input progressively completes commands and valid arguments at the
current position. Inside a Stasis definition, `:` always remains language syntax. Ctrl+K opens a
temporary progressive command bar while editing; Enter executes the selected command and returns
focus to the editor, while Esc closes the bar without changing code. Help, parsing, validation,
preview, and progressive argument completion should be generated from one command grammar so their
accepted forms cannot drift.

Smalltalk-style workspace actions currently operate on the current selection, or on the current
token when there is no selection: Alt+D performs Do, Alt+P performs Print, and Alt+I performs
Inspect. A compiler-owned smallest-enclosing-expression selection is planned polish. The actions
are available only for global/session expressions that can execute
honestly in the current runtime. Expressions that depend on parameters or locals are disabled with
an explicit explanation because the runtime does not expose a paused lexical stack. The TUI must
not fake this context or instrument a future tick. Inspect pins a live structured view in the
lower-right pane and refreshes it as the game runs until dismissed or replaced. Print writes an
immutable one-time value to the transcript. Do does not add routine output beyond a minimal success
state, but always surfaces failures and diagnostics. The default inspector refreshes visible values
at a readable UI cadence and does not append each refresh to the transcript.

The concise default inspector is the primary continuous observation surface. Explicit inspections
can pin one value, while change-only watches remain available for notifications. Inspector refresh,
completion, rendering, or a slow terminal must not block the game tick.

Ctrl+Z/Ctrl+Y operate only on the current text buffer. `:undo` and `:redo` remain a separate history
for accepted source/runtime swaps. The first TUI slice includes basic syntax coloring. Transcript
scrollback/unread preservation, matching-delimiter feedback, and diagnostic span underlines are
planned follow-up work; full semantic highlighting is not required.

The interactive frontend should own terminal raw mode, alternate-screen entry, event dispatch,
layout, rendering, and guaranteed restoration as one RAII-guarded lifecycle. The deterministic
`--live-script` and `--live-json` paths remain non-TUI protocol clients with their current bounded,
fail-fast behavior. Invalid human commands remain nonfatal and return focus to the current editor.

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
:edit tick
:inspect
:inspect score
:watch score
:set score 10
:print score
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
`hero.damage`. Scoped candidates carry compiler-owned file, semantic-owner, visibility-span, and
type metadata, so locals from another function or an out-of-scope block are excluded. Each row
stays concise with kind and type/signature/source context. `:palette QUERY [--page N --limit N]
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

`:inspect`, `:set`, and `:do` accept compiler-indexed scalar paths (`i32`, `f32`, `f64`, or
`bool`). `:do` is a semicolon-separated assignment transaction:

```text
:do --preview
score = 20;
ready = true;
:end
```

All paths and values are validated before any assignment is written. Preview performs no write.
Calls, arbitrary addresses, and unsupported expressions fail clearly instead of using a second
parser or lowering pipeline.

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
stasis run --interactive --live-script live.commands --live-json
```
