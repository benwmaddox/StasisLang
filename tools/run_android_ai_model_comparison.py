#!/usr/bin/env python3
"""Run a repeatable, isolated GPT-5.6 Workshop model comparison."""
from __future__ import annotations

import argparse
import json
import re
import shutil
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import android_ai_agent_host as host

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_BASELINE = ROOT / "mobile/android/app/src/main/assets/workshop_sample"
DEFAULT_MODELS = ("gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna")
DEFAULT_ACCEPTANCE_TEST = ROOT / "tools/fixtures/ball_20_comparison_acceptance.test.stasis"


def reset_copy(baseline: Path, destination: Path) -> None:
    if destination.exists():
        shutil.rmtree(destination)
    shutil.copytree(baseline, destination)


def summarize(trace: dict[str, Any], process_code: int, acceptance: dict[str, Any] | None = None) -> dict[str, Any]:
    exchanges = [event for event in trace.get("events", []) if event.get("kind") == "openai_exchange"]
    tool_events = [event for event in trace.get("events", []) if event.get("kind") == "tool_observations"]
    rollback_batches = sum(
        1 for event in tool_events
        if any(item.get("status") == "rolled_back" for item in event.get("summary", []))
    )
    final_tests = [
        event.get("final_test", {})
        for event in trace.get("events", [])
        if event.get("kind") in {"automatic_test_after_writes", "final_test_after_done_or_empty"}
    ]
    usage = trace.get("usage_summary", {})
    totals = usage.get("totals", {})
    input_tokens = int(totals.get("input_tokens", 0) or 0)
    cached_input_tokens = int(totals.get("cached_input_tokens", 0) or 0)
    response_models = sorted({
        event.get("response", {}).get("response_model") for event in exchanges
    } - {None})
    return {
        "requested_model": trace.get("meta", {}).get("model"),
        "response_models": response_models,
        "passed": process_code == 0 and trace.get("exit_code") == 0,
        "acceptance_passed": bool(acceptance and acceptance.get("ok")),
        "acceptance_tests_passed": (acceptance or {}).get("acceptance_tests_passed", 0),
        "acceptance_tests_total": (acceptance or {}).get("acceptance_tests_total", 0),
        "process_exit_code": process_code,
        "trace_exit_code": trace.get("exit_code"),
        "total_seconds": trace.get("meta", {}).get("elapsed_seconds", 0.0),
        "model_seconds": sum(float(event.get("elapsed_seconds") or 0.0) for event in exchanges),
        "tool_seconds": sum(float(event.get("elapsed_seconds") or 0.0) for event in tool_events),
        "calls": usage.get("calls", 0),
        "tool_batches": len(tool_events),
        "validation_retries": sum(
            1 for event in trace.get("events", []) if event.get("kind") == "response_validation_errors"
        ),
        "actions": trace.get("meta", {}).get("total_actions", 0),
        "successful_writes": trace.get("meta", {}).get("successful_write_count", 0),
        "rolled_back_writes": trace.get("meta", {}).get("rolled_back_write_count", 0),
        "rollback_batches": rollback_batches,
        "input_tokens": input_tokens,
        "cached_input_tokens": cached_input_tokens,
        "cached_input_percent": (100.0 * cached_input_tokens / input_tokens) if input_tokens else 0.0,
        "cache_write_input_tokens": totals.get("cache_write_input_tokens", 0),
        "output_tokens": totals.get("output_tokens", 0),
        "estimated_cost_usd": usage.get("estimated_cost_usd"),
        "latest_test_ok": final_tests[-1].get("ok") if final_tests else None,
        "acceptance_output_tail": (acceptance or {}).get("output_tail", "")[-2000:],
    }


def run_acceptance(project: Path, acceptance_test: Path) -> dict[str, Any]:
    target = project / "tests/comparison_acceptance.test.stasis"
    shutil.copyfile(acceptance_test, target)
    result = host.run_behavior_tests(project)
    source = acceptance_test.read_text(encoding="utf-8")
    total = len(re.findall(r"\btest\s+`[^`]+`\s*\(", source))
    output = str(result.get("output_tail", ""))
    failed = len(re.findall(r"comparison_acceptance\.test\.stasis\s+::", output))
    result["acceptance_tests_total"] = total
    result["acceptance_tests_passed"] = max(0, total - failed)
    return result


