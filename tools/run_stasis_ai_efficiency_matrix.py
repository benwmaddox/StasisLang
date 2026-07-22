#!/usr/bin/env python3
"""Compare general coding agents with Stasis AI on scaled, resettable projects."""
from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
BASELINE = ROOT / "mobile/android/app/src/main/assets/workshop_sample"
GUIDE = ROOT / "docs/agent_workflow.md"
ACCEPTANCE = ROOT / "tools/fixtures/ball_20_comparison_acceptance.test.stasis"
DEFAULT_PROMPT = "make the ball 20 pixels square and keep it centered at its position and update collision behavior and tests"
SCALE_LAYOUT = {"small": (0, 0), "medium": (8, 25), "large": (32, 50)}
MODEL = "gpt-5.6-sol"
REASONING = "medium"


@dataclass(frozen=True)
class RunResult:
    returncode: int
    elapsed_seconds: float
    timed_out: bool


def _write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8", newline="\n")


def prepare_project(destination: Path, scale: str, guide: Path = GUIDE) -> dict[str, int]:
    if scale not in SCALE_LAYOUT:
        raise ValueError(f"unknown scale: {scale}")
    if destination.exists():
        shutil.rmtree(destination)
    shutil.copytree(BASELINE, destination)
    _write(destination / "AGENTS.md", guide.read_text(encoding="utf-8"))

    file_count, functions_per_file = SCALE_LAYOUT[scale]
    imports = ['import "main.stasis";']
    for file_index in range(file_count):
        name = f"aaa_eval_padding_{file_index:02d}.stasis"
        imports.insert(0, f'import "{name}";')
        functions = []
        for function_index in range(functions_per_file):
            ordinal = file_index * functions_per_file + function_index
            functions.append(
                f"function eval_padding_{ordinal:04d}(value: i32): i32 {{\n"
                f"    return value + {ordinal % 17};\n"
                "}\n"
            )
        _write(destination / "src" / name, "\n".join(functions))

    entry = "src/main.stasis"
    if imports != ['import "main.stasis";']:
        entry = "src/eval_entry.stasis"
        _write(destination / entry, "\n".join(imports) + "\n")
    manifest = {
        "manifest_version": 1,
        "name": f"ai_efficiency_{scale}",
        "entry": entry,
        "tests": "tests",
        "output": "build",
    }
    _write(destination / "stasis.json", json.dumps(manifest, indent=2) + "\n")
    return {
        "padding_files": file_count,
        "padding_functions": file_count * functions_per_file,
        "stasis_files": len(list(destination.rglob("*.stasis"))),
    }


def run_process(command: list[str], cwd: Path, stdout_path: Path, stderr_path: Path,
                env: dict[str, str], timeout_seconds: int) -> RunResult:
    started = time.perf_counter()
    timed_out = False
    try:
        completed = subprocess.run(
            command,
            cwd=cwd,
            env=env,
            text=True,
            encoding="utf-8",
            errors="replace",
            capture_output=True,
            timeout=timeout_seconds,
        )
        returncode = completed.returncode
        stdout = completed.stdout
        stderr = completed.stderr
    except subprocess.TimeoutExpired as error:
        timed_out = True
        returncode = 124
        stdout = error.stdout or ""
        stderr = error.stderr or ""
    _write(stdout_path, stdout)
    _write(stderr_path, stderr)
    return RunResult(returncode, time.perf_counter() - started, timed_out)


def usage_from_jsonl(path: Path, last_only: bool) -> dict[str, int]:
    usages: list[dict[str, Any]] = []
    if not path.exists():
        return {}
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        usage = event.get("usage") if isinstance(event, dict) else None
        if isinstance(usage, dict):
            usages.append(usage)
        elif isinstance(event, dict) and any(key.endswith("tokens") for key in event):
            usages.append(event)
    if last_only and usages:
        usages = usages[-1:]
    keys = {key for usage in usages for key in usage if key.endswith("tokens")}
    return {key: sum(int(usage.get(key, 0) or 0) for usage in usages) for key in sorted(keys)}


def count_events(path: Path, event_type: str, item_type: str | None = None) -> int:
    count = 0
    if not path.exists():
        return count
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if event.get("event") == event_type:
            count += len(event.get("calls", [])) if event_type == "tool_calls" else 1
        if event.get("type") == event_type and (item_type is None or event.get("item", {}).get("type") == item_type):
            count += 1
    return count


def estimated_cost(usage: dict[str, int]) -> float:
    total = usage.get("input_tokens", 0)
    cached = usage.get("cached_input_tokens", 0)
    cache_write = usage.get("cache_write_input_tokens", 0)
    uncached = max(0, total - cached - cache_write)
    output = usage.get("output_tokens", 0)
    return (uncached * 5.0 + cached * 0.5 + cache_write * 6.25 + output * 30.0) / 1_000_000


def initialize_git(project: Path) -> None:
    subprocess.run(["git", "init", "--quiet"], cwd=project, check=True)
    subprocess.run(["git", "config", "user.email", "ai-eval@localhost"], cwd=project, check=True)
    subprocess.run(["git", "config", "user.name", "Stasis AI Eval"], cwd=project, check=True)
    subprocess.run(["git", "add", "."], cwd=project, check=True)
    subprocess.run(["git", "commit", "--quiet", "-m", "baseline"], cwd=project, check=True)


