#!/usr/bin/env python3
"""Run the Android workshop AI tool loop on the host.

This mirrors the phone-side JSON action flow closely enough to test AI edits locally
before installing to a device. It intentionally uses a temp-free real project path so
successful edits remain in the working tree for review/commit.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import time
from datetime import datetime, timezone
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_PROJECT = ROOT / "mobile/android/app/src/main/assets/workshop_sample"
DEFAULT_MODEL = "gpt-5.6-sol"
DEFAULT_REASONING_EFFORT = "medium"
DEFAULT_TRACE_DIR = ROOT / "artifacts/android_ai_runs"
MAX_TURNS = 15
MAX_WORKING_NOTES_CHARS = 2_000
MAX_INITIAL_SYMBOLS = 256
MAX_INITIAL_SYMBOL_INDEX_CHARS = 16 * 1024
MAX_FAST_PATH_SYMBOLS = 32
MAX_FAST_PATH_SOURCE_CHARS = 24 * 1024
PROMPT_CACHE_KEY = "stasis-android-ai-agent-v2"


@dataclass
class Symbol:
    kind: str
    name: str
    owner: str
    file: str
    signature: str
    source: str
    start: int
    end: int


def load_env_file(path: Path) -> None:
    if not path.is_file():
        return
    for raw_line in path.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        name, value = line.split("=", 1)
        name = name.strip()
        value = value.strip().strip("'\"")
        if name and name not in os.environ:
            os.environ[name] = value

def read_text(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def write_text(path: Path, text: str) -> None:
    path.write_text(text, encoding="utf-8", newline="\n")


def find_matching_brace(source: str, open_index: int) -> int:
    depth = 0
    index = open_index
    while index < len(source):
        ch = source[index]
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                return index + 1
        index += 1
    return -1


def read_identifier(source: str, start: int) -> tuple[str, int]:
    end = start
    while end < len(source) and (source[end].isalnum() or source[end] == "_"):
        end += 1
    return source[start:end], end


def parse_struct_names(source: str) -> set[str]:
    return set(re.findall(r"\bstruct\s+([A-Za-z_][A-Za-z0-9_]*)", source))


def receiver_type(signature: str) -> str | None:
    open_paren = signature.find("(")
    close_paren = signature.find(")", open_paren + 1)
    if open_paren < 0 or close_paren < 0:
        return None
    first = signature[open_paren + 1:close_paren].split(",", 1)[0].strip()
    if ":" not in first:
        return None
    name, typ = [part.strip() for part in first.split(":", 1)]
    return typ if name == "self" else None


def first_parameter_type(signature: str) -> str | None:
    open_paren = signature.find("(")
    close_paren = signature.find(")", open_paren + 1)
    if open_paren < 0 or close_paren < 0:
        return None
    first = signature[open_paren + 1:close_paren].split(",", 1)[0].strip()
    if ":" not in first:
        return None
    return first.split(":", 1)[1].strip()


def owner_for_function(file: str, name: str, signature: str, structs: set[str]) -> str:
    if name in {"main", "init", "tick", "render", "on_code_swap"}:
        return "Main"
    receiver = receiver_type(signature)
    if receiver in structs:
        return receiver
    first_type = first_parameter_type(signature)
    if first_type in structs:
        return first_type
    if file.startswith("src/systems/"):
        return Path(file).stem.title()
    return "Root"


def parse_symbols(project: Path) -> list[Symbol]:
    files = sorted(project.glob("src/**/*.stasis"))
    structs: set[str] = set()
    sources: dict[str, str] = {}
    for file_path in files:
        rel = file_path.relative_to(project).as_posix()
        source = read_text(file_path)
        sources[rel] = source
        structs.update(parse_struct_names(source))

    symbols: list[Symbol] = []
    for rel, source in sources.items():
        cursor = 0
        while cursor < len(source):
            matches = [(source.find(token, cursor), token) for token in ("struct ", "global ", "function ")]
            matches = [(idx, token) for idx, token in matches if idx >= 0]
            if not matches:
                break
            start, token = min(matches, key=lambda item: item[0])
            name, name_end = read_identifier(source, start + len(token))
            body_start = source.find("{", name_end)
            end = find_matching_brace(source, body_start) if body_start >= 0 else -1
            if not name or body_start < 0 or end < 0:
                cursor = start + len(token)
                continue
            full_source = source[start:end]
            if token == "struct ":
                symbols.append(Symbol("struct", name, name, rel, f"struct {name}", full_source, start, end))
            elif token == "global ":
                symbols.append(Symbol("global", name, "Globals", rel, f"global {name}", full_source, start, end))
            else:
                signature = source[start + len(token):body_start].strip()
                func_name = signature.split("(", 1)[0].strip()
                owner = owner_for_function(rel, func_name, signature, structs)
                symbols.append(Symbol("function", func_name, owner, rel, signature, full_source, start, end))
            cursor = end
    return symbols


def symbol_json(symbol: Symbol, include_source: bool) -> dict[str, Any]:
    result: dict[str, Any] = {
        "kind": symbol.kind,
        "name": symbol.name,
        "owner": symbol.owner,
        "file": symbol.file,
        "signature": symbol.signature,
    }
    if include_source:
        result["source"] = symbol.source
        if symbol.kind == "global":
            body = symbol.source[symbol.source.find("{"):]
            result["backing_struct_source"] = f"struct {symbol.name} {body}"
    return result


DEFAULT_MODEL_PRICING_PER_MILLION = {
    "gpt-5.6-sol": {
        "input": 5.00,
        "cached_input": 0.50,
        "cache_write": 6.25,
        "output": 30.00,
        "source": "User-provided pricing on 2026-07-10: gpt-5.6-sol $5.00 input / $0.50 cached input / $30.00 output per 1M tokens; cache writes use the documented GPT-5.6 1.25x input rate.",
    }
}

def response_usage_from_body(body: dict[str, Any]) -> dict[str, int]:
    usage = body.get("usage") if isinstance(body, dict) else {}
    if not isinstance(usage, dict):
        usage = {}
    input_tokens = int(usage.get("input_tokens") or usage.get("prompt_tokens") or 0)
    output_tokens = int(usage.get("output_tokens") or usage.get("completion_tokens") or 0)
    total_tokens = int(usage.get("total_tokens") or input_tokens + output_tokens)
    input_details = usage.get("input_tokens_details") or usage.get("prompt_tokens_details") or {}
    if not isinstance(input_details, dict):
        input_details = {}
    cached_tokens = int(input_details.get("cached_tokens") or 0)
    cache_write_tokens = int(input_details.get("cache_write_tokens") or 0)
    return {
        "input_tokens": input_tokens,
        "cached_input_tokens": cached_tokens,
        "cache_write_input_tokens": cache_write_tokens,
        "uncached_input_tokens": max(input_tokens - cached_tokens - cache_write_tokens, 0),
        "output_tokens": output_tokens,
        "total_tokens": total_tokens,
    }


def aggregate_trace_usage(trace_events: list[dict[str, Any]], model: str) -> dict[str, Any]:
    totals = {"input_tokens": 0, "cached_input_tokens": 0, "cache_write_input_tokens": 0, "uncached_input_tokens": 0, "output_tokens": 0, "total_tokens": 0}
    calls = 0
    per_call = []
    for event in trace_events:
        if event.get("kind") == "openai_exchange":
            usage = event.get("usage", {})
            if not isinstance(usage, dict):
                usage = {}
        else:
            continue
        calls += 1
        per_call.append({"turn": event.get("turn"), **usage})
        for key in totals:
            totals[key] += int(usage.get(key) or 0)
    pricing = DEFAULT_MODEL_PRICING_PER_MILLION.get(model, {})
    cost = None
    if pricing:
        cost = (
            totals["uncached_input_tokens"] * float(pricing["input"])
            + totals["cached_input_tokens"] * float(pricing["cached_input"])
            + totals["cache_write_input_tokens"] * float(pricing.get("cache_write", pricing["input"]))
            + totals["output_tokens"] * float(pricing["output"])
        ) / 1_000_000.0
    return {
        "calls": calls,
        "totals": totals,
        "per_call": per_call,
        "estimated_cost_usd": cost,
        "pricing_per_million_tokens": pricing or None,
        "pricing_note": pricing.get("source") if pricing else "No pricing estimate configured for this model.",
    }


def write_trace_file(path: Path, meta: dict[str, Any], trace_events: list[dict[str, Any]], exit_code: int, started_at_iso: str, elapsed_seconds: float, total_actions: int) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    usage_summary = aggregate_trace_usage(trace_events, meta.get("model", ""))
    payload = {
        "meta": {**meta, "started_at": started_at_iso, "elapsed_seconds": elapsed_seconds, "total_actions": total_actions},
        "usage_summary": usage_summary,
        "events": trace_events,
        "exit_code": exit_code,
    }
    write_text(path, json.dumps(payload, indent=2))


def default_trace_file() -> Path:
    timestamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    return DEFAULT_TRACE_DIR / f"android_ai_run_{timestamp}.json"

def run_command(args: list[str]) -> dict[str, Any]:
    environment = os.environ.copy()
    environment["CARGO_INCREMENTAL"] = "0"
    proc = subprocess.run(args, cwd=ROOT, env=environment, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    return {"ok": proc.returncode == 0, "returncode": proc.returncode, "output_tail": proc.stdout[-6000:]}


def run_compile_check() -> dict[str, Any]:
    return run_command(["cargo", "test", "-p", "stasis_android_bridge", "android_bundled_touch_pong_sample_compile_plan_is_runnable", "--", "--nocapture"])


def behavior_test_expectations() -> dict[str, Any]:
    return {
        "test_name": "android_bundled_touch_pong_enemy_paddle_speed_schedule_is_linear",
        "required_file": "tests/enemy_paddle_speed_schedule.test.stasis",
        "syntax": "test `descriptive name`(): bool { if (condition) { return false; } return true; }",
        "expected_checks": ["1500 at ball age 0", "875 at ball age 1800", "250 at ball age 3600 and above", "reset_ball restores age 0 and speed 1500"],
        "implementation_hint": "Use persistent GameState fields for ball_age_ticks and enemy_paddle_speed_x100. Reset ball_age_ticks in reset_ball(), increment it once per tick/update_ball while the ball is alive, and update enemy_paddle_speed_x100 from ball_age_ticks before moving the enemy paddle. Clamp enemy_paddle_speed_x100 so it never drops below 250 after 60 seconds.",
    }


def test_file_path(project: Path, file: str) -> Path:
    relative = Path(file.replace("\\", "/"))
    if relative.is_absolute() or ".." in relative.parts or not relative.parts or relative.parts[0] != "tests":
        raise ValueError("test file path must be under tests/")
    return project / relative


def list_test_files(project: Path) -> list[dict[str, Any]]:
    root = project / "tests"
    if not root.is_dir():
        return []
    files: list[dict[str, Any]] = []
    for path in sorted(root.rglob("*")):
        if not path.is_file():
            continue
        relative = path.relative_to(project).as_posix()
        files.append({
            "file": relative,
            "kind": "stasis_test" if relative.endswith(".test.stasis") else "unknown",
            "runnable_on_host": relative.endswith(".test.stasis"),
        })
    return files


def write_test_file(project: Path, file: str, source: str) -> dict[str, Any]:
    if not source.strip():
        return {"status": "validation_error", "file": file, "error": "write_test_file requires non-empty source"}
    if not file.endswith(".test.stasis"):
        return {"status": "validation_error", "file": file, "error": "AI tests must use the .test.stasis extension and return bool from each test declaration"}
    path = test_file_path(project, file)
    path.parent.mkdir(parents=True, exist_ok=True)
    if not re.search(r"\btest\s+`[^`]+`\s*\(\s*\)\s*:\s*bool\s*\{", source):
        return {"status": "validation_error", "file": file, "error": "test source must declare test `name`(): bool { ... } and return true or false"}
    write_text(path, source.rstrip() + "\n")
    return {"status": "written", "file": path.relative_to(project).as_posix(), "kind": "stasis_test", "runnable_on_host": True}


def read_test_file(project: Path, file: str) -> dict[str, Any]:
    path = test_file_path(project, file)
    return {"file": path.relative_to(project).as_posix(), "exists": path.is_file(), "source": read_text(path) if path.is_file() else ""}


def reset_paddle_speed_feature(project: Path) -> None:
    path = project / "src/main.stasis"
    source = read_text(path)
    source = re.sub(r"\n    ball_age_ticks: i32;\n    enemy_paddle_speed_x100: i32;", "", source)
    source = re.sub(r"\n    GameState\.ball_age_ticks = 0;\n    GameState\.enemy_paddle_speed_x100 = 1500;", "", source)
    source = re.sub(
        r"function update_enemy_paddle\(\): void \{.*?\n\}\n\nfunction update_ball",
        "function update_enemy_paddle(): void {\n    let enemy_speed = 4;\n\n    if (GameState.ai_y < GameState.ball_y) {\n        GameState.ai_y += enemy_speed;\n    }\n\n    if (GameState.ai_y > GameState.ball_y) {\n        GameState.ai_y -= enemy_speed;\n    }\n\n    if (GameState.ai_y < 36) {\n        GameState.ai_y = 36;\n    }\n\n    if (GameState.ai_y > GameState.screen_h - 36) {\n        GameState.ai_y = GameState.screen_h - 36;\n    }\n}\n\nfunction update_ball",
        source,
        count=1,
        flags=re.S,
    )
    source = re.sub(
        r"function update_ball\(\): void \{\n    GameState\.ball_age_ticks \+= 1;\n    GameState\.enemy_paddle_speed_x100 = get_enemy_paddle_speed_x100\(GameState\.ball_age_ticks\);\n\n",
        "function update_ball(): void {\n",
        source,
        count=1,
    )
    source = re.sub(r"\n    GameState\.ball_age_ticks = 0;\n    GameState\.enemy_paddle_speed_x100 = 1500;", "", source)
    source = re.sub(r"\n\nfunction get_enemy_paddle_speed_x100\(ball_age_ticks: i32\): i32 \{.*?\n\}\s*$", "\n", source, flags=re.S)
    write_text(path, source.rstrip() + "\n")
    for generated_test in (project / "tests").glob("*enemy_paddle_speed*.test.stasis"):
        generated_test.unlink()

def run_behavior_tests(project: Path) -> dict[str, Any]:
    result = run_command(["cargo", "run", "-p", "stasis", "--release", "--", "test", "--dir", str(project / "tests")])
    tests = list_test_files(project)
    stasis_tests = [test for test in tests if test.get("kind") == "stasis_test"]
    result["test_files"] = tests
    result["stasis_test_file_count"] = len(stasis_tests)
    if not stasis_tests:
        result["ok"] = False
        result["missing_test"] = "Add or update at least one tests/*.test.stasis file that returns bool before returning done."
    if not result.get("ok"):
        result["behavior_test_expectations"] = behavior_test_expectations()
    return result

def validate_single_replacement_source(name: str, new_source: str) -> tuple[bool, str]:
    stripped = new_source.strip()
    if stripped.startswith("function "):
        keyword = "function"
    elif stripped.startswith("struct "):
        keyword = "struct"
    elif stripped.startswith("global "):
        keyword = "global"
    else:
        return False, "replacement source must start with function, struct, or global"
    declared, _ = read_identifier(stripped, len(keyword) + 1)
    if declared != name:
        return False, f"replacement source defines {declared}, expected {name}"
    body_start = stripped.find("{")
    body_end = find_matching_brace(stripped, body_start) if body_start >= 0 else -1
    if body_start < 0 or body_end != len(stripped):
        return False, "replacement source must contain exactly one top-level declaration"
    body = stripped[body_start + 1:body_end - 1]
    if "function " in body or "struct " in body or "global " in body:
        return False, "replacement body must not contain nested function, struct, or global declarations"
    return True, "ok"

def delete_symbol(project: Path, name: str, file: str = "", owner: str = "", kind: str = "", compile_after_write: bool = True) -> dict[str, Any]:
    symbols = [s for s in parse_symbols(project) if s.name == name]
    if file:
        symbols = [s for s in symbols if s.file == file]
    if owner:
        symbols = [s for s in symbols if s.owner == owner]
    if kind:
        expected = "global" if kind == "global" else "struct" if kind in {"struct", "replace_struct"} else "function" if kind in {"function", "replace_function"} else kind
        symbols = [s for s in symbols if s.kind == expected]
    if not symbols:
        return {"status": "not_found", "name": name, "available_names": [s.name for s in parse_symbols(project)]}
    if len(symbols) > 1:
        return {"status": "ambiguous", "name": name, "matches": [symbol_json(s, False) for s in symbols]}
    target = symbols[0]
    path = project / target.file
    source = read_text(path)
    updated = source[:target.start].rstrip() + "\n\n" + source[target.end:].lstrip()
    write_text(path, updated.rstrip() + "\n")
    if not compile_after_write:
        return {"status": "deleted", "file": target.file, "name": target.name, "kind": target.kind, "compile": {"status": "pending_batch_compile"}}
    compile_result = run_compile_check()
    if not compile_result.get("ok"):
        write_text(path, source)
        return {"status": "rolled_back", "file": target.file, "name": target.name, "kind": target.kind, "compile": compile_result}
    return {"status": "deleted", "file": target.file, "name": target.name, "kind": target.kind, "compile": compile_result}


def delete_test_file(project: Path, file: str) -> dict[str, Any]:
    path = test_file_path(project, file)
    if not path.is_file():
        return {"status": "not_found", "file": path.relative_to(project).as_posix()}
    path.unlink()
    return {"status": "deleted", "file": path.relative_to(project).as_posix()}

def replace_symbol(project: Path, name: str, file: str, new_source: str, compile_after_write: bool = True) -> dict[str, Any]:
    valid, validation_message = validate_single_replacement_source(name, new_source)
    if not valid:
        return {"status": "validation_error", "file": file, "name": name, "error": validation_message}
    path = project / file
    source = read_text(path)
    symbols = [s for s in parse_symbols(project) if s.file == file and s.name == name]
    if symbols:
        target = symbols[0]
        updated = source[:target.start] + new_source.rstrip() + source[target.end:]
        status = "written"
    else:
        updated = source.rstrip() + "\n\n" + new_source.rstrip() + "\n"
        status = "created"
    write_text(path, updated)
    if not compile_after_write:
        return {"status": status, "file": file, "name": name, "compile": {"status": "pending_batch_compile"}}
    compile_result = run_compile_check()
    if not compile_result["ok"]:
        write_text(path, source)
        return {"status": "rolled_back", "file": file, "name": name, "compile": compile_result}
    return {"status": status, "file": file, "name": name, "compile": compile_result}


def write_project_file(project: Path, file: str, source: str, compile_after_write: bool = True) -> dict[str, Any]:
    path = project / file
    if not path.is_file():
        return {"status": "not_found", "file": file}
    original = read_text(path)
    write_text(path, source.rstrip() + "\n")
    if not compile_after_write:
        return {"status": "written", "file": file, "compile": {"status": "pending_batch_compile"}}
    compile_result = run_compile_check()
    if not compile_result["ok"]:
        write_text(path, original)
        return {"status": "rolled_back", "file": file, "compile": compile_result}
    return {"status": "written", "file": file, "compile": compile_result}


def tool_specs() -> list[dict[str, Any]]:
    def spec(tool: str, purpose: str, required: list[str], optional: list[str], args: dict[str, Any]) -> dict[str, Any]:
        return {"tool": tool, "purpose": purpose, "required_args": required, "optional_args": optional, "example": {"tool": tool, "args": args}}
    return [
        spec("list_symbols", "List editable symbols compactly.", [], [], {}),
        spec("list_owner_symbols", "List symbols and preferred receiver calls for one owner/type.", ["owner"], [], {"owner": "GameState"}),
        spec("read_symbol", "Read one symbol source.", ["name"], ["kind", "file", "owner"], {"name": "update_enemy_paddle"}),
        spec("write_symbol", "Create or replace exactly one Stasis function/global/struct. Writes in one tool-call batch compile together after all batch tools run and roll back together on compile failure. The new_source must not contain additional top-level or nested declarations.", ["file", "name", "new_source"], ["kind", "owner"], {"file": "src/main.stasis", "name": "tick", "new_source": "function tick(): void {\n}"}),
        spec("delete_symbol", "Delete exactly one Stasis function/global/struct by name, with optional file/owner/kind disambiguation. Source deletes batch-compile and roll back on compile failure.", ["name"], ["file", "owner", "kind"], {"name": "unused_helper", "file": "src/main.stasis", "kind": "function"}),
        spec("list_tests", "List test files under tests/.", [], [], {}),
        spec("read_test_file", "Read one Stasis test file under tests/.", ["file"], [], {"file": "tests/paddle_speed.test.stasis"}),
        spec("write_test_file", "Create or replace a real Stasis test under tests/. Use test `name`(): bool and return true or false; never write JSON assertions or assert_runtime helpers.", ["file", "source"], [], {"file": "tests/paddle_speed.test.stasis", "source": "import \"../src/main.stasis\";\n\ntest `enemy paddle speed`(): bool {\n    init();\n    update_enemy_paddle();\n    return GameState.enemy_paddle_speed_x100 == 1500;\n}"}),
        spec("delete_test_file", "Delete one obsolete or duplicate Stasis test file under tests/.", ["file"], [], {"file": "tests/obsolete.test.stasis"}),
        spec("run_tests", "Run the one-shot local Stasis JIT test runner (never watch mode). This is successful only when real tests/*.test.stasis declarations return true.", [], [], {}),
        spec("get_diagnostics", "Return last local diagnostics.", [], [], {}),
    ]



def response_contract() -> dict[str, Any]:
    return {
        "required": "Return exactly one JSON object. Include user-visible working_notes of at most 2000 characters.",
        "accepted_response_shapes": [
            {"mode": "tool_calls", "working_notes": "Intent: inspect. Observed: facts. Next: action. Blocker: none.", "summary": "short optional status", "tool_calls": [{"tool": "read_symbol", "args": {"name": "tick"}}]},
            {"mode": "done", "working_notes": "Intent: finish. Observed: verified. Next: none. Blocker: none.", "summary": "what was verified"},
            {"mode": "edits", "working_notes": "Intent: finish. Observed: writes and tests passed. Next: apply. Blocker: none.", "summary": "short change summary", "edits": [{"kind": "replace_function", "owner": "Player", "name": "jump", "file": "src/player.stasis", "new_source": "function jump(self: Player): void {\n}"}]},
        ],
        "tool_call_rules": [
            "Use the exact top-level property tool_calls for tool use.",
            "Do not use read_file; inspect source with list_symbols/list_owner_symbols/read_symbol and inspect tests with list_tests/read_test_file.",
            "Each tool call must contain exactly tool and args.",
            "tool must be a non-empty string matching one entry in tool_specs.",
            "args must be an object containing that tool's documented arguments.",
        ],
        "invalid_aliases": {
            "calls": "Use tool_calls instead.",
            "name": "Inside each tool call, use tool instead.",
            "function": "Inside each tool call, use tool instead.",
            "arguments": "Inside each tool call, use args instead.",
            "type": "Do not use type for tool calls.",
            "source": "For write_symbol, use new_source instead.",
        },
    }


def is_simple_tuning_prompt(prompt: str) -> bool:
    normalized = prompt.strip().lower()
    if not normalized or len(normalized) > 160 or "\n" in normalized:
        return False
    if any(term in normalized for term in ("add a new", "create a new", "multiplayer", "network", "system", "level editor")):
        return False
    return any(term in normalized for term in (
        "size", "width", "height", "bigger", "smaller", "double", "half",
        "increase", "decrease", "faster", "slower", "speed", "color", "position",
    ))

def build_shared_context(project: Path, prompt: str) -> dict[str, Any]:
    symbols = parse_symbols(project)
    globals_payload = []
    for symbol in symbols:
        if symbol.kind == "global":
            body = symbol.source[symbol.source.find("{"):]
            globals_payload.append({"kind": "global", "name": symbol.name, "file": symbol.file, "backing_struct_type": symbol.name, "backing_struct_source": f"struct {symbol.name} {body}"})
    compact_symbols: list[dict[str, Any]] = []
    serialized_chars = 2
    for symbol in symbols:
        compact = symbol_json(symbol, False)
        candidate_chars = len(json.dumps(compact, separators=(",", ":")))
        separator_chars = 0 if not compact_symbols else 1
        if (len(compact_symbols) >= MAX_INITIAL_SYMBOLS
                or serialized_chars + separator_chars + candidate_chars > MAX_INITIAL_SYMBOL_INDEX_CHARS):
            break
        serialized_chars += separator_chars + candidate_chars
        compact_symbols.append(compact)
    fast_path_enabled = is_simple_tuning_prompt(prompt)
    fast_path_symbols: list[dict[str, Any]] = []
    fast_path_chars = 2
    if fast_path_enabled:
        for symbol in symbols:
            source_symbol = symbol_json(symbol, True)
            candidate_chars = len(json.dumps(source_symbol, separators=(",", ":")))
            separator_chars = 0 if not fast_path_symbols else 1
            if (len(fast_path_symbols) >= MAX_FAST_PATH_SYMBOLS
                    or fast_path_chars + separator_chars + candidate_chars > MAX_FAST_PATH_SOURCE_CHARS):
                break
            fast_path_chars += separator_chars + candidate_chars
            fast_path_symbols.append(source_symbol)
    return {
        "cache_layout": "Stable shared context is first. Volatile per-turn tool observations live in turn_state after this object.",
        "stasis_basics": {
            "language": [
                "Declare typed arguments and returns with function name(arg_name: Type, other: Type): ReturnType { ... }; use void when there is no return value.",
                "Declare value types with struct TypeName { field_name: Type; ... } and access fields as value.field_name.",
                "A persistent instance of a struct is normally declared with global instance_name: StructType; for example global state: GameState; then access state.score.",
                "A direct named global block, global Name { field_name: Type; ... }, is also valid and is accessed as Name.field_name; do not confuse it with a struct type declaration.",
                "Arithmetic, comparison, and assignment operators are infix.",
                "Receiver calls value.method(args) are preferred when the first parameter is self: Type; function-form calls remain valid.",
                "Common types are bool, i32, u8, u16, u32, f32, f64, void, fixed arrays Type[N], and bounded text ascii[N] or utf8[N].",
            ],
            "runtime": [
                "main initializes state, tick advances deterministic fixed-tick simulation, and render projects current state into render commands.",
                "on_code_swap is only for migration or reinitialization required after a hot swap.",
                "Gameplay progression is tick-based rather than dt-based.",
            ],
            "editing": [
                "Functions, structs, globals, imports, and tests are editable units; inspect a symbol before replacing it.",
                "Function-body and tuning changes can fast reload; struct/global layout changes require reset compatibility handling.",
            ],
            "tests": ["Behavior tests use test `name`(): bool and return true or false."],
        },
        "protocol": {
            "response_contract": response_contract(),
            "available_tools": [s["tool"] for s in tool_specs()],
            "tool_specs": tool_specs(),
            "stasis_style_rules": {"use_function_keyword": True, "use_receiver_style_when_possible": True, "do_not_use_rust_references": True},
        },
        "workflow_rules": {
            "behavior_test_expectations": behavior_test_expectations(),
            "architecture_recommendations": [
                "write_symbol writes are compiled once after the whole tool-call batch, so a batch may create helpers and edit callers together.",
                "Use lifecycle-local state for time since creation; reset it in reset/create functions and increment it during tick.",
                "Use on_code_swap() for post-hot-swap migration or reinitialization if running state needs adjustment.",
                "Make the smallest structural change with clear state fields and testable invariants.",
            ],
        },
        "user_request": {"scope": "entire_workspace", "user_prompt": prompt},
        "project_context": {
            "project_globals": globals_payload,
            "project_symbol_index": {
                "symbols": compact_symbols,
                "included_count": len(compact_symbols),
                "available_count": len(symbols),
                "truncated": len(compact_symbols) < len(symbols),
            },
            "fast_path": {
                "enabled": fast_path_enabled,
                "reason": "short tuning request" if fast_path_enabled else "standard agent path",
                "source_symbols": fast_path_symbols,
                "included_count": len(fast_path_symbols),
                "available_count": len(symbols),
                "truncated": fast_path_enabled and len(fast_path_symbols) < len(symbols),
                "instruction": "Use these bounded current sources to write the complete change and a behavior test in the first response; do not spend a read-only turn when the needed source is present." if fast_path_enabled else "Use the normal inspect-write-test loop.",
            },
            "selected_symbols": [],
            "selected_symbols_are_context_only": True,
        },
    }


def build_agent_request(project: Path, prompt: str, turn_state: dict[str, Any]) -> dict[str, Any]:
    return {
        "shared_context": build_shared_context(project, prompt),
        "turn_state": turn_state,
    }


def retain_working_notes(turn_state: dict[str, Any], working_notes: str) -> dict[str, Any]:
    retained = dict(turn_state)
    if working_notes:
        retained["working_notes"] = working_notes
    return retained


def parse_json_object(text: str) -> dict[str, Any]:
    stripped = text.strip()
    try:
        return json.loads(stripped)
    except json.JSONDecodeError:
        pass
    start = stripped.find("{")
    if start < 0:
        raise ValueError(f"No JSON object found in model response: {stripped[:200]}")
    depth = 0
    in_string = False
    escape = False
    for index in range(start, len(stripped)):
        ch = stripped[index]
        if escape:
            escape = False
            continue
        if ch == "\\":
            escape = True
            continue
        if ch == '"':
            in_string = not in_string
            continue
        if in_string:
            continue
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                return json.loads(stripped[start:index + 1])
    raise ValueError(f"Unclosed JSON object in model response: {stripped[:200]}")


def response_json_schema() -> dict[str, Any]:
    return {
        "type": "object",
        "properties": {
            "mode": {"type": "string"},
            "working_notes": {"type": "string", "minLength": 1, "maxLength": MAX_WORKING_NOTES_CHARS},
            "summary": {"type": "string"},
            "tool_calls": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "tool": {"type": "string"},
                        "args": {"type": "object", "additionalProperties": True},
                    },
                    "required": ["tool", "args"],
                    "additionalProperties": False,
                },
            },
            "edits": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "kind": {"type": "string"},
                        "owner": {"type": "string"},
                        "name": {"type": "string"},
                        "file": {"type": "string"},
                        "new_source": {"type": "string"},
                    },
                    "required": ["kind", "name", "file", "new_source"],
                    "additionalProperties": False,
                },
            },
        },
        "required": ["mode", "working_notes"],
        "additionalProperties": False,
    }

def summarize_request_for_trace(request: dict[str, Any]) -> dict[str, Any]:
    shared_context = request.get("shared_context", {})
    protocol = shared_context.get("protocol", {}) if isinstance(shared_context, dict) else {}
    project_context = shared_context.get("project_context", {}) if isinstance(shared_context, dict) else {}
    turn_state = request.get("turn_state", {})
    globals_payload = project_context.get("project_globals", []) if isinstance(project_context, dict) else []
    selected_symbols = project_context.get("selected_symbols", []) if isinstance(project_context, dict) else []
    symbol_index = project_context.get("project_symbol_index", {}) if isinstance(project_context, dict) else {}
    return {
        "shared_context_keys": sorted(shared_context.keys()) if isinstance(shared_context, dict) else [],
        "available_tools": protocol.get("available_tools", []) if isinstance(protocol, dict) else [],
        "project_global_count": len(globals_payload) if isinstance(globals_payload, list) else 0,
        "selected_symbol_count": len(selected_symbols) if isinstance(selected_symbols, list) else 0,
        "project_symbol_index_count": symbol_index.get("included_count", 0) if isinstance(symbol_index, dict) else 0,
        "project_symbol_index_truncated": symbol_index.get("truncated", False) if isinstance(symbol_index, dict) else False,
        "turn_phase": turn_state.get("phase") if isinstance(turn_state, dict) else None,
        "turn_state_keys": sorted(turn_state.keys()) if isinstance(turn_state, dict) else [],
        "cache_breakpoint_after": "shared_context",
    }


def summarize_response_for_trace(body: dict[str, Any], parsed: dict[str, Any]) -> dict[str, Any]:
    return {
        "response_id": body.get("id"),
        "response_model": body.get("model"),
        "status": body.get("status"),
        "usage": response_usage_from_body(body),
        "mode": parsed.get("mode"),
        "summary": parsed.get("summary", ""),
        "working_notes": parsed.get("working_notes", ""),
        "tool_call_count": len(parsed.get("tool_calls") or []),
        "edit_count": len(parsed.get("edits") or []),
        "response_keys": sorted(parsed.keys()),
    }


def build_openai_payload(model: str, request: dict[str, Any]) -> dict[str, Any]:
    stable_instruction = (
        "Return only one JSON object matching request.shared_context.protocol.response_contract exactly. "
        "Follow request.shared_context.stasis_basics as the authoritative language/runtime orientation. "
        "When request.shared_context.project_context.fast_path.enabled is true, use its current bounded source_symbols to write the small tuning change and a behavior test in the first response without redundant reads. "
        "Every response must include working_notes of at most 2000 characters with concise Intent, Observed, Next, and Blocker facts. These are user-visible state notes, not private chain-of-thought. "
        "The initial project_symbol_index is a compact source-free inventory; use it to choose a direct read_symbol target and do not call list_symbols when it already identifies the target. "
        "Use mode=tool_calls to inspect/write with the provided fine-grained symbol, import, and test tools. Do not use read_file; use list_symbols/list_owner_symbols/read_symbol/read_imports/list_tests/read_test_file instead. "
        "For tool calls, the top-level key is tool_calls and each call is exactly {\"tool\":\"name\",\"args\":{...}}. "
        "Do not use aliases such as calls, name, function, arguments, type, or source. "
        "After a tool-call batch with writes, compile runs locally once and failed compiles roll back the whole batch. Use run_tests to verify the behavior. "
        "For behavior-changing requests, add or update a tests/*.test.stasis test using test `name`(): bool and return true or false; assert_runtime helpers and JSON scenario tests are not Stasis syntax. Return mode=tool_calls for tools, mode=edits with edits if returning direct edits, or mode=done only when tests pass. "
        "Use Stasis syntax only."
    )
    shared_context = request.get("shared_context", {})
    turn_state = request.get("turn_state", {})
    schema = response_json_schema()
    return {
        "model": model,
        "reasoning": {"effort": "low" if shared_context.get("project_context", {}).get("fast_path", {}).get("enabled") else DEFAULT_REASONING_EFFORT},
        "prompt_cache_key": PROMPT_CACHE_KEY,
        "prompt_cache_options": {"mode": "explicit", "ttl": "30m"},
        "text": {"format": {"type": "json_schema", "name": "stasis_host_ai_response", "strict": False, "schema": schema}},
        "input": [
            {"role": "system", "content": [{"type": "input_text", "text": stable_instruction}]},
            {
                "role": "user",
                "content": [{
                    "type": "input_text",
                    "text": "Stable request.shared_context: " + json.dumps(shared_context, separators=(",", ":")),
                    "prompt_cache_breakpoint": {"mode": "explicit"},
                }],
            },
            {
                "role": "user",
                "content": [{"type": "input_text", "text": "Volatile request.turn_state: " + json.dumps(turn_state, separators=(",", ":"))}],
            },
        ],
    }


def validate_openai_payload(payload: dict[str, Any]) -> list[str]:
    errors: list[str] = []
    if not payload.get("prompt_cache_key"):
        errors.append("prompt_cache_key is required")
    if payload.get("prompt_cache_options") != {"mode": "explicit", "ttl": "30m"}:
        errors.append("prompt_cache_options must use explicit 30m caching")
    if payload.get("reasoning") not in ({"effort": "medium"}, {"effort": "low"}):
        errors.append("reasoning effort must be medium or bounded fast-path low")
    input_items = payload.get("input")
    if not isinstance(input_items, list) or len(input_items) != 3:
        errors.append("input must contain stable instruction, stable context, and volatile context messages")
        return errors
    stable_content = input_items[1].get("content") if isinstance(input_items[1], dict) else None
    if not isinstance(stable_content, list) or len(stable_content) != 1:
        errors.append("stable context must use one input_text content block")
    elif stable_content[0].get("prompt_cache_breakpoint") != {"mode": "explicit"}:
        errors.append("stable context must end with an explicit prompt cache breakpoint")
    if any(item.get("type") == "prompt_cache_breakpoint" for item in input_items if isinstance(item, dict)):
        errors.append("prompt cache breakpoints must be attached to input_text blocks")
    return errors


def summarize_openai_payload(payload: dict[str, Any]) -> dict[str, Any]:
    messages = []
    for item in payload.get("input", []):
        if not isinstance(item, dict):
            continue
        content = item.get("content", [])
        blocks = []
        if isinstance(content, list):
            for block in content:
                if isinstance(block, dict):
                    blocks.append({
                        "type": block.get("type"),
                        "text_length": len(str(block.get("text", ""))),
                        "prompt_cache_breakpoint": block.get("prompt_cache_breakpoint"),
                    })
        messages.append({"role": item.get("role"), "content": blocks})
    return {
        "model": payload.get("model"),
        "reasoning": payload.get("reasoning"),
        "prompt_cache_key": payload.get("prompt_cache_key"),
        "prompt_cache_options": payload.get("prompt_cache_options"),
        "messages": messages,
    }


def call_openai(api_key: str, model: str, request: dict[str, Any], trace_events: list[dict[str, Any]] | None = None, turn: int = 0) -> dict[str, Any]:
    payload = build_openai_payload(model, request)
    payload_errors = validate_openai_payload(payload)
    if payload_errors:
        raise RuntimeError("OpenAI payload preflight failed: " + "; ".join(payload_errors))
    if trace_events is not None:
        trace_events.append({"kind": "openai_request", "turn": turn, "payload": summarize_openai_payload(payload)})
    req = urllib.request.Request(
        "https://api.openai.com/v1/responses",
        data=json.dumps(payload).encode("utf-8"),
        headers={"Authorization": f"Bearer {api_key}", "Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=120) as response:
            body = json.loads(response.read().decode("utf-8"))
    except urllib.error.HTTPError as error:
        detail = error.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"OpenAI request failed with HTTP {error.code}: {detail}") from error
    except urllib.error.URLError as error:
        raise RuntimeError(f"OpenAI request failed: {error.reason}") from error
    text = body.get("output_text", "")
    if not text:
        chunks: list[str] = []
        for item in body.get("output", []):
            for content in item.get("content", []):
                if "text" in content:
                    chunks.append(content["text"])
        text = "".join(chunks)
    parsed = parse_json_object(text)
    if trace_events is not None:
        trace_events.append({"kind": "openai_exchange", "turn": turn, "request": summarize_request_for_trace(request), "response": summarize_response_for_trace(body, parsed), "usage": response_usage_from_body(body)})
    return parsed


def validate_response_shape(response: dict[str, Any]) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    errors: list[dict[str, Any]] = []
    contract = response_contract()
    mode = response.get("mode")
    working_notes = response.get("working_notes")
    if not isinstance(working_notes, str) or not working_notes.strip() or len(working_notes) > MAX_WORKING_NOTES_CHARS:
        errors.append({"kind": "validation_error", "error": "response requires nonempty string working_notes within 2000 characters", "maximum_characters": MAX_WORKING_NOTES_CHARS, "accepted_shape": "Intent: ... Observed: ... Next: ... Blocker: ..."})
        return [], errors
    if mode not in {"tool_calls", "done", "edits"}:
        errors.append({"kind": "validation_error", "error": "response requires top-level mode equal to tool_calls, done, or edits", "received_keys": sorted(response.keys()), "received_mode": mode, "response_contract": contract})
        return [], errors
    allowed_top_level = {"tool_calls": {"mode", "working_notes", "summary", "tool_calls"}, "done": {"mode", "working_notes", "summary"}, "edits": {"mode", "working_notes", "summary", "edits"}}[mode]
    extra_top_level = sorted(set(response.keys()) - allowed_top_level)
    if extra_top_level:
        errors.append({"kind": "validation_error", "error": "response contains unsupported top-level properties for this mode", "mode": mode, "unsupported_properties": extra_top_level, "accepted_top_level_properties": sorted(allowed_top_level), "response_contract": contract})
        return [], errors
    if mode == "done":
        return [], errors
    if mode == "edits":
        if not isinstance(response.get("edits"), list):
            errors.append({"kind": "validation_error", "error": "mode=edits requires top-level edits array", "received_edits_type": type(response.get("edits")).__name__, "response_contract": contract})
        return [], errors
    raw_calls = response.get("tool_calls")
    if not isinstance(raw_calls, list):
        errors.append({"kind": "validation_error", "error": "mode=tool_calls requires top-level tool_calls array", "received_tool_calls_type": type(raw_calls).__name__, "response_contract": contract})
        return [], errors
    normalized: list[dict[str, Any]] = []
    for index, call in enumerate(raw_calls):
        if not isinstance(call, dict):
            errors.append({"kind": "validation_error", "index": index, "error": "each tool call must be an object with tool and args"})
            continue
        extra = sorted(set(call.keys()) - {"tool", "args"})
        if extra:
            errors.append({"kind": "validation_error", "index": index, "error": "tool call contains unsupported top-level properties", "unsupported_properties": extra, "accepted_shape": {"tool": "read_symbol", "args": {"name": "tick"}}, "response_contract": contract})
            continue
        tool = call.get("tool")
        args = call.get("args")
        if not isinstance(tool, str) or not tool:
            errors.append({"kind": "validation_error", "index": index, "error": "tool call requires non-empty string property: tool"})
            continue
        if not isinstance(args, dict):
            errors.append({"kind": "validation_error", "index": index, "error": "tool call requires object property: args"})
            continue
        normalized.append({"tool": tool, "args": args})
    return normalized, errors
def summarize_observations(observations: list[dict[str, Any]]) -> list[dict[str, Any]]:
    summary = []
    for observation in observations:
        result = observation.get("result", {})
        compile_result = result.get("compile") if isinstance(result, dict) else None
        summary.append({
            "tool": observation.get("tool"),
            "args": observation.get("args"),
            "status": result.get("status") if isinstance(result, dict) else None,
            "compile_ok": compile_result.get("ok") if isinstance(compile_result, dict) else None,
            "test_ok": result.get("ok") if isinstance(result, dict) and observation.get("tool") == "run_tests" else None,
            "error_tail": (compile_result or result).get("output_tail", "")[-600:] if isinstance((compile_result or result), dict) else "",
        })
    return summary

def execute_tool_batch(project: Path, tool_calls: list[dict[str, Any]], last_diagnostics: dict[str, Any]) -> tuple[list[dict[str, Any]], dict[str, Any]]:
    original_sources: dict[Path, str] = {}
    observations: list[dict[str, Any]] = []
    pending_run_tests: list[int] = []
    wrote_source = False

    def snapshot_file(file: str) -> None:
        path = project / file
        if path.is_file() and path not in original_sources:
            original_sources[path] = read_text(path)

    for call in tool_calls:
        tool = call.get("tool", "")
        tool_args = call.get("args") or {}
        if tool == "run_tests" and wrote_source:
            observations.append({"tool": tool, "args": tool_args, "result": {"status": "pending_batch_compile"}})
            pending_run_tests.append(len(observations) - 1)
            continue
        if tool == "write_symbol":
            snapshot_file(tool_args.get("file", ""))
            result = replace_symbol(project, tool_args.get("name", ""), tool_args.get("file", ""), tool_args.get("new_source", ""), compile_after_write=False)
            wrote_source = result.get("status") in {"written", "created"} or wrote_source
        elif tool == "delete_symbol":
            target_file = tool_args.get("file", "")
            if not target_file:
                candidates = [s for s in parse_symbols(project) if s.name == tool_args.get("name", "")]
                if tool_args.get("owner", ""):
                    candidates = [s for s in candidates if s.owner == tool_args.get("owner", "")]
                if tool_args.get("kind", ""):
                    expected = "global" if tool_args.get("kind") == "global" else "struct" if tool_args.get("kind") in {"struct", "replace_struct"} else "function" if tool_args.get("kind") in {"function", "replace_function"} else tool_args.get("kind")
                    candidates = [s for s in candidates if s.kind == expected]
                if len(candidates) == 1:
                    target_file = candidates[0].file
            if target_file:
                snapshot_file(target_file)
            result = delete_symbol(project, tool_args.get("name", ""), target_file, tool_args.get("owner", ""), tool_args.get("kind", ""), compile_after_write=False)
            wrote_source = result.get("status") == "deleted" or wrote_source
        elif tool == "write_file":
            snapshot_file(tool_args.get("file", ""))
            result = write_project_file(project, tool_args.get("file", ""), tool_args.get("source", ""), compile_after_write=False)
            wrote_source = result.get("status") == "written" or wrote_source
        else:
            result = execute_tool(project, tool, tool_args, last_diagnostics)
        observations.append({"tool": tool, "args": tool_args, "result": result})

    batch_compile: dict[str, Any] | None = None
    if wrote_source:
        batch_compile = run_compile_check()
        if not batch_compile.get("ok"):
            for path, source in original_sources.items():
                write_text(path, source)
            restored_compile = run_compile_check()
            for observation in observations:
                if observation.get("tool") in {"write_symbol", "delete_symbol", "write_file"}:
                    result = observation.get("result", {})
                    if isinstance(result, dict) and result.get("status") in {"written", "created", "deleted"}:
                        result["status"] = "rolled_back"
                        result["compile"] = batch_compile
                        result["restored_compile"] = restored_compile
                if observation.get("tool") == "run_tests":
                    observation["result"] = {"status": "blocked_by_compile_failure", "compile": batch_compile}
            return observations, batch_compile
        for observation in observations:
            if observation.get("tool") in {"write_symbol", "delete_symbol", "write_file"}:
                result = observation.get("result", {})
                if isinstance(result, dict) and result.get("status") in {"written", "created", "deleted"}:
                    result["compile"] = batch_compile

    latest_diagnostics = batch_compile or last_diagnostics
    for index in pending_run_tests:
        test_result = run_behavior_tests(project)
        observations[index]["result"] = test_result
        latest_diagnostics = test_result
    return observations, latest_diagnostics
def execute_tool(project: Path, tool: str, args: dict[str, Any], last_diagnostics: dict[str, Any]) -> dict[str, Any]:
    symbols = parse_symbols(project)
    if tool == "list_symbols":
        return {"symbols": [symbol_json(s, False) for s in symbols]}
    if tool == "list_owner_symbols":
        owner = args.get("owner", "")
        owned = [s for s in symbols if s.owner == owner or s.name == owner]
        return {"owner": owner, "symbols": [symbol_json(s, False) for s in owned]}
    if tool == "read_symbol":
        candidates = [s for s in symbols if s.name == args.get("name")]
        if args.get("file"):
            candidates = [s for s in candidates if s.file == args.get("file")]
        if args.get("owner"):
            candidates = [s for s in candidates if s.owner == args.get("owner")]
        if not candidates:
            return {"status": "not_found", "name": args.get("name"), "available_names": [s.name for s in symbols]}
        if len(candidates) > 1:
            return {"status": "ambiguous", "matches": [symbol_json(s, False) for s in candidates]}
        return symbol_json(candidates[0], True)
    if tool == "list_tests":
        return {"test_count": len(list_test_files(project)), "files": list_test_files(project)}
    if tool == "read_test_file":
        return read_test_file(project, args.get("file", ""))
    if tool == "write_test_file":
        return write_test_file(project, args.get("file", ""), args.get("source", ""))
    if tool == "write_symbol":
        return replace_symbol(project, args.get("name", ""), args.get("file", ""), args.get("new_source", ""))
    if tool == "delete_symbol":
        return delete_symbol(project, args.get("name", ""), args.get("file", ""), args.get("owner", ""), args.get("kind", ""))
    if tool == "delete_test_file":
        return delete_test_file(project, args.get("file", ""))
    if tool == "write_file":
        return write_project_file(project, args.get("file", ""), args.get("source", ""))
    if tool == "run_tests":
        return run_behavior_tests(project)
    if tool == "get_diagnostics":
        return last_diagnostics
    return {"status": "unsupported_tool", "tool": tool, "supported_tools": [s["tool"] for s in tool_specs()]}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--prompt", required=True)
    parser.add_argument("--project-root", type=Path, default=DEFAULT_PROJECT)
    parser.add_argument("--model", default=DEFAULT_MODEL)
    parser.add_argument("--reset-paddle-speed-feature", action="store_true", help="Reset the bundled Pong sample to the baseline before the requested paddle-speed feature.")
    parser.add_argument("--trace-file", type=Path, help="Write the run trace JSON to this file; defaults to artifacts/android_ai_runs/.")
    parser.add_argument("--preflight", action="store_true", help="Validate the outgoing Responses payload locally without calling OpenAI or editing the project.")
    args = parser.parse_args()
    load_env_file(ROOT / ".env")
    project = args.project_root.resolve()
    trace_file = args.trace_file or default_trace_file()
    request = build_agent_request(project, args.prompt, {"phase": "initial", "instruction": "Inspect the workspace with fine-grained tools, make the requested behavior change, add or update tests, run tests, then return done only after tests pass."})
    trace_events: list[dict[str, Any]] = []
    trace_meta = {"prompt": args.prompt, "model": args.model, "project_root": str(project), "reset_paddle_speed_feature": bool(args.reset_paddle_speed_feature)}
    trace_events.append({"kind": "trace_meta", "meta": trace_meta})
    trace_events.append({"kind": "initial_request", "summary": summarize_request_for_trace(request)})
    print(f"Trace file: {trace_file}")
    started_at_iso = datetime.now(timezone.utc).isoformat()
    started_at_perf = time.perf_counter()
    if args.preflight:
        payload = build_openai_payload(args.model, request)
        errors = validate_openai_payload(payload)
        trace_events.append({"kind": "payload_preflight", "ok": not errors, "errors": errors,
                             "payload": summarize_openai_payload(payload)})
        write_trace_file(trace_file, trace_meta, trace_events, 0 if not errors else 1, started_at_iso, time.perf_counter() - started_at_perf, 0)
        print(json.dumps({"preflight_ok": not errors, "errors": errors}, indent=2))
        return 0 if not errors else 1
    api_key = os.environ.get("OPENAI_API_KEY")
    if not api_key:
        trace_events.append({"kind": "api_error", "error": "OPENAI_API_KEY is required"})
        write_trace_file(trace_file, trace_meta, trace_events, 2, started_at_iso, time.perf_counter() - started_at_perf, 0)
        print("OPENAI_API_KEY is required", file=sys.stderr)
        return 2
    if args.reset_paddle_speed_feature:
        reset_paddle_speed_feature(project)
    last_diagnostics: dict[str, Any] = {}
    total_actions = 0
    working_notes = ""
    for turn in range(1, MAX_TURNS + 1):
        try:
            response = call_openai(api_key, args.model, request, trace_events, turn)
        except Exception as error:
            trace_events.append({"kind": "api_error", "turn": turn, "error_type": type(error).__name__, "error": str(error)})
            write_trace_file(trace_file, trace_meta, trace_events, 1, started_at_iso, time.perf_counter() - started_at_perf, total_actions)
            print(json.dumps({"error": str(error), "trace_file": str(trace_file)}, indent=2), file=sys.stderr)
            return 1
        mode = response.get("mode")
        tool_calls, response_validation_errors = validate_response_shape(response)
        print(json.dumps({"turn": turn, "mode": mode, "working_notes": response.get("working_notes", ""), "response_keys": sorted(response.keys()), "summary": response.get("summary", ""), "tool_call_count": len(tool_calls), "validation_error_count": len(response_validation_errors), "edit_count": len(response.get("edits") or [])}, indent=2))
        if response_validation_errors:
            print(json.dumps({"response_validation_errors": response_validation_errors}, indent=2))
            trace_events.append({"kind": "response_validation_errors", "turn": turn, "errors": response_validation_errors})
            if turn >= MAX_TURNS:
                write_trace_file(trace_file, trace_meta, trace_events, 1, started_at_iso, time.perf_counter() - started_at_perf, total_actions)
                return 1
            request = build_agent_request(project, args.prompt, retain_working_notes({"phase": "response_validation_error", "tool_observations": response_validation_errors, "instruction": "Your previous JSON response shape was invalid. Return exactly one JSON object matching shared_context.protocol.response_contract, including bounded working_notes. For tool use, use mode=tool_calls and a top-level tool_calls array. Each call must be {\"tool\":\"name\",\"args\":{...}} with no aliases such as calls, name, function, arguments, type, or source."}, working_notes))
            trace_events.append({"kind": "next_request", "turn": turn, "summary": summarize_request_for_trace(request)})
            continue
        working_notes = response["working_notes"].strip()
        trace_events.append({"kind": "working_notes", "turn": turn, "notes": working_notes})
        if mode == "edits" or (mode != "tool_calls" and response.get("edits")):
            observations = []
            for edit in response.get("edits") or []:
                result = replace_symbol(project, edit.get("name", ""), edit.get("file", ""), edit.get("new_source", ""))
                observations.append({"tool": "edit", "args": {"file": edit.get("file", ""), "name": edit.get("name", "")}, "result": result})
            final = run_behavior_tests(project)
            print(json.dumps({"edit_observations": summarize_observations(observations), "final_test": final}, indent=2))
            trace_events.append({"kind": "direct_edit_observations", "turn": turn, "observations": observations, "final_test": final})
            if final.get("ok"):
                write_trace_file(trace_file, trace_meta, trace_events, 0, started_at_iso, time.perf_counter() - started_at_perf, total_actions)
                return 0
            if turn >= MAX_TURNS:
                write_trace_file(trace_file, trace_meta, trace_events, 1, started_at_iso, time.perf_counter() - started_at_perf, total_actions)
                return 1
            request = build_agent_request(project, args.prompt, retain_working_notes({"phase": "direct_edit_test_failure", "tool_observations": observations, "test_observation": final, "instruction": "Direct edits were applied or rolled back, but the required local behavior test failed. Update working_notes, inspect the exact failure, then fix it using precise tool calls."}, working_notes))
            continue
        if mode != "tool_calls" or not tool_calls:
            final = run_behavior_tests(project)
            print(json.dumps({"final_test": final}, indent=2))
            trace_events.append({"kind": "final_test_after_done_or_empty", "turn": turn, "final_test": final})
            if final.get("ok"):
                write_trace_file(trace_file, trace_meta, trace_events, 0, started_at_iso, time.perf_counter() - started_at_perf, total_actions)
                return 0
            if turn >= MAX_TURNS:
                write_trace_file(trace_file, trace_meta, trace_events, 1, started_at_iso, time.perf_counter() - started_at_perf, total_actions)
                return 1
            request = build_agent_request(project, args.prompt, retain_working_notes({"phase": "done_or_empty_test_failure", "tool_observations": [], "test_observation": final, "instruction": "The model returned done, but the required local behavior test failed. Update working_notes with the failure and fix it using precise tool calls."}, working_notes))
            continue
        observations, last_diagnostics = execute_tool_batch(project, tool_calls, last_diagnostics)
        total_actions += len(tool_calls)
        print(json.dumps({"tool_observations": summarize_observations(observations)}, indent=2))
        trace_events.append({"kind": "tool_observations", "turn": turn, "observations": observations, "summary": summarize_observations(observations)})
        request = build_agent_request(project, args.prompt, retain_working_notes({"phase": "tool_observations", "tool_observations": observations, "instruction": "Use observations and retained working_notes to continue or return mode=done. Update Intent, Observed, Next, and Blocker on the next response. If tests fail, fix the exact required state/checks. Do not repeat identical tool calls."}, working_notes))
        trace_events.append({"kind": "next_request", "turn": turn, "summary": summarize_request_for_trace(request)})
    print(json.dumps({"error": "tool_call_limit", "actions": total_actions, "last_diagnostics": last_diagnostics}, indent=2))
    trace_events.append({"kind": "tool_call_limit", "actions": total_actions, "last_diagnostics": last_diagnostics})
    write_trace_file(trace_file, trace_meta, trace_events, 1, started_at_iso, time.perf_counter() - started_at_perf, total_actions)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
