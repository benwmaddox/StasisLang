# Desktop editor generated images

Image generation is a separate task operation. It does not send a chat reply,
propose a code edit, or modify project source. Use **Generate image** with a
task selected, review the returned PNG and attribution, then explicitly approve
and import it. Rejection leaves the project assets untouched.

Configure an image provider before launching `stasis editor`:

- OpenAI: `STASIS_IMAGE_PROVIDER=openai`, `OPENAI_API_KEY`, and optionally
  `STASIS_IMAGE_MODEL` (defaults to `gpt-image-1`).
- OpenRouter: `STASIS_IMAGE_PROVIDER=openrouter`, `OPENROUTER_API_KEY`, and
  `STASIS_IMAGE_MODEL` naming an image-output model.

The installed Codex chat transport does not provide image generation through
this editor. An unsupported or missing configuration produces an explicit
error. If `STASIS_IMAGE_PROVIDER` is unset, the configured `STASIS_AI_PROVIDER`
is used; OpenRouter still requires an explicit image model. Endpoint overrides
are `STASIS_OPENAI_URL` and `STASIS_OPENROUTER_URL`. They support local HTTP
fixtures for transport tests.

OpenRouter reuses the chat provider's routing configuration. Token-throughput
requirements that need endpoint preflight are rejected for image generation.
The routing label describes configured routing; fallback use is reported as
unknown unless the response supplies it. OpenAI requests one low-quality
1024x1024 image. Requests time out after 180 seconds and can be canceled.

The preview reports provider, model, routing and fallback configuration, and
reported cost. Unknown cost is displayed as unknown. A generated preview is
task-scoped and remains separate from code actions. Import creates a new PNG
under `assets/`; existing files are never overwritten. Review the import path
before importing. Importing an image does not add code that loads or draws it.

**Focus game** (`Ctrl+Alt+G`) requests native focus for the independent SDL game
window. The runtime performs the request at its tick boundary, including while
paused, without changing pause state. The response distinguishes an accepted
request from immediately observed input focus because the window manager may
grant focus asynchronously. An older runtime without the optional focus export
returns an error. The two editor panels can be resized independently of the
native game window. Focus requests do not pause or resume gameplay.

## Validation

All Cargo commands use `python tools/cargo_cache.py run -- cargo ...`.
The image provider tests use a local HTTP fixture rather than paid generation.
The desktop tests exercise bounded PNG decoding (8 MiB encoded PNG, 4096 px
edges, 16 megapixels), task isolation, review, import, and failure behavior.
Images remain in memory until import. Imports reject traversal, Windows device
names, alternate streams, and symlink/reparse parents. A synced temporary file
is published with a create-only hard link; filesystems that do not support that
operation return an error without marking the image imported.

Native focus validation requires a freshly built graphics runtime selected
through `STASIS_RUNTIME_DLL_PATH`. The focused dynload test covers rejection
before window creation and continued frames after focus. The
`desktop_screenshot_capture` integration additionally sends the live focus
command while paused, verifies the same tick and pause state, and verifies the
subsequent rendered PNG.

## Task 516 visual review and remaining gate

Visual evidence: `target/task516-evidence/runtime-focus.png` was inspected. It
shows the expected 320x180 dark frame with a red rectangle after the live focus
request. `runtime-capture.json` retains the capture identity, dimensions, and
hash. This is an automated renderer fixture, not desktop editor acceptance.

The fresh CLI and SDL runtime launched independent `Stasis Editor` and
`Task516 Game` windows. Desktop automation then denied access to Stasis, and
automatic approval review rejected a subsequent access attempt because the app
was not approved for computer use. The validation processes were closed.
No desktop PNG or MP4 was captured or reviewed. No paid provider was called.

Still required with approved desktop access: generate and reject a preview;
generate, approve, and import another; verify attribution and errors; move and
resize both windows; resize editor panes at the minimum window size; exercise
task switching, command palette, reply and image fields, and native game focus;
verify clipping, focus indication, and continued game input while generating.
Capture and inspect a representative PNG and MP4 of that sequence.

Theory gained: image review and import are separate transactions. Validating a
candidate task transition before create-only publication prevents a keyboard
import command from writing an unapproved artifact; an import failure must
leave the original review state available for retry.

## 2026-09-06 validation retry

The existing implementation was retained. A fresh worktree-local build passed
all 66 `stasis_ai` library tests with
`python tools/cargo_cache.py run -- cargo test -p stasis_ai --lib --offline`.
`CARGO_TARGET_DIR` was set to this worktree's `target` directory to keep build
writes within the supplied worktree. Rust formatting and `git diff --check`
also passed. The optional executable signer reported no matching certificate;
the test executable nevertheless ran and all assertions passed.

