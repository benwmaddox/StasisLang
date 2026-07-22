# Generalist and built-in AI efficiency matrix

Date: 2026-07-22

## Method

Every run used `gpt-5.6-sol` with medium reasoning and a fresh Git-initialized copy of the same
Pong project. The task was:

> make the ball 20 pixels square and keep it centered at its position and update collision behavior and tests

The small project contains the ten baseline Stasis files. The medium project adds eight directly
imported files containing 200 unrelated functions. The large fixture adds 32 files and 1,600
unrelated functions; it is retained in the harness for the next provider iteration. A hidden
four-test acceptance file is copied in only after the agent exits. Each agent run is capped at 300
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
provider change is accepted unless it preserves the small pass and fixes the medium failure within
the same five-minute bound.
