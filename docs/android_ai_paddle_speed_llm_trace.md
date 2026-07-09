# Android AI Paddle-Speed Trace

This is the full available transcript from the local host regression run for the prompt:

> enemy paddle should have speed change. when ball created, it should be 3x ball speed. at 60 seconds it should be 0.5 ball speed. it should change in a linear way. repeat on each ball creation.

Run mode: `python tools\android_ai_agent_host.py --reset-paddle-speed-feature --prompt ...`

Notes:
- The run started by resetting the bundled Pong sample to remove the feature and generated paddle-speed tests.
- The harness did not persist raw OpenAI request/response bodies for this run, so this is the complete available console-level back and forth: model summaries, tool calls, validation/test observations, and final result.
- No API key or credential is included.

## Turn 1

Model response:

```json
{
  "mode": "tool_calls",
  "summary": "Inspecting workspace symbols and tests before making changes",
  "tool_calls": [
    {"tool":"list_symbols","args":{}},
    {"tool":"list_tests","args":{}}
  ]
}
```

Tool observations:

```json
[
  {"tool":"list_symbols","status":null},
  {"tool":"list_tests","status":null}
]
```

Interpretation: good first step. It used fine-grained discovery and did not use `read_file`.

## Turn 2

Model response summary:

```json
{
  "mode": "tool_calls",
  "summary": "Inspecting state and tests before patching",
  "tool_call_count": 9
}
```

Tool calls:

```json
[
  {"tool":"read_symbol","args":{"name":"GameState"}},
  {"tool":"read_symbol","args":{"name":"update_enemy_paddle"}},
  {"tool":"read_symbol","args":{"name":"update_ball"}},
  {"tool":"read_symbol","args":{"name":"reset_ball"}},
  {"tool":"read_symbol","args":{"name":"tick"}},
  {"tool":"read_symbol","args":{"name":"init"}},
  {"tool":"read_symbol","args":{"name":"on_code_swap"}},
  {
    "tool":"write_test_file",
    "args":{
      "file":"tests/enemy_paddle_speed_schedule.ai_test.json",
      "source":"{... assert_runtime_i32 used expected_max ...}"
    }
  },
  {"tool":"run_tests","args":{}}
]
```

Important observations:

```json
{
  "write_test_file": {
    "status": "validation_error",
    "error": "unsupported assertion shape: expected_max should be max"
  },
  "run_tests": {
    "ok": false,
    "error_tail": "unknown global path 'GameState.ball_age_ticks' in current jit path"
  }
}
```

Interpretation: useful failure. The model tried to create a test before code, which is the desired flow, but used an unsupported test assertion property. The host returned a validation error instead of accepting a test Android could not run. The runtime test also correctly showed the reset sample did not yet have the required globals.

## Turn 3

Model response summary:

```json
{
  "mode": "tool_calls",
  "summary": "add state and linear enemy speed schedule",
  "tool_call_count": 6
}
```

Tool calls:

```json
[
  {
    "tool":"write_symbol",
    "args":{
      "file":"src/main.stasis",
      "name":"GameState",
      "owner":"Globals",
      "new_source":"global GameState { ... ball_age_ticks: i32; enemy_paddle_speed_x100: i32; ... }"
    }
  },
  {
    "tool":"write_symbol",
    "args":{
      "file":"src/main.stasis",
      "name":"update_enemy_paddle",
      "owner":"Root",
      "new_source":"function update_enemy_paddle(): void { speed_delta_x100 = 1250 * age / 3600; ... }"
    }
  },
  {
    "tool":"write_symbol",
    "args":{
      "file":"src/main.stasis",
      "name":"reset_ball",
      "owner":"Root",
      "new_source":"function reset_ball(): void { ... ball_age_ticks = 0; enemy_paddle_speed_x100 = 1500; }"
    }
  },
  {
    "tool":"write_symbol",
    "args":{
      "file":"src/main.stasis",
      "name":"init",
      "owner":"Main",
      "new_source":"function init(): void { ... ball_age_ticks = 0; enemy_paddle_speed_x100 = 1500; ... }"
    }
  },
  {
    "tool":"write_test_file",
    "args":{
      "file":"tests/enemy_paddle_speed_schedule.ai_test.json",
      "source":"{... uses max instead of expected_max ...}"
    }
  },
  {"tool":"run_tests","args":{}}
]
```

