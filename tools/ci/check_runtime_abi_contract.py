#!/usr/bin/env python3
"""Verify copied HostFrame and render ABI values against their canonical sources."""

from __future__ import annotations

import argparse
import ast
import hashlib
import json
import operator
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
ANDROID = Path("crates/stasis_android_bridge/src/lib.rs")
JAVA_RENDERER = Path(
    "mobile/android/app/src/main/java/com/stasislang/workshop/StasisPreviewRenderer.java"
)
WORKSHOP = Path(
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/MainActivity.java"
)
JNI = Path("mobile/android/app/src/main/cpp/stasis_mobile_smoke.c")
NATIVE_HOST = Path("runtime/stasis_graphics.c")
REQUIRED = (
    RENDER_HEADER, HOST_FRAME, GFX_CMD, DYNLOAD, DESKTOP, AOT, ANDROID,
    JAVA_RENDERER, WORKSHOP, JNI, NATIVE_HOST,
)

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
        rf"^\s*(?:pub\s+)?const\s+({re.escape(prefix)}[A-Z0-9_]+):\s*(?:usize|i32)\s*=\s*([^;]+);",
        text, re.M,
    )
    return resolve({name: expression for name, expression in pairs})


def java_constants(text: str) -> dict[str, int]:
    pairs = re.findall(r"^\s*(?:private\s+)?static\s+final\s+int\s+([A-Z][A-Z0-9_]+)\s*=\s*([^;]+);", text, re.M)
    return resolve({name: expression for name, expression in pairs})


RENDER_TO_GFX = {
    "STASIS_RENDER_V2_MAGIC": "GFX_CMD_MAGIC",
    "STASIS_RENDER_CURRENT_VERSION": "GFX_CMD_VERSION",
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
    "STASIS_RENDER_I_DROPPED_RECTS": "GFX_I_DROPPED_RECTS",
    "STASIS_RENDER_I_SPRITE_BASE": "GFX_I_SPRITE_BASE",
    "STASIS_RENDER_I_TEXT_BASE": "GFX_I_TEXT_BASE",
    "STASIS_RENDER_I_ORDER_BASE": "GFX_I_ORDER_BASE",
    "STASIS_RENDER_F_LINE_BASE": "GFX_F_LINE_BASE",
    "STASIS_RENDER_F_SPRITE_BASE": "GFX_F_SPRITE_BASE",
    "STASIS_RENDER_F_RECT_REVERSE_BASE": "GFX_F_RECT_REVERSE_BASE",
    "STASIS_RENDER_F_TEXT_BASE": "GFX_F_TEXT_BASE",
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
    "STASIS_RENDER_MAX_ORDER": "GFX_MAX_ORDER",
    "STASIS_RENDER_ORDER_KIND_SCALE": "GFX_ORDER_KIND_SCALE",
    "STASIS_RENDER_ORDER_LINE": "GFX_ORDER_LINE",
    "STASIS_RENDER_ORDER_SPRITE": "GFX_ORDER_SPRITE",
    "STASIS_RENDER_ORDER_TEXT": "GFX_ORDER_TEXT",
    "STASIS_RENDER_ORDER_RECT": "GFX_ORDER_RECT",
}

RENDER_TO_RUST = {
    "STASIS_RENDER_I32_COUNT": "STASIS_RENDER_I32_COUNT",
    "STASIS_RENDER_V2_I32_COUNT": "STASIS_RENDER_V2_I32_COUNT",
    "STASIS_RENDER_F32_COUNT": "STASIS_RENDER_F32_COUNT",
    "STASIS_RENDER_U8_COUNT": "STASIS_RENDER_U8_COUNT",
    "STASIS_RENDER_V2_MAGIC": "STASIS_RENDER_MAGIC",
    "STASIS_RENDER_V2_VERSION": "STASIS_RENDER_V2_VERSION",
    "STASIS_RENDER_V3_VERSION": "STASIS_RENDER_V3_VERSION",
    "STASIS_RENDER_CURRENT_VERSION": "STASIS_RENDER_VERSION",
    "STASIS_RENDER_I_ORDER_COUNT": "STASIS_RENDER_ORDER_COUNT_INDEX",
    "STASIS_RENDER_I_RECT_COUNT": "STASIS_RENDER_RECT_COUNT_INDEX",
    "STASIS_RENDER_I_ORDER_BASE": "STASIS_RENDER_ORDER_BASE",
    "STASIS_RENDER_MAX_ORDER": "STASIS_RENDER_MAX_ORDER",
    "STASIS_RENDER_I_SPRITE_BASE": "STASIS_RENDER_SPRITE_BASE",
    "STASIS_RENDER_MAX_LINES": "STASIS_RENDER_MAX_LINES",
    "STASIS_RENDER_LINE_F32_STRIDE": "STASIS_RENDER_LINE_STRIDE",
    "STASIS_RENDER_MAX_SPRITES": "STASIS_RENDER_MAX_SPRITES",
    "STASIS_RENDER_SPRITE_I32_STRIDE": "STASIS_RENDER_SPRITE_STRIDE_I32",
    "STASIS_RENDER_F_SPRITE_BASE": "STASIS_RENDER_SPRITE_BASE_F32",
    "STASIS_RENDER_SPRITE_F32_STRIDE": "STASIS_RENDER_SPRITE_STRIDE_F32",
    "STASIS_RENDER_I_TEXT_BASE": "STASIS_RENDER_TEXT_BASE_I32",
    "STASIS_RENDER_F_TEXT_BASE": "STASIS_RENDER_TEXT_BASE_F32",
    "STASIS_RENDER_MAX_TEXT": "STASIS_RENDER_MAX_TEXT",
    "STASIS_RENDER_TEXT_I32_STRIDE": "STASIS_RENDER_TEXT_STRIDE_I32",
    "STASIS_RENDER_TEXT_F32_STRIDE": "STASIS_RENDER_TEXT_STRIDE_F32",
}

