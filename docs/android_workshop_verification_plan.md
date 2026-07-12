# Android Workshop AI verification implementation plan

## Outcome

Workshop must not treat model-authored tests as independent proof that an AI
game edit is correct. A queued edit advances through explicit edit, generated
test, independent verification, repair, and apply phases. Production code is
hot-swapped only after the required verification policy passes, or after the
user explicitly accepts an inconclusive result.

## Runtime flow

1. `runAiPatch` claims the queue entry and opens a recovery transaction for the
   selected project.
2. `runAiAgentLoop` performs model calls and provisional tool writes. Compile
   failures and generated-test failures continue to roll back their complete
   write batch.
3. Successful writes plus passing generated tests produce
   `READY_FOR_VERIFICATION`; they no longer directly finalize the queue item.
4. `WorkshopAiVerificationPolicy` classifies the provisional change from the
   prompt and changed symbols:
   - `LOW`: copy, color, asset reference, or one non-behavioral tuning value.
   - `GAMEPLAY`: movement, geometry, collision, score, health, timers, or input.
   - `STRUCTURAL`: structs, globals, initialization, reset, lifecycle, or swap.
   - `VISUAL`: render commands, sprite geometry, camera, or layout.
5. `WorkshopAiVerificationRunner` runs request-independent checks in a
   temporary test overlay. Low-risk edits use compile plus existing/generated
   tests. Gameplay and structural edits require boundary/transition coverage.
   Visual edits also capture a logical render snapshot; pixel screenshots are
   reserved for appearance checks that cannot be answered logically.
6. A failed verification result returns compact evidence to the primary agent
   for repair. Verification is rerun after the repaired batch. At most two
   verify/repair cycles are allowed.
7. A passing result reaches `APPLYING`, then `applyAiCodeResponse` performs the
   normal commit/hot-swap path. Failed or cancelled verification restores the
   recovery transaction. An inconclusive result requires an explicit user
   choice before application.

## Independent checks

The first implementation is deterministic and local. It validates that a
behavior-changing request added or updated a runnable `.test.stasis` file and
that the tests exercise observable update/render behavior. Geometry and
threshold requests must cover just-inside, exact-boundary, and just-outside
values. Lifecycle requests must cover initial, transitioned, and reset state.

The runner records a structured result with policy, checks passed/total,
failure evidence, elapsed time, temporary files, and whether a model reviewer
was needed. Temporary verifier files are removed in `finally` and never become
project assets.

## Conditional reviewer

A separate verifier model is used only when deterministic checks are
inconclusive, multiple gameplay subsystems changed, or the user selected
thorough verification. It receives the original request, compact changed-symbol
summaries, generated-test results, and relevant logical snapshots. Its tools
are restricted to reads, temporary verification-test writes, tests, runtime
probes, and screenshots; it cannot edit production source. Use Sol where the
provider exposes it, otherwise the selected model. Limit the reviewer to two
turns and keep its provider/usage separate in traces and UI.

## Context and caching

The original stable request remains byte-stable. Follow-up observations for a
successful write contain symbol/file identity, before/after hashes, compile and
test status, and a short diagnostic. Full `new_source` is retained only for a
failed attempt that needs repair. Verifier context has its own stable cacheable
prefix. Codex subscription usage is never converted to dollars; API fallback
continues to use official per-token estimates and the device monthly limit.

## Android UI

The queue and collapsed in-game strip use persisted phases:

`queued -> preparing -> editing -> compiling -> generated tests -> verifying
-> repairing -> applying -> verified`

The expanded status shows primary turns, total actions, generated-test status,
independent verification ratio, repair cycles, failed write batches, restored
writes, provider/model, elapsed time, and provider-appropriate limits/cost.
Terminal outcomes are `verified and applied`, `applied with verification
warning`, `verification failed; restored`, and `cancelled; restored`.

## Implementation slices

### Slice 1: state and gate

