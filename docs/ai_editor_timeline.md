# Desktop task timeline

The desktop editor follows the task-flow reference in commit `9c6f874e`:
`docs/ai_editor_ux_direction.md` and
`docs/evidence/ai-editor/task-flow-reference.jpg`.

Task navigation stays secondary to the current objective, chronological activity,
and persistent reply composer. The game continues in its independent native
window. Interface sizes are expressed in egui points for display scaling.

## Native window placement

The top-bar **Tile Editor + Game** command restores both windows and places the
editor on the left and the game on the right of the game's usable monitor area.
First launch uses the same arrangement. Normal window rectangles are remembered
per project in `.stasis_cache/editor-windows.json`; minimized and maximized
rectangles do not overwrite normal placement. Saved windows retain their own
connected monitors, and disconnected-monitor positions are clamped into a usable
monitor area. Very small work areas take priority over the preferred editor
minimum of 520 by 600 points.

Third-party tiling window managers can override application window placement.
Set the editor and game windows to floating in that manager before using Tile.

Window operations use the live runtime queue between ticks, without pausing the
game or replacing its input. **Focus game** in the command palette restores a
minimized game and requests native focus, preserving maximized and fullscreen
presentation. Clicking either native window transfers input through the OS.
The renderer remains independent of the editor.

SDL host rectangles include decorations and use platform-native desktop
coordinates (physical pixels on Windows). The editor converts these to egui
points and reapplies sizing after a monitor-scale change. Optional host exports
extend runtime ABI 3; older runtimes report unsupported placement explicitly.
The additive live commands do not change the live envelope schema version.

The native editor enables AccessKit, names task navigation and editing controls,
uses a visible focus outline, and disables transition animation for reduced
motion. Compact layouts keep task creation and the reply composer reachable.

## Serial task queue

The desktop editor runs one task at a time. Creating another task while work is
active stores only its objective in a durable FIFO queue; it does not contact an
AI provider or create conversation history. Provider completion never advances
the queue. The active task remains open until the user explicitly marks it done
after approval and validation, or confirms cancellation.

After that resolution, the next queued objective is shown at a queue gate. The
user can **Start task (Enter)**, **Move to back (B)**, or **Reject (Del)** it;
rejection requires confirmation. Starting is the point where a fresh conversation and first
provider request are created. Queue order and lifecycle survive editor restart,
and legacy sessions with multiple active tasks are normalized to one active task
plus an ordered queue.

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

Provider summaries and stored thread entries allow 16,384 characters. The
structured response schema advertises that same bound, and Stasis defensively
truncates a longer summary if a provider ignores it. Semantic proposal IDs,
descriptions, payload sizes, repair state, and per-response uniqueness are
validated while the provider can still correct a rejected tool call; a malformed
proposal is never acknowledged and then rejected only during task publication.

**Export chat as HTML** in the task header or command palette writes an explicit,
local snapshot of the active task. The standalone page presents the chronological
user, agent, host, attachment, semantic-action, generated-asset, and focused-test
activity as a simple chat. It includes retained semantic diffs and embeds PNG/JPEG
media only after its saved SHA-256 still matches, up to 64 MiB total. Missing,
changed, unverified, or over-budget media is identified in place rather than read
or linked. Text and diffs are HTML-escaped, the page has a network-denying content
security policy, and no provider request or metadata lookup occurs during export.

New activity records a wall-clock timestamp. Completed AI replies also retain
their request duration, input/output tokens, estimated cost, provider, model, and
route; the page reveals these details on hover or keyboard focus. Older saved
tasks remain compatible, but show unavailable for timestamps and per-turn values
that were not historically retained. Aggregate task usage remains visible in the
export header. A manual export is a point-in-time file; export again to include
later task activity.

Projects can opt into quiet automatic snapshots with
`ai.editor.auto_persist_html_transcripts: true` in `stasis.json`. The editor
coalesces task changes off the UI thread and atomically refreshes one stable file
per task under `.stasis_cache/logs/ai-transcripts/`; unchanged tasks are not
rewritten. The cache directory is excluded by the standard project `.gitignore`
entry for `.stasis_cache/`. Automatic snapshots use the same escaping, content
security policy, integrity checks, size limit, and retained-diff sources as the
manual export. Success is silent, repeated failures are rate-limited, and the
existing **Erase history** action removes these automatic transcripts too.

