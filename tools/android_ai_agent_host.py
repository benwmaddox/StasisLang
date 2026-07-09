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
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_PROJECT = ROOT / "mobile/android/app/src/main/assets/workshop_sample"
DEFAULT_MODEL = "gpt-5.4-mini"
MAX_TURNS = 5


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
                symbols.append(Symbol("struct", name, name, f"struct {name}", rel, full_source, start, end))
            elif token == "global ":
                symbols.append(Symbol("global", name, "Globals", f"global {name}", rel, full_source, start, end))
            else:
                signature = source[start + len(token):body_start].strip()
                func_name = signature.split("(", 1)[0].strip()
                owner = owner_for_function(rel, func_name, signature, structs)
                symbols.append(Symbol("function", func_name, owner, signature, rel, full_source, start, end))
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


def preferred_call(symbol: Symbol) -> str:
    if symbol.kind != "function":
        return symbol.name
    params = symbol.signature[symbol.signature.find("(") + 1:symbol.signature.find(")")].strip()
    pieces = [part.strip() for part in params.split(",") if part.strip()]
    if pieces and pieces[0].startswith("self:") and pieces[0].split(":", 1)[1].strip() == symbol.owner:
        args = ", ".join(part.split(":", 1)[0].strip() for part in pieces[1:])
        receiver = symbol.owner[:1].lower() + symbol.owner[1:]
        return f"{receiver}.{symbol.name}({args})"
    args = ", ".join(part.split(":", 1)[0].strip() for part in pieces)
    return f"{symbol.name}({args})"


def run_command(args: list[str]) -> dict[str, Any]:
    proc = subprocess.run(args, cwd=ROOT, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    return {"ok": proc.returncode == 0, "returncode": proc.returncode, "output_tail": proc.stdout[-6000:]}


def run_compile_check() -> dict[str, Any]:
    return run_command(["cargo", "test", "-p", "stasis_android_bridge", "android_bundled_touch_pong_sample_compile_plan_is_runnable", "--", "--nocapture"])


def run_behavior_tests() -> dict[str, Any]:
    return run_command(["cargo", "test", "-p", "stasis_android_bridge", "android_bundled_touch_pong_enemy_paddle_speed_schedule_is_linear", "--", "--ignored", "--nocapture"])


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

def replace_symbol(project: Path, name: str, file: str, new_source: str) -> dict[str, Any]:
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
    compile_result = run_compile_check()
    if not compile_result["ok"]:
        write_text(path, source)
        return {"status": "rolled_back", "file": file, "name": name, "compile": compile_result}
    return {"status": status, "file": file, "name": name, "compile": compile_result}


def write_project_file(project: Path, file: str, source: str) -> dict[str, Any]:
    path = project / file
    if not path.is_file():
        return {"status": "not_found", "file": file}
    original = read_text(path)
    write_text(path, source.rstrip() + "\n")
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
        spec("read_file", "Read a source file.", ["file"], [], {"file": "src/main.stasis"}),
        spec("write_symbol", "Create or replace exactly one Stasis function/global/struct, then compile locally and roll back on compile failure. The new_source must not contain additional top-level or nested declarations.", ["file", "name", "new_source"], ["kind", "owner"], {"file": "src/main.stasis", "name": "tick", "new_source": "function tick(): void {\n}"}),
        spec("run_tests", "Run the local host behavior test for the requested edit.", [], [], {}),
        spec("get_diagnostics", "Return last local diagnostics.", [], [], {}),
    ]


def build_request(project: Path, prompt: str) -> dict[str, Any]:
    symbols = parse_symbols(project)
    globals_payload = []
    for symbol in symbols:
        if symbol.kind == "global":
            body = symbol.source[symbol.source.find("{"):]
            globals_payload.append({"kind": "global", "name": symbol.name, "file": symbol.file, "backing_struct_type": symbol.name, "backing_struct_source": f"struct {symbol.name} {body}"})
    return {
        "scope": "entire_workspace",
        "available_tools": [s["tool"] for s in tool_specs()],
        "tool_specs": tool_specs(),
        "stasis_style_rules": {"use_function_keyword": True, "use_receiver_style_when_possible": True, "do_not_use_rust_references": True},
        "architecture_recommendations": [
            "Use lifecycle-local state for time since creation; reset it in reset/create functions and increment it during tick.",
            "Use on_code_swap() for post-hot-swap migration or reinitialization if running state needs adjustment.",
            "Make the smallest structural change with clear state fields and testable invariants.",
        ],
        "project_globals": globals_payload,
        "user_prompt": prompt,
        "selected_symbols": [],
        "selected_symbols_are_context_only": True,
    }


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

