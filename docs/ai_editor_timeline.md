# Desktop task timeline

The desktop editor follows the task-flow reference in commit `9c6f874e`:
`docs/ai_editor_ux_direction.md` and
`docs/evidence/ai-editor/task-flow-reference.jpg`.

Task navigation stays secondary to the current objective, chronological activity,
and persistent reply composer. The game continues in its independent native
window. Interface sizes are expressed in egui points for display scaling.

Activity belongs to the task session, not to a rendered frame. Successful user,
provider, attachment, semantic-action, generated-asset, host, and focused-test
operations append typed entries with task-local sequence numbers. State changes
retain their historical status; action controls use current state and the existing
host execution path. Older saved tasks have no cross-type ordering evidence;
their recovered snapshots are explicitly distinguished from recorded activity.

Provider selection is task-owned and copied when a request is admitted. Resolved
provider metadata remains descriptive; it cannot change the selected transport
for a later request. The thread-context meter uses the controller's retained
character budget, not an estimate of the model's token window.

## Validation

Run focused checks through the repository Cargo wrapper:

```powershell
python tools/cargo_cache.py run -- cargo test -p stasis_ai
python tools/cargo_cache.py run -- cargo test -p stasis --bin stasis desktop_editor
```

On Windows, native renderer evidence can be captured without a provider or game
connection. This is a deterministic UI fixture, not evidence of executed gameplay
edits or live test results. The opt-in test opens a native eframe window, captures
the renderer's screenshot event, writes a PNG, and closes the window:

```powershell
New-Item -ItemType Directory -Force artifacts/task519 | Out-Null
$env:STASIS_EDITOR_EVIDENCE_PNG = "$PWD/artifacts/task519/wide.png"
$env:STASIS_EDITOR_EVIDENCE_WIDTH = "1100"
$env:STASIS_EDITOR_EVIDENCE_SCALE = "1"
python tools/cargo_cache.py run -- cargo test -p stasis --bin stasis capture_native_task_timeline -- --test-threads=1
```

Set width to `680` for compact layout, or scale to `1.5` for high-DPI layout.
Set `STASIS_EDITOR_EVIDENCE_REPAIR=1` to show failed validation and repair.
Set `STASIS_EDITOR_EVIDENCE_ATTACHMENTS=1` to show attachment and generated-asset
review using the repository's arena artwork as an explicitly labeled fixture.
Use `reference` instead of `1` for the shorter message/attachment/reply overview.
Unset these variables after capture. Inspect each PNG for readable typography,
card hierarchy, reachable navigation, and a visible composer without clipping.

## Inspected native evidence

These PNGs come from the native eframe renderer, with fixture state and repository
artwork. They do not claim a live provider request, generated artwork, or executed
gameplay change. Long timelines are scrolled to their latest events; earlier
cards remain in the scrollable history.

| PNG | Evidence |
| --- | --- |
| [Overview](evidence/ai-editor/task519/overview.png) | Wide task rail; user, attachment, and AI reply in sequence; fixed composer. |
| [Compact](evidence/ai-editor/task519/compact.png) | 680-point navigation and task canvas; separate provider and usage rows. |
| [Assets](evidence/ai-editor/task519/assets.png) | Inline asset thumbnail, review controls, provenance, and secondary queued tasks. |
| [Passed](evidence/ai-editor/task519/passed.png) | Historical semantic states and focused-test results; completion remains gated. |
| [High DPI](evidence/ai-editor/task519/high-dpi.png) | 1.5 pixels per point, failed tests and repair, readable header and persistent composer. |

Visual evidence: all five PNGs above were inspected. No live-provider MP4 was
captured for this presentation slice.

Recovery validation (2026-09-06): 75 `stasis_ai --lib` tests and 45 desktop
tests passed through the Cargo wrapper, including a fresh native high-DPI
capture. Formatting and staged/unstaged diff checks passed. The header now
reports needs repair, canceled, and done instead of showing only validation
state; a regression test covers those states. The full repository shell gate
could not start its checks because `dirname` and `python3` were unavailable in
the invoked Windows shell. Optional signing reported no matching certificate,
but both test executables ran successfully. No test processes remained.

Theory gained: chronological state belongs to the task, while commands require
the current task/entity identity and current execution state. Pointer tests that
accept a proposal whose ID sorts after a later proposal demonstrate this
distinction; the same rule applies to generated-asset review and future cards.


## Review corrections

The busy primary action says `Cancel task` and opens a confirmation identifying
its original task. `Keep task open` dismisses it without stopping work;
`Permanently cancel task` uses the existing task cancellation path. Task switching
does not redirect a pending confirmation.

Image generation and import are explicitly unavailable in this desktop shell.
Their buttons are disabled with explanatory tooltips, and command-palette intents
settle once with a task-owned host diagnostic. They never claim generation or
import, and approved assets retain their pending handoff state.

Active, idle tasks allow provider selection while disconnected. Reconnect keeps
the previous request payload and task identity but snapshots the newly selected
provider under a new request ID. Ordinary retry keeps its original provider.

To capture the cancellation prompt, set `STASIS_EDITOR_EVIDENCE_CANCEL=1` with
the existing native evidence command. The capture uses fixture state.


Review validation: 76 AI library tests and 48 desktop tests passed through
`tools/cargo_cache.py`; formatting and diff checks passed. No test processes
remained. Optional certificate signing failed, but test executables ran.

Visual evidence: [cancel-confirmation.png](evidence/ai-editor/task519/cancel-confirmation.png)
was captured from the native renderer and inspected. It shows the task-specific
warning, both confirmation choices, and disabled generation control without
clipping. Interaction assertions cover dismissal and confirmation after switching
tasks. No MP4 of that interaction was captured.


## Base integration

