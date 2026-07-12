# Android Workshop GPT-5.6 model comparison (2026-07-12)

## Method

- Prompt: `make the ball 20 pixels square and keep it centered at its position and update collision behavior and tests`
- Fresh byte-identical Pong project copy per model.
- Models: `gpt-5.6-sol`, `gpt-5.6-terra`, and `gpt-5.6-luna`.
- Responses API, standard service tier, medium reasoning, identical stable context and tools.
- Maximum 25 model turns, 12 tool calls per response, and two read-only inspection batches.
- The prebuilt Stasis test runner was reused; selected-project compile and tests remained mandatory.
- Each model's generated tests ran first. A model-independent four-test acceptance fixture then checked centered rendering, wall bounds, paddle/render alignment, and full-exit scoring.
- The stable harness context contains request-generic behavior-test expectations and game-geometry invariants; it contains no task-specific expected values or hidden acceptance assertions.
- Costs use [OpenAI standard API token pricing](https://developers.openai.com/api/docs/pricing) and actual uncached, cached, cache-write, and output tokens reported by each response.

This is one trial per model. It is useful for finding large differences, but repeated trials are required for stable averages and variance.

## Results

| Model | Generated tests | Independent acceptance | Total | Model | Tools | Calls | Actions | Rollbacks | Input | Cached | Cache rate | Cache write | Output | Estimated cost |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| GPT-5.6 Sol | pass | 4/4 | 57.70s | 56.72s | 0.67s | 4 | 17 | 5 | 28,495 | 9,807 | 34.4% | 3,269 | 5,555 | $0.26908 |
| GPT-5.6 Terra | pass | 3/4 | 54.05s | 53.29s | 0.75s | 4 | 16 | 4 | 27,510 | 9,807 | 35.6% | 3,269 | 7,914 | $0.16746 |
| GPT-5.6 Luna | pass | 2/4 | 32.56s | 31.19s | 1.07s | 5 | 19 | 8 | 40,215 | 13,076 | 32.5% | 3,269 | 6,276 | $0.06692 |

## Quality findings

- Sol completed the full requested geometry change: centered 20x20 rendering, 10-pixel wall bounds, paddle collision aligned with rendered paddle bounds, full-exit scoring, and compatible tests. It repaired one failed write batch before finishing.
- Terra completed rendering, walls, scoring, and broad collision behavior, but its paddle collision missed the independently checked rendered-edge case.
- Luna completed centered rendering and wall bounds, but missed the independently checked paddle edge and full-exit scoring behavior.

For this task, Sol had the highest quality. Luna was fastest and cheapest but incomplete. Terra also remained incomplete and happened to be only 3.65 seconds faster than Sol in this single run, so it did not establish a compelling middle-ground result. A generated-test pass alone was not a sufficient quality signal; all three generated suites passed while the independent fixture separated them.

The cacheable prefix was written once per model (3,269 tokens) and reused on later calls. Aggregate cached-input percentages are only 32.5-35.6% because later tool observations and source-bearing repair turns are intentionally volatile. There were zero response-schema retries in all three clean trials; remaining extra calls came from genuine inspection and repair work.

## Reproduction

Run each model as a separate bounded command, then summarize the shared output directory:

```powershell
python tools/android_ai_agent_host.py --project-root <fresh-sol-copy> --model gpt-5.6-sol --service-tier standard --trace-file <output>/traces/gpt-5.6-sol.json --prompt "make the ball 20 pixels square and keep it centered at its position and update collision behavior and tests"
python tools/android_ai_agent_host.py --project-root <fresh-terra-copy> --model gpt-5.6-terra --service-tier standard --trace-file <output>/traces/gpt-5.6-terra.json --prompt "make the ball 20 pixels square and keep it centered at its position and update collision behavior and tests"
python tools/android_ai_agent_host.py --project-root <fresh-luna-copy> --model gpt-5.6-luna --service-tier standard --trace-file <output>/traces/gpt-5.6-luna.json --prompt "make the ball 20 pixels square and keep it centered at its position and update collision behavior and tests"
python tools/run_android_ai_model_comparison.py --summarize-only --skip-warmup --output-dir <output> --prompt "make the ball 20 pixels square and keep it centered at its position and update collision behavior and tests"
```
