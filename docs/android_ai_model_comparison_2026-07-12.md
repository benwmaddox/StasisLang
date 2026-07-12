# Android Workshop GPT-5.6 model comparison (2026-07-12)

## Method

- Prompt: `make the ball 20 pixels square and keep it centered at its position and update collision behavior and tests`
- Fresh byte-identical Pong project copy per model.
- Models: `gpt-5.6-sol`, `gpt-5.6-terra`, and `gpt-5.6-luna`.
- Responses API, standard service tier, medium reasoning, identical stable context and tools.
- Maximum 25 model turns, 12 tool calls per response, and two read-only inspection batches.
- The prebuilt Stasis test runner was reused; selected-project compile and tests remained mandatory.
- Each model's generated tests ran first. A model-independent four-test acceptance fixture then checked centered rendering, wall bounds, paddle/render alignment, and full-exit scoring.
- Costs use [OpenAI standard API token pricing](https://developers.openai.com/api/docs/pricing) and actual uncached, cached, cache-write, and output tokens reported by each response.

This is one trial per model. It is useful for finding large differences, but repeated trials are required for stable averages and variance.

## Results

| Model | Generated tests | Independent acceptance | Total | Model | Tools | Calls | Actions | Rollbacks | Input | Cached | Cache write | Output | Estimated cost |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| GPT-5.6 Sol | pass | 7/7 pass | 65.27s | 64.30s | 0.67s | 6 | 16 | 5 | 36,042 | 19,836 | 0 | 5,233 | $0.24794 |
| GPT-5.6 Terra | pass | 5/6 pass | 27.94s | 27.62s | 0.31s | 3 | 13 | 0 | 15,721 | 9,918 | 0 | 4,821 | $0.08930 |
| GPT-5.6 Luna | pass | 3/6 pass | 13.43s | 13.10s | 0.33s | 3 | 11 | 0 | 15,336 | 6,612 | 3,306 | 2,080 | $0.02269 |

## Quality findings

- Sol completed the full requested geometry change: centered 20x20 rendering, 10-pixel wall bounds, paddle collision aligned with rendered paddle centers, full-exit scoring, and compatible tests. It needed one rolled-back repair batch.
- Terra completed rendering, walls, scoring, and broad AABB collision, but used the old logical paddle center. The independent test caught a one-pixel mismatch between collision and rendering.
- Luna completed centered 20x20 rendering and changed direct paddle-overlap bounds. It did not update wall bounds, paddle-center alignment, or full-exit scoring, so three independent checks failed.

For this task, Sol had the highest quality, Terra had the best middle-ground result, and Luna was fastest and cheapest but incomplete. A generated-test pass alone was not a sufficient quality signal; the independent acceptance fixture changed the conclusion materially.

## Reproduction

Run each model as a separate bounded command, then summarize the shared output directory:

```powershell
python tools/android_ai_agent_host.py --project-root <fresh-sol-copy> --model gpt-5.6-sol --service-tier standard --trace-file <output>/traces/gpt-5.6-sol.json --prompt "make the ball 20 pixels square and keep it centered at its position and update collision behavior and tests"
python tools/android_ai_agent_host.py --project-root <fresh-terra-copy> --model gpt-5.6-terra --service-tier standard --trace-file <output>/traces/gpt-5.6-terra.json --prompt "make the ball 20 pixels square and keep it centered at its position and update collision behavior and tests"
python tools/android_ai_agent_host.py --project-root <fresh-luna-copy> --model gpt-5.6-luna --service-tier standard --trace-file <output>/traces/gpt-5.6-luna.json --prompt "make the ball 20 pixels square and keep it centered at its position and update collision behavior and tests"
python tools/run_android_ai_model_comparison.py --summarize-only --skip-warmup --output-dir <output> --prompt "make the ball 20 pixels square and keep it centered at its position and update collision behavior and tests"
```
