#!/usr/bin/env python3
"""Verify copied HostFrame and render ABI values against their canonical sources."""

from __future__ import annotations

import argparse
import ast
import hashlib
import json
import operator
import os
import re
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
RENDER_HEADER = Path("runtime/stasis_render_contract.h")
HOST_FRAME = Path("src/stdlib/internal/host_frame.stasis")
GFX_CMD = Path("src/stdlib/internal/gfx_cmd.stasis")
DYNLOAD = Path("crates/stasis_dynload/src/lib.rs")
DESKTOP = Path("apps/stasis/src/lib.rs")
AOT = Path("apps/stasis/src/compiler_backend.rs")
TOOLCHAIN = Path("apps/stasis/src/toolchain_cli.rs")
RELEASE_PROVENANCE = Path("tools/generate_release_provenance.py")
PACKAGE_PROVENANCE = Path("tools/verify_package_provenance.py")
WEB = Path("runtime/web/game.js")
ANDROID = Path("crates/stasis_android_bridge/src/lib.rs")
JAVA_RENDERER = Path(
    "mobile/android/app/src/main/java/com/stasislang/workshop/StasisPreviewRenderer.java"
)
WORKSHOP = Path(
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/MainActivity.java"
)
JNI = Path("mobile/android/app/src/main/cpp/stasis_mobile_smoke.c")
NATIVE_HOST = Path("runtime/stasis_graphics.c")
DESKTOP_MANIFEST_FIXTURE = Path("tests/stasis/seams/desktop_manifest_assets_probe.stasis")
DESKTOP_MANIFEST_HARNESS = Path("apps/stasis/tests/desktop_manifest_assets_seam.rs")
DESKTOP_INPUT_FRAME_HARNESS = Path("apps/stasis/tests/desktop_input_frame_seam.rs")
DESKTOP_DISPLAY_METRICS_HARNESS = Path("apps/stasis/tests/desktop_display_metrics_seam.rs")
GENERATED_MOBILE_AOT_C = Path("runtime/tests/stasis_generated_mobile_integration.c")
GENERATED_MOBILE_AOT_RUST = Path("apps/stasis/tests/generated_mobile_aot_runtime_seam.rs")
DESKTOP_RENDER_RECOVERY = Path("apps/stasis/tests/desktop_render_recovery_seam.rs")
DESKTOP_ERROR_TOAST = Path("apps/stasis/tests/desktop_error_toast_seam.rs")
PLAY_ERROR_TOASTS = Path("apps/stasis/src/play_error_toasts.rs")
RENDER_PARITY_FRAME = Path("samples/render_parity/frame.stasis")
RENDER_PARITY_TRACE = Path("samples/render_parity/trace.stasis")
JIT_AOT_REPLAY_FIXTURE = Path("tests/stasis/seams/jit_aot_host_replay_probe.stasis")
RENDER_DOWNSTREAM = (
    GFX_CMD, DYNLOAD, DESKTOP, AOT, TOOLCHAIN, RELEASE_PROVENANCE,
    PACKAGE_PROVENANCE, WEB, ANDROID, JAVA_RENDERER, NATIVE_HOST,
    DESKTOP_MANIFEST_FIXTURE, DESKTOP_MANIFEST_HARNESS,
    DESKTOP_INPUT_FRAME_HARNESS, DESKTOP_DISPLAY_METRICS_HARNESS,
    GENERATED_MOBILE_AOT_C, GENERATED_MOBILE_AOT_RUST,
    DESKTOP_RENDER_RECOVERY, DESKTOP_ERROR_TOAST, PLAY_ERROR_TOASTS,
    RENDER_PARITY_FRAME, RENDER_PARITY_TRACE,
    JIT_AOT_REPLAY_FIXTURE,
)
REQUIRED = (
    RENDER_HEADER, HOST_FRAME, GFX_CMD, DYNLOAD, DESKTOP, AOT, TOOLCHAIN,
    RELEASE_PROVENANCE, PACKAGE_PROVENANCE, WEB, ANDROID, JAVA_RENDERER,
    WORKSHOP, JNI, NATIVE_HOST, DESKTOP_MANIFEST_FIXTURE,
    DESKTOP_MANIFEST_HARNESS, DESKTOP_INPUT_FRAME_HARNESS,
    DESKTOP_DISPLAY_METRICS_HARNESS, GENERATED_MOBILE_AOT_C,
    GENERATED_MOBILE_AOT_RUST, DESKTOP_RENDER_RECOVERY, DESKTOP_ERROR_TOAST,
    PLAY_ERROR_TOASTS,
    RENDER_PARITY_FRAME, RENDER_PARITY_TRACE,
    JIT_AOT_REPLAY_FIXTURE,
)
IGNORED_SOURCE_DIRS = {
    ".git",
    ".gradle",
    ".stasis_cache",
    "build",
    "dist",
    "node_modules",
    "target",
    "vendor",
}

# These spellings represented compatibility layers for retired render-command
# layouts. Keep rejection scoped to render consumers: other versioned
# contracts (host frame, workshop projects, etc.) may retain their own
# historical versions.
LEGACY_RENDER_PATTERNS = (
    re.compile(r"\bSTASIS_RENDER_V[2-6](?:_[A-Z0-9_]+)?\b"),
    re.compile(r"\bGFX_CMD_V[2-6](?:_[A-Z0-9_]+)?\b"),
    re.compile(r"\b(?:STASIS_RENDER|GFX_CMD)_CURRENT_VERSION\b"),
    re.compile(r"\b(?:GFX_CMD|STASIS_RENDER)_(?:LEGACY|OLD)_[A-Z0-9_]+\b"),
    re.compile(r"\b(?:LEGACY|OLD)_(?:GFX|RENDER)_[A-Z0-9_]+\b"),
    re.compile(r"\bstasis_(?:render|jit_render)_v2_trace(?:_native)?\b"),
    re.compile(r"\b(?:34608|108676|96388)\b"),
)


def repository_stasis_sources(directory: Path) -> list[Path]:
    sources: list[Path] = []
    for current_root, directories, filenames in os.walk(directory):
        directories[:] = sorted(
            name for name in directories if name not in IGNORED_SOURCE_DIRS
        )
        for filename in sorted(filenames):
            if filename.endswith(".stasis"):
                sources.append(Path(current_root) / filename)
    return sources

