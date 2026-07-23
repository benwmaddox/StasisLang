# Android Workshop Grok 4.5 OpenRouter trial (2026-07-22)

## Method

- Model: `x-ai/grok-4.5` through OpenRouter.
- Prompt: `make the ball 20 pixels square and keep it centered at its position and update collision behavior and tests`.
- Fresh Android Workshop Pong project for the initial attempt.
- Medium reasoning, JSON Schema responses, native tool calls, no provider
  fallbacks, $2/M input and $6/M output ceilings, and a $5 cumulative guard.
- Maximum 15 provider turns and 50 tool calls per response.
- Model-authored Stasis tests ran before a four-test independent acceptance
  fixture. The hidden fixture was removed before each repair request; Grok saw
  only a short failure summary.

## Outcome

| Stage | Provider calls | Model seconds | Local tool seconds | Input | Cached | Output | Reasoning | Cost | Acceptance |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Initial | 2 | 40.70 | 0.02 before cold build | 13,223 | 5,376 | 3,923 | 1,151 | $0.04084 | 2/4 |
| Repair 1 | 3 | 59.66 | 0.44 | 22,290 | 10,624 | 4,891 | 2,187 | $0.05587 | 3/4 |
| Repair 2 | 3 | 28.33 | 0.43 | 19,348 | 10,624 | 2,275 | 436 | $0.03429 | 4/4 |
| Total | 8 | 128.69 | 0.89 recorded warm tools | 54,861 | 26,624 | 11,089 | 3,774 | $0.13100 | 4/4 after feedback |

The initial response batched eight inspection tools in 5.37 seconds. Its second
response took 35.33 seconds and returned five actions: three source/test writes,
one additional test write, and `run_tests`. Its generated suite passed, but the
independent fixture found paddle-contact and full-exit scoring errors.

The first repair used calls of 5.89, 13.97, and 39.81 seconds. It fixed rendered
paddle centers and full-exit scoring, improving acceptance to 3/4. The remaining
difference was inclusive versus exclusive exact-edge contact, a convention not
stated explicitly in the original prompt.

The clarified second repair used calls of 4.57, 5.34, and 18.42 seconds. It made
the collision threshold inclusive, updated both sides of the boundary test, and
reached 4/4 independent acceptance. Generated plus independent validation then
reported 13 passing tests across three files.

## Setup timing caveat

The initial write batch triggered a cold release build in the new worktree. The
outer command reached its five-minute bound before that build completed. A
separate bounded continuation finished the one-time build in 146 seconds. Later
warm compile/test batches took about 0.4 seconds, so the cold build is reported
as setup cost and excluded from model back-and-forth time.

The trial also exposed two harness regressions on current `main`: the test runner
still used the removed `--dir` CLI form, and partial acceptance reporting could
mislabel infrastructure failures. The OpenRouter PR updates the harness to use a
temporary manifest with `stasis test --workspace`, removes that manifest after
testing, and distinguishes infrastructure failure from partial assertion
failure.
