# AI providers

Stasis live workspace AI uses one provider-neutral agent loop and one host `ToolExecutor`. Changing the transport does not change symbol selection, atomic writes, compilation, tests, cancellation gates, or completion validation.

## Codex subscription (default)

No provider setting is required. Install and sign in to Codex, then optionally set:

```powershell
$env:STASIS_AI_PROVIDER = "codex"
$env:STASIS_AI_MODEL = "gpt-5.6-sol"
$env:STASIS_AI_REASONING_EFFORT = "medium"
```

## OpenRouter

The OpenRouter adapter uses HTTPS chat-completions streaming with a strict `response_format.json_schema`. Tool arguments are native JSON objects. Each request gives every offered host action a deterministic opaque `action_id`; the provider schema accepts only those IDs, and the host resolves the ID back to a concrete tool only after validation. Provider responses using concrete tool names instead of an offered ID are rejected. Providers that cannot honor required structured parameters are excluded.

```powershell
$env:STASIS_AI_PROVIDER = "openrouter"
$env:OPENROUTER_API_KEY = "..."
$env:STASIS_AI_MODEL = "openai/gpt-oss-120b"
$env:STASIS_AI_ROUTE_ONLY = "cerebras"
$env:STASIS_AI_ROUTE_ORDER = "cerebras"
$env:STASIS_AI_ALLOW_FALLBACKS = "false"
$env:STASIS_AI_ROUTE_SORT = "throughput"
```

For OpenRouter Nitro routing, select the Nitro model variant and omit `STASIS_AI_ROUTE_ONLY` unless a provider pin is also required:

```powershell
$env:STASIS_AI_MODEL = "openai/gpt-oss-120b:nitro"
$env:STASIS_AI_ROUTE_SORT = "throughput"
```

Routing variables are comma-separated where applicable:

- `STASIS_AI_ROUTE_ONLY`: hard provider allow-list.
- `STASIS_AI_ROUTE_ORDER`: preferred provider order.
- `STASIS_AI_ALLOW_FALLBACKS`: `true` or `false`.
- `STASIS_AI_ROUTE_SORT`: `price`, `throughput`, or `latency`. `price` requests lowest-price routing.
- `STASIS_AI_PREFERRED_MIN_THROUGHPUT`: soft tokens/second target.
- `STASIS_AI_PREFERRED_THROUGHPUT_POLICY`: `allow_below` (default) or `fail`. `fail` preflights endpoint metadata and pins only qualifying endpoints.
- `STASIS_AI_HARD_MIN_THROUGHPUT`: hard tokens/second floor. Stasis preflights endpoint metadata and fails closed when no healthy, explicitly allowed endpoint qualifies; it never knowingly routes below the floor.
- `STASIS_AI_MAX_PRICE`: maximum completion price accepted by the OpenRouter routing policy.
- `STASIS_AI_TIMEOUT_SECONDS`: whole provider request timeout (default 120 seconds).
- `STASIS_OPENROUTER_URL`: test/private gateway override; normally unset.

Do not set both preferred and hard throughput thresholds. Metadata/preflight duration is logged separately from response header, first reasoning, first content, first action, inference-total, and turn-total timing. Inference timing and observed throughput exclude metadata preflight time. Usage records contain configured and resolved model/provider, route and fallback state, token/cache/reasoning counts, observed completion throughput, cost when returned by OpenRouter, and structured-validation status.

## Credentialed evaluation (opt in)

Normal tests never contact a paid service. A compact Cerebras action-selection probe is explicit and credential gated:

```powershell
$env:STASIS_RUN_OPENROUTER_EVAL = "1"
$env:OPENROUTER_API_KEY = "..."
python tools/cargo_cache.py run -- cargo run -p stasis_ai --example openrouter_cerebras_eval
```

The probe above selects an action only; it does not edit source. A separate opt-in example launches the real `stasis ai` path against an explicitly supplied disposable project and requests one bounded function addition. The existing host executor owns the atomic write, compile, test, and completion gates:

```powershell
$env:STASIS_RUN_OPENROUTER_EDIT_EVAL = "1"
$env:OPENROUTER_API_KEY = "..."
$env:STASIS_OPENROUTER_EDIT_EVAL_PROJECT = "D:\scratch\disposable-stasis-project"
$env:STASIS_EVAL_STASIS_EXE = "D:\path\to\stasis.exe"
python tools/cargo_cache.py run -- cargo run -p stasis_ai --example openrouter_cerebras_edit_eval
```

This evaluation is paid and mutates the named project; it is never run by normal tests. A successful provider transport is not code-validity success: the command succeeds only after the host reports its tested atomic-write receipt. Inspect the disposable project and `build/ai-traces/*.jsonl` plus `*.usage.jsonl`. Neither credentialed evaluation has been run as part of repository validation. If any action ID or arguments are invalid, the host executes none of that batch, retains every selection in the transcript, and asks the model to replace only the rejected IDs.

## Desktop task replies