def write_markdown(path: Path, prompt: str, rows: list[dict[str, Any]]) -> None:
    lines = [
        "# Android Workshop GPT-5.6 comparison",
        "",
        f"Prompt: `{prompt}`",
        "",
        "All runs use fresh copies of the same baseline, medium reasoning, standard API service, and a 25-turn cap.",
        "",
        "| Model | Harness | Acceptance | Total s | Model s | Tool s | Calls | Tool batches | Actions | Schema retries | Failed write batches | Restored writes | Input | Cached | Cache % | Cache write | Output | Est. USD |",
        "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for row in rows:
        lines.append(
            f"| {row['requested_model']} | {'pass' if row['passed'] else 'fail'} | "
            f"{row['acceptance_tests_passed']}/{row['acceptance_tests_total']} | "
            f"{row['total_seconds']:.1f} | {row['model_seconds']:.1f} | {row['tool_seconds']:.1f} | "
            f"{row['calls']} | {row['tool_batches']} | {row['actions']} | {row['validation_retries']} | "
            f"{row['rollback_batches']} | {row['rolled_back_writes']} | {row['input_tokens']} | {row['cached_input_tokens']} | "
            f"{row['cached_input_percent']:.1f}% | "
            f"{row['cache_write_input_tokens']} | {row['output_tokens']} | {row['estimated_cost_usd'] or 0.0:.4f} |"
        )
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--prompt", required=True)
    parser.add_argument("--baseline", type=Path, default=DEFAULT_BASELINE)
    parser.add_argument("--models", nargs="+", default=list(DEFAULT_MODELS))
    parser.add_argument("--service-tier", choices=("standard", "priority"), default="standard")
    parser.add_argument("--output-dir", type=Path)
    parser.add_argument("--skip-warmup", action="store_true")
    parser.add_argument("--acceptance-test", type=Path, default=DEFAULT_ACCEPTANCE_TEST)
    parser.add_argument("--summarize-only", action="store_true")
    args = parser.parse_args()

    timestamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    output_dir = (args.output_dir or ROOT / "artifacts/android_ai_comparisons" / timestamp).resolve()
    projects_dir = output_dir / "projects"
    traces_dir = output_dir / "traces"
    traces_dir.mkdir(parents=True, exist_ok=True)

    if not args.skip_warmup and not args.summarize_only:
        warmup = projects_dir / "_warmup"
        reset_copy(args.baseline.resolve(), warmup)
        warmup_result = host.run_behavior_tests(warmup)
        if not warmup_result.get("ok"):
            raise RuntimeError(f"comparison warmup failed: {warmup_result.get('output_tail', warmup_result)}")

    rows: list[dict[str, Any]] = []
    for model in args.models:
        project = projects_dir / model
        trace_file = traces_dir / f"{model}.json"
        if args.summarize_only:
            stored_exit = json.loads(trace_file.read_text(encoding="utf-8")).get("exit_code")
            process_code = int(stored_exit) if stored_exit is not None else 124
        else:
            reset_copy(args.baseline.resolve(), project)
            command = [
                sys.executable,
                str(ROOT / "tools/android_ai_agent_host.py"),
                "--project-root", str(project),
                "--model", model,
                "--service-tier", args.service_tier,
                "--trace-file", str(trace_file),
                "--prompt", args.prompt,
            ]
            process_code = subprocess.run(command, cwd=ROOT, text=True).returncode
        trace = json.loads(trace_file.read_text(encoding="utf-8"))
        acceptance = run_acceptance(project, args.acceptance_test.resolve())
        rows.append(summarize(trace, process_code, acceptance))

    summary = {
        "prompt": args.prompt,
        "models": list(args.models),
        "reasoning_effort": host.DEFAULT_REASONING_EFFORT,
        "service_tier": args.service_tier,
        "max_turns": host.MAX_TURNS,
        "rows": rows,
    }
    (output_dir / "summary.json").write_text(json.dumps(summary, indent=2), encoding="utf-8")
    write_markdown(output_dir / "summary.md", args.prompt, rows)
    print(json.dumps({"output_dir": str(output_dir), "rows": rows}, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