- Add `WorkshopAiRunPhase`, `WorkshopAiVerificationPolicy`, and unit tests.
- Extend `AiAgentResult` with generated-test and changed-symbol evidence.
- Replace the current automatic-finalize return with a ready-for-verification
  result and run verification before `applyAiCodeResponse`.
- Trace and display verification phases without changing the current edit
  protocol.

### Slice 2: temporary verification runner

- Add a temporary `.test.stasis` overlay with guaranteed cleanup.
- Add deterministic coverage checks for gameplay geometry, thresholds,
  lifecycle/reset, input, and logical render output.
- Return exact check ratios and failure evidence to the repair loop.

### Slice 3: transaction and resume

- Persist phase and verification summary with the active queue entry.
- Restore all provisional files on cancellation, terminal verification failure,
  or incompatible restart.
- Resume safe verification after an ordinary activity recreation without
  replaying successful model calls.

### Slice 4: conditional reviewer

- Add the restricted verifier tool contract and two-turn loop.
- Invoke it only for inconclusive/high-risk/thorough cases.
- Feed failures back to the primary repair loop; never allow verifier source
  writes.

### Slice 5: compact observations and instrumentation

- Compact successful write observations and preserve full failure evidence.
- Record model time, tool time, verification time, cache usage, schema retries,
  failed batches, restored writes, and reviewer calls.
- Show phase and verification pills in the collapsed and expanded UI.

### Slice 6: shared acceptance fixtures

- Use the same under-tested Pong geometry fixture in the host harness and
  Android unit/integration tests.
- Add lifecycle, input, and visual fixtures as separate bounded cases.
- Keep each live model invocation under 300 seconds and report single-trial
  timing as a sample rather than a stable average.

## Completion gates

- The Pong edit whose self-authored tests pass but whose paddle/offscreen
  behavior is wrong is rejected or repaired before application.
- No production hot swap occurs before required verification completes.
- Temporary verifier tests are always removed.
- Cancellation and restart either resume a safe phase or restore the complete
  transaction.
- Low-risk requests do not incur a verifier model call.
- Schema retries remain zero in the standard comparison.
- Local compile/test/verification remains below two seconds on the benchmark
  project, with model latency reported separately.

## Implementation status

Implemented on `codex/android-workshop-next`:

- Typed, persisted queue phases and verification metrics in the expanded panel
  and collapsed in-game HUD.
- A pre-apply verification gate with explicit verified, inconclusive, and
  failed outcomes.
- Request-local risk classification for low, gameplay, structural, and visual
  changes.
- Deterministic generated-test auditing for observable calls and boundary-case
  evidence.
- A restricted independent reviewer that can author one temporary test but
  cannot edit production source; API review uses Sol and Codex review stays on
  the signed-in subscription transport.
- A two-call reviewer ceiling and at most two primary repair cycles using exact
  failed-test evidence.
- Temporary test discovery proof and guaranteed cleanup.
- Atomic source/test transaction capture, app-private persistence per queue
  item, restore on cancellation/failure, and restore-before-fail on process
  restart.
- Compacted successful write observations using source length and SHA-256 while
  retaining full failed attempts for repair.
- Trace fields for verification ratio, reviewer calls, repair cycles, failed
  write batches, and restored writes.

Device acceptance completed on a Galaxy S21 over wireless ADB:

- The shared Pong request reached the restricted verifier at primary step 5
  after 12 tool actions.
- Generated-test audit passed 3/3 checks with observable behavior, five
  comparisons, and 18 numeric cases.
- The first temporary verifier test failed, was removed, and returned exact
  evidence to the primary agent.
- One repair cycle ran; the second temporary verifier test passed as the fifth
  native test and was removed.
- No transaction or temporary verification file remained after application.
- End-to-end elapsed time was 147.2 seconds; local compile/test work remained a
  small fraction of the two primary/reviewer model round trips.

The device run exposed and prompted fixes for verification ratio accounting,
the final `verified` phase label, live queue-row phase refresh, and keyboard
dismissal when work starts. Process-kill recovery remains covered by the atomic
transaction unit tests and should be exercised again during broader queue and
cancellation acceptance.