DESCRIPTOR_PATTERNS = {
    "i32": r'X\(I32,\s*"i32",\s*STASIS_RENDER_I32_COUNT\s*\*\s*sizeof\(int32_t\),\s*_Alignof\(int32_t\)\)',
    "f32": r'X\(F32,\s*"f32",\s*STASIS_RENDER_F32_COUNT\s*\*\s*sizeof\(float\),\s*_Alignof\(float\)\)',
    "u8": r'X\(U8,\s*"u8",\s*STASIS_RENDER_U8_COUNT\s*\*\s*sizeof\(uint8_t\),\s*_Alignof\(uint8_t\)\)',
}

BINOPS = {
    ast.Add: operator.add, ast.Sub: operator.sub, ast.Mult: operator.mul,
    ast.FloorDiv: operator.floordiv, ast.Mod: operator.mod,
    ast.LShift: operator.lshift, ast.RShift: operator.rshift,
    ast.BitOr: operator.or_, ast.BitAnd: operator.and_, ast.BitXor: operator.xor,
}


@dataclass(frozen=True)
class Mismatch:
    producer: str
    consumer: str
    field: str
    expected: object
    actual: object

    def __str__(self) -> str:
        return (
            f"ABI mismatch: producer={self.producer} consumer={self.consumer} "
            f"field={self.field} expected={self.expected} actual={self.actual}"
        )


def _eval(node: ast.AST, values: dict[str, int]) -> int:
    if isinstance(node, ast.Constant) and isinstance(node.value, int):
        return node.value
    if isinstance(node, ast.Name):
        return values[node.id]
    if isinstance(node, ast.BinOp) and type(node.op) in BINOPS:
        return BINOPS[type(node.op)](_eval(node.left, values), _eval(node.right, values))
    if isinstance(node, ast.UnaryOp) and isinstance(node.op, (ast.UAdd, ast.USub, ast.Invert)):
        value = _eval(node.operand, values)
        return value if isinstance(node.op, ast.UAdd) else -value if isinstance(node.op, ast.USub) else ~value
    raise ValueError(f"unsupported integer expression: {ast.dump(node)}")


def resolve(expressions: dict[str, str]) -> dict[str, int]:
    pending = dict(expressions)
    values: dict[str, int] = {}
    while pending:
        progressed = False
        for name, expression in list(pending.items()):
            cleaned = re.sub(r"(?<=\d)_(?=[0-9A-Fa-f])", "", expression)
            cleaned = re.sub(r"(?<=\d)[uU](?:[lL]{1,2})?\b", "", cleaned).strip()
            try:
                values[name] = _eval(ast.parse(cleaned, mode="eval").body, values)
            except (KeyError, SyntaxError, ValueError):
                continue
            del pending[name]
            progressed = True
        if not progressed:
            names = ", ".join(sorted(pending))
            raise ValueError(f"could not resolve constants: {names}")
    return values


def c_constants(text: str) -> dict[str, int]:
    text = re.sub(r"\\\r?\n\s*", " ", text)
    expressions = {}
    for line in text.splitlines():
        parts = line.split(maxsplit=2)
        if len(parts) != 3 or parts[0] != "#define":
            continue
        name, expression = parts[1:]
        if re.fullmatch(r"STASIS_(?:RENDER|GRAPHICS)_[A-Z0-9_]+", name):
            expressions[name] = expression.split("/*", 1)[0].strip()
    return resolve(expressions)


def stasis_constants(text: str) -> dict[str, int]:
    pairs = re.findall(r"^const\s+([A-Z][A-Z0-9_]+):\s*i32\s*=\s*([^;]+);", text, re.M)
    return resolve({name: expression.split("//", 1)[0] for name, expression in pairs})


def rust_constants(text: str, prefix: str) -> dict[str, int]:
    pairs = re.findall(
        rf"^\s*(?:pub\s+)?const\s+({re.escape(prefix)}[A-Z0-9_]+):\s*(?:usize|i32|i64)\s*=\s*([^;]+);",
        text, re.M,
    )
    return resolve({name: expression for name, expression in pairs})


def python_constants(text: str, prefix: str) -> dict[str, int]:
    pairs = re.findall(
        rf"^({re.escape(prefix)}[A-Z0-9_]*)\s*=\s*([0-9][0-9_]*)\s*$", text, re.M
    )
    return {name: int(expression.replace("_", "")) for name, expression in pairs}


def java_constants(text: str) -> dict[str, int]:
    pairs = re.findall(r"^\s*(?:private\s+)?static\s+final\s+int\s+([A-Z][A-Z0-9_]+)\s*=\s*([^;]+);", text, re.M)
    return resolve({name: expression for name, expression in pairs})


def javascript_constants(text: str) -> dict[str, int]:
    pairs = re.findall(
        r"^\s*const\s+(GFX_[A-Z0-9_]+)\s*=\s*([^;]+);", text, re.M
    )
    return resolve({name: expression for name, expression in pairs})


