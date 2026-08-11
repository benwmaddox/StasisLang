#!/usr/bin/env python3
"""Install and verify a generated Android AOT shell through structured markers."""

from __future__ import annotations

import argparse
import json
import re
import struct
import subprocess
import time
import zlib
from pathlib import Path


SCHEMA = "stasis.seam_test.v1"
MARKER = re.compile(r"Stasis seam: (\{[^\r\n]+\})")


class SeamError(RuntimeError):
    pass


def _run(
    adb: Path,
    serial: str | None,
    *arguments: str,
    text: bool = True,
    required: bool = True,
):
    command = [str(adb)]
    if serial:
        command.extend(("-s", serial))
    command.extend(arguments)
    result = subprocess.run(command, capture_output=True, text=text, check=False)
    if required and result.returncode != 0:
        stderr = (
            result.stderr.strip()
            if text
            else result.stderr.decode(errors="replace").strip()
        )
        raise SeamError(f"adb {' '.join(arguments)} failed: {stderr}")
    return result.stdout


def parse_markers(log: str, test_id: str) -> list[dict]:
    markers = []
    for match in MARKER.finditer(log):
        try:
            value = json.loads(match.group(1))
        except json.JSONDecodeError:
            continue
        if value.get("schema") == SCHEMA and value.get("test_id") == test_id:
            markers.append(value)
    return markers


def validate_markers(markers: list[dict], expectations: dict) -> dict:
    stable_frame = expectations["stable_frame"]
    events = {(item.get("event"), item.get("frame")): item for item in markers}
    if ("initialized", 0) not in events or ("frame", 1) not in events:
        raise SeamError("Android shell did not emit initialized and first-frame markers")
    stable = events.get(("stable", stable_frame))
    if stable is None:
        raise SeamError(f"Android shell did not reach stable frame {stable_frame}")
    expected = {"rejected": 0, "validation": 0}
    for key in ("state_checksum", "command_trace"):
        if key in expectations:
            expected[key] = expectations[key]
    mismatches = {
        key: {"expected": value, "actual": stable.get(key)}
        for key, value in expected.items()
        if stable.get(key) != value
    }
    if (
        stable.get("accepted", 0) < stable_frame
        or stable.get("presented", 0) < stable_frame
    ):
        mismatches["stable_frames"] = {
            "expected": stable_frame,
            "accepted": stable.get("accepted"),
            "presented": stable.get("presented"),
        }
    if mismatches:
        raise SeamError(f"Android stable-frame marker mismatch: {mismatches}")
    return stable


def validate_touch_markers(markers: list[dict], expectations: dict) -> list[dict]:
    touch = expectations["touch"]
    probes = {
        item.get("probe_sequence"): item
        for item in markers
        if item.get("event") == "probe"
    }
    observed = []
    ticks = []
    exact_fields = (
        "probe_kind",
        "down_count",
        "move_count",
        "up_count",
        "state_transitions",
        "input_phase",
        "is_down",
        "went_down",
        "went_up",
        "state_checksum",
    )
    tolerance = touch["coordinate_tolerance"]
    for expected in touch["probes"]:
        sequence = expected["sequence"]
        marker = probes.get(sequence)
        if marker is None:
            raise SeamError(f"Android touch seam is missing probe sequence {sequence}")
        for field in exact_fields:
            expected_key = "kind" if field == "probe_kind" else field
            if expected_key in expected and marker.get(field) != expected[expected_key]:
                raise SeamError(
                    f"Android touch probe {sequence} {field} mismatch: "
                    f"expected={expected[expected_key]} actual={marker.get(field)}"
                )
        if marker.get("pointer_id") != 1 or marker.get("pointer_count", 0) < 2:
            raise SeamError(
                f"Android touch probe {sequence} pointer identity mismatch: "
                f"id={marker.get('pointer_id')} count={marker.get('pointer_count')}"
            )
        for field in ("x", "y"):
            if field in expected and abs(marker.get(field, 0.0) - expected[field]) > tolerance:
                raise SeamError(
                    f"Android touch probe {sequence} {field} mismatch: "
                    f"expected={expected[field]} actual={marker.get(field)} "
                    f"tolerance={tolerance}"
                )
            minimum = expected.get(f"{field}_min")
            if minimum is not None and marker.get(field, 0.0) < minimum:
                raise SeamError(
                    f"Android touch probe {sequence} {field} below minimum: "
                    f"expected>={minimum} actual={marker.get(field)}"
                )
        for field in ("x_n", "y_n"):
            if field in expected and abs(marker.get(field, 0.0) - expected[field]) > 0.05:
                raise SeamError(
                    f"Android touch probe {sequence} {field} mismatch: "
                    f"expected={expected[field]} actual={marker.get(field)}"
                )
        ticks.append(marker.get("probe_tick"))
        observed.append(marker)
    safe_viewport = touch.get("safe_viewport")
    if safe_viewport:
        for marker in observed:
            for field, expected in zip(
                ("safe_x", "safe_y", "safe_w", "safe_h"), safe_viewport
            ):
                if abs(marker.get(field, 0.0) - expected) > tolerance:
                    raise SeamError(
                        f"Android touch probe {marker['probe_sequence']} {field} mismatch: "
                        f"expected={expected} actual={marker.get(field)} tolerance={tolerance}"
                    )
    if any(not isinstance(tick, int) for tick in ticks) or any(
        current >= following for current, following in zip(ticks, ticks[1:])
    ):
        raise SeamError(f"Android touch probe ticks are not strictly ordered: {ticks}")
    final = observed[-1]
    if (
        "final_command_trace" in touch
        and final.get("command_trace") != touch["final_command_trace"]
    ):
        raise SeamError(
            "Android touch final command trace mismatch: "
            f"expected={touch['final_command_trace']} actual={final.get('command_trace')}"
        )
    return observed


