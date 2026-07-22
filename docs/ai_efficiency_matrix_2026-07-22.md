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