RENDER_TO_GFX = {
    "STASIS_RENDER_MAGIC": "GFX_CMD_MAGIC",
    "STASIS_RENDER_VERSION": "GFX_CMD_VERSION",
    "STASIS_RENDER_FLAG_CLEAR": "GFX_FLAG_CLEAR",
    "STASIS_RENDER_FLAG_PRESENT": "GFX_FLAG_PRESENT",
    "STASIS_RENDER_I_MAGIC": "GFX_I_MAGIC",
    "STASIS_RENDER_I_VERSION": "GFX_I_VERSION",
    "STASIS_RENDER_I_FLAGS": "GFX_I_FLAGS",
    "STASIS_RENDER_I_LINE_COUNT": "GFX_I_LINE_COUNT",
    "STASIS_RENDER_I_SPRITE_COUNT": "GFX_I_SPRITE_COUNT",
    "STASIS_RENDER_I_DROPPED_LINES": "GFX_I_DROPPED_LINES",
    "STASIS_RENDER_I_DROPPED_SPRITES": "GFX_I_DROPPED_SPRITES",
    "STASIS_RENDER_I_TEXT_COUNT": "GFX_I_TEXT_COUNT",
    "STASIS_RENDER_I_DROPPED_TEXT": "GFX_I_DROPPED_TEXT",
    "STASIS_RENDER_I_TEXT_BYTES_USED": "GFX_I_TEXT_BYTES_USED",
    "STASIS_RENDER_I_LOGICAL_W": "GFX_I_LOGICAL_W",
    "STASIS_RENDER_I_LOGICAL_H": "GFX_I_LOGICAL_H",
    "STASIS_RENDER_I_NATIVE_W": "GFX_I_NATIVE_W",
    "STASIS_RENDER_I_NATIVE_H": "GFX_I_NATIVE_H",
    "STASIS_RENDER_I_DRAWABLE_W": "GFX_I_DRAWABLE_W",
    "STASIS_RENDER_I_DRAWABLE_H": "GFX_I_DRAWABLE_H",
    "STASIS_RENDER_I_SAFE_X": "GFX_I_SAFE_X",
    "STASIS_RENDER_I_SAFE_Y": "GFX_I_SAFE_Y",
    "STASIS_RENDER_I_SAFE_W": "GFX_I_SAFE_W",
    "STASIS_RENDER_I_SAFE_H": "GFX_I_SAFE_H",
    "STASIS_RENDER_I_DISPLAY_GENERATION": "GFX_I_DISPLAY_GENERATION",
    "STASIS_RENDER_I_DENSITY_GENERATION": "GFX_I_DENSITY_GENERATION",
    "STASIS_RENDER_I_ORDER_COUNT": "GFX_I_ORDER_COUNT",
    "STASIS_RENDER_I_DROPPED_ORDER": "GFX_I_DROPPED_ORDER",
    "STASIS_RENDER_I_RECT_COUNT": "GFX_I_RECT_COUNT",
    "STASIS_RENDER_I_CLIP_COUNT": "GFX_I_CLIP_COUNT",
    "STASIS_RENDER_I_DROPPED_RECTS": "GFX_I_DROPPED_RECTS",
    "STASIS_RENDER_I_SPRITE_BASE": "GFX_I_SPRITE_BASE",
    "STASIS_RENDER_I_TEXT_BASE": "GFX_I_TEXT_BASE",
    "STASIS_RENDER_I_ORDER_BASE": "GFX_I_ORDER_BASE",
    "STASIS_RENDER_F_LINE_BASE": "GFX_F_LINE_BASE",
    "STASIS_RENDER_F_SPRITE_BASE": "GFX_F_SPRITE_BASE",
    "STASIS_RENDER_F_RECT_REVERSE_BASE": "GFX_F_RECT_REVERSE_BASE",
    "STASIS_RENDER_F_TEXT_BASE": "GFX_F_TEXT_BASE",
    "STASIS_RENDER_F_CLIP_BASE": "GFX_F_CLIP_BASE",
    "STASIS_RENDER_MAX_GEOMETRY": "GFX_MAX_GEOMETRY",
    "STASIS_RENDER_GEOMETRY_F32_STRIDE": "GFX_GEOMETRY_STRIDE_F32",
    "STASIS_RENDER_MAX_LINES": "GFX_MAX_LINES",
    "STASIS_RENDER_LINE_F32_STRIDE": "GFX_LINE_STRIDE_F32",
    "STASIS_RENDER_MAX_SPRITES": "GFX_MAX_SPRITES",
    "STASIS_RENDER_SPRITE_I32_STRIDE": "GFX_SPRITE_STRIDE_I32",
    "STASIS_RENDER_SPRITE_F32_STRIDE": "GFX_SPRITE_STRIDE_F32",
    "STASIS_RENDER_MAX_TEXT": "GFX_MAX_TEXT",
    "STASIS_RENDER_TEXT_I32_STRIDE": "GFX_TEXT_STRIDE_I32",
    "STASIS_RENDER_TEXT_F32_STRIDE": "GFX_TEXT_STRIDE_F32",
    "STASIS_RENDER_TEXT_MAX_BYTES": "GFX_TEXT_MAX_BYTES",
    "STASIS_RENDER_MAX_CLIPS": "GFX_MAX_CLIPS",
    "STASIS_RENDER_CLIP_F32_STRIDE": "GFX_CLIP_STRIDE_F32",
    "STASIS_RENDER_MAX_ORDER": "GFX_MAX_ORDER",
    "STASIS_RENDER_ORDER_KIND_SCALE": "GFX_ORDER_KIND_SCALE",
    "STASIS_RENDER_ORDER_LINE": "GFX_ORDER_LINE",
    "STASIS_RENDER_ORDER_SPRITE": "GFX_ORDER_SPRITE",
    "STASIS_RENDER_ORDER_TEXT": "GFX_ORDER_TEXT",
    "STASIS_RENDER_ORDER_RECT": "GFX_ORDER_RECT",
    "STASIS_RENDER_ORDER_CLIP_PUSH": "GFX_ORDER_CLIP_PUSH",
    "STASIS_RENDER_ORDER_CLIP_POP": "GFX_ORDER_CLIP_POP",
}