RENDER_TO_JAVA = {
    "STASIS_RENDER_V2_MAGIC": "RENDER_MAGIC",
    "STASIS_RENDER_V2_VERSION": "RENDER_V2_VERSION",
    "STASIS_RENDER_V3_VERSION": "RENDER_V3_VERSION",
    "STASIS_RENDER_CURRENT_VERSION": "RENDER_VERSION",
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
}


def compare(producer: str, consumer: str, expected: dict[str, int], actual: dict[str, int], mapping: dict[str, str]) -> list[Mismatch]:
    failures = []
    for source_name, consumer_name in mapping.items():
        value = actual.get(consumer_name, "missing")
        if value != expected[source_name]:
            failures.append(Mismatch(producer, consumer, source_name, expected[source_name], value))
    return failures


def literal_array(text: str, name: str) -> int | str:
    patterns = (
        rf"\b{name}\s*:\s*(?:i32|f32|u8)\[([0-9_]+)\]",
        rf"\b{name}\s*:\s*Vec<[^>]+>\s*=\s*vec!\[[^;]+;\s*([0-9_]+)\]",
        rf"\b{name}\[([0-9_]+)\]",
    )
    for pattern in patterns:
        match = re.search(pattern, text)
        if match:
            return int(match.group(1).replace("_", ""))
    return "missing"


def label(path: Path) -> str:
    return path.as_posix()


def required_write(text: str, lane: str, index: int) -> bool:
    return re.search(rf"\b{lane}\[{index}\]\s*=", text) is not None


def check(root: Path = ROOT, overlays: dict[Path, str] | None = None) -> tuple[list[Mismatch], dict[str, object]]:
    overlays = overlays or {}
    sources = {path: overlays.get(path, (root / path).read_text(encoding="utf-8")) for path in REQUIRED}
    render = c_constants(sources[RENDER_HEADER])
    host = stasis_constants(sources[HOST_FRAME])
    gfx = stasis_constants(sources[GFX_CMD])
    rust = rust_constants(sources[DYNLOAD], "STASIS_RENDER_")
    java = java_constants(sources[JAVA_RENDERER])
    failures = compare(label(RENDER_HEADER), label(GFX_CMD), render, gfx, RENDER_TO_GFX)
    failures += compare(label(RENDER_HEADER), label(DYNLOAD), render, rust, RENDER_TO_RUST)
    failures += compare(label(RENDER_HEADER), label(JAVA_RENDERER), render, java, RENDER_TO_JAVA)
    checks = len(RENDER_TO_GFX) + len(RENDER_TO_RUST) + len(RENDER_TO_JAVA)

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
        if actual != expected:
            producer = HOST_FRAME if name.startswith("host_") else RENDER_HEADER
            failures.append(Mismatch(label(producer), label(source), f"{name}.length", expected, actual))

    for consumer in (DESKTOP, AOT):
        text = sources[consumer]
        for name, expected in arrays.items():
            actual = literal_array(text, name)
            checks += 1
            if actual != expected:
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
        for path in directory.rglob("*.stasis"):
            text = path.read_text(encoding="utf-8")
            for name, expected in arrays.items():
                if not re.search(rf"\bglobal\s+{re.escape(name)}\s*:", text):
                    continue
                actual = literal_array(text, name)
                checks += 1
                if actual != expected:
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

    digest = hashlib.sha256((sources[RENDER_HEADER] + sources[HOST_FRAME]).encode()).hexdigest()
    evidence = {
        "schema": "stasis.seam_test.v1",
        "test_id": "IT-001",
        "status": "passed" if not failures else "failed",
        "target": "repository-source",
        "fixture_revision": digest,
        "checks": checks,
        "consumers": [label(path) for path in REQUIRED[2:]],
        "contract": {**{key: render[key] for key in ("STASIS_RENDER_CURRENT_VERSION", "STASIS_RENDER_I32_COUNT", "STASIS_RENDER_F32_COUNT", "STASIS_RENDER_U8_COUNT")}, "HOST_I32_COUNT": host["HOST_I32_COUNT"], "HOST_F32_COUNT": host["HOST_F32_COUNT"]},
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
