# Generalist and built-in AI efficiency matrix

Date: 2026-07-22

## Method

Every run used `gpt-5.6-sol` with medium reasoning and a fresh Git-initialized copy of the same
Pong project. The task was:

> make the ball 20 pixels square and keep it centered at its position and update collision behavior and tests

The small project contains the ten baseline Stasis files. The medium project adds eight directly
imported files containing 200 unrelated functions. The large fixture adds 32 files and 1,600
unrelated functions; it is retained in the harness for the next provider iteration. A hidden
four-test acceptance file is copied in only after the agent exits. Each agent run is capped at 600
seconds. Cached input is costed at 10% of uncached input.

These are single controlled trials, so time and token totals are directional rather than variance
estimates.

## Results

| Approach | Instructions | Size | Hidden acceptance | Agent s | Acceptance s | Total s | Input | Cached | Output | Est. USD |
|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Generalist | original #319 | small | 3/4 | 225.9 | 0.1 | 226.0 | 699,426 | 648,192 | 9,642 | 0.870 |
| Generalist | non-mutating format + geometry checks | small | 3/4 | 211.6 | 0.1 | 211.7 | 633,800 | 579,328 | 8,188 | 0.808 |
| Generalist | explicit equality and adjacent threshold checks | small | 4/4 | 278.0 | 0.1 | 278.1 | 782,492 | 727,040 | 10,493 | 0.956 |
| Generalist | explicit equality and adjacent threshold checks | medium | 4/4 | 201.0 | 0.1 | 201.1 | 652,734 | 596,480 | 8,670 | 0.840 |
| Generalist | equality checks, one-word discovery | large | 4/4 | 261.1 | 0.1 | 261.2 | 969,217 | 907,008 | 10,163 | 1.069 |
| Generalist | consolidated file-scoped discovery | large | 4/4 | 204.1 | 0.1 | 204.2 | 685,771 | 623,872 | 8,368 | 0.872 |
| Built-in Stasis AI | current | small | 4/4 | 182.1 | 0.1 | 182.2 | 139,196 | 0 | 8,757 | 0.959 |
| Built-in Stasis AI | current | medium | 2/4 | 127.2 | 0.1 | 127.3 | 140,125 | 12,032 | 5,726 | 0.818 |
| Built-in Stasis AI | fair explicit contract | medium | 3/4 | 263.1 | 0.1 | 263.2 | 186,548 | 40,192 | 12,965 | 1.141 |
| Built-in Stasis AI | fair contract + transition-aware guidance | medium | 3/4 | 414.8 | 0.1 | 414.9 | 314,465 | 47,104 | 20,753 | 1.983 |

## Retained instruction changes

The first generalist refinement reduced time, tokens, and unrelated file churn but remained
incorrect, so it was not sufficient by itself. Explicit equality and adjacent-value tests for
every changed inequality raised hidden acceptance from 3/4 to 4/4 on both small and medium
projects. The extra small-project verification cost is retained because correctness is the primary
gate.

The guide now also uses `stasis fmt --check` instead of mutating whole-project formatting. In the
original run, `stasis fmt` rewrote comment-only placeholder files unrelated to the request. The
refined runs left those files unchanged and required a final changed-file audit.

On the large fixture, replacing a series of one-word queries with one file-scoped function
inventory preserved 4/4 acceptance while reducing agent time by 57.0 seconds, actions from 52 to
34, input tokens by 29.2%, and estimated weighted cost by 18.4%. The trace also eliminated an
invalid `--kind global` attempt by documenting the exact `globals` spelling.

## Fair-contract rerun

The revised prompt explicitly states every hidden acceptance behavior, including inclusive paddle
contact and strict full-exit scoring. With that contract, the retained generalist workflow passed
4/4 on medium in 293.1 seconds using 47 actions, 1,097,919 input tokens, and an estimated $1.292.
A single refinement that discouraged repeated validation and generated-output inspection also
passed 4/4, but regressed to 329.7 seconds, 48 actions, and an estimated $1.298, so the instruction
was not retained.

A final generalist attempt added a narrow stop condition after atomic edit validation and the final
diff audit. It still passed 4/4 and improved slightly to 288.4 seconds and 46 actions, but input
rose to 1,336,968 tokens and estimated weighted cost regressed to $1.333. The stop instruction was
therefore not retained; the existing guide remains the better overall-cost configuration.

## Next provider target