RENDER_TO_RUST = {
    "STASIS_RENDER_I32_COUNT": "STASIS_RENDER_I32_COUNT",
    "STASIS_RENDER_F32_COUNT": "STASIS_RENDER_F32_COUNT",
    "STASIS_RENDER_U8_COUNT": "STASIS_RENDER_U8_COUNT",
    "STASIS_RENDER_MAGIC": "STASIS_RENDER_MAGIC",
    "STASIS_RENDER_VERSION": "STASIS_RENDER_VERSION",
    "STASIS_RENDER_I_ORDER_COUNT": "STASIS_RENDER_ORDER_COUNT_INDEX",
    "STASIS_RENDER_I_RECT_COUNT": "STASIS_RENDER_RECT_COUNT_INDEX",
    "STASIS_RENDER_I_CLIP_COUNT": "STASIS_RENDER_CLIP_COUNT_INDEX",
    "STASIS_RENDER_I_ORDER_BASE": "STASIS_RENDER_ORDER_BASE",
    "STASIS_RENDER_MAX_ORDER": "STASIS_RENDER_MAX_ORDER",
    "STASIS_RENDER_MAX_CLIPS": "STASIS_RENDER_MAX_CLIPS",
    "STASIS_RENDER_CLIP_F32_STRIDE": "STASIS_RENDER_CLIP_STRIDE_F32",
    "STASIS_RENDER_I_SPRITE_BASE": "STASIS_RENDER_SPRITE_BASE",
    "STASIS_RENDER_MAX_LINES": "STASIS_RENDER_MAX_LINES",
    "STASIS_RENDER_MAX_GEOMETRY": "STASIS_RENDER_MAX_GEOMETRY",
    "STASIS_RENDER_GEOMETRY_F32_STRIDE": "STASIS_RENDER_GEOMETRY_STRIDE_F32",
    "STASIS_RENDER_LINE_F32_STRIDE": "STASIS_RENDER_LINE_STRIDE",
    "STASIS_RENDER_MAX_SPRITES": "STASIS_RENDER_MAX_SPRITES",
    "STASIS_RENDER_SPRITE_I32_STRIDE": "STASIS_RENDER_SPRITE_STRIDE_I32",
    "STASIS_RENDER_F_SPRITE_BASE": "STASIS_RENDER_SPRITE_BASE_F32",
    "STASIS_RENDER_SPRITE_F32_STRIDE": "STASIS_RENDER_SPRITE_STRIDE_F32",
    "STASIS_RENDER_F_CLEAR_BASE": "STASIS_RENDER_F_CLEAR_BASE",
    "STASIS_RENDER_F_LINE_BASE": "STASIS_RENDER_F_LINE_BASE",
    "STASIS_RENDER_F_RECT_REVERSE_BASE": "STASIS_RENDER_RECT_REVERSE_BASE_F32",
    "STASIS_RENDER_I_TEXT_BASE": "STASIS_RENDER_TEXT_BASE_I32",
    "STASIS_RENDER_F_TEXT_BASE": "STASIS_RENDER_TEXT_BASE_F32",
    "STASIS_RENDER_F_CLIP_BASE": "STASIS_RENDER_CLIP_BASE_F32",
    "STASIS_RENDER_MAX_TEXT": "STASIS_RENDER_MAX_TEXT",
    "STASIS_RENDER_TEXT_I32_STRIDE": "STASIS_RENDER_TEXT_STRIDE_I32",
    "STASIS_RENDER_TEXT_F32_STRIDE": "STASIS_RENDER_TEXT_STRIDE_F32",
    "STASIS_RENDER_TEXT_MAX_BYTES": "STASIS_RENDER_U8_COUNT",
}

RENDER_TO_WEB = {
    "STASIS_RENDER_MAGIC": "GFX_CMD_MAGIC",
    "STASIS_RENDER_VERSION": "GFX_CMD_VERSION",
    "STASIS_RENDER_FLAG_CLEAR": "GFX_FLAG_CLEAR",
    "STASIS_RENDER_FLAG_PRESENT": "GFX_FLAG_PRESENT",
    "STASIS_RENDER_I_MAGIC": "GFX_I_MAGIC",
    "STASIS_RENDER_I_VERSION": "GFX_I_VERSION",
    "STASIS_RENDER_I_FLAGS": "GFX_I_FLAGS",
    "STASIS_RENDER_I_LINE_COUNT": "GFX_I_LINE_COUNT",
    "STASIS_RENDER_I_SPRITE_COUNT": "GFX_I_SPRITE_COUNT",
    "STASIS_RENDER_I_TEXT_COUNT": "GFX_I_TEXT_COUNT",
    "STASIS_RENDER_I_TEXT_BYTES_USED": "GFX_I_TEXT_BYTES_USED",
    "STASIS_RENDER_I_ORDER_COUNT": "GFX_I_ORDER_COUNT",
    "STASIS_RENDER_I_RECT_COUNT": "GFX_I_RECT_COUNT",
    "STASIS_RENDER_I_SPRITE_BASE": "GFX_I_SPRITE_BASE",
    "STASIS_RENDER_I_TEXT_BASE": "GFX_I_TEXT_BASE",
    "STASIS_RENDER_I_ORDER_BASE": "GFX_I_ORDER_BASE",
    "STASIS_RENDER_F_CLEAR_BASE": "GFX_F_CLEAR_BASE",
    "STASIS_RENDER_F_LINE_BASE": "GFX_F_LINE_BASE",
    "STASIS_RENDER_F_SPRITE_BASE": "GFX_F_SPRITE_BASE",
    "STASIS_RENDER_F_RECT_REVERSE_BASE": "GFX_F_RECT_REVERSE_BASE",
    "STASIS_RENDER_F_TEXT_BASE": "GFX_F_TEXT_BASE",
    "STASIS_RENDER_F_CLIP_BASE": "GFX_F_CLIP_BASE",
    "STASIS_RENDER_MAX_GEOMETRY": "GFX_MAX_GEOMETRY",
    "STASIS_RENDER_GEOMETRY_F32_STRIDE": "GFX_GEOMETRY_STRIDE_F32",
    "STASIS_RENDER_MAX_LINES": "GFX_MAX_LINES",
    "STASIS_RENDER_LINE_F32_STRIDE": "GFX_LINE_STRIDE_F32",
    "STASIS_RENDER_MAX_SPRITES": "GFX_MAX_SPRITES",
    "STASIS_RENDER_SPRITE_I32_STRIDE": "GFX_SPRITE_STRIDE_I32",
    "STASIS_RENDER_SPRITE_F32_STRIDE": "GFX_SPRITE_STRIDE_F32",
    "STASIS_RENDER_MAX_TEXT": "GFX_MAX_TEXT",
    "STASIS_RENDER_TEXT_I32_STRIDE": "GFX_TEXT_STRIDE_I32",
    "STASIS_RENDER_TEXT_F32_STRIDE": "GFX_TEXT_STRIDE_F32",
    "STASIS_RENDER_TEXT_MAX_BYTES": "GFX_TEXT_MAX_BYTES",
    "STASIS_RENDER_MAX_CLIPS": "GFX_MAX_CLIPS",
    "STASIS_RENDER_CLIP_F32_STRIDE": "GFX_CLIP_STRIDE_F32",
    "STASIS_RENDER_MAX_ORDER": "GFX_MAX_ORDER",
    "STASIS_RENDER_ORDER_KIND_SCALE": "GFX_ORDER_KIND_SCALE",
    "STASIS_RENDER_ORDER_LINE": "GFX_ORDER_LINE",
    "STASIS_RENDER_ORDER_SPRITE": "GFX_ORDER_SPRITE",
    "STASIS_RENDER_ORDER_TEXT": "GFX_ORDER_TEXT",
    "STASIS_RENDER_ORDER_RECT": "GFX_ORDER_RECT",
    "STASIS_RENDER_ORDER_CLIP_PUSH": "GFX_ORDER_CLIP_PUSH",
    "STASIS_RENDER_ORDER_CLIP_POP": "GFX_ORDER_CLIP_POP",
}

