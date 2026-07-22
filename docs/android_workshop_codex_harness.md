# Android Workshop Codex Harness

## Outcome

The primary Workshop AI provider is Codex running entirely on the Android
device. Direct OpenAI API access remains an explicit fallback with one
device-wide monthly USD limit. Desktop pairing is optional and is not required
for normal Workshop use.

This follows the Codex authentication split: ChatGPT sign-in uses subscription
entitlements, while API-key sign-in uses usage-based Platform billing. See
[Codex authentication](https://learn.chatgpt.com/docs/auth).

## Primary boundary

```text
Android Workshop UI
  voice, prompts, approvals, progress
        |
        | JNI / in-process calls
        v
Phone-native Codex Rust components
  ChatGPT device login, token refresh, account limits, agent turns
        |
        | controlled Workshop tool bridge
        v
Stasis project and runtime on the same phone
  symbols, compile, tests, frames, screenshots, diagnostics
```

The phone owns the source, runtime state, credentials, conversations, and tool
execution. It does not expose an app-server listener or copy credentials to
another machine.

## Upstream compatibility audit

The Android build is pinned to official Codex revision
`5c19155cbd93bfa099016e7487259f61669823ff`. The official `codex-login` crate
successfully compiles for `aarch64-linux-android` after disabling desktop
native-TLS defaults in favor of the existing Rustls stack. The repeatable build
is `mobile/android/build_codex_native.ps1`; it fetches that exact revision,
applies the checked-in TLS patch, builds a native library, and packages it into
the Workshop flavor.

The full `codex-app-server` crate does not yet build unchanged for Android. Its
Code Mode dependency requests a prebuilt Android Rusty V8 archive that is not
published for the pinned V8 release. The phone-native integration therefore
uses the official login/account layer plus a narrow authenticated Responses
transport and reuses Workshop's bounded Java tool loop. It does not embed Code
Mode or V8. Code Mode remains gated out on Android rather than replaced with a
fake implementation.

## Authentication

The Workshop calls Codex's official device-code flow from its native Rust
library. It displays the verification URL and one-time code, opens the Android
browser, copies the code to the clipboard before navigation, polls in the
native layer, and stores the resulting `auth.json` under
the application's private files directory. The credentials never leave the
phone. The selectable in-app code and completion status remain visible when
the user returns from the browser. Activity resume performs an immediate status
check, repeated browser visits reuse the same login, and a single lifecycle-aware
poll is active at a time. Continue checks status without dismissing the result,
and transient token-poll network failures retry during the 15-minute login
window. The matching clipboard entry is cleared after successful sign-in. The
app-server protocol documents this frontend-owned flow as
`chatgptDeviceCode`; see the
[Codex app-server documentation](https://learn.chatgpt.com/docs/app-server).

The current first slice provides:

- a phone-native Codex provider presented as the primary path; existing API-key
  installations retain their fallback selection until the turn bridge is ready
- real ChatGPT device-code login using upstream Codex code
- persistent account detection and ChatGPT plan display
- authenticated subscription-backed Codex Responses streaming, current default
  model discovery, and reuse of the existing bounded Workshop edit/test tools
- immediate provider-choice persistence and a one-time signed-in migration from
  the historical API fallback default to the now-functional Codex turn bridge
- direct rough-layout sketching from Context & Images, with explicit
  save-and-attach behavior and queued `design_sketch` intent metadata
- native Codex primary/secondary rate-limit reads using the official
  `usedPercent`, `windowDurationMins`, and `resetsAt` contract
- an explicit OpenAI API-key fallback
- a deterministic pinned-source Android build

## Workshop tool harness

The provider-neutral agent loop, turn limits, and Workshop tool-name catalog live in the shared
Rust `stasis_ai` crate. The Android Codex native build copies that crate beside its pinned upstream
Codex wrapper and exports the versioned contract to Java. Android retains its platform-specific
tool handlers, queue, foreground-service lifecycle, image tools, and richer descriptions; the
desktop TUI uses the same Rust contract with the smaller set supported by the live protocol.

The agent turn layer should expose only controlled Workshop operations, not a
general Android shell:

- list and read symbols
- update source or a symbol
- compile and run tests
- capture the rendered game
- inspect runtime diagnostics
- request explicit approval for consequential changes

The existing Workshop Responses API harness already implements this operation
set and its deterministic edit/compile/test loop. The phone-native Codex client
should adapt those handlers at the tool-call boundary instead of duplicating
their behavior.

The initial cached request includes a source-free `project_symbol_index` with
kind, name, owner, file, and signature. It is bounded
to 256 symbols and 16 KiB, reports truncation, and lets straightforward prompts
read the likely target directly instead of spending the first turn on
`list_symbols`. Full source remains opt-in through `read_symbol`.

The same cached section includes compact `stasis_basics` covering typed function
arguments and returns, struct fields, persistent `global instance: StructType`
state, direct named global blocks, common scalar/array/text types,
receiver-form versus function-form calls, deterministic
`main`/`tick`/`render` lifecycle ownership, limited `on_code_swap` use,
hot-reload layout implications, and real `.test.stasis` test shape. These rules
replace per-symbol derived call suggestions.

Every model response also carries required, user-visible `working_notes`,
bounded to 2,000 characters. The note records concise `Intent`, `Observed`,
`Next`, and `Blocker` facts rather than private chain-of-thought. The latest
valid note is shown in the Workshop status, written to the private bounded trace,
and supplied with retained tool observations on the next otherwise stateless
call. Oversized, missing, empty, or non-string notes fail response validation.

All calls keep the complete original request as one identical cacheable prefix:
the user goal, Stasis basics, symbol index, globals, selected source, full tool
examples, and architecture/game rules. Only cumulative observations, test
results, and working notes follow the boundary. Direct API requests send an
explicit 30-minute cache breakpoint/options. The ChatGPT subscription transport
rejects that API-only field, so Codex subscription requests retain the identical
full prefix and cache key while using implicit caching. The private trace records
exact cacheable characters, an approximate token count, and provider-reported
cached tokens. After a batch writes a behavior test, Workshop
finishes locally when writes compiled and all runnable tests pass instead of
requesting a redundant final model response.

AI Settings also offers an opt-in Fast Codex mode for ChatGPT-signed-in calls.
It requests the model catalog's `priority` service tier while retaining the same
model, reasoning effort, tools, and cacheable request prefix. Standard remains
the default because Fast consumes subscription allowance more quickly. The API
key fallback does not reuse this subscription Fast setting.

The configured GPT-5.6 model applies to both providers. Phone-native Codex
resolves the requested slug against the signed-in account's visible model
catalog and rejects an unavailable model rather than silently substituting the
catalog default.

While the Workshop panel is closed, active and pending AI work appears in a
compact status strip below the performance HUD. It shows queue state, agent
step, action count, phase, and live elapsed time without taking over the game. Tapping the
strip opens Workshop; it hides while the panel is open and when no work is
queued or running.

For host-only API timing, `tools/android_ai_agent_host.py --service-tier
priority` opts into the API's separately billed Priority processing. This is a
useful request/cache latency comparison, but it is not billed against ChatGPT
subscription Fast-mode allowance and is reported separately.

The host runner compiles the selected `--project-root` through the same
`compile_android_workshop_project` bridge used by Android before accepting
writes or final test success. Source and test writes are one atomic batch:
failed tools, compilation, or changed tests restore every touched file.
Follow-up requests retain the initial shared-context bytes so the explicit
cache breakpoint remains stable while only turn state changes. Traces identify
the requested provider/model, the response model, per-model-call and tool-batch
elapsed time, and successful versus rolled-back write counts. Host API traces
and phone-native Codex subscription traces must not be combined as if they were
the same provider or allowance.

Behavior-test expectations in the shared context are request-generic. They
require observable coverage and both sides of changed boundaries without
embedding values from an earlier task. Game geometry guidance also treats
stored positions, rendered rectangles, half extents, collision bounds, wall
bounds, and offscreen transitions as one contract on both host and Android.

Both the Android Workshop and host comparison harness allow up to 25 model
turns per queued AI request. The separate safety cap remains 12 tool calls in
one model response.

Android Workshop now places a verification gate between tested provisional
writes and application. Queue phases persist `editing`, `compiling`, `generated
tests`, `verifying`, `repairing`, and `applying`. Gameplay, structural, and
visual changes receive a separate restricted reviewer call that may author one
temporary `.test.stasis` file but cannot write production source. The native
test total must increase, the temporary file is always removed, and a failed
review test returns bounded evidence to the primary agent for at most two
repair cycles. Inconclusive verification requires an explicit Apply Anyway or
Restore choice.

The project transaction includes all `.stasis` source and test files. It is
persisted app-private against the active queue item, restored on cancellation
or failure, and restored before an interrupted queue item is marked failed on
restart. Successful write observations are reduced to identity, character
count, SHA-256, and results before a follow-up call; failed attempts retain
their complete source for repair.

The first phone-native acceptance run used the shared 20-pixel Pong ball
request. The generated-test audit passed, the first independent temporary test
found an incorrect collision edge, one primary repair cycle ran, and the second
independent test passed. Both temporary files were removed and the transaction
store was empty after application. Total elapsed time was 147.2 seconds; the
trace records two verifier calls, one repair cycle, one failed write batch, and
the final verified result.

Use `tools/run_android_ai_model_comparison.py` to summarize isolated Sol,
Terra, and Luna runs with actual model time, local tool time, token/cache usage,
estimated standard API cost, and a model-independent acceptance suite. Keep
each live model invocation as a separate bounded command; use
`--summarize-only` after all traces exist. The first recorded comparison is in
`docs/android_ai_model_comparison_2026-07-12.md`. Summary rows include the
independent pass ratio, tool batches, schema retries, failed write batches,
restored write counts, and cached-input percentage so a self-authored-test pass
cannot mask incomplete behavior or retry waste.

## Limits

- **Codex subscription:** do not estimate dollars. Display Codex account
  rate-limit data as remaining percentage for the five-hour and weekly windows.
  Refresh after a Codex action with a 30-minute attempt debounce and retain the
  last successful snapshot if a refresh fails.
- **Direct API fallback:** show estimated API cost and enforce the device-wide
  monthly USD limit after every response or image generation.

## Remaining slices

1. Add the minimal upstream Codex model client and thread/turn stream without
   the V8-backed Code Mode component.
2. Adapt the existing Workshop tool handlers to Codex tool calls and verify a
   complete edit-compile-test-screenshot turn on the phone.
3. Add cancellation, approvals, and persisted thread resume.
4. Encrypt or wrap the private Codex credential file with Android Keystore
   protection and add logout/account-switch controls.
5. Retain paired desktop execution only as an optional provider for large
   desktop repositories.