The desktop editor sends replies through the UI-neutral `stasis_ai::task_controller`
controller. Each request captures bounded context from one task and carries an
immutable task ID and request ID. Switching the selected task never changes the
destination of an in-flight reply. Provider work runs off the UI thread; polling
only collects completed work. Editing tools produce task-owned semantic proposals;
they do not write source during a provider turn. Accept approves a proposal, and
Apply submits the approved payload to the host's atomic semantic-edit path.

Each action retains its payload and prior revisions. Provider repair responses
may replace only rejected actions or actions marked for repair; they cannot
regenerate accepted or applied work. Repaired proposals require fresh acceptance.
Legacy task records without payloads remain readable but cannot execute edits.

Applying an edit invalidates prior focused-test validation. Changing the test
scope or repairing an action also invalidates it, including a run already in
flight. Background test results carry a run ID, so an obsolete result cannot
finish a newer validation run. Provider context contains compact action metadata;
executable payloads and revision history stay in the task rather than being
copied into every request. Done requires resolved actions and passing validation
for current project sources. Host results and failure replies belong to the originating task even
when another task is selected.

Host operations are serialized. Cancellation stops queued work; an atomic edit
already executing finishes or rolls back, and any committed receipt is retained
on its originating task. Source conflicts and failed edit tests preserve the
previous source and return a repairable failure. An empty focused-test selection
cannot unlock Done.

Cancellation and reconnect invalidate the old request before another response can
be accepted. A late response cannot append to the thread or update its metrics.
Retry is a provider operation and does not reset focused-test validation. Worker
capacity includes canceled calls until they exit, so repeated cancellation cannot
create an unbounded number of background workers. Provider failures use a safe
display message rather than forwarding transport errors or credentials.

Live-session client clones have separate response ownership. Caller request IDs
are local to each clone; the session assigns wire IDs and restores the caller ID
only when delivering to its owner. Ownership survives `edit_preparing` and
`completion_preparing` progress messages until the final reply, so cancellation
can still target a preparing edit.
Existing request validation, bounded output,
queue backpressure, and host execution gates still apply.

Unsolicited session events (`request_id == 0`, including watch updates and watch
errors) have one explicitly selected recipient. The terminal UI claims that role
when constructed, so its cloned client receives events even while the original
client stays alive. Creating background clients does not transfer event ownership
or change request-specific reply routing. Dropping the event recipient selects
the oldest remaining client.

## Live edit round trips

Live edit requests provide current source, symbol identity, and reference results in
`resolved_targets`. The adjacent `edit_execution` guidance tells the model to
reuse that context, use supported arithmetic syntax, and set `complete: true`
on its final write response. Missing dependencies still allow additional reads.
Model tools and desktop proposals do not carry source hashes. Host transaction
input verification and manual editor hash guards remain internal. This guidance
reduces round trips; it is not a correctness gate.

The structured response exposes one completion flag rather than a repeatable
finish action. The provider decoder maps it to the existing host `finish_task`
gate after all calls; it cannot bypass receipt, tests, or task-contract validation.
Legacy finish calls remain compatible internally.
`mode: "done", complete: true` also requests this gate, allowing a separate
completion turn after inspecting a successful write receipt. It does not turn a
rejected write into a successful task.

Live replacements require ID and new source; deletes require ID alone.
Name/file selectors remain available for read discovery; `add_symbol` creates
new symbols in the same atomic batch. Legacy workshop selectors remain supported.
The host registers IDs returned by reads as well as symbol indexes, so a correct
read-derived ID does not fall back to model-authored selectors. Prefetched
definitions replace redundant discovery suggestions and baseline-test prompts;
the discovery tools remain available for missing dependencies.

The brick-layout sample keeps test acceptance criteria in behavioral instructions,
not exact assertion text. Equivalent equality and early-return assertions do not
require extra repair turns. Required test edits and passing tests remain gates;
the instructions alone are not an independent semantic oracle.

Writes validate automatically. A following `run_tests` in the same response
reuses a passing receipt, or is skipped when the preceding write failed, so
tests of unchanged code cannot be mistaken for validation of a rejected edit.

Resolved targets carry identity once in their definition; matching task requirements
retain the symbol ID, name, and behavioral constraints without repeating file,
kind, owner, and signature. Unresolved targets retain their discovery selectors.
Repeated identical repair errors in a turn's model-facing observations may use
`error_from_observation`, a zero-based index of an earlier full error in that same
array. Short errors remain inline when a reference would cost more bytes. Tool
order, results, and the full audit events are unchanged.

Completed OpenRouter streams retain reported usage even when structured-response
decoding fails, and the agent emits that usage before returning the error. Missing
usage and incomplete transport failures cannot be reconstructed from these records.

## Security

Keep `OPENROUTER_API_KEY` only in the process environment or a secret manager. Never place it in prompts, project files, command transcripts, or bug reports. Stasis uses the key only as the Authorization header and sanitizes transport errors; audit logs omit provider envelopes and credentials. Treat prompts, source, tool observations, and model output as data sent to the selected provider. Review provider retention and privacy terms before enabling a remote transport.
