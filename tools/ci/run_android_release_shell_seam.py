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
    expected = {
        "state_checksum": expectations["state_checksum"],
        "command_trace": expectations["command_trace"],
        "rejected": 0,
        "validation": 0,
    }
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
        log_path.write_text(log, encoding="utf-8")
        stable = validate_markers(markers, expectations)
        first_pid = _run(
            args.adb, args.serial, "shell", "pidof", package_id, required=False
        ).strip()
        if not first_pid:
            raise SeamError("generated Android shell exited before capture")
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