Tool observations:

```json
{
  "write_symbol": "all source writes compiled",
  "write_test_file": {"status":"written"},
  "run_tests": {
    "ok": false,
    "error_tail": "after 60 seconds, speed reaches 0.5x: left 1500 right 250"
  }
}
```

Interpretation: it added the state and fixed the test shape, but the implementation was behaviorally wrong. The test caught it.

## Turn 4

Model response summary:

```json
{
  "mode":"tool_calls",
  "summary":"inspect ball update flow",
  "tool_call_count":5
}
```

Tool calls:

```json
[
  {"tool":"list_symbols","args":{}},
  {"tool":"read_symbol","args":{"name":"update_ball"}},
  {"tool":"read_symbol","args":{"name":"tick"}},
  {"tool":"read_symbol","args":{"name":"main"}},
  {"tool":"read_symbol","args":{"name":"reset_ball"}}
]
```

Interpretation: good recovery behavior. It did not repeat the same writes; it inspected the relevant lifecycle symbols.

## Turn 5

Model response summary:

```json
{
  "mode":"tool_calls",
  "summary":"Adjust enemy paddle speed and add coverage",
  "tool_call_count":3
}
```

Tool calls:

```json
[
  {
    "tool":"write_symbol",
    "args":{
      "file":"src/main.stasis",
      "name":"update_enemy_paddle",
      "new_source":"function update_enemy_paddle(): void { if age <= 1800 then interpolate too fast, else 250; ... }"
    }
  },
  {
    "tool":"write_test_file",
    "args":{
      "file":"tests/android_bundled_touch_pong_enemy_paddle_speed_schedule_is_linear.ai_test.json",
      "source":"{... duplicate scenario test ...}"
    }
  },
  {"tool":"run_tests","args":{}}
]
```

Tool observations:

```json
{
  "write_symbol": {"status":"written", "compile_ok":true},
  "write_test_file": {"status":"written"},
  "run_tests": {
    "ok": false,
    "error_tail": "after 30 seconds, speed is halfway from 3x to 0.5x: left 250 right 875"
  }
}
```

Interpretation: this was a poor correction. It changed the interpolation duration to 30 seconds and created a duplicate test. The behavior test caught the math issue, but the duplicate test showed why `delete_test_file` is useful.

## Turn 6

Model response summary:

```json
{
  "mode":"tool_calls",
  "summary":"inspect ball lifecycle and speed update path",
  "tool_call_count":2
}
```

Tool calls:

```json
[
  {"tool":"list_symbols","args":{}},
  {"tool":"list_tests","args":{}}
]
```

Interpretation: it checked symbols/tests again rather than blindly rewriting. It still had not cleaned the duplicate test.

## Turn 7

Model response summary:

```json
{
  "mode":"tool_calls",
  "summary":"Inspecting existing test and relevant symbols before updating behavior.",
  "tool_call_count":5
}
```

Tool calls:

```json
[
  {"tool":"read_symbol","args":{"name":"update_enemy_paddle"}},
  {"tool":"read_symbol","args":{"name":"update_ball"}},
  {"tool":"read_symbol","args":{"name":"reset_ball"}},
  {"tool":"read_test_file","args":{"file":"tests/android_bundled_touch_pong_enemy_paddle_speed_schedule_is_linear.ai_test.json"}},
  {"tool":"read_test_file","args":{"file":"tests/enemy_paddle_speed_schedule.ai_test.json"}}
]
```

Interpretation: fine-grained reads worked well here. It inspected tests by name rather than reading whole files.

## Turn 8

Model response summary:

```json
{
  "mode":"tool_calls",
  "summary":"Updating AI test and verifying linear enemy paddle speed behavior.",
  "tool_call_count":2
}
```

Tool calls:

```json
[
  {
    "tool":"write_test_file",
    "args":{
      "file":"tests/android_bundled_touch_pong_enemy_paddle_speed_schedule_is_linear.ai_test.json",
      "source":"{... same valid scenario ...}"
    }
  },
  {"tool":"run_tests","args":{}}
]
```

Tool observations:

