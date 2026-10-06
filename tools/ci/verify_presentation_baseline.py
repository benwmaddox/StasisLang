#!/usr/bin/env python3
"""Verify the Android Workshop's no-CLEAR presentation capture."""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path

try:
    from verify_render_parity import read_capture
except ImportError:
    from .verify_render_parity import read_capture


def parse_rect(value: str, label: str) -> tuple[int, int, int, int]:
    try:
        rect = tuple(int(part) for part in value.split(","))
    except ValueError as error:
        raise ValueError(f"{label} must be x,y,width,height") from error
    if len(rect) != 4 or rect[0] < 0 or rect[1] < 0 or rect[2] <= 0 or rect[3] <= 0:
        raise ValueError(f"{label} must be a positive in-bounds x,y,width,height")
    return rect


def _pixel(width: int, rgba: bytes, x: int, y: int) -> tuple[int, int, int, int]:
    offset = (y * width + x) * 4
    return tuple(rgba[offset : offset + 4])  # type: ignore[return-value]


def _near(pixel: tuple[int, int, int, int], expected: tuple[int, int, int, int], delta: int) -> bool:
    return pixel[3] >= 245 and max(abs(pixel[i] - expected[i]) for i in range(3)) <= delta


def verify_capture(
    width: int,
    height: int,
    rgba: bytes,
    surface: tuple[int, int, int, int],
    viewport: tuple[int, int, int, int],
) -> dict:
    sx, sy, sw, sh = surface
    vx, vy, vw, vh = viewport
    if len(rgba) != width * height * 4:
        raise ValueError("capture pixel payload has the wrong size")
    if sx + sw > width or sy + sh > height:
        raise ValueError("preview surface is outside the captured screen")
    if vx < sx or vy < sy or vx + vw > sx + sw or vy + vh > sy + sh:
        raise ValueError("logical viewport is outside the preview surface")
    if abs(vw * 360 - vh * 640) > 640:
        raise ValueError("logical viewport does not match the 640x360 sample aspect ratio")

    poisoned_magenta_pixels = 0
    for y in range(sy, sy + sh):
        for x in range(sx, sx + sw):
            pixel = _pixel(width, rgba, x, y)
            if pixel[0] > 180 and pixel[1] < 80 and pixel[2] > 180:
                poisoned_magenta_pixels += 1
    if poisoned_magenta_pixels:
        raise ValueError(
            f"physical preview surface retains poisoned magenta pixels: "
            f"{poisoned_magenta_pixels}"
        )

    black = (0, 0, 0, 255)
    scene = (230, 38, 20, 255)
    # Check all preview-surface margins and the logical background on a fixed
    # grid. The scene's solid interior is checked separately at pixel scale.
    margin_samples = margin_black = 0
    background_samples = background_black = 0
    for y in range(sy, sy + sh, 3):
        for x in range(sx, sx + sw, 3):
            in_viewport = vx <= x < vx + vw and vy <= y < vy + vh
            if not in_viewport:
                margin_samples += 1
                margin_black += _near(_pixel(width, rgba, x, y), black, 8)
                continue
            logical_x = (x - vx) * 640 / vw
            logical_y = (y - vy) * 360 / vh
            in_scene = 80 <= logical_x < 240 and 45 <= logical_y < 135
            if not in_scene:
                background_samples += 1
                background_black += _near(_pixel(width, rgba, x, y), black, 8)

    if margin_samples and margin_black / margin_samples < 0.995:
        raise ValueError(
            f"physical preview margins are not black: {margin_black}/{margin_samples} samples"
        )
    if background_samples == 0 or background_black / background_samples < 0.995:
        raise ValueError(
            f"present-only logical background is not black: "
            f"{background_black}/{background_samples} samples"
        )

    scale_x = vw / 640
    scale_y = vh / 360
    scene_left = vx + round(80 * scale_x)
    scene_top = vy + round(45 * scale_y)
    scene_right = vx + round(240 * scale_x)
    scene_bottom = vy + round(135 * scale_y)
    inset_x = max(1, round((scene_right - scene_left) * 0.04))
    inset_y = max(1, round((scene_bottom - scene_top) * 0.04))
    red_samples = red_matches = 0
    for y in range(scene_top + inset_y, scene_bottom - inset_y, 2):
        for x in range(scene_left + inset_x, scene_right - inset_x, 2):
            red_samples += 1
            red_matches += _near(_pixel(width, rgba, x, y), scene, 32)
    if red_samples == 0 or red_matches / red_samples < 0.95:
        raise ValueError(f"red scene coverage is too low: {red_matches}/{red_samples} samples")

    sample_x = min(vx + round(100 * scale_x), width - 1)
    sample_y = min(vy + round(70 * scale_y), height - 1)
    return {
        "schema": "stasis.workshop_present_only_capture.v1",
        "capture_size": [width, height],
        "preview_surface": list(surface),
        "logical_viewport": list(viewport),
        "logical_size": [640, 360],
        "physical_margin_black_samples": [margin_black, margin_samples],
        "logical_background_black_samples": [background_black, background_samples],
        "red_scene_samples": [red_matches, red_samples],
        "poisoned_magenta_pixels": poisoned_magenta_pixels,
        "scene_probe_rgba": list(_pixel(width, rgba, sample_x, sample_y)),
        "expected_scene_probe_rgba": list(scene),
    }


def verify_reset_placeholder(
    width: int,
    height: int,
    rgba: bytes,
    surface: tuple[int, int, int, int],
) -> dict:
    sx, sy, sw, sh = surface
    if len(rgba) != width * height * 4:
        raise ValueError("capture pixel payload has the wrong size")
    if sx + sw > width or sy + sh > height:
        raise ValueError("preview surface is outside the captured screen")

    background = (15, 20, 28, 255)
    label = (66, 153, 225, 255)
    background_samples = background_matches = label_matches = magenta_samples = 0
    for y in range(sy, sy + sh):
        for x in range(sx, sx + sw):
            pixel = _pixel(width, rgba, x, y)
            if pixel[0] > 180 and pixel[1] < 80 and pixel[2] > 180:
                magenta_samples += 1
            if _near(pixel, label, 18):
                label_matches += 1
            else:
                background_samples += 1
                background_matches += _near(pixel, background, 10)

    if magenta_samples:
        raise ValueError(f"reset surface retains poisoned magenta pixels: {magenta_samples}")
    if label_matches < 25:
        raise ValueError(f"initialized restore label is missing: {label_matches} blue pixels")
    if background_samples == 0 or background_matches / background_samples < 0.97:
        raise ValueError(
            f"reset surface is not initialized dark: "
            f"{background_matches}/{background_samples} background pixels"
        )
    return {
        "schema": "stasis.workshop_present_only_reset_capture.v1",
        "capture_size": [width, height],
        "preview_surface": list(surface),
        "initialized_background_rgba8": list(background),
        "restore_label_rgba8": list(label),
        "initialized_background_samples": [background_matches, background_samples],
        "restore_label_pixels": label_matches,
        "poisoned_magenta_pixels": magenta_samples,
    }


def verify_alpha_probe(
    width: int,
    height: int,
    rgba: bytes,
    surface: tuple[int, int, int, int],
    viewport: tuple[int, int, int, int],
) -> dict:
    evidence = verify_capture(width, height, rgba, surface, viewport)
    vx, vy, vw, vh = viewport
    probe_x = vx + round(100 * vw / 640)
    probe_y = vy + round(65 * vh / 360)
    expected = (115, 147, 10, 255)
    actual = _pixel(width, rgba, probe_x, probe_y)
    if not _near(actual, expected, 35):
        raise ValueError(
            "post-replay alpha probe did not blend over the accepted red frame: "
            f"actual={actual} expected={expected}"
        )
    evidence.update(
        {
            "schema": "stasis.workshop_present_only_alpha_capture.v1",
            "alpha_probe_logical": [100, 65],
            "alpha_probe_rgba": list(actual),
            "alpha_probe_expected_rgba": list(expected),
        }
    )
    return evidence


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--capture", required=True, type=Path)
    parser.add_argument("--surface", required=True, help="physical preview bounds x,y,width,height")
    parser.add_argument(
        "--mode",
        choices=("present", "present-alpha-probe", "reset-placeholder"),
        default="present",
    )
    parser.add_argument("--viewport", help="fitted logical viewport x,y,width,height")
    parser.add_argument("--evidence", type=Path)
    args = parser.parse_args()
    try:
        surface = parse_rect(args.surface, "--surface")
        width, height, rgba = read_capture(args.capture)
        if args.mode in ("present", "present-alpha-probe"):
            if not args.viewport:
                raise ValueError("--viewport is required for present modes")
            viewport = parse_rect(args.viewport, "--viewport")
            if args.mode == "present-alpha-probe":
                evidence = verify_alpha_probe(width, height, rgba, surface, viewport)
            else:
                evidence = verify_capture(width, height, rgba, surface, viewport)
        else:
            evidence = verify_reset_placeholder(width, height, rgba, surface)
        evidence["capture"] = str(args.capture.resolve())
        evidence["capture_sha256"] = hashlib.sha256(args.capture.read_bytes()).hexdigest()
        encoded = json.dumps(evidence, indent=2, sort_keys=True)
        if args.evidence:
            args.evidence.parent.mkdir(parents=True, exist_ok=True)
            args.evidence.write_text(encoded + "\n", encoding="utf-8")
        print(encoded)
        return 0
    except Exception as error:
        print(f"presentation baseline verification failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
