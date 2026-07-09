# Android AI Paddle-Speed Real Run Review

Full raw log: `docs/android_ai_paddle_speed_real_run_full_log.json`

Prompt:

> enemy paddle should have speed change. when ball created, it should be 3x ball speed. at 60 seconds it should be 0.5 ball speed. it should change in a linear way. repeat on each ball creation.

Run command:

```powershell
python tools\android_ai_agent_host.py --reset-paddle-speed-feature --trace-file docs\android_ai_paddle_speed_real_run_full_log.json --prompt "enemy paddle should have speed change. when ball created, it should be 3x ball speed. at 60 seconds it should be 0.5 ball speed. it should change in a linear way. repeat on each ball creation."
```

## Result

The real LLM run succeeded.

It took 5 LLM calls, 15 tool actions, and 23.25 seconds wall-clock time. The final targeted behavior test passed.

## Token And Cost Summary

From the API-reported `usage` fields in the full log:

```json
{
  "calls": 5,
  "input_tokens": 17078,
  "cached_input_tokens": 5376,
  "uncached_input_tokens": 11702,
  "output_tokens": 3319,
  "total_tokens": 20397,
  "estimated_cost_usd": 0.0241152
}
```

Per-call usage:

```json
[
  {"turn":1,"input_tokens":2455,"cached_input_tokens":0,"uncached_input_tokens":2455,"output_tokens":105,"total_tokens":2560},
  {"turn":2,"input_tokens":3072,"cached_input_tokens":0,"uncached_input_tokens":3072,"output_tokens":321,"total_tokens":3393},
  {"turn":3,"input_tokens":3508,"cached_input_tokens":1792,"uncached_input_tokens":1716,"output_tokens":2479,"total_tokens":5987},
  {"turn":4,"input_tokens":5002,"cached_input_tokens":1792,"uncached_input_tokens":3210,"output_tokens":353,"total_tokens":5355},
  {"turn":5,"input_tokens":3041,"cached_input_tokens":1792,"uncached_input_tokens":1249,"output_tokens":61,"total_tokens":3102}
]
```

Cost estimate assumption:

```json
{
  "input_per_1m": 0.75,
  "cached_input_per_1m": 0.075,
  "output_per_1m": 4.50
}
```

Pricing updated from user-provided OpenAI standard short-context rates on 2026-07-09: `gpt-5.4-mini` is `$0.75` input, `$0.075` cached input, and `$4.50` output per 1M tokens. The full log preserves exact API token usage so this can be recalculated if rates change.

## Flow Review

Turn 1:
- The model used `list_symbols` and `list_tests`.
- Good: it did not use `read_file`.
- Good: it started with discovery at the fine-grained level.

Turn 2:
- It read targeted symbols: `update_enemy_paddle`, `update_ball`, `reset_ball`, `GameState`, and `on_code_swap`.
- Good: symbol-level inspection worked as intended.

Turn 3:
- It wrote `GameState`, `reset_ball`, `update_ball`, `update_enemy_paddle`, and attempted to write a test.
- Good: it batched related source writes and the batch compiled.
- Good: it created a behavior test before claiming completion.
- Bad: the test used `expected_max` instead of supported `max`, and `write_test_file` rejected it with a validation error.
- Interesting: the Rust behavior test still passed after the source changes, but the run did not stop because the AI scenario test write failed.

Turn 4:
- It corrected the AI test file shape from `expected_max` to `max`.
- It ran tests again.
- The run passed.

Turn 5:
- It returned `mode=done` with a correct summary.

## Quality Notes

What went well:
- The LLM never used `read_file`.
- It used fine-grained symbol and test tools.
- It created a test and corrected a test validation error from tool feedback.
- The whole run completed in 5 LLM calls, much better than the prior 11-turn run.
- Cache usage appeared from turn 3 onward: 1,792 cached input tokens per call.

What was weaker:
- It did not use the new delete tools in this run.
- It wrote the new test under `tests/enemy_paddle_speed.ai_test.json`, replacing the previously committed canonical test name from the reset path.
- It moved speed schedule calculation into `update_ball` and removed explicit `init()` assignments for `ball_age_ticks` and `enemy_paddle_speed_x100`. The behavior test passes, but the committed version is cleaner because it initializes persistent state directly and keeps enemy speed calculation closer to enemy paddle behavior.

Recommendation:
- Keep the full log as the run artifact.
- Do not automatically accept the LLM-produced sample changes from this particular run without review.
- Add or keep broader invariant tests for paddle bounds and initialization so future runs cannot pass the speed test while weakening adjacent behavior.