def run_acceptance(project: Path, stasis: Path, env: dict[str, str]) -> RunResult:
    shutil.copyfile(ACCEPTANCE, project / "tests/comparison_acceptance.test.stasis")
    return run_process(
        [str(stasis), "test"], project,
        project / "build/eval-acceptance.stdout.txt",
        project / "build/eval-acceptance.stderr.txt", env, 300,
    )


def run_one(mode: str, scale: str, run_root: Path, stasis: Path, codex: Path,
            prompt: str, timeout_seconds: int) -> dict[str, Any]:
    project = run_root / "project"
    stats = prepare_project(project, scale)
    initialize_git(project)
    env = os.environ.copy()
    env["PATH"] = str(stasis.parent) + os.pathsep + env.get("PATH", "")
    env["STASIS_CODEX_EXE"] = str(codex)
    env["STASIS_AI_MODEL"] = MODEL
    env["STASIS_AI_REASONING_EFFORT"] = REASONING
    preflight = run_process(
        [str(stasis), "check"], project,
        run_root / "preflight.stdout.txt", run_root / "preflight.stderr.txt", env, 300,
    )
    if preflight.returncode != 0:
        raise RuntimeError(f"{scale} project preflight failed; see {run_root / 'preflight.stderr.txt'}")
    stdout = run_root / "agent.stdout.jsonl"
    stderr = run_root / "agent.stderr.txt"

    if mode == "generalist":
        command = [
            str(codex), "exec", "--cd", str(project), "--model", MODEL,
            "-c", f'model_reasoning_effort="{REASONING}"',
            "--sandbox", "workspace-write", "--ephemeral", "--json", prompt,
        ]
    elif mode == "stasis":
        command = [str(stasis), "--json", "ai", prompt]
    else:
        raise ValueError(f"unknown mode: {mode}")
    result = run_process(command, project, stdout, stderr, env, timeout_seconds)
    acceptance = run_acceptance(project, stasis, env)

    if mode == "generalist":
        usage = usage_from_jsonl(stdout, last_only=True)
        trace = stdout
    else:
        usage_files = sorted((project / "build/ai-traces").glob("*.usage.jsonl"))
        usage = usage_from_jsonl(usage_files[-1], last_only=False) if usage_files else {}
        traces = sorted(path for path in (project / "build/ai-traces").glob("*.jsonl") if ".usage." not in path.name)
        trace = traces[-1] if traces else stdout
    trace_text = trace.read_text(encoding="utf-8", errors="replace") if trace.exists() else ""
    return {
        "mode": mode,
        "scale": scale,
        **stats,
        "returncode": result.returncode,
        "timed_out": result.timed_out,
        "elapsed_seconds": round(result.elapsed_seconds, 3),
        "acceptance_seconds": round(acceptance.elapsed_seconds, 3),
        "total_seconds": round(result.elapsed_seconds + acceptance.elapsed_seconds, 3),
        "acceptance_passed": acceptance.returncode == 0,
        "input_tokens": usage.get("input_tokens", 0),
        "cached_input_tokens": usage.get("cached_input_tokens", 0),
        "cache_write_input_tokens": usage.get("cache_write_input_tokens", 0),
        "output_tokens": usage.get("output_tokens", 0),
        "estimated_cost_usd": round(estimated_cost(usage), 6),
        "command_actions": count_events(trace, "item.completed", "command_execution"),
        "tool_calls": count_events(trace, "tool_calls"),
        "run_root": str(run_root),
    }


def write_summary(output: Path, rows: list[dict[str, Any]], prompt: str) -> None:
    _write(output / "summary.json", json.dumps({"prompt": prompt, "rows": rows}, indent=2) + "\n")
    lines = [
        "# Stasis AI efficiency matrix", "", f"Prompt: `{prompt}`", "",
        "| Mode | Scale | Files | Padding symbols | Correct | Agent s | Acceptance s | Total s | Input | Cached | Cache write | Output | Est. USD |",
        "|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for row in rows:
        lines.append(
            f"| {row['mode']} | {row['scale']} | {row['stasis_files']} | {row['padding_functions']} | "
            f"{'yes' if row['acceptance_passed'] else 'no'} | {row['elapsed_seconds']:.1f} | "
            f"{row['acceptance_seconds']:.1f} | {row['total_seconds']:.1f} | "
            f"{row['input_tokens']} | {row['cached_input_tokens']} | {row['cache_write_input_tokens']} | "
            f"{row['output_tokens']} | {row['estimated_cost_usd']:.4f} |"
        )
    _write(output / "summary.md", "\n".join(lines) + "\n")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--stasis", type=Path, default=ROOT / "target/debug/stasis.exe")
    parser.add_argument("--codex", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--modes", nargs="+", choices=("generalist", "stasis"), default=["generalist", "stasis"])
    parser.add_argument("--scales", nargs="+", choices=tuple(SCALE_LAYOUT), default=list(SCALE_LAYOUT))
    parser.add_argument("--prompt", default=DEFAULT_PROMPT)
    parser.add_argument("--timeout-seconds", type=int, default=600)
    args = parser.parse_args()
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=True)
    rows = []
    for scale in args.scales:
        for mode in args.modes:
            run_root = output / f"{scale}-{mode}"
            run_root.mkdir(parents=True, exist_ok=True)
            row = run_one(mode, scale, run_root, args.stasis.resolve(), args.codex.resolve(), args.prompt, args.timeout_seconds)
            rows.append(row)
            write_summary(output, rows, args.prompt)
            print(json.dumps(row), flush=True)
    return 0 if all(row["acceptance_passed"] for row in rows) else 1


if __name__ == "__main__":
    raise SystemExit(main())
