#!/usr/bin/env python3
"""Verify the packaged iOS simulator no-CLEAR presentation baseline."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
from pathlib import Path

try:
    from verify_presentation_baseline import verify_capture
    from verify_render_parity import read_capture
except ImportError:
    from .verify_presentation_baseline import verify_capture
    from .verify_render_parity import read_capture


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def load_receipt(path: Path, phase: str) -> dict:
    value = json.loads(path.read_text(encoding="utf-8"))
    if value.get("schema") != "stasis.ios.presentation_baseline.v1":
        raise ValueError(f"{path}: unexpected schema")
    if value.get("phase") != phase or value.get("frame") != 1:
        raise ValueError(f"{path}: expected {phase!r} frame 1")
    if value.get("logical") != [640, 360]:
        raise ValueError(f"{path}: logical canvas is not 640x360")
    if value.get("poison_target") != "physical-window":
        raise ValueError(f"{path}: full physical-window poison was not proven")
    if value.get("poison_state_restored") is not True:
        raise ValueError(f"{path}: poison changed selected renderer state")
    if value.get("baseline_state_restored") is not True:
        raise ValueError(f"{path}: baseline clear changed renderer state")
    for field in ("poison_selected_target_state", "baseline_selected_target_state"):
        state = value.get(field)
        if not isinstance(state, list) or len(state) != 40:
            raise ValueError(f"{path}: {field} does not contain exact restored state")
        if any(
            type(word) is not int or word < -(1 << 31) or word >= (1 << 31)
            for word in state
        ):
            raise ValueError(f"{path}: {field} contains a non-i32 state word")
        if state[:20] != state[20:]:
            raise ValueError(f"{path}: {field} does not contain exact restored state")
    render = value.get("render", {})
    if (
        render.get("accepted") != render.get("presented")
        or not isinstance(render.get("accepted"), int)
        or render["accepted"] < 1
        or render.get("rejected") != 0
        or render.get("validation") != 0
        or render.get("command_trace") in (None, 0)
    ):
        raise ValueError(f"{path}: frame was not accepted and presented cleanly")
    resource = value.get("resource", {})
    if (
        resource.get("state") != 1
        or not isinstance(resource.get("surface_generation"), int)
        or resource["surface_generation"] < 1
        or not isinstance(resource.get("renderer_generation"), int)
        or resource["renderer_generation"] < 1
        or resource.get("restore_failures") != 0
    ):
        raise ValueError(f"{path}: renderer lifecycle was not ready")
    return value


def rotate_rgba(
    width: int, height: int, rgba: bytes, clockwise: bool
) -> tuple[int, int, bytes]:
    output = bytearray(len(rgba))
    output_width, output_height = height, width
    for y in range(height):
        for x in range(width):
            source = (y * width + x) * 4
            if clockwise:
                output_x, output_y = height - 1 - y, x
            else:
                output_x, output_y = y, width - 1 - x
            destination = (output_y * output_width + output_x) * 4
            output[destination : destination + 4] = rgba[source : source + 4]
    return output_width, output_height, bytes(output)


def checked_rect(value: object, label: str) -> tuple[int, int, int, int]:
    if not isinstance(value, list) or len(value) != 4:
        raise ValueError(f"{label} is not a four-element rectangle")
    rect = tuple(round(float(component)) for component in value)
    if rect[0] < 0 or rect[1] < 0 or rect[2] <= 0 or rect[3] <= 0:
        raise ValueError(f"{label} is not a positive rectangle")
    return rect  # type: ignore[return-value]


def magenta_pixels(rgba: bytes) -> int:
    return sum(
        1
        for offset in range(0, len(rgba), 4)
        if rgba[offset] > 180 and rgba[offset + 1] < 80 and rgba[offset + 2] > 180
    )


def verify_frame(path: Path, receipt: dict) -> dict:
    width, height, rgba = read_capture(path)
    viewport = checked_rect(receipt.get("drawable_viewport"), "drawable viewport")
    candidates = [("app-landscape", width, height, rgba)]
    if width < height:
        candidates = [
            ("hardware-native-portrait-clockwise", *rotate_rgba(width, height, rgba, True)),
            ("hardware-native-portrait-counterclockwise", *rotate_rgba(width, height, rgba, False)),
        ]
    matches: list[tuple[str, int, int, bytes, dict]] = []
    failures: list[str] = []
    for encoding, candidate_width, candidate_height, candidate_rgba in candidates:
        try:
            pixels = verify_capture(
                candidate_width,
                candidate_height,
                candidate_rgba,
                (0, 0, candidate_width, candidate_height),
                viewport,
            )
            poison_pixels = magenta_pixels(candidate_rgba)
            if poison_pixels:
                raise ValueError(f"physical target retains {poison_pixels} magenta pixels")
            matches.append(
                (encoding, candidate_width, candidate_height, candidate_rgba, pixels)
            )
        except ValueError as error:
            failures.append(f"{encoding}: {error}")
    if len(matches) != 1:
        raise ValueError(
            f"{path}: expected one valid screenshot orientation, got {len(matches)}; "
            + "; ".join(failures)
        )
    encoding, normalized_width, normalized_height, _, pixels = matches[0]
    safe = checked_rect(receipt.get("safe_drawable"), "safe drawable")
    if safe[0] + safe[2] > normalized_width or safe[1] + safe[3] > normalized_height:
        raise ValueError(f"{path}: safe drawable is outside the physical screenshot")
    return {
        "path": str(path),
        "sha256": sha256(path),
        "encoded_size": [width, height],
        "normalized_size": [normalized_width, normalized_height],
        "encoding": encoding,
        "poisoned_magenta_pixels": 0,
        "pixels": pixels,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--initial-receipt", type=Path, required=True)
    parser.add_argument("--initial-frame", type=Path, required=True)
    parser.add_argument("--relaunch-receipt", type=Path, required=True)
    parser.add_argument("--relaunch-frame", type=Path, required=True)
    parser.add_argument("--provenance", type=Path, required=True)
    parser.add_argument("--mobile-manifest", type=Path, required=True)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    initial = load_receipt(args.initial_receipt, "initial")
    relaunch = load_receipt(args.relaunch_receipt, "relaunch")
    initial_frame = verify_frame(args.initial_frame, initial)
    relaunch_frame = verify_frame(args.relaunch_frame, relaunch)
    for field in ("native_viewport", "drawable_viewport", "safe_drawable"):
        if initial.get(field) != relaunch.get(field):
            raise ValueError(f"fresh-process relaunch changed {field}")
    if initial_frame["normalized_size"] != relaunch_frame["normalized_size"]:
        raise ValueError("fresh-process relaunch changed the physical capture size")
    source = args.source.read_text(encoding="utf-8")
    if re.search(r"\bclear\s*\(", source):
        raise ValueError("presentation baseline source contains a guest clear call")
    if "fill_rect(80.0, 45.0, 160.0, 90.0, 0.9, 0.15, 0.08, 1.0)" not in source:
        raise ValueError("presentation baseline source is missing the canonical red rectangle")
    if not re.search(r"\binit_window\s*\(\s*640\s*,\s*360\s*,", source):
        raise ValueError("presentation baseline source is not the canonical 640x360 fixture")
    provenance = json.loads(args.provenance.read_text(encoding="utf-8"))
    manifest = json.loads(args.mobile_manifest.read_text(encoding="utf-8"))
    if provenance.get("schema") != "stasis.release_provenance.v1":
        raise ValueError("unexpected package provenance schema")
    if manifest.get("schema") != "stasis.mobile_package.v2":
        raise ValueError("unexpected mobile package schema")
    if manifest.get("target") != "ios-simulator-arm64" or manifest.get("development_build") is not True:
        raise ValueError("evidence package is not the development iOS simulator target")

    evidence = {
        "schema": "stasis.ios.presentation_baseline.evidence.v1",
        "logical": [640, 360],
        "poison_target": "physical-window",
        "initial": {"receipt": initial, "frame": initial_frame},
        "relaunch": {"receipt": relaunch, "frame": relaunch_frame},
        "source": {"path": str(args.source), "sha256": sha256(args.source), "guest_clear": False},
        "package": {
            "target": manifest["target"],
            "source_commit": provenance.get("source_commit"),
            "provenance_sha256": sha256(args.provenance),
            "mobile_manifest_sha256": sha256(args.mobile_manifest),
        },
        "surface_lifecycle_qualified": "fresh-process-relaunch",
        "same_process_context_loss_qualified": False,
        "physical_device_qualified": False,
    }
    args.output.write_text(
        json.dumps(evidence, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(json.dumps(evidence, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
