#!/usr/bin/env python3
"""Verify iOS simulator receipts from the production mobile presentation path."""

from __future__ import annotations

import argparse
import json
import struct
from pathlib import Path


def load_receipt(path: Path, stage: str) -> dict:
    value = json.loads(path.read_text(encoding="utf-8"))
    if value.get("schema") != "stasis.ios.aspect_fit.v1":
        raise SystemExit(f"{path}: unexpected schema")
    if value.get("stage") != stage:
        raise SystemExit(f"{path}: expected stage {stage!r}")
    if value.get("logical") != [1600, 720]:
        raise SystemExit(f"{path}: logical canvas is not 1600x720")
    return value


def rect_inside(inner: list[float], outer: list[float], label: str) -> None:
    ix, iy, iw, ih = inner
    ox, oy, ow, oh = outer
    if iw <= 0 or ih <= 0 or ow <= 0 or oh <= 0:
        raise SystemExit(f"{label}: non-positive rectangle")
    epsilon = 1.1
    if (
        ix < ox - epsilon
        or iy < oy - epsilon
        or ix + iw > ox + ow + epsilon
        or iy + ih > oy + oh + epsilon
    ):
        raise SystemExit(f"{label}: {inner!r} is outside {outer!r}")


def validate_presentation(value: dict, require_inset: bool) -> None:
    drawable = value["drawable"]
    safe = value["safe_drawable"]
    viewport = value["drawable_viewport"]
    rect_inside(safe, [0.0, 0.0, float(drawable[0]), float(drawable[1])], "safe drawable")
    rect_inside(viewport, safe, "fitted drawable viewport")
    if abs(viewport[2] / viewport[3] - 1600.0 / 720.0) > 0.01:
        raise SystemExit(f"fitted viewport has wrong aspect ratio: {viewport!r}")
    if require_inset:
        right = safe[0] + safe[2]
        bottom = safe[1] + safe[3]
        if (
            abs(safe[0]) < 0.5
            and abs(safe[1]) < 0.5
            and abs(right - drawable[0]) < 0.5
            and abs(bottom - drawable[1]) < 0.5
        ):
            raise SystemExit("simulator reported no safe-area inset")


def png_size(path: Path) -> tuple[int, int]:
    data = path.read_bytes()
    if data[:8] != b"\x89PNG\r\n\x1a\n" or data[12:16] != b"IHDR":
        raise SystemExit(f"{path}: not a PNG screenshot")
    return struct.unpack(">II", data[16:24])


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--actual", type=Path, required=True)
    parser.add_argument("--left", type=Path, required=True)
    parser.add_argument("--pointer", type=Path, required=True)
    parser.add_argument("--right", type=Path, required=True)
    parser.add_argument("--left-screenshot", type=Path, required=True)
    parser.add_argument("--right-screenshot", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    actual = load_receipt(args.actual, "actual")
    left = load_receipt(args.left, "landscape-left")
    pointer = load_receipt(args.pointer, "pointer")
    right = load_receipt(args.right, "landscape-right")
    for value in (actual, left, pointer, right):
        validate_presentation(value, require_inset=True)
        if value["native"][0] <= value["native"][1]:
            raise SystemExit(f"{value['stage']}: simulator is not landscape")

    if left["safe_drawable"][0] <= 0.0:
        raise SystemExit("left orientation did not retain the cutout inset")
    if abs(right["safe_drawable"][0]) > 0.5:
        raise SystemExit("right orientation did not move the cutout inset")
    if right["display_generation"] <= left["display_generation"]:
        raise SystemExit("orientation/safe-area change did not advance display generation")
    observed = pointer["pointer"]
    expected = {
        "id": 1,
        "down": 1,
        "went_down": 1,
    }
    for key, value in expected.items():
        if observed.get(key) != value:
            raise SystemExit(f"pointer {key}={observed.get(key)!r}, expected {value!r}")
    for key, value in (
        ("x", 160.0),
        ("y", 72.0),
        ("x_normalized", 0.1),
        ("y_normalized", 0.1),
    ):
        if abs(float(observed[key]) - value) > 0.01:
            raise SystemExit(f"pointer {key}={observed[key]!r}, expected {value!r}")

    left_png = png_size(args.left_screenshot)
    right_png = png_size(args.right_screenshot)
    if left_png[0] <= left_png[1] or right_png[0] <= right_png[1]:
        raise SystemExit("simulator screenshots are not landscape")

    evidence = {
        "schema": "stasis.ios.aspect_fit.evidence.v1",
        "logical": [1600, 720],
        "actual": actual,
        "landscape_left": left,
        "pointer": pointer,
        "landscape_right": right,
        "screenshots": {
            "landscape_left": {
                "path": str(args.left_screenshot),
                "pixels": list(left_png),
            },
            "landscape_right": {
                "path": str(args.right_screenshot),
                "pixels": list(right_png),
            },
        },
        "physical_device_qualified": False,
    }
    args.output.write_text(
        json.dumps(evidence, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )


if __name__ == "__main__":
    main()
