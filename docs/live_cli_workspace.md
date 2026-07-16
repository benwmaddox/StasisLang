# Interactive live workspace

`stasis run --interactive` starts the manifest entry in the normal in-process graphical runner
and opens a desktop-focused local terminal prompt. Rendering remains on the main thread. Terminal
input enters the bounded `stasis_runner::live` protocol queue and every request is observed or
committed at a normalized between-tick boundary. Android Workshop shares compiler-owned semantic
edit and receipt contracts, but intentionally keeps its own mobile interaction model.

The default terminal is a human workspace view, not a protocol dump. It prints concise scalar,
symbol, edit, scratch, status, and diagnostic lines; large semantic plans are summarized by changed
symbols/files and reload class. Add `--live-json` only for clients that need complete schema-v1
response envelopes.

The project must provide the graphical lifecycle entry points `main`, `tick`, and `render`.
`on_code_swap` is optional. This mode is local-only and does not open a network listener.

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
:preview
:apply
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

The line editor provides session command history and compiler-backed Tab completion. Press
Ctrl-C or enter `:abort` to discard a multiline buffer without submitting it. Symbol results are
paged; selectors accept `--file`, `--owner`, and `--signature` for same-name overloads and receiver
methods.

Add, update, delete, read, list, and completion operate on the compiler-owned symbol and type
indexes. Edits use the same semantic selectors, expected source hashes, import reconciliation,
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

Layout-changing edits return a deterministic diagnostic until the first-class migration contract
tracked by Maddox #153 is available. Compiler, test, stale-hash, receipt, or `on_code_swap`
invocation failure restores the prior disk sources, dispatch table, and bounded typed runtime
state. The current `on_code_swap(): void` ABI has no application-level rejection return value.

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