RENDER_TO_TOOLCHAIN_PROVENANCE = {
    "STASIS_RENDER_VERSION": "GFX_CMD_VERSION",
}

RENDER_TO_PACKAGE_PROVENANCE = {
    "STASIS_RENDER_VERSION": "CURRENT_COMMAND_BUFFER_VERSION",
}

RENDER_TO_JAVA = {
    "STASIS_RENDER_MAGIC": "RENDER_MAGIC",
    "STASIS_RENDER_VERSION": "RENDER_VERSION",
    "STASIS_RENDER_FLAG_CLEAR": "FLAG_CLEAR",
    "STASIS_RENDER_FLAG_PRESENT": "FLAG_PRESENT",
    **{source: target.removeprefix("GFX_") for source, target in RENDER_TO_GFX.items()
       if target.startswith(("GFX_I_", "GFX_F_", "GFX_MAX_", "GFX_ORDER_"))},
    "STASIS_RENDER_GEOMETRY_F32_STRIDE": "GEOMETRY_F32_STRIDE",
    "STASIS_RENDER_LINE_F32_STRIDE": "LINE_F32_STRIDE",
    "STASIS_RENDER_SPRITE_I32_STRIDE": "SPRITE_I32_STRIDE",
    "STASIS_RENDER_SPRITE_F32_STRIDE": "SPRITE_F32_STRIDE",
    "STASIS_RENDER_TEXT_I32_STRIDE": "TEXT_I32_STRIDE",
    "STASIS_RENDER_TEXT_F32_STRIDE": "TEXT_F32_STRIDE",
    "STASIS_RENDER_TEXT_MAX_BYTES": "TEXT_U8_CAPACITY",
    "STASIS_RENDER_I32_COUNT": "FRAME_I32_CAPACITY",
    "STASIS_RENDER_F32_COUNT": "FRAME_F32_CAPACITY",
    "STASIS_RENDER_F_CLEAR_BASE": "F_CLEAR_BASE",
}


def compare(producer: str, consumer: str, expected: dict[str, int], actual: dict[str, int], mapping: dict[str, str]) -> list[Mismatch]:
    failures = []
    for source_name, consumer_name in mapping.items():
        value = actual.get(consumer_name, "missing")
        if value != expected[source_name]:
            failures.append(Mismatch(producer, consumer, source_name, expected[source_name], value))
    return failures


def check_literal(
    producer: Path,
    consumer: Path,
    text: str,
    field: str,
    expected: object,
    needle: str,
) -> Mismatch | None:
    if needle in text:
        return None
    return Mismatch(label(producer), label(consumer), field, expected, "missing")


def literal_array(text: str, name: str) -> int | str:
    patterns = (
        rf"\b{name}\s*:\s*(?:i32|f32|u8)\[([0-9_]+)\]",
        rf"\b{name}\s*:\s*Vec<[^>]+>\s*=\s*vec!\[[^;]+;\s*(?:stasis_dynload::)?(STASIS_RENDER_[A-Z0-9_]+)\]",
        rf"\b{name}\s*:\s*Vec<[^>]+>\s*=\s*vec!\[[^;]+;\s*([0-9_]+)\]",
        rf"\b{name}\[([0-9_]+)\]",
    )
    for pattern in patterns:
        match = re.search(pattern, text)
        if match:
            value = match.group(1)
            return value if value.startswith("STASIS_RENDER_") else int(value.replace("_", ""))
    return "missing"


def array_matches(actual: int | str, expected: int, name: str) -> bool:
    if actual == expected:
        return True
    aliases = {
        "gfx_cmd_i32": "STASIS_RENDER_I32_COUNT",
        "gfx_cmd_f32": "STASIS_RENDER_F32_COUNT",
        "gfx_cmd_u8": "STASIS_RENDER_U8_COUNT",
    }
    if actual == aliases.get(name):
        return True
    return False


def label(path: Path) -> str:
    return path.as_posix()


def required_write(text: str, lane: str, index: int) -> bool:
    return re.search(rf"\b{lane}\[{index}\]\s*=", text) is not None


def without_c_comments(text: str) -> str:
    return re.sub(r"/\*.*?\*/|//[^\r\n]*", "", text, flags=re.S)


