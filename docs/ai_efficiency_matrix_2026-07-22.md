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
| Built-in Stasis AI | threshold guidance only | medium | 4/4 | 294.1 | 0.1 | 294.3 | 251,328 | 22,016 | 14,575 | 1.595 |
| Built-in Stasis AI | targeted initial context + threshold guidance | medium | 4/4 | 265.7 | 0.1 | 265.8 | 225,996 | 92,672 | 12,403 | 1.085 |

## Retained instruction changes

The first generalist refinement reduced time, tokens, and unrelated file churn but remained
incorrect, so it was not sufficient by itself. Explicit equality and adjacent-value tests for
every changed inequality raised hidden acceptance from 3/4 to 4/4 on both small and medium
projects. The extra small-project verification cost is retained because correctness is the primary
gate.

The guide now also uses `stasis fmt --check` instead of mutating whole-project formatting. In the
original run, `stasis fmt` rewrote comment-only placeholder files unrelated to the request. The
refined runs left those files unchanged and required a final changed-file audit.

## Retained built-in changes

The original built-in run was fast but missed paddle-contact and scoring boundaries. Explicit
geometry and public-update threshold guidance restored 4/4 acceptance. When the default starting
inventory is truncated, adding at most 32 deduplicated prompt-matched symbols to initial context
then reduced that correct run from 294.1 to 265.7 seconds, from 63 to 42 tool calls, and from an
estimated $1.595 to $1.085. The refined run reported 92,672 cached input tokens (41.0% of input),
so there is no cache regression in this comparison.

Correctness is the primary gate. The ten-minute cap is a stability ceiling, not a reason to retain
a faster incorrect result. Small and large fixture runs remain the next regression checks.
