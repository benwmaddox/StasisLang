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


def replace_symbol(project: Path, name: str, file: str, new_source: str) -> dict[str, Any]:
    path = project / file
    source = read_text(path)
    symbols = [s for s in parse_symbols(project) if s.file == file and s.name == name]
    if symbols:
        target = symbols[0]
        updated = source[:target.start] + new_source.rstrip() + source[target.end:]
        write_text(path, updated)
        status = "written"
    else:
        updated = source.rstrip() + "\n\n" + new_source.rstrip() + "\n"
        write_text(path, updated)
        status = "created"
    compile_result = run_compile_check()
    return {"status": status if compile_result["ok"] else "compile_failed", "file": file, "name": name, "compile": compile_result}


def tool_specs() -> list[dict[str, Any]]:
    def spec(tool: str, purpose: str, required: list[str], optional: list[str], args: dict[str, Any]) -> dict[str, Any]:
        return {"tool": tool, "purpose": purpose, "required_args": required, "optional_args": optional, "example": {"tool": tool, "args": args}}
    return [
        spec("list_symbols", "List editable symbols compactly.", [], [], {}),
        spec("list_owner_symbols", "List symbols and preferred receiver calls for one owner/type.", ["owner"], [], {"owner": "GameState"}),
        spec("read_symbol", "Read one symbol source.", ["name"], ["kind", "file", "owner"], {"name": "update_enemy_paddle"}),
        spec("read_file", "Read a source file.", ["file"], [], {"file": "src/main.stasis"}),
        spec("write_symbol", "Create or replace a Stasis function/global/struct, then compile locally.", ["file", "name", "new_source"], ["kind", "owner"], {"file": "src/main.stasis", "name": "tick", "new_source": "function tick(): void {\n}"}),
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


def call_openai(api_key: str, model: str, request: dict[str, Any]) -> dict[str, Any]:
    prompt = (
        "Return only one JSON object. Use mode=tool_calls to inspect/write with the provided tools. "
        "After write_symbol, compile runs locally. Use run_tests to verify the behavior. "
        "Return mode=done with empty tool_calls when the requested work is complete. "
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
    return json.loads(text)


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
        tool_calls = response.get("tool_calls") or []
        print(json.dumps({"turn": turn, "mode": mode, "summary": response.get("summary", ""), "tool_call_count": len(tool_calls)}, indent=2))
        if mode != "tool_calls" or not tool_calls:
            final = run_behavior_tests()
            print(json.dumps({"final_test": final}, indent=2))
            return 0 if final.get("ok") else 1
        observations = []
        for call in tool_calls:
            tool = call.get("tool", "")
            tool_args = call.get("args") or {}
            result = execute_tool(project, tool, tool_args, last_diagnostics)
            total_actions += 1
            if tool in {"write_symbol", "run_tests"}:
                last_diagnostics = result
            observations.append({"tool": tool, "args": tool_args, "result": result})
        request = {"original_request": build_request(project, args.prompt), "tool_observations": observations, "instruction": "Use observations to continue or return mode=done. If tests fail, inspect and fix. Do not repeat identical tool calls."}
    print(json.dumps({"error": "tool_call_limit", "actions": total_actions, "last_diagnostics": last_diagnostics}, indent=2))
    return 1


if __name__ == "__main__":
    raise SystemExit(main())