Integrated base `fbd9b697` while retaining the chronological activity model,
provider recovery, explicit task cancellation, and persistent composer. Semantic
action cards now render the base compiler-owned preview and revision history.
Both card acceptance and composer controls require a current preview; host Apply
retains exact-payload and source-fingerprint checks. Action thread positions and
activity sequence numbers are both retained for their respective consumers.

Validation: 64 desktop tests and 77 AI library tests passed through the Cargo
wrapper after integration. The pointer acceptance fixture now plans real semantic
proposals and verifies the displayed action identity despite opposite ID ordering.

Visual evidence: [merged-semantic-preview.png](evidence/ai-editor/task519/merged-semantic-preview.png)
was captured natively and inspected; it shows a compiler-derived source diff in
the chronological card with readable controls and the persistent composer.
Set `STASIS_EDITOR_EVIDENCE_SEMANTIC=1` to reproduce it. No MP4 was captured.

Theory gained: timeline sequence and compiler preview identity are independent:
activity controls presentation order, while task/action/revision/payload and source
fingerprints control acceptance and application. Combined ordering and stale-source
tests support this invariant for future card types.

## Bounded live progress (task 522)

Each provider request retains at most 32 typed progress events in its controller
snapshot. The client, task, and request IDs are captured at admission; switching
UI tasks cannot redirect a reporter. Retry gets a new request ID and fresh bounded
history. Cancellation, callback closure, stale IDs, and terminal state reject late
events. Consecutive duplicates are coalesced; the queued and terminal states are
retained at capacity. Progress contains fixed labels and timing values, never
provider reasoning, response fragments, or transport errors.

The timeline shows the latest provider and host request for the selected task.
Provider first-response and first-action milestones are request-wide, while
contacting-provider can recur across turns. OpenRouter records first nonempty
content and the first root `tool_calls` key at the same millisecond used in its
usage audit. These latencies start at the inference POST, excluding queue,
source inspection, metadata lookup, and approval wait. Providers without streaming
hooks report response completion as first response and leave first action
unmeasured. Unknown route metadata never claims fallback.

The host has one worker, eight admitted requests, and at most 32 events for each
of the session's 32 tasks. Admission stays occupied until its result is drained.
Progress is observational: a callback panic cannot interrupt source rollback.
Cancellation requests do not pretend that an in-flight atomic operation stopped;
the host retains its actual completion or failure after the request to cancel.
Queued canceled operations never execute. Late events and results cannot replace
a newer request's progress.

Expandable details separate provider-boundary latency from source apply and the
compile/test pipeline. The pipeline includes subsequent per-file compilations
and scenario execution. Task-to-tests-passed starts with the first admitted
message in this editor session and ends at the host's verified test result,
including retries and approval wait; UI polling time is excluded. Missing or
truncated measurements display as unmeasured. Progress snapshots are transient;
the existing task activity and validation receipts retain completed outcomes.

The desktop semantic source-write path has no runtime swap acknowledgment.
`CommittingBetweenTicks` is a typed stage for hosts that can observe that boundary;
this executor does not emit it or claim hot-swap latency. Extending it requires a
runtime acknowledgment bound to the reviewed source revision.

### Reproduction and limits

Use the Cargo wrapper for focused checks (`--lib` for `stasis_ai`, `--bin stasis`
with filter `desktop` for the editor). In restricted worktrees set
`CARGO_TARGET_DIR` to a directory inside that worktree first.

The existing native evidence test accepts `STASIS_EDITOR_EVIDENCE_PROGRESS=0..4`
to capture queued, apply, compile, focused tests, and completed host states. Set
`STASIS_EDITOR_EVIDENCE_PNG` to the desired PNG path; a sibling JSON file records
the typed fixture events. These are explicitly labeled synthetic states, not
executed edits or a live provider session.

A credentialed provider trace can be reproduced with `OPENROUTER_API_KEY` and
`STASIS_RUN_OPENROUTER_EVAL=1`, then:

```powershell
python tools/cargo_cache.py run -- cargo run -p stasis_ai --example openrouter_cerebras_eval
```

The example suppresses response content, requires an action, and compares typed
first-response/action timing against the provider usage audit. The required live
OpenRouter/UI acceptance trace remains unverified in this run because no API key
is configured. It must be captured in a credentialed editor session before that
acceptance criterion can be claimed.

Theory gained: a progress label is evidence only when its owner observes the
boundary. The source-apply path and its rollback tests show why successful source
validation cannot stand in for a between-ticks runtime commit; a future swap
observer must carry the same immutable revision and request identity.

Validation (2026-09-06): 87 `stasis_ai --lib` tests and 76 desktop-filtered
`stasis --bin stasis` tests passed on the final source. The OpenRouter example
compiled with `cargo check`; formatting, unsafe-boundary, and diff checks passed.
All Cargo commands used the repository wrapper. The full shell entrypoint could
not start because `bash` is unavailable. One intermediate native capture was
blocked by Device Guard; the final freshly built test executable and all five
captures ran successfully through the repository signing runner. Optional signing
reported no certificate. No test processes remained.

Visual evidence: [phase3.png](evidence/ai-editor/task522/phase3.png) was inspected
at native resolution for readable phase and latency labels, full-width cards,
and a visible composer. [progress-fixture.mp4](evidence/ai-editor/task522/progress-fixture.mp4)
was verified as 150 frames at 1100x900 over five seconds; its decoded
[contact sheet](evidence/ai-editor/task522/video-contact.png) was inspected for
queued, applying, compiling, running-tests, and completed ordering. The sibling
`phase0.json` through `phase4.json` audits match these five synthetic states and
the displayed 145 ms first-action value. This is fixture evidence only; the live
OpenRouter trace remains outstanding.