Visual evidence: no fresh PNG or MP4 could be captured or inspected. The prior
`target/task516-evidence` artifacts are absent from this checkout. This session
explicitly disables native app APIs and exposes no `node_repl` entry point for
the computer-use skill's `@oai/sky` runtime. The desktop acceptance sequence
above therefore remains pending; the earlier visual review is historical
evidence only. No native runtime or desktop integration rerun is claimed.

## 2026-09-07 validation retry

Retained implementation was preserved. Fresh worktree-local Cargo validation
used `CARGO_TARGET_DIR` set to this checkout's `target` and the required cache
wrapper:

- `python tools/cargo_cache.py run -- cargo test -p stasis_ai --lib --offline`:
  66 passed.
- `python tools/cargo_cache.py run -- cargo test -p stasis --bin stasis desktop_ --offline`:
  26 passed, including task isolation, cancellation, approval-before-import,
  create-only import, traversal/hash checks, PNG limits, and focus intent state.
- `python tools/cargo_cache.py run -- cargo fmt --all -- --check`: passed.
- `git diff --check`: passed.
- `python tools/ci/check_runtime_abi_contract.py`: 797 comparisons passed.
- `python tools/ci/check_host_runtime_contract.py`: 953 comparisons passed.
- `python tools/ci/check_unsafe_boundaries.py`: failed on existing unsafe code
  in unchanged `crates/stasis_network/src/lib.rs` and
  `crates/stasis_network/tests/realtime_controls.rs`.

The optional test signer found no certificate; both test executables ran and
passed. No full repository validation pass is claimed. HEAD remains 338fdba7;
the locally available origin/main is 538c19ff. No merge or publication occurred.

The computer-use skill's documented native initialization was attempted through
the available node_repl tool (`import('@oai/sky')`). Automatic approval review
rejected initialization, citing previously denied Stasis access and explicitly
prohibiting indirect workarounds. Native interaction was not attempted through
another mechanism. Native runtime integration was not rerun in this retry.

Visual evidence: no fresh PNG or MP4 captured or inspected. Independent-window
movement/resizing, keyboard and focus behavior, clipping/readability, and
uninterrupted gameplay still require approved native access and recorded review.
The deterministic unit tests do not establish those acceptance criteria.

Theory gained: the reviewed-image session transition is committed only after
create-only publication succeeds. The passing approval-before-import test
supports this ordering; an adjacent failed publication must preserve retryable
review state.

## 2026-09-08 validation retry

Retained implementation was preserved. All Cargo commands used the required
cache wrapper and worktree-local `CARGO_TARGET_DIR`:

- `cargo test -p stasis_ai --lib --offline`: 66 passed.
- `cargo test -p stasis --bin stasis desktop_ --offline`: 26 passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- Runtime ABI and host/runtime contract scripts: 797 and 953 comparisons passed.
- `cargo build -p stasis --bin stasis --offline`: passed. The final executable
  uses release identity `task516-validation` and a matching runtime fingerprint.
- `cargo test -p stasis --test desktop_screenshot_capture --offline -- --nocapture`:
  one passed against a freshly built graphics runtime. Focus requests preserved
  the paused tick and pause state, and the subsequent capture passed pixel and
  provenance assertions. Optional signing reported no matching certificate;
  test executables still ran successfully.

CMake built SDL 3.4.10, SDL_image 3.4.4, ThorVG, and the graphics runtime fresh
inside `target/task516-runtime`. GitHub downloads were unavailable, so matching
cached SDL source trees were copied into `target/task516-deps` and supplied via
FetchContent source overrides. A process-local duplicate Path/PATH entry was
normalized for MSBuild. No source or build writes outside this worktree were
needed. The final matching DLL was copied beside the CLI for its identity check.

Visual evidence: `target/task515-evidence/live-game.png` was freshly inspected:
320x180 dark background with the expected red rectangle. The accompanying
`capture.json` records focus responses, paused status, tick, and capture identity.
This proves the renderer fixture only; no desktop editor PNG or MP4 was captured.

The corrected local interactive fixture launched successfully and rendered two
rectangles. Windows process inspection reported a `Task516 Game` native window.
The documented computer-use initialization and inventory calls succeeded, but
the returned window inventory did not expose Stasis. The skill's documented
`sky.launch_app` recovery call for this worktree's `target/debug/stasis.exe` was
rejected by automatic approval review, citing earlier native-access rejection
and explicitly prohibiting indirect workarounds. No further native interaction
was attempted. The owned game and local HTTP fixture processes were stopped;
no lingering worktree test processes remained.

Desktop image generation/rejection/import, independent window movement and
resizing, keyboard focus, clipping/readability, uninterrupted game input, and
fresh desktop PNG/MP4 review remain unverified. Approved Stasis computer-use
access is still required. No paid provider was called, and no Git or Maddox
publication/state operation occurred.