Built-in Stasis AI remains faster and consumes far fewer raw input tokens, but its medium result
missed paddle-contact and scoring-boundary behavior. The next iteration will use the trace to
improve test discovery and threshold coverage, then rerun small, medium, and large fixtures. No
provider change is accepted unless it preserves the small pass and fixes the medium failure.
Correctness is the primary gate; the ten-minute process cap is only a stability ceiling.

Two refinements were rejected after cross-size testing. Targeted initial context plus general
threshold guidance passed medium 4/4 in 265.7 seconds but failed the strict full-exit scoring case
on small. Adding strict full-exit guidance passed small 4/4 in 170.1 seconds but failed inclusive
rectangle contact on medium in 239.1 seconds. The traces identify the next candidate rule as
inclusive rectangle contact paired with strict full-exit scoring, but it was not added because the
two-iteration limit was reached.

The fair rerun stated every hidden behavior directly in the user prompt. The current built-in
workflow improved to 3/4 but retained an old paddle-center argument instead of deriving the center
from the rendered paddle rectangle. One generic refinement required public-path tests to account
for earlier state transitions and derive boundaries from observed output. It failed the same case
while increasing time, tools, tokens, and cost, so it was reverted. No built-in prompt change from
this rerun was retained.

A final built-in attempt added bounded scalar initial state to runtime validation so transition
boundaries could be exercised before and after an edit. Deterministic tool, fresh-runtime, CLI, and
red/green replay tests passed, but the model used the capability only for render validation. The
task still scored 3/4 with the same paddle-center failure, while rising to 342.4 seconds, 50 tool
calls, 308,626 input tokens, and an estimated $1.849. The capability and its prompt instruction
were therefore reverted rather than adding unproven surface area.

## Tested-write completion

The next refinement removed runtime-state validation from the built-in AI contract. Human
`stasis validate` and TUI `:validate` remain available, but AI writes now compile and run project
tests atomically, and the agent cannot report completion without a successful write batch. A
separate live `run_tests` tool was also removed because it only repeated the result already
returned by the write. Repeated failures from the same atomic batch now include full diagnostics
once instead of duplicating them for every write observation.

The first lean run passed medium 4/4 in 201.9 seconds with 38 tool calls, 160,967 input tokens,
25,344 cached input tokens, 9,540 output tokens, and an estimated $0.977. Its first small run was
faster and cheaper but failed one paddle-contact case, so that configuration was rejected. The
trace showed that the model copied an old collision constant instead of deriving contact from the
rendered rectangle after movement.

A generic game-geometry instruction corrected that reasoning: collision and geometry changes use
rendered rectangle bounds as the coordinate source of truth and derive test inputs after the
update function's movement order. The retained configuration passed 4/4 at every scale:

| Size | Total s | Tool calls | Input | Cached | Output | Est. USD |
|---|---:|---:|---:|---:|---:|---:|
| small | 255.6 | 31 | 131,859 | 22,016 | 12,972 | 0.949 |
| medium | 262.6 | 41 | 183,149 | 30,208 | 12,620 | 1.158 |
| large | 204.7 | 40 | 224,678 | 32,000 | 9,551 | 1.266 |

These single-run timings vary enough that the change is not claimed as a universal speedup. It is
retained because it removes an ineffective validation loop, keeps all acceptance checks green,
and makes the mandatory completion path one tested atomic edit rather than an edit plus a second
synthetic runtime protocol.

## Medium phase timing sample

A subsequent single medium-project run added out-of-band event timing without changing the model
payload. It passed 4/4 in 274.1 seconds with 43 tool calls. Provider waits consumed 271.6 seconds
(99.2% of the traced AI action); initial context and every local tool batch together consumed 2.3
seconds. The independent acceptance run took another 0.1 seconds.

| Phase | Time |
|---|---:|
| Initial symbol context | 0.4s |
| Turn 1: choose narrow discovery queries | 13.1s |
| Five local symbol searches | 0.2s |
| Turn 2: choose source/reference batch | 16.4s |
| Sixteen local reads and reference lookups | 0.8s |
| Turn 3: identify remaining test/state context | 13.8s |
| Seven local context reads | 0.3s |
| Turn 4: design the source and boundary-test batch | 124.7s |
| Atomic compile/tests, failed pre-existing test | 0.3s |
| Turn 5: diagnose failure and prepare retry | 96.7s |
| Atomic compile/tests, successful retry | 0.3s |
| Turn 6: final summary | 6.9s |
| Independent four-test acceptance | 0.1s |

This sample shows that making local validation faster cannot materially approach a one-minute
task by itself. The largest opportunity is reducing or bounding the two long edit/repair provider
turns while preserving first-pass accuracy.