def call_openai(api_key: str, model: str, request: dict[str, Any]) -> dict[str, Any]:
    prompt = (
        "Return only one JSON object. Use mode=tool_calls to inspect/write with the provided tools. "
        "After write_symbol, compile runs locally and failed compiles roll back. Use run_tests to verify the behavior. "
        "Return mode=tool_calls for tools, mode=edits with edits if returning direct edits, or mode=done only when tests pass. "
        "Use Stasis syntax only. Request: " + json.dumps(request, separators=(",", ":"))
    )
    schema = {"type": "object", "additionalProperties": True}
    payload = {"model": model, "text": {"format": {"type": "json_schema", "name": "stasis_host_ai_response", "strict": False, "schema": schema}}, "input": prompt}
    req = urllib.request.Request(
        "https://api.openai.com/v1/responses",
        data=json.dumps(payload).encode("utf-8"),
        headers={"Authorization": f"Bearer {api_key}", "Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=120) as response:
        body = json.loads(response.read().decode("utf-8"))
    text = body.get("output_text", "")
    if not text:
        chunks: list[str] = []
        for item in body.get("output", []):
            for content in item.get("content", []):
                if "text" in content:
                    chunks.append(content["text"])
        text = "".join(chunks)
    return parse_json_object(text)


def validate_tool_call_response(response: dict[str, Any]) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    errors: list[dict[str, Any]] = []
    if response.get("mode") != "tool_calls":
        return [], errors
    if "tool_calls" not in response:
        errors.append({
            "kind": "validation_error",
            "error": "mode=tool_calls requires top-level tool_calls array; do not use calls/type/name/arguments aliases",
            "received_keys": sorted(response.keys()),
            "accepted_shape": {"mode": "tool_calls", "tool_calls": [{"tool": "read_file", "args": {"file": "src/main.stasis"}}]},
        })
        return [], errors
    raw_calls = response.get("tool_calls")
    if not isinstance(raw_calls, list):
        errors.append({
            "kind": "validation_error",
            "error": "tool_calls must be an array",
            "received_tool_calls_type": type(raw_calls).__name__,
            "accepted_shape": {"tool": "read_file", "args": {"file": "src/main.stasis"}},
        })
        return [], errors
    normalized: list[dict[str, Any]] = []
    for index, call in enumerate(raw_calls):
        if not isinstance(call, dict):
            errors.append({"kind": "validation_error", "index": index, "error": "each tool call must be an object with tool and args"})
            continue
        extra = sorted(set(call.keys()) - {"tool", "args"})
        if extra:
            errors.append({
                "kind": "validation_error",
                "index": index,
                "error": "tool call contains unsupported top-level properties",
                "unsupported_properties": extra,
                "accepted_shape": {"tool": "read_file", "args": {"file": "src/main.stasis"}},
            })
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

def execute_tool(project: Path, tool: str, args: dict[str, Any], last_diagnostics: dict[str, Any]) -> dict[str, Any]:
    symbols = parse_symbols(project)
    if tool == "list_symbols":
        return {"symbols": [symbol_json(s, False) for s in symbols]}
    if tool == "list_owner_symbols":
        owner = args.get("owner", "")
        owned = [s for s in symbols if s.owner == owner or s.name == owner]
        return {"owner": owner, "symbols": [dict(symbol_json(s, False), preferred_call=preferred_call(s)) for s in owned]}
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
    if tool == "read_file":
        file = args.get("file", "")
        return {"file": file, "source": read_text(project / file)}
    if tool == "write_symbol":
        return replace_symbol(project, args.get("name", ""), args.get("file", ""), args.get("new_source") or args.get("source", ""))
    if tool == "write_file":
        return write_project_file(project, args.get("file", ""), args.get("source", ""))
    if tool == "run_tests":
        return run_behavior_tests()
    if tool == "get_diagnostics":
        return last_diagnostics
    return {"status": "unsupported_tool", "tool": tool, "supported_tools": [s["tool"] for s in tool_specs()]}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--prompt", required=True)
    parser.add_argument("--project-root", type=Path, default=DEFAULT_PROJECT)
    parser.add_argument("--model", default=DEFAULT_MODEL)
    args = parser.parse_args()
    load_env_file(ROOT / ".env")
    api_key = os.environ.get("OPENAI_API_KEY")
    if not api_key:
        print("OPENAI_API_KEY is required", file=sys.stderr)
        return 2

    project = args.project_root.resolve()
    request = build_request(project, args.prompt)
    last_diagnostics: dict[str, Any] = {}
    total_actions = 0
    for turn in range(1, MAX_TURNS + 1):
        response = call_openai(api_key, args.model, request)
        mode = response.get("mode")
        tool_calls, response_validation_errors = validate_tool_call_response(response)
        print(json.dumps({"turn": turn, "mode": mode, "response_keys": sorted(response.keys()), "summary": response.get("summary", ""), "tool_call_count": len(tool_calls), "validation_error_count": len(response_validation_errors), "edit_count": len(response.get("edits") or [])}, indent=2))
        if response_validation_errors:
            print(json.dumps({"response_validation_errors": response_validation_errors}, indent=2))
            if turn >= MAX_TURNS:
                return 1
            request = {"original_request": build_request(project, args.prompt), "tool_observations": response_validation_errors, "instruction": "Your previous JSON response shape was invalid. Return exactly one JSON object. For tool use, use mode=tool_calls and a top-level tool_calls array. Each call must be {\"tool\":\"name\",\"args\":{...}} with no aliases such as calls, name, function, arguments, or type."}
            continue
        if mode == "edits" or (mode != "tool_calls" and response.get("edits")):
            observations = []
            for edit in response.get("edits") or []:
                result = replace_symbol(project, edit.get("name", ""), edit.get("file", ""), edit.get("new_source", ""))
                observations.append({"tool": "edit", "args": {"file": edit.get("file", ""), "name": edit.get("name", "")}, "result": result})
            final = run_behavior_tests()
            print(json.dumps({"edit_observations": summarize_observations(observations), "final_test": final}, indent=2))
            if final.get("ok"):
                return 0
            if turn >= MAX_TURNS:
                return 1
            request = {
                "original_request": build_request(project, args.prompt),
                "tool_observations": observations,
                "test_observation": final,
                "instruction": "Direct edits were applied or rolled back, but the required local behavior test failed. Inspect the current source and fix it using tool calls. Use precise write_symbol calls; write_file is reserved for host recovery and is not part of the normal tool list.",
            }
            continue
        if mode != "tool_calls" or not tool_calls:
            final = run_behavior_tests()
            print(json.dumps({"final_test": final}, indent=2))
            if final.get("ok"):
                return 0
            if turn >= MAX_TURNS:
                return 1
            request = {
                "original_request": build_request(project, args.prompt),
                "tool_observations": [],
                "test_observation": final,
                "instruction": "The model returned done or an unsupported shape, but the required local behavior test failed. Inspect the current source and fix it using tool calls. Fix duplicate or misplaced symbols with precise write_symbol calls; write_file is not part of the normal tool list.",
            }
            continue
        observations = []
        for call in tool_calls:
            tool = call.get("tool", "")
            tool_args = call.get("args") or {}
            result = execute_tool(project, tool, tool_args, last_diagnostics)
            total_actions += 1
            if tool in {"write_symbol", "run_tests"}:
                last_diagnostics = result
            observations.append({"tool": tool, "args": tool_args, "result": result})
        print(json.dumps({"tool_observations": summarize_observations(observations)}, indent=2))
        request = {"original_request": build_request(project, args.prompt), "tool_observations": observations, "instruction": "Use observations to continue or return mode=done. If tests fail, inspect and fix. Do not repeat identical tool calls."}
    print(json.dumps({"error": "tool_call_limit", "actions": total_actions, "last_diagnostics": last_diagnostics}, indent=2))
    return 1


if __name__ == "__main__":
    raise SystemExit(main())