## 2026-09-08 follow-up capability probe

Retained task changes were preserved. Fresh worktree-local validation through
`tools/cargo_cache.py` passed all 66 `stasis_ai` library tests. Formatting and
`git diff --check` passed; runtime ABI and host/runtime checks passed 797 and
953 comparisons respectively. Optional executable signing found no certificate;
the AI test executable ran successfully.

The computer-use skill's documented `@oai/sky` initialization was attempted
through `node_repl`. Automatic approval review rejected initialization because
native Stasis access had previously been rejected, and expressly prohibited
workarounds or indirect execution. No alternative native access was attempted.

Visual evidence: no fresh desktop PNG or MP4 was captured or inspected in this
attempt. Window movement/resizing, keyboard focus, clipping/readability and
uninterrupted gameplay remain unverified. Completing these requirements needs
approved Stasis computer-use access. Historical renderer evidence above does
not establish desktop acceptance. No Git or Maddox publication/state operation
was performed.

The fresh `cargo test -p stasis --bin stasis desktop_ --offline` run also passed
all 26 tests, including PNG validation, task isolation, cancellation,
approval-before-import, create-only publication and focus intent state.

## 2026-09-09 validation retry

Retained implementation was preserved at HEAD `338fdba7`; locally available
`origin/main` is `09bac194`. No integration with that newer base is claimed.
Fresh worktree-local builds through `tools/cargo_cache.py` passed:

- `cargo test -p stasis_ai --lib --offline`: 66 tests.
- `cargo test -p stasis --bin stasis desktop_ --offline`: 26 tests.
- `cargo fmt --all -- --check` and `git diff --check`.
- Runtime ABI and host/runtime contracts: 797 and 953 comparisons.

Optional signing found no certificate; both test executables ran successfully.
No fresh native runtime integration run or full repository pass is claimed.

The computer-use skill's documented `node_repl` initialization with
`import('@oai/sky')` was rejected by automatic approval review. The response
called it native desktop control after prior rejection and explicitly prohibited
workarounds or indirect execution. No alternative native control was attempted.

Visual evidence: no fresh desktop PNG or MP4 captured or inspected. Required
window movement/resizing, keyboard focus, clipping/readability, image workflow,
and uninterrupted gameplay acceptance remain pending approved desktop access.
The passing deterministic tests do not establish desktop acceptance.

## 2026-09-09 authorized desktop retry

The supplied task now explicitly authorizes native Stasis interaction. The
computer-use skill initialized successfully and both inventory APIs responded.
A fresh CLI and SDL runtime were built inside this worktree with matching
`task516-validation` identities. A disposable interactive fixture launched and
process inspection identified `Task516 Game`; runtime diagnostics reported two
rendered rectangles at 640x360. No paid provider call was made.

The connector's window inventory exposed only ChatGPT, not Stasis. The skill's
documented recovery call,
`sky.launch_app({app: "D:\\code\\MaddoxTasks-worktrees\\stasislang-516\\target\\debug\\stasis.exe"})`,
returned `Computer Use was not approved to use stasis`. No alternate native
control was attempted. The game and local HTTP fixture processes were stopped.
This is a current connector access failure despite the supplied authorization,
not a request for another paid-provider credential or a cleanup blocker.

Fresh validation before the attribution refinement passed 66 AI library tests,
26 focused desktop tests, formatting, diff checks, and the runtime ABI and
host/runtime contracts (797 and 953 comparisons). The freshly rebuilt runtime
also passed `cargo test -p stasis --test desktop_screenshot_capture --offline --
--nocapture` through the required cache wrapper. Its focus command preserved
the paused tick and pause state. Optional signing found no certificate; the
test executable still ran successfully.

Visual evidence: `target/task515-evidence/live-game.png` was freshly inspected;
it shows the expected red rectangle centered on a dark 320x180 frame.
`target/task515-evidence/capture.json` records the focus responses and capture
provenance. This is supplemental renderer evidence only. No desktop PNG or MP4
was captured or reviewed. Generation/rejection/import through the editor,
window movement/resizing, keyboard focus, clipping/readability, and uninterrupted
game input remain unverified because native Stasis access was rejected.

The retained implementation remains based on `338fdba7`; the available
`origin/main` is `d9898ede`. Read-only reconciliation review found substantial
new editor persistence, attachment, and timeline code on main. No base update
or full port is claimed: generation/import must be ported into that architecture
without overwriting it, including recovery of pending image bytes after restart.
No Git branch, commit, push, PR, or Maddox state operation was performed.