def check(root: Path = ROOT, overlays: dict[Path, str] | None = None) -> tuple[list[Mismatch], dict[str, object]]:
    overlays = overlays or {}
    sources = {path: overlays.get(path, (root / path).read_text(encoding="utf-8")) for path in REQUIRED}
    render = c_constants(sources[RENDER_HEADER])
    host = stasis_constants(sources[HOST_FRAME])
    gfx = stasis_constants(sources[GFX_CMD])
    rust = rust_constants(sources[DYNLOAD], "STASIS_RENDER_")
    toolchain_provenance = rust_constants(sources[TOOLCHAIN], "GFX_CMD_")
    package_provenance = python_constants(sources[PACKAGE_PROVENANCE], "CURRENT_COMMAND_BUFFER_VERSION")
    java = java_constants(sources[JAVA_RENDERER])
    web = javascript_constants(sources[WEB])
    failures = compare(label(RENDER_HEADER), label(GFX_CMD), render, gfx, RENDER_TO_GFX)
    checks = 0
    for consumer in RENDER_DOWNSTREAM:
        text = sources[consumer]
        for pattern in LEGACY_RENDER_PATTERNS:
            match = pattern.search(text)
            checks += 1
            if match:
                failures.append(Mismatch(
                    label(RENDER_HEADER), label(consumer),
                    "render_abi.legacy_token", "current-only render ABI", match.group(0),
                ))
    failures += compare(label(RENDER_HEADER), label(DYNLOAD), render, rust, RENDER_TO_RUST)
    failures += compare(label(RENDER_HEADER), label(JAVA_RENDERER), render, java, RENDER_TO_JAVA)
    failures += compare(label(RENDER_HEADER), label(WEB), render, web, RENDER_TO_WEB)
    failures += compare(label(RENDER_HEADER), label(TOOLCHAIN), render, toolchain_provenance, RENDER_TO_TOOLCHAIN_PROVENANCE)
    failures += compare(label(RENDER_HEADER), label(PACKAGE_PROVENANCE), render, package_provenance, RENDER_TO_PACKAGE_PROVENANCE)
    checks += (
        len(RENDER_TO_GFX)
        + len(RENDER_TO_RUST)
        + len(RENDER_TO_JAVA)
        + len(RENDER_TO_WEB)
        + len(RENDER_TO_TOOLCHAIN_PROVENANCE)
        + len(RENDER_TO_PACKAGE_PROVENANCE)
    )
    for mismatch in (
        check_literal(RENDER_HEADER, RELEASE_PROVENANCE, sources[RELEASE_PROVENANCE],
                      "command_buffer.name", "gfx_cmd", 'COMMAND_BUFFER_NAME = "gfx_cmd"'),
        check_literal(RENDER_HEADER, RELEASE_PROVENANCE, sources[RELEASE_PROVENANCE],
                      "command_buffer.version.source", "render_contract_version(root)",
                      "command_buffer_version = render_contract_version(root)"),
        check_literal(RENDER_HEADER, RELEASE_PROVENANCE, sources[RELEASE_PROVENANCE],
                      "command_buffer.version.emission", "command_buffer_version",
                      '"version": command_buffer_version'),
        check_literal(RENDER_HEADER, TOOLCHAIN, sources[TOOLCHAIN],
                      "command_buffer.name", "gfx_cmd", 'GFX_CMD_NAME: &str = "gfx_cmd"'),
        check_literal(RENDER_HEADER, PACKAGE_PROVENANCE, sources[PACKAGE_PROVENANCE],
                      "command_buffer.name", "gfx_cmd", 'COMMAND_BUFFER_NAME = "gfx_cmd"'),
    ):
        checks += 1
        if mismatch is not None:
            failures.append(mismatch)

    fixture_text = without_c_comments(sources[DESKTOP_MANIFEST_FIXTURE])
    canonical_gfx_import = 'import "../../../src/stdlib/internal/gfx_cmd.stasis";'
    checks += 1
    if canonical_gfx_import not in fixture_text:
        failures.append(Mismatch(
            label(GFX_CMD), label(DESKTOP_MANIFEST_FIXTURE), "gfx_cmd.import",
            canonical_gfx_import, "missing",
        ))
    for lane in ("i32", "f32", "u8"):
        checks += 1
        if re.search(rf"\bglobal\s+gfx_cmd_{lane}\s*:", fixture_text):
            failures.append(Mismatch(
                label(GFX_CMD), label(DESKTOP_MANIFEST_FIXTURE),
                f"gfx_cmd_{lane}.declaration", "provided by canonical import",
                "manual declaration",
            ))
        checks += 1
        if re.search(rf"\bgfx_cmd_{lane}\s*\[[^\]]+\]\s*=", fixture_text):
            failures.append(Mismatch(
                label(GFX_CMD), label(DESKTOP_MANIFEST_FIXTURE),
                f"gfx_cmd_{lane}.write", "canonical helper call", "manual ABI write",
            ))

    harness_text = sources[DESKTOP_MANIFEST_HARNESS]
    harness_capacities = {
        "gfx_cmd_i32": r"vec!\[\s*0\s*;\s*STASIS_RENDER_I32_COUNT\s*\]",
        "gfx_cmd_f32": r"vec!\[\s*0(?:\.0)?\s*;\s*STASIS_RENDER_F32_COUNT\s*\]",
        "gfx_cmd_u8": r"vec!\[\s*0\s*;\s*STASIS_RENDER_U8_COUNT\s*\]",
    }
    for lane, pattern in harness_capacities.items():
        checks += 1
        if not re.search(pattern, harness_text):
            failures.append(Mismatch(
                label(DYNLOAD), label(DESKTOP_MANIFEST_HARNESS),
                f"{lane}.host_capacity", "canonical STASIS_RENDER_*_COUNT reference",
                "missing",
            ))

    current_trace_consumers = {
        DESKTOP_INPUT_FRAME_HARNESS: sources[DESKTOP_INPUT_FRAME_HARNESS],
        DESKTOP_DISPLAY_METRICS_HARNESS: sources[DESKTOP_DISPLAY_METRICS_HARNESS],
        DESKTOP_MANIFEST_HARNESS: harness_text,
    }
    numeric_trace_patterns = (
        r"\bconst\s+[A-Z0-9_]*TRACE\s*:\s*i32\s*=\s*-?[0-9]",
        r"assert_eq!\s*\(\s*\[[^\]]*trace[^\]]*\]\s*,\s*\[\s*-?[0-9]",
        r"assert_eq!\s*\(\s*[A-Za-z0-9_]*trace\s*,\s*-?[0-9][0-9_]{4,}",
    )
    for consumer, text in current_trace_consumers.items():
        checks += 1
        if any(re.search(pattern, text, re.S) for pattern in numeric_trace_patterns):
            failures.append(Mismatch(
                label(RENDER_HEADER), label(consumer),
                "current_render_trace.fixed_numeric_oracle",
                "nonzero and semantic trace relationships", "fixed numeric trace",
            ))

    checks += 1
    replay_trace_call = re.search(
        r"native_render_trace\s*\(\s*gfx_cmd_i32\s*,\s*35120\s*,\s*"
        r"gfx_cmd_f32\s*,\s*126084\s*,\s*gfx_cmd_u8\s*,\s*65536\s*\)",
        sources[JIT_AOT_REPLAY_FIXTURE], re.S,
    )
    if replay_trace_call is None:
        failures.append(Mismatch(
            label(RENDER_HEADER), label(JIT_AOT_REPLAY_FIXTURE),
            "render_trace.current_capacities", "35120/126084/65536", "missing",
        ))

    trace_relationships = {
        DESKTOP_INPUT_FRAME_HARNESS: (
            'assert_ne!(trace, 0, "native render trace must accept the guest frame")',
            "down_trace, move_trace,",
            "move_trace, up_trace,",
            "down_trace, up_trace,",
        ),
        DESKTOP_DISPLAY_METRICS_HARNESS: (
            "assert_ne!(trace, 0",
            "duplicate_trace, restored_trace,",
            "portrait_down_trace, landscape_down_trace",
            "landscape_down_trace, landscape_release_trace",
            "restored_portrait_trace, quiet_trace",
            "distinct_semantic_traces",
        ),
        DESKTOP_MANIFEST_HARNESS: (
            "trace, 0,",
        ),
    }
    for consumer, needles in trace_relationships.items():
        for index, needle in enumerate(needles):
            checks += 1
            if needle not in sources[consumer]:
                failures.append(Mismatch(
                    label(RENDER_HEADER), label(consumer),
                    f"current_render_trace.semantic_relation.{index}", needle, "missing",
                ))

    mobile_aot_c = without_c_comments(sources[GENERATED_MOBILE_AOT_C])
    mobile_aot_rust = without_c_comments(sources[GENERATED_MOBILE_AOT_RUST])
    fixed_trace_patterns = {
        GENERATED_MOBILE_AOT_C: (
            r"\bIT012_EXPECTED_TRACE\b",
            r"(?:#define\s+[A-Z0-9_]*TRACE|const\s+uint32_t\s+[A-Za-z0-9_]*trace)\s+(?:=\s*)?[0-9]",
            r"\bsubmitted_trace\s*==\s*[0-9][0-9A-Fa-f_xXuUlL]*\b",
            r"\b2880741754[uUlL]*\b",
        ),
        GENERATED_MOBILE_AOT_RUST: (
            r"\bEXPECTED_TRACE\b",
            r"\bconst\s+[A-Z0-9_]*TRACE\s*:\s*u32\s*=\s*[0-9]",
            r"assert_eq!\(\s*trace\s*,\s*[0-9][0-9_]*",
            r"\b2_880_741_754\b",
        ),
    }
    for consumer, patterns in fixed_trace_patterns.items():
        text = mobile_aot_c if consumer == GENERATED_MOBILE_AOT_C else mobile_aot_rust
        checks += 1
        if any(re.search(pattern, text) for pattern in patterns):
            failures.append(Mismatch(
                label(RENDER_HEADER), label(consumer), "it012.fixed_trace_oracle",
                "semantic trace derived from canonical frame", "fixed numeric trace",
            ))

    semantic_oracle_patterns = {
        "function": r"static\s+uint32_t\s+it012_expected_frame_trace\s*\(\s*void\s*\)",
        "i32.heap_capacity": r"calloc\s*\(\s*\(size_t\)STASIS_RENDER_I32_COUNT\s*,",
        "f32.heap_capacity": r"calloc\s*\(\s*\(size_t\)STASIS_RENDER_F32_COUNT\s*,",
        "u8.heap_capacity": r"calloc\s*\(\s*\(size_t\)STASIS_RENDER_U8_COUNT\s*,",
        "magic": r"expected_i32\s*\[\s*STASIS_RENDER_I_MAGIC\s*\]\s*=\s*STASIS_RENDER_MAGIC",
        "version": r"expected_i32\s*\[\s*STASIS_RENDER_I_VERSION\s*\]\s*=\s*STASIS_RENDER_VERSION",
        "flags": r"STASIS_RENDER_FLAG_CLEAR\s*\|\s*STASIS_RENDER_FLAG_PRESENT",
        "rect.payload": r"rect_base\s*=\s*STASIS_RENDER_F_RECT_REVERSE_BASE",
        "text.metadata": r"text_i32_base\s*=\s*STASIS_RENDER_I_TEXT_BASE",
        "text.payload": r"text_f32_base\s*=\s*STASIS_RENDER_F_TEXT_BASE",
        "order.rect": r"STASIS_RENDER_ORDER_RECT\s*\*\s*STASIS_RENDER_ORDER_KIND_SCALE",
        "order.text": r"STASIS_RENDER_ORDER_TEXT\s*\*\s*STASIS_RENDER_ORDER_KIND_SCALE",
        "validation": r"stasis_render_validate\s*\(\s*expected_i32\s*,\s*expected_f32\s*\)",
        "trace": r"stasis_render_trace\s*\(\s*expected_i32\s*,\s*expected_f32\s*,\s*expected_u8\s*\)",
        "comparison": r"CHECK\s*\(\s*submitted_trace\s*==\s*expected_trace\s*\)",
        "i32.free": r"free\s*\(\s*expected_i32\s*\)",
        "f32.free": r"free\s*\(\s*expected_f32\s*\)",
        "u8.free": r"free\s*\(\s*expected_u8\s*\)",
    }
    for field, pattern in semantic_oracle_patterns.items():
        checks += 1
        if not re.search(pattern, mobile_aot_c):
            failures.append(Mismatch(
                label(RENDER_HEADER), label(GENERATED_MOBILE_AOT_C),
                f"it012.semantic_oracle.{field}", "canonical semantic frame reference",
                "missing",
            ))
    for field, needle in (
        ("trace.parse", 'strip_prefix("trace=")'),
        ("trace.evidence", '"trace": trace'),
    ):
        checks += 1
        if needle not in mobile_aot_rust:
            failures.append(Mismatch(
                label(GENERATED_MOBILE_AOT_C), label(GENERATED_MOBILE_AOT_RUST),
                f"it012.{field}", needle, "missing",
            ))
    for lane, pattern in DESCRIPTOR_PATTERNS.items():
        checks += 1
        if not re.search(pattern, sources[RENDER_HEADER]):
            failures.append(Mismatch(label(RENDER_HEADER), label(JNI),
                                     f"descriptor.{lane}", "canonical count*sizeof/_Alignof", "missing"))

    arrays = {
        "host_i32": host["HOST_I32_COUNT"], "host_f32": host["HOST_F32_COUNT"],
        "gfx_cmd_i32": render["STASIS_RENDER_I32_COUNT"],
        "gfx_cmd_f32": render["STASIS_RENDER_F32_COUNT"],
        "gfx_cmd_u8": render["STASIS_RENDER_U8_COUNT"],
    }
    for name, expected in arrays.items():
        source = HOST_FRAME if name.startswith("host_") else GFX_CMD
        actual = literal_array(sources[source], name)
        checks += 1
        if not array_matches(actual, expected, name):
            producer = HOST_FRAME if name.startswith("host_") else RENDER_HEADER
            failures.append(Mismatch(label(producer), label(source), f"{name}.length", expected, actual))

    for consumer in (DESKTOP, AOT):
        text = sources[consumer]
        for name, expected in arrays.items():
            actual = literal_array(text, name)
            checks += 1
            if not array_matches(actual, expected, name):
                producer = HOST_FRAME if name.startswith("host_") else RENDER_HEADER
                failures.append(Mismatch(label(producer), label(consumer), f"{name}.length", expected, actual))

    for name, expected in arrays.items():
        marker = rf"\b{re.escape(name)},\s*{expected}\);"
        checks += 1
        if not re.search(marker, sources[AOT]):
            failures.append(Mismatch(label(RENDER_HEADER if name.startswith("gfx_") else HOST_FRAME), label(AOT), f"{name}.registration_length", expected, "missing"))

    desktop_host = rust_constants(sources[DESKTOP], "HOST_")
    for name, actual in desktop_host.items():
        if name not in host:
            continue
        checks += 1
        if actual != host[name]:
            failures.append(Mismatch(label(HOST_FRAME), label(DESKTOP), name, host[name], actual))

    android_text = sources[ANDROID]
    android_counts = rust_constants(android_text, "HOST_")
    for name in ("HOST_I32_COUNT", "HOST_F32_COUNT"):
        checks += 1
        if android_counts.get(name) != host[name]:
            failures.append(Mismatch(label(HOST_FRAME), label(ANDROID), name, host[name], android_counts.get(name, "missing")))

    active_host = {
        name: value for name, value in host.items()
        if (name.startswith("HOST_I_") and not any(token in name for token in ("KEY_", "POINTER_BASE", "POINTER_STRIDE")))
        or (name.startswith("HOST_F_") and "POINTER" not in name)
    }
    for name, index in active_host.items():
        lane = "host_i32" if name.startswith("HOST_I_") else "host_f32"
        checks += 1
        if not required_write(android_text, lane, index):
            failures.append(Mismatch(label(HOST_FRAME), label(ANDROID), name, f"{lane}[{index}] write", "missing"))

        native_lane = "out_i32" if name.startswith("HOST_I_") else "out_f32"
        checks += 1
        if not required_write(sources[NATIVE_HOST], native_lane, index):
            failures.append(Mismatch(label(HOST_FRAME), label(NATIVE_HOST), name, f"{native_lane}[{index}] write", "missing"))

    aot_fields = (
        "HOST_I_SCREEN_W_PX", "HOST_I_SCREEN_H_PX", "HOST_I_VERSION",
        "HOST_I_NATIVE_W_PX", "HOST_I_NATIVE_H_PX", "HOST_I_DRAWABLE_W_PX",
        "HOST_I_DRAWABLE_H_PX", "HOST_I_DISPLAY_GENERATION",
        "HOST_I_DENSITY_GENERATION", "HOST_F_CONTENT_SCALE", "HOST_F_RASTER_SCALE",
        "HOST_F_LOGICAL_W", "HOST_F_LOGICAL_H", "HOST_F_SAFE_X", "HOST_F_SAFE_Y",
        "HOST_F_SAFE_W", "HOST_F_SAFE_H",
    )
    for name in aot_fields:
        lane = "host_i32" if name.startswith("HOST_I_") else "host_f32"
        checks += 1
        if not required_write(sources[AOT], lane, host[name]):
            failures.append(Mismatch(label(HOST_FRAME), label(AOT), name, f"{lane}[{host[name]}] write", "missing"))

    for base in ("src", "samples", "tests", "mobile", "vscode-stasis"):
        directory = root / base
        if not directory.exists():
            continue
        for path in repository_stasis_sources(directory):
            text = path.read_text(encoding="utf-8")
            for name, expected in arrays.items():
                if not re.search(rf"\bglobal\s+{re.escape(name)}\s*:", text):
                    continue
                actual = literal_array(text, name)
                checks += 1
                if not array_matches(actual, expected, name):
                    producer = HOST_FRAME if name.startswith("host_") else RENDER_HEADER
                    failures.append(Mismatch(label(producer), path.relative_to(root).as_posix(), f"{name}.length", expected, actual))

    header_match = re.search(
        r"static\s+final\s+int\s+RENDER_FRAME_HEADER_SIZE\s*=\s*([0-9_]+);",
        sources[WORKSHOP],
    )
    workshop_header = int(header_match.group(1).replace("_", "")) if header_match else "missing"
    header_size = render["STASIS_RENDER_I_DENSITY_GENERATION"] + 1
    checks += 1
    if workshop_header != header_size:
        failures.append(Mismatch(label(RENDER_HEADER), label(WORKSHOP), "RENDER_FRAME_HEADER_SIZE", header_size, workshop_header))

    for name in ("STASIS_RENDER_I32_COUNT", "STASIS_RENDER_F32_COUNT", "STASIS_RENDER_U8_COUNT"):
        checks += 1
        if name not in sources[JNI]:
            failures.append(Mismatch(label(RENDER_HEADER), label(JNI), name, "canonical macro reference", "missing"))
    checks += 1
    descriptor_initializer = re.compile(
        r"static\s+const\s+StasisJniFrameDescriptor\s+stasis_jni_frame_descriptors\s*\[\s*\]\s*="
        r"\s*\{\s*STASIS_RENDER_BUFFER_DESCRIPTORS\s*\(\s*STASIS_JNI_FRAME_DESCRIPTOR\s*\)\s*\}\s*;"
    )
    if not descriptor_initializer.search(without_c_comments(sources[JNI])):
        failures.append(Mismatch(label(RENDER_HEADER), label(JNI),
                                 "STASIS_RENDER_BUFFER_DESCRIPTORS.initializer",
                                 "canonical descriptor invocation", "missing"))

    digest = hashlib.sha256((sources[RENDER_HEADER] + sources[HOST_FRAME]).encode()).hexdigest()
    evidence = {
        "schema": "stasis.seam_test.v1",
        "test_id": "IT-001",
        "status": "passed" if not failures else "failed",
        "target": "repository-source",
        "fixture_revision": digest,
        "checks": checks,
        "consumers": [label(path) for path in REQUIRED[2:]],
        "contract": {**{key: render[key] for key in ("STASIS_RENDER_VERSION", "STASIS_RENDER_I32_COUNT", "STASIS_RENDER_F32_COUNT", "STASIS_RENDER_U8_COUNT")}, "HOST_I32_COUNT": host["HOST_I32_COUNT"], "HOST_F32_COUNT": host["HOST_F32_COUNT"]},
        "failures": [str(failure) for failure in failures],
    }
    return failures, evidence


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--evidence", type=Path, default=ROOT / "target/seam-tests/it-001-runtime-abi.json")
    args = parser.parse_args()
    failures, evidence = check()
    args.evidence.parent.mkdir(parents=True, exist_ok=True)
    args.evidence.write_text(json.dumps(evidence, indent=2) + "\n", encoding="utf-8")
    for failure in failures:
        print(failure)
    if failures:
        return 1
    print(f"IT-001 runtime ABI contract passed ({evidence['checks']} comparisons)")
    print(f"evidence: {args.evidence}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
