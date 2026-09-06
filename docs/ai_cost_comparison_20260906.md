# AI edit cost comparison, 2026-09-06

PR #695 was merged to main as `ee325778` before this optimization.

## Method

Run the same centered-chevron task on fresh copies of `samples/asset_breakout`.
Use `openai/gpt-oss-120b:nitro`, low reasoning, OpenRouter provider restriction
`only=[cerebras]`, and no provider fallback for both comparison groups. This is
a benchmark setting, not a change to the application's default routing policy.
Times below are internal request-to-finished times, excluding process startup.
Prompt and completion tokens are summed across turns; completion already
includes reasoning tokens. Costs are reported by the provider, not estimated
from a price list. Three samples per revision cannot establish long-run rates.

Baseline executable: the pre-optimization hash-free live loop used for PR #695.
The optimized executable also incorporates the desktop work merged from main;
the exercised AI CLI path is the subject of this comparison.

Local trace root: `artifacts/throughput-demo-20260906/`, with each run's
`build/ai-traces/*.jsonl`, `*.timing.jsonl`, and `*.usage.jsonl`.

## Baseline

| Run | Seconds | Turns | Prompt tokens | Completion tokens | USD |
| --- | ---: | ---: | ---: | ---: | ---: |
| cost-baseline-a | 6.759 | 4 | 28023 | 6134 | 0.01440855 |
| cost-baseline-b | 3.035 | 1 | 5489 | 1627 | 0.00314140 |
| cost-baseline-c | 3.806 | 2 | 12145 | 3480 | 0.00686075 |

All three completed, independently passed eight game tests, and had their source
diffs inspected. The first and third needed repairs after failing tests.

## Discarded first iteration

The first design offered both ID-only and full-selector replacement forms.
The model still chose full selectors in all three initial responses. Its initial
prompt dropped from 5489 to 4814 tokens, but smaller input alone was insufficient:

- `cost-a` failed after a test rejection, a malformed repair batch, and finally
  truncated provider JSON. Rollback preserved the baseline source. The final
  malformed response's usage was not recorded: the existing transport publishes
  usage only after decoding succeeds. Do not treat this run's logged cost as its
  complete bill.
- `cost-b` passed tests but omitted a requested center-coordinate assertion.
- `cost-c` passed tests but assigned velocities without asserting preservation.

These are not successful full-task cost results. The final design instead makes
live replacements ID-only, exposes additions separately, and makes the existing
behavioral test requirements explicit in each target's purpose. It does not add
literal test-source spelling gates or remove compilation, tests, or rollback.

## Final revision

| Run | Seconds | Turns | Prompt tokens | Completion tokens | USD |
| --- | ---: | ---: | ---: | ---: | ---: |
| cost-final-a | 2.070 | 1 | 4801 | 1195 | 0.00257660 |
| cost-final-b | 3.944 | 2 | 11009 | 3960 | 0.00682315 |
| cost-final-c | 1.883 | 1 | 4801 | 1388 | 0.00272135 |

All three completed, independently passed eight tests, and their source/test
diffs were inspected. Each included edge/center coordinate assertions, explicit
velocity-preservation assertions, and brick repopulation checks. The code changes
only update the initializer and call it once from on_code_swap, preserving the
presentation toggle and unrelated gameplay state.

Every replacement now emitted only `symbol_id` and `new_source`. Each task had
one accepted edit compilation. Run B's first response interleaved an unnecessary
addition with `run_tests`; the atomic-order guard rejected it before compilation,
and the second response repaired the batch. A requested test after a successful
write reused its receipt rather than compiling/testing again.

For these groups, total cost fell from USD 0.02441070 to 0.01212110 (50.3%),
median task time from 3.806s to 2.070s, and initial prompt tokens from 5489 to
4801 (12.5%). Total prompt/completion tokens fell from 45657/11241 to 20611/6543.
Retry counts, generation lengths, caching, and service latency vary; these are
observations from this small sample, not guaranteed percentage improvements.
The discarded iteration above is excluded from steady-state comparison totals,
not from the record of development attempts.

## Validation boundaries

Passing model-authored tests alone does not prove that every requested assertion
was written. Inspect generated tests and source as well as receipts. Task-context
acceptance instructions are not an independent semantic oracle.

Local regression suites cover the ID registry, ID-only argument validation,
separate additions in an atomic tested batch, completion flag decoding, legacy
compatibility, rejected writes, and desktop preview/application behavior.
Final focused suites: 82 AI library tests, 77 live-editor tests, and 72 desktop
tests, all passing. Formatting and diff checks pass.

Visual evidence: `artifacts/throughput-demo-20260906/cost-result.png`, extracted
from `cost-result.mp4`, is a representative final-state image, not a recording of
the live swap itself. The runtime receipts and tests establish the applied-edit
results; no visual timing claim is based on this still.