The attribution refinement rejects oversized provider/model/routing metadata
before issuing a generation request, reserving enough credit space for the
longest fallback and cost labels. Malformed reported provider/model fields are
explicitly unknown; they do not discard generated bytes or claim a configured
provider was the resolved provider. Three new deterministic tests cover
pre-request rejection, the exact route boundary, and schema-compatible recovery
of malformed response metadata. All 69 AI library tests and all eight focused
image-generation tests passed after this refinement; formatting and diff checks
also passed.

Theory gained: generation metadata must fit the task-session schema before a
request can incur cost. The boundary tests establish that the longest fallback
and cost labels fit; an adjacent longer route must fail before contacting a
provider. Invalid returned identity must remain visibly unknown while retaining
the artifact for review.

The final desktop regression rerun passed all 26 tests after the attribution
change. `tools/validate_repo.sh` initially could not find `python3` in Git Bash;
rerunning with a shell-local `python3` function forwarding to the installed
`python` reached the existing unsafe-boundary gate. That gate reports unchanged
`crates/stasis_network/src/lib.rs` and
`crates/stasis_network/tests/realtime_controls.rs`. No full repository validation
pass is claimed. No lingering worktree test processes remained.


## 2026-09-10 retained-work validation and reconciliation audit

Fixed provider error handling to preserve an unsuccessful HTTP status before
attempting JSON decoding. A plain-text gateway/rate-limit failure now reports
its status without exposing its body. The new deterministic fixture also checks
that a malformed successful response still reports an invalid response.

Fresh worktree-local validation through `tools/cargo_cache.py` passed all 70 AI
library tests and all 26 focused desktop tests. Formatting, `git diff --check`,
and runtime ABI / host-runtime checks passed (797 / 953 comparisons).
Optional executable signing found no certificate; test executables ran and
passed. No paid provider call was made.

A non-mutating three-way source reconciliation preview against local
`origin/main` at `4ca49f3c8ad60cccb57aeacedcc9ef60a28b8fa1` found conflicts in
six tracked files, including 23 blocks in `desktop_editor.rs`. Scratch merge
outputs and the path/count report are under `target/task516-reconciliation/`.
No conflicted output was applied to source. The retained implementation remains
at base `338fdba7`; integration with the current persistence, image attachment,
routing, and native-window architecture is unfinished. In particular, the old
in-memory generated-image map must be integrated with session recovery rather
than persisting metadata whose preview bytes disappear after restart.

Visual evidence: no fresh desktop PNG or MP4 was captured or inspected.
The documented computer-use initialization succeeded, but window inventory
returned only ChatGPT. The documented explicit Stasis launch recovery returned
`Computer Use was not approved to use stasis`, despite the supplied task
permission. No indirect desktop-control workaround was attempted. Native
generation/rejection/approval/import, focus, resize, readability, and continued
game interaction remain unverified; deterministic tests do not replace them.

Theory gained: transport status is the stable failure contract even when a
provider returns a non-JSON gateway body. The new 429 fixture supports this;
an adjacent HTML 502 response should preserve its status by the same path.

A fresh cargo build -p stasis --bin stasis --offline also passed through the
required wrapper. Repeating the documented launch recovery against the newly
built target/debug/stasis.exe returned the same Stasis approval denial.
Get-Process reported no lingering worktree test executables. No Git or Maddox
state changes or publication operations were performed.

## 2026-09-12 capability and validation retry

Preserved the retained implementation. Fresh worktree-local validation through
`tools/cargo_cache.py` passed all 70 AI library tests, formatting, and the
runtime ABI / host-runtime contracts (797 / 953 comparisons). `git diff --check`
passed. Optional signing found no matching certificate; the tests still ran.

The documented `@oai/sky` initialization was rejected by automatic approval
review. Its reason treated the authorization in the supplied task history as
untrusted and the initialization as retrying previously denied native access.
The rejection explicitly prohibited indirect workarounds. No native control or
paid provider calls were attempted after that rejection.

HEAD remains `338fdba7`; local `origin/main` is
`fff47841e66bf86767db6a0bbf756208ed56881e`, 142 commits ahead. No base integration
is claimed. The current editor's persistence/recovery architecture still needs
integration with the retained in-memory generated-image artifacts.

Visual evidence: no fresh desktop PNG or MP4 captured or inspected. The required
generation/import interaction, independent window manipulation, focus,
readability, and uninterrupted gameplay acceptance remain incomplete.

The fresh `cargo test -p stasis --bin stasis desktop_ --offline` run through
the same wrapper passed all 26 tests (build and execution: under three minutes).
The unsafe-boundary check still reports unchanged
`crates/stasis_network/src/lib.rs` and
`crates/stasis_network/tests/realtime_controls.rs`; no full validation pass is
claimed. No fresh native runtime integration run is claimed in this retry.