## Validation

Desktop Apply validates the reviewed candidate in a staged workspace using a
bounded child process before publishing source changes. The staged workspace
includes the project's test and data fixtures. Apply rechecks the original
input fingerprint after validation and retains the child's test receipt.
Focused tests also run in a child process: JIT test execution activates
process-global runtime state, so running it inside the editor/game process
would replace the live game's globals. The watcher compiles and commits the
validated edit between ticks, preserving the active runtime state.

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

Every active task exposes `Reject... (Ctrl+Esc)`, including failed or disconnected tasks.
While work is busy, rejection becomes the primary action. The confirmation identifies its
original task; `Keep task (Esc)` dismisses it without stopping work and `Reject task (Enter)`
uses the existing permanent cancellation path. Task switching does not redirect a pending
confirmation. A successful rejection advances to the next queued-task gate, or leaves the
editor ready for a new objective when the queue is empty. Rejection does not silently roll back
source changes that the user already accepted and applied.

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
content and the start of the first object in the root `tool_calls` array at
the same millisecond used in its
usage audit. The required empty `tool_calls: []` field in a done response does
not count as an action; its first-action latency remains unmeasured. These
latencies start at the inference POST, excluding queue,
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

CI follow-up: stroke widths now use explicit `f32` literals, and the two
non-progress convenience wrappers are test-only. Rust 1.97.1 compiled the exact
workspace/all-targets command successfully; its execution stopped with 277
library tests passed and 13 unrelated signing, path, and source-text failures.
The exact Windows `network` filter passed 7 tests, the AI library passed 88, and
the desktop filter passed 76. Formatting and diff checks passed.

Visual evidence: no new media was captured for this correction. Layout is
unchanged; a deterministic SSE test verifies that a done reply with a split,
empty tool-call array emits no first-action event and leaves its audit latency
null. The existing nonempty-action test still checks event/audit timing equality.
Theory gained: a schema-required array key is not an action; the first contained
object is the observable streaming boundary.

Retained merge validation (2026-09-07): `MERGE_HEAD` and `origin/main` both
resolve to `538c19ffc59d95a155dc2896a1c372e76d4debd5`; no unresolved paths
remain. The combined source passes 102 AI library tests and 86 desktop-filtered
binary tests through the Cargo wrapper, formatting, unsafe-boundary, and diff
checks. No test processes remained. The full repository shell gate could not
launch because Bash is unavailable. The opt-in OpenRouter example built and ran,
but rejected execution because `OPENROUTER_API_KEY` is absent. The live trace
acceptance criterion therefore remains unverified; deterministic provider and
host timing tests passed. The prepared merge is preserved for worker publication.

Visual evidence: no new media captured for merge validation; existing synthetic
captures do not satisfy the outstanding live OpenRouter/UI trace requirement.

Exact-base merge repair (2026-09-08): resolved against
`cd4d3d4a4604303885e3bc3412566395988d8bc0`, preserving progress callbacks,
image preflight and one-shot image consumption, provider failure usage audits,
and persistence. History erasure recreates a progress-enabled controller.
Updated progress tests for mutable session admission and persistence fixtures
for bounded host channels and request IDs. No unresolved paths remain.

Validation: 123 AI library tests and 95 desktop tests passed through
`tools/cargo_cache.py`, with a fresh worktree-local build. Native evidence capture
passed. Formatting, unsafe-boundary, and diff checks passed; no lingering test
processes were found. Bash is unavailable, so the full shell gate could not
launch. The opt-in OpenRouter example built but failed explicitly because
`OPENROUTER_API_KEY` is absent; process, user, and machine probes also found no
key. Deterministic tests are the available fallback, not live acceptance evidence.

Visual evidence: `artifacts/task522-merged-progress.png` was captured and
inspected: the focused-tests phase, separate 145 ms provider first-action label,
and image-enabled composer remain readable. Its sibling JSON agrees with those
synthetic values. No new MP4 or live OpenRouter/UI trace was captured.

Theory gained: provider progress and image consumption must share the same
request entry point, including controller recreation after history erasure.
The combined AI, image, and persistence tests support retaining both contracts
when adding another controller lifecycle path.