```json
{
  "write_test_file": {"status":"written"},
  "run_tests": {
    "ok": false,
    "error_tail": "after 30 seconds, speed is halfway from 3x to 0.5x: left 250 right 875"
  }
}
```

Interpretation: not ideal. It rewrote the test without fixing the code. The loop continued instead of ending.

## Turn 9

Model response summary:

```json
{
  "mode":"tool_calls",
  "summary":"Inspecting symbols to adjust enemy paddle speed schedule.",
  "tool_call_count":5
}
```

Tool calls:

```json
[
  {"tool":"list_symbols","args":{}},
  {"tool":"read_symbol","args":{"name":"GameState","kind":"global","file":"src/main.stasis"}},
  {"tool":"read_symbol","args":{"name":"tick","file":"src/main.stasis"}},
  {"tool":"read_symbol","args":{"name":"reset_ball","file":"src/main.stasis"}},
  {"tool":"read_symbol","args":{"name":"update_ball","file":"src/main.stasis"}}
]
```

Interpretation: another reasonable inspection step. No `read_file` usage.

## Turn 10

Model response summary:

```json
{
  "mode":"tool_calls",
  "summary":"Update enemy paddle speed schedule and add verification test.",
  "tool_call_count":3
}
```

Tool calls:

```json
[
  {
    "tool":"write_symbol",
    "args":{
      "file":"src/main.stasis",
      "name":"update_enemy_paddle",
      "new_source":"function update_enemy_paddle(): void { if age <= 0 -> 1500; else if age >= 3600 -> 250; else 1500 - ((age * 1250) / 3600); move paddle; }"
    }
  },
  {
    "tool":"write_test_file",
    "args":{
      "file":"tests/enemy_paddle_speed_schedule.ai_test.json",
      "source":"{... scenario checks 1500, 875, 250, clamp, reset ...}"
    }
  },
  {"tool":"run_tests","args":{}}
]
```

Tool observations:

```json
{
  "write_symbol": {"status":"written", "compile_ok":true},
  "write_test_file": {"status":"written"},
  "run_tests": {
    "ok": true,
    "output_tail": "test tests::android_bundled_touch_pong_enemy_paddle_speed_schedule_is_linear ... ok"
  }
}
```

Interpretation: successful final model-driven correction. The test caught prior mistakes and verified the intended schedule.

## Turn 11

Model response:

```json
{
  "mode":"done",
  "summary":"Verified linear enemy paddle speed scaling from 3x at ball creation to 0.5x at 60s, clamped thereafter, and reset on new ball creation; added and passed an AI scenario test."
}
```

Final host test:

```json
{
  "ok": true,
  "test_files": [
    "tests/android_bundled_touch_pong_enemy_paddle_speed_schedule_is_linear.ai_test.json",
    "tests/enemy_paddle_speed_schedule.ai_test.json"
  ],
  "ai_test_file_count": 2
}
```

## Manual Cleanup After The Run

After the successful model run, I made two cleanup changes before committing:

1. Removed the duplicate generated test file and kept `tests/enemy_paddle_speed_schedule.ai_test.json`.
2. Restored enemy paddle screen-bound clamping, because the final model rewrite had dropped the `ai_y` min/max clamp even though the paddle-speed test did not cover that behavior.

This is an important lesson: the current test caught the speed schedule, but not all existing paddle invariants. A follow-up improvement should add a broader scenario or invariant test for paddle bounds so the LLM cannot accidentally regress that while changing speed behavior.

## Assessment

What went well:
- The model never used `read_file`.
- It used symbol-level reads and test-level reads.
- It wrote a test before final success.
- It received validation feedback for a bad test shape and corrected it.
- It recovered from multiple failed behavior tests without manual intervention.

What was weak:
- It needed 11 turns, which is high.
- It produced a duplicate test file.
- It temporarily made incorrect interpolation math.
- The final passing code dropped an unrelated paddle-bound clamp, showing that the test suite needs broader invariant coverage.

Changes made afterward to improve future runs:
- Added `delete_symbol` and `delete_test_file` tools so the LLM can remove duplicates/obsolete items.
- Kept `read_file` unavailable to push fine-grained edits.
- Added stricter Android-compatible AI scenario shapes: `set_runtime_i32`, `assert_runtime_i32.equals`, and `assert_runtime_i32.max`.