def logical_to_native(
    logical: list[float], logical_size: list[int], native_size: tuple[int, int]
) -> tuple[int, int]:
    logical_width, logical_height = logical_size
    native_width, native_height = native_size
    scale = min(native_width / logical_width, native_height / logical_height)
    offset_x = (native_width - logical_width * scale) / 2.0
    offset_y = (native_height - logical_height * scale) / 2.0
    return (
        max(0, min(native_width - 1, round(offset_x + logical[0] * scale))),
        max(0, min(native_height - 1, round(offset_y + logical[1] * scale))),
    )


def read_png_rgb(path: Path) -> tuple[int, int, list[tuple[int, int, int]]]:
    data = path.read_bytes()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise SeamError(f"capture is not a PNG: {path}")
    offset = 8
    width = height = color_type = bit_depth = None
    compressed = bytearray()
    while offset + 12 <= len(data):
        length = struct.unpack(">I", data[offset : offset + 4])[0]
        kind = data[offset + 4 : offset + 8]
        payload = data[offset + 8 : offset + 8 + length]
        offset += length + 12
        if kind == b"IHDR":
            width, height, bit_depth, color_type, _, _, interlace = struct.unpack(
                ">IIBBBBB", payload
            )
            if bit_depth != 8 or color_type not in (2, 6) or interlace != 0:
                raise SeamError("capture must be a non-interlaced 8-bit RGB/RGBA PNG")
        elif kind == b"IDAT":
            compressed.extend(payload)
        elif kind == b"IEND":
            break
    if width is None or height is None or color_type is None:
        raise SeamError("capture PNG is missing IHDR")
    channels = 3 if color_type == 2 else 4
    raw = zlib.decompress(bytes(compressed))
    stride = width * channels
    rows: list[bytearray] = []
    cursor = 0
    for _ in range(height):
        filter_kind = raw[cursor]
        cursor += 1
        row = bytearray(raw[cursor : cursor + stride])
        cursor += stride
        prior = rows[-1] if rows else bytearray(stride)
        for index in range(stride):
            left = row[index - channels] if index >= channels else 0
            above = prior[index]
            upper_left = prior[index - channels] if index >= channels else 0
            if filter_kind == 1:
                row[index] = (row[index] + left) & 0xFF
            elif filter_kind == 2:
                row[index] = (row[index] + above) & 0xFF
            elif filter_kind == 3:
                row[index] = (row[index] + ((left + above) // 2)) & 0xFF
            elif filter_kind == 4:
                estimate = left + above - upper_left
                distances = (
                    abs(estimate - left),
                    abs(estimate - above),
                    abs(estimate - upper_left),
                )
                predictor = (left, above, upper_left)[distances.index(min(distances))]
                row[index] = (row[index] + predictor) & 0xFF
            elif filter_kind != 0:
                raise SeamError(f"capture PNG uses unsupported filter {filter_kind}")
        rows.append(row)
    pixels = []
    for row in rows:
        pixels.extend(
            tuple(row[index : index + 3]) for index in range(0, stride, channels)
        )
    return width, height, pixels


def validate_regions(capture: Path, expectations: dict) -> list[dict]:
    width, height, pixels = read_png_rgb(capture)
    logical_width, logical_height = expectations["logical_size"]
    scale = min(width / logical_width, height / logical_height)
    offset_x = (width - logical_width * scale) / 2.0
    offset_y = (height - logical_height * scale) / 2.0
    observed = []
    for region in expectations["regions"]:
        x = max(0, min(width - 1, round(offset_x + region["center"][0] * scale)))
        y = max(0, min(height - 1, round(offset_y + region["center"][1] * scale)))
        radius = max(2, round(scale * 3))
        samples = [
            pixels[row * width + column]
            for row in range(max(0, y - radius), min(height, y + radius + 1))
            for column in range(max(0, x - radius), min(width, x + radius + 1))
        ]
        average = [
            round(sum(pixel[channel] for pixel in samples) / len(samples))
            for channel in range(3)
        ]
        if any(
            abs(average[index] - region["rgb"][index]) > region["tolerance"]
            for index in range(3)
        ):
            raise SeamError(
                f"capture region {region['name']} color mismatch: "
                f"expected={region['rgb']} observed={average} tolerance={region['tolerance']}"
            )
        observed.append({"name": region["name"], "pixel": [x, y], "rgb": average})
    return observed


def restore_device_state(
    adb: Path,
    serial: str | None,
    package_id: str,
    installed: bool,
    immersive_confirmation: str,
) -> list[str]:
    errors = []
    operations = []
    if installed:
        operations.extend(
            (
                ("force-stop", ("shell", "am", "force-stop", package_id)),
                ("uninstall", ("uninstall", package_id)),
            )
        )
    if immersive_confirmation and immersive_confirmation != "null":
        operations.append(
            (
                "restore immersive confirmation",
                (
                    "shell",
                    "settings",
                    "put",
                    "secure",
                    "immersive_mode_confirmations",
                    immersive_confirmation,
                ),
            )
        )
    else:
        operations.append(
            (
                "restore immersive confirmation",
                (
                    "shell",
                    "settings",
                    "delete",
                    "secure",
                    "immersive_mode_confirmations",
                ),
            )
        )
    for name, arguments in operations:
        try:
            _run(adb, serial, *arguments)
        except (OSError, SeamError) as error:
            errors.append(f"{name}: {error}")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--adb", type=Path, required=True)
    parser.add_argument("--serial")
    parser.add_argument("--apk", type=Path, required=True)
    parser.add_argument("--package-manifest", type=Path, required=True)
    parser.add_argument("--expectations", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--timeout-seconds", type=int, default=90)
    args = parser.parse_args()

    started = time.monotonic()
    package = json.loads(args.package_manifest.read_text(encoding="utf-8"))
    provenance_path = args.package_manifest.parent / package["provenance"]
    provenance = json.loads(provenance_path.read_text(encoding="utf-8"))
    expectations = json.loads(args.expectations.read_text(encoding="utf-8"))
    package_id = package["package_id"]
    test_id = expectations["test_id"]
    args.output.mkdir(parents=True, exist_ok=True)
    log_path = args.output / "logcat.txt"
    initial_capture_path = args.output / "initial-frame.png"
    capture_path = args.output / "stable-frame.png"
    evidence_path = args.output / "evidence.json"
    preinstalled = bool(
        _run(
            args.adb,
            args.serial,
            "shell",
            "pm",
            "path",
            package_id,
            required=False,
        ).strip()
    )
    if preinstalled:
        raise SeamError(
            f"refusing to replace preinstalled test package {package_id}; remove it explicitly"
        )
    immersive_confirmation = _run(
        args.adb,
        args.serial,
        "shell",
        "settings",
        "get",
        "secure",
        "immersive_mode_confirmations",
        required=False,
    ).strip()
    evidence = {
        "schema": SCHEMA,
        "test_id": test_id,
        "status": "failed",
        "target": "android-arm64-release-shell",
        "package_id": package_id,
        "package": package,
        "build_identity": {
            key: provenance.get(key)
            for key in (
                "schema",
                "development_build",
                "release_tag",
                "source_commit",
                "dirty_state",
                "command_buffer",
            )
        },
        "artifacts": {"log": str(log_path), "capture": str(capture_path)},
    }
    installed = False
    try:
        _run(
            args.adb,
            args.serial,
            "shell",
            "settings",
            "put",
            "secure",
            "immersive_mode_confirmations",
            "confirmed",
        )
        _run(args.adb, args.serial, "install", "-r", str(args.apk))
        installed = True
        _run(args.adb, args.serial, "logcat", "-c")
        component = f"{package_id}/.MainActivity"
        _run(
            args.adb,
            args.serial,
            "shell",
            "am",
            "start",
            "-W",
            "-n",
            component,
            "--es",
            "stasis.seam_test_id",
            test_id,
        )
        deadline = time.monotonic() + args.timeout_seconds
        log = ""
        markers: list[dict] = []
        while time.monotonic() < deadline:
            log = _run(
                args.adb,
                args.serial,
                "logcat",
                "-d",
                "-v",
                "brief",
                "Stasis:I",
                "*:S",
            )
            markers = parse_markers(log, test_id)
            if any(item.get("event") == "stable" for item in markers):
                break
            time.sleep(1)
        stable = validate_markers(markers, expectations)
        first_pid = _run(
            args.adb, args.serial, "shell", "pidof", package_id, required=False
        ).strip()
        if not first_pid:
            raise SeamError("generated Android shell exited before capture")
        touch_probes = []
        if "touch" in expectations:
            initial_capture_path.write_bytes(
                _run(args.adb, args.serial, "exec-out", "screencap", "-p", text=False)
            )
            native_width, native_height, _ = read_png_rgb(initial_capture_path)
            evidence["artifacts"]["initial_capture"] = str(initial_capture_path)
            evidence["input_surface"] = {
                "width": native_width,
                "height": native_height,
            }
            injected_gestures = []
            for gesture in expectations["touch"]["gestures"]:
                start_x, start_y = logical_to_native(
                    gesture["start"],
                    expectations["logical_size"],
                    (native_width, native_height),
                )
                end_x, end_y = logical_to_native(
                    gesture["end"],
                    expectations["logical_size"],
                    (native_width, native_height),
                )
                _run(
                    args.adb,
                    args.serial,
                    "shell",
                    "input",
                    "touchscreen",
                    "swipe",
                    str(start_x),
                    str(start_y),
                    str(end_x),
                    str(end_y),
                    str(gesture["duration_ms"]),
                )
                injected_gestures.append(
                    {
                        "name": gesture["name"],
                        "start": [start_x, start_y],
                        "end": [end_x, end_y],
                        "duration_ms": gesture["duration_ms"],
                    }
                )
                gesture_deadline = min(deadline, time.monotonic() + 10)
                while time.monotonic() < gesture_deadline:
                    log = _run(
                        args.adb,
                        args.serial,
                        "logcat",
                        "-d",
                        "-v",
                        "brief",
                        "Stasis:I",
                        "*:S",
                    )
                    markers = parse_markers(log, test_id)
                    if any(
                        item.get("probe_sequence") == gesture["after_sequence"]
                        for item in markers
                    ):
                        break
                    time.sleep(0.25)
                else:
                    raise SeamError(
                        f"Android gesture {gesture['name']} did not reach probe "
                        f"sequence {gesture['after_sequence']}"
                    )
            touch_probes = validate_touch_markers(markers, expectations)
            evidence["injected_gestures"] = injected_gestures
        log_path.write_text(log, encoding="utf-8")
        capture_path.write_bytes(
            _run(args.adb, args.serial, "exec-out", "screencap", "-p", text=False)
        )
        regions = validate_regions(capture_path, expectations)
        time.sleep(1)
        second_pid = _run(
            args.adb, args.serial, "shell", "pidof", package_id, required=False
        ).strip()
        if second_pid != first_pid:
            raise SeamError("generated Android shell did not remain alive after stable frames")
        evidence.update(
            {
                "status": "passed",
                "stable_marker": stable,
                "lifecycle_events": [item["event"] for item in markers],
                "process_id": first_pid,
                "regions": regions,
            }
        )
        if touch_probes:
            evidence["touch_probes"] = touch_probes
    except (OSError, KeyError, ValueError, SeamError, zlib.error) as error:
        evidence["failure"] = str(error)
        raise
    finally:
        cleanup_errors = []
        if installed and not log_path.is_file():
            try:
                diagnostic_log = _run(
                    args.adb,
                    args.serial,
                    "logcat",
                    "-d",
                    "-v",
                    "brief",
                    "Stasis:I",
                    "*:S",
                    required=False,
                )
                log_path.write_text(diagnostic_log, encoding="utf-8")
            except (OSError, SeamError) as cleanup_error:
                cleanup_errors.append(f"diagnostic log: {cleanup_error}")
        cleanup_errors.extend(
            restore_device_state(
                args.adb,
                args.serial,
                package_id,
                installed,
                immersive_confirmation,
            )
        )
        evidence["device_state_restored"] = not cleanup_errors
        evidence["elapsed_ms"] = round((time.monotonic() - started) * 1000)
        evidence["timeout_seconds"] = args.timeout_seconds
        if cleanup_errors:
            evidence["status"] = "failed"
            evidence["cleanup_errors"] = cleanup_errors
        evidence_path.write_text(
            json.dumps(evidence, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        if cleanup_errors:
            raise SeamError(f"Android seam cleanup failed: {cleanup_errors}")
    print(json.dumps(evidence, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
