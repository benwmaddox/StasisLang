#!/usr/bin/env python3
"""Install and verify a generated Android AOT shell through structured markers."""

from __future__ import annotations

import argparse
import json
import re
import struct
import subprocess
import time
import xml.etree.ElementTree as ET
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
        if expected.get("on_boundary"):
            normalized = (marker.get("x_n", 0.5), marker.get("y_n", 0.5))
            boundary_distance = min(
                abs(value - boundary)
                for value in normalized
                for boundary in (0.0, 1.0)
            )
            if boundary_distance > 0.02:
                raise SeamError(
                    f"Android touch probe {sequence} did not clamp to a viewport "
                    f"boundary: x_n={normalized[0]} y_n={normalized[1]}"
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


def validate_orientation_markers(
    markers: list[dict], expectations: dict, surfaces: dict[str, tuple[int, int]]
) -> list[dict]:
    orientation = expectations["orientation"]
    probes = {
        item.get("probe_sequence"): item
        for item in markers
        if item.get("event") == "probe"
    }
    logical_width, logical_height = expectations["logical_size"]
    coordinate_tolerance = orientation["coordinate_tolerance"]
    surface_tolerance = orientation["surface_tolerance"]
    touch_x, touch_y = orientation["touch"]
    configured_width, configured_height = orientation["display_size"]
    observed = []
    for stage in orientation["stages"]:
        sequence = stage["sequence"]
        marker = probes.get(sequence)
        if marker is None:
            raise SeamError(
                f"Android orientation seam is missing probe sequence {sequence}"
            )
        if marker.get("probe_kind") != stage["kind"]:
            raise SeamError(
                f"Android orientation probe {sequence} kind mismatch: "
                f"expected={stage['kind']} actual={marker.get('probe_kind')}"
            )
        for field, expected, tolerance in (
            ("logical_w", logical_width, 0.01),
            ("logical_h", logical_height, 0.01),
            ("x", touch_x, coordinate_tolerance),
            ("y", touch_y, coordinate_tolerance),
            ("x_n", touch_x / logical_width, 0.05),
            ("y_n", touch_y / logical_height, 0.05),
        ):
            if abs(marker.get(field, 0.0) - expected) > tolerance:
                raise SeamError(
                    f"Android orientation probe {sequence} {field} mismatch: "
                    f"expected={expected} actual={marker.get(field)} "
                    f"tolerance={tolerance}"
                )
        if (
            marker.get("pointer_id") != 1
            or marker.get("pointer_count", 0) < 2
            or marker.get("went_up") != 1
        ):
            raise SeamError(
                f"Android orientation probe {sequence} pointer mismatch: "
                f"id={marker.get('pointer_id')} count={marker.get('pointer_count')} "
                f"went_up={marker.get('went_up')}"
            )
        surface = surfaces.get(stage["name"])
        if surface is None:
            raise SeamError(f"Android orientation stage {stage['name']} has no capture")
        surface_width, surface_height = surface
        expected_surface = (
            (configured_width, configured_height)
            if stage["orientation"] == "portrait"
            else (configured_height, configured_width)
        )
        if any(
            abs(actual - expected) > surface_tolerance
            for actual, expected in zip(surface, expected_surface)
        ):
            raise SeamError(
                f"Android orientation stage {stage['name']} configured size mismatch: "
                f"expected={expected_surface[0]}x{expected_surface[1]} "
                f"actual={surface_width}x{surface_height} "
                f"tolerance={surface_tolerance}"
            )
        if surface_width % 2 == 0 or surface_height % 2 == 0:
            raise SeamError(
                f"Android orientation stage {stage['name']} is not odd-sized: "
                f"{surface_width}x{surface_height}"
            )
        is_portrait = surface_height > surface_width
        if is_portrait != (stage["orientation"] == "portrait"):
            raise SeamError(
                f"Android orientation stage {stage['name']} surface mismatch: "
                f"{surface_width}x{surface_height}"
            )
        native_scale = min(
            surface_width / logical_width, surface_height / logical_height
        )
        expected_drawable = (
            int(logical_width * native_scale),
            int(logical_height * native_scale),
        )
        expected_sizes = {
            "native": (surface_width, surface_height),
            "drawable": expected_drawable,
        }
        for prefix, (expected_width, expected_height) in expected_sizes.items():
            actual_width = marker.get(f"{prefix}_w", 0)
            actual_height = marker.get(f"{prefix}_h", 0)
            if (
                abs(actual_width - expected_width) > surface_tolerance
                or abs(actual_height - expected_height) > surface_tolerance
            ):
                raise SeamError(
                    f"Android orientation probe {sequence} {prefix} size mismatch: "
                    f"expected={expected_width}x{expected_height} "
                    f"actual={actual_width}x{actual_height} "
                    f"tolerance={surface_tolerance}"
                )
        expected_scale = min(
            expected_drawable[0] / logical_width,
            expected_drawable[1] / logical_height,
        )
        if abs(marker.get("content_scale", 0.0) - expected_scale) > 0.02:
            raise SeamError(
                f"Android orientation probe {sequence} content_scale mismatch: "
                f"expected={expected_scale:.4f} actual={marker.get('content_scale')}"
            )
        expected_raster = max(1.0, min(8.0, expected_scale))
        if abs(marker.get("raster_scale", 0.0) - expected_raster) > 0.02:
            raise SeamError(
                f"Android orientation probe {sequence} raster_scale mismatch: "
                f"expected={expected_raster:.4f} actual={marker.get('raster_scale')}"
            )
        for field, expected in zip(
            ("safe_x", "safe_y", "safe_w", "safe_h"),
            orientation["safe_viewport"],
        ):
            if abs(marker.get(field, 0.0) - expected) > coordinate_tolerance:
                raise SeamError(
                    f"Android orientation probe {sequence} {field} mismatch: "
                    f"expected={expected} actual={marker.get(field)}"
                )
        expected_checksum = (
            4000
            + sequence * 100
            + stage["kind"] * 10
            + marker.get("display_generation", 0)
        )
        if marker.get("state_checksum") != expected_checksum:
            raise SeamError(
                f"Android orientation probe {sequence} state_checksum mismatch: "
                f"expected={expected_checksum} actual={marker.get('state_checksum')}"
            )
        for field in ("display_generation", "density_generation"):
            frame_field = f"frame_{field}"
            if marker.get(frame_field) != marker.get(field):
                raise SeamError(
                    f"Android orientation probe {sequence} {frame_field} mismatch: "
                    f"guest={marker.get(field)} frame={marker.get(frame_field)}"
                )
        if not marker.get("command_trace"):
            raise SeamError(
                f"Android orientation probe {sequence} has an empty command trace"
            )
        observed.append(marker)
    ticks = [item.get("probe_tick") for item in observed]
    display_generations = [item.get("display_generation") for item in observed]
    density_generations = [item.get("density_generation") for item in observed]
    if any(not isinstance(value, int) for value in ticks) or any(
        current >= following for current, following in zip(ticks, ticks[1:])
    ):
        raise SeamError(f"Android orientation probe ticks are not ordered: {ticks}")
    if any(not isinstance(value, int) or value <= 0 for value in display_generations) or any(
        current >= following
        for current, following in zip(display_generations, display_generations[1:])
    ):
        raise SeamError(
            "Android orientation display generations are not strictly ordered: "
            f"{display_generations}"
        )
    if any(not isinstance(value, int) or value <= 0 for value in density_generations) or any(
        current >= following
        for current, following in zip(density_generations, density_generations[1:])
    ):
        raise SeamError(
            "Android orientation density generations are not strictly ordered: "
            f"{density_generations}"
        )
    if len({item["command_trace"] for item in observed}) != len(observed):
        raise SeamError("Android orientation stages did not produce distinct frame traces")
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


def outside_letterbox_point(
    logical_size: list[int], native_size: tuple[int, int]
) -> tuple[int, int]:
    logical_width, logical_height = logical_size
    native_width, native_height = native_size
    scale = min(native_width / logical_width, native_height / logical_height)
    offset_x = (native_width - logical_width * scale) / 2.0
    offset_y = (native_height - logical_height * scale) / 2.0
    if offset_x >= 2.0:
        return round(offset_x / 2.0), native_height // 2
    if offset_y >= 2.0:
        return native_width // 2, round(offset_y / 2.0)
    raise SeamError(
        "Android touch fixture requires a real letterbox bar on the captured surface"
    )


def release_shell_target(package: dict) -> str:
    target = package.get("target")
    if target not in ("android-arm64", "android-x86_64"):
        raise SeamError(f"invalid Android package target: {target!r}")
    return f"{target}-release-shell"


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
        if region.get("location") == "outside_letterbox":
            x, y = outside_letterbox_point(
                expectations["logical_size"], (width, height)
            )
        else:
            x = max(
                0, min(width - 1, round(offset_x + region["center"][0] * scale))
            )
            y = max(
                0, min(height - 1, round(offset_y + region["center"][1] * scale))
            )
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


def dismiss_system_dialog_action(adb: Path, serial: str | None) -> bool:
    hierarchy = _run(
        adb,
        serial,
        "shell",
        "uiautomator",
        "dump",
        "/dev/tty",
        required=False,
    )
    start = hierarchy.find("<?xml")
    end = hierarchy.rfind("</hierarchy>")
    if start < 0 or end < start:
        return False
    try:
        root = ET.fromstring(hierarchy[start : end + len("</hierarchy>")])
    except ET.ParseError:
        return False
    system_anr_titles = {
        "Pixel Launcher isn't responding",
        "System UI isn't responding",
    }
    alert_titles = {
        node.attrib.get("text", "")
        for node in root.iter("node")
        if node.attrib.get("resource-id", "") == "android:id/alertTitle"
    }
    if system_anr_titles.isdisjoint(alert_titles):
        return False
    actions = {
        node.attrib.get("text", ""): node.attrib.get("bounds", "")
        for node in root.iter("node")
    }
    # Keep the system component alive when Android offers the non-destructive
    # action. Closing Pixel Launcher can leave the emulator compositor black.
    for label in ("Wait", "Close app"):
        bounds = actions.get(label, "")
        match = re.fullmatch(r"\[(\d+),(\d+)\]\[(\d+),(\d+)\]", bounds)
        if match is None:
            continue
        left, top, right, bottom = (int(value) for value in match.groups())
        _run(
            adb,
            serial,
            "shell",
            "input",
            "tap",
            str((left + right) // 2),
            str((top + bottom) // 2),
            required=False,
        )
        return True
    return False


def ensure_test_activity_foreground(
    adb: Path,
    serial: str | None,
    package_id: str,
    component: str,
) -> bool:
    windows = _run(
        adb,
        serial,
        "shell",
        "dumpsys",
        "window",
        "windows",
        required=False,
    )
    current_focus = next(
        (line for line in windows.splitlines() if "mCurrentFocus=" in line),
        "",
    )
    system_dialog_present = "AppNotRespondingDialog" in windows
    if package_id in current_focus and not system_dialog_present:
        return False
    dismissed = (
        dismiss_system_dialog_action(adb, serial)
        if current_focus or system_dialog_present
        else False
    )
    if (current_focus or system_dialog_present) and not dismissed:
        _run(
            adb,
            serial,
            "shell",
            "input",
            "keyevent",
            "KEYCODE_BACK",
            required=False,
        )
    _run(
        adb,
        serial,
        "shell",
        "am",
        "start",
        "-W",
        "-n",
        component,
        required=False,
    )
    return True


def capture_until_regions_match(
    adb: Path,
    serial: str | None,
    capture: Path,
    expectations: dict,
    deadline: float,
    package_id: str,
    component: str,
) -> list[dict]:
    """Wait for the stable marker's frame to reach Android's compositor."""
    last_error: SeamError | None = None
    while time.monotonic() < deadline:
        if ensure_test_activity_foreground(adb, serial, package_id, component):
            time.sleep(0.25)
        capture.write_bytes(
            _run(adb, serial, "exec-out", "screencap", "-p", text=False)
        )
        try:
            return validate_regions(capture, expectations)
        except SeamError as error:
            last_error = error
            # Android 15 may layer a launcher ANR above the focused test
            # activity without reporting AppNotRespondingDialog through
            # dumpsys window. Only inspect the UI hierarchy after a capture
            # mismatch, then dismiss a verified launcher/System UI ANR and
            # restore the test activity before trying the framebuffer again.
            if dismiss_system_dialog_action(adb, serial):
                _run(
                    adb,
                    serial,
                    "shell",
                    "am",
                    "start",
                    "-W",
                    "-n",
                    component,
                    required=False,
                )
            time.sleep(0.25)
    if last_error is not None:
        raise last_error
    raise SeamError("capture region deadline expired before the first frame")


def parse_wm_size_override(output: str) -> str | None:
    match = re.search(r"^Override size:\s*(\d+x\d+)\s*$", output, re.MULTILINE)
    return match.group(1) if match else None


def wait_for_surface(
    adb: Path,
    serial: str | None,
    orientation: str,
    expected_size: tuple[int, int],
    tolerance: int,
    deadline: float,
    capture: Path,
) -> tuple[int, int]:
    last_size = None
    while time.monotonic() < deadline:
        capture.write_bytes(
            _run(adb, serial, "exec-out", "screencap", "-p", text=False)
        )
        width, height, _ = read_png_rgb(capture)
        last_size = (width, height)
        matches = (height > width) == (orientation == "portrait")
        expected = all(
            abs(actual - target) <= tolerance
            for actual, target in zip(last_size, expected_size)
        )
        if matches and expected and width % 2 == 1 and height % 2 == 1:
            return last_size
        time.sleep(0.25)
    raise SeamError(
        f"Android surface did not reach odd {orientation} dimensions "
        f"{expected_size[0]}x{expected_size[1]}: {last_size}"
    )


def restore_device_state(
    adb: Path,
    serial: str | None,
    package_id: str,
    installed: bool,
    immersive_confirmation: str,
    device_state: dict | None = None,
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
    if device_state is not None:
        size = device_state.get("wm_size_override")
        operations.append(
            (
                "restore display size",
                ("shell", "wm", "size", size if size else "reset"),
            )
        )
        for namespace, key in (
            ("system", "user_rotation"),
            ("system", "accelerometer_rotation"),
        ):
            value = device_state.get(key, "null")
            action = "delete" if not value or value == "null" else "put"
            arguments = ("shell", "settings", action, namespace, key)
            if action == "put":
                arguments += (value,)
            operations.append((f"restore {key}", arguments))
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
    orientation = expectations.get("orientation")
    device_state = None
    if orientation is not None:
        device_state = {
            "wm_size_override": parse_wm_size_override(
                _run(args.adb, args.serial, "shell", "wm", "size")
            ),
            "accelerometer_rotation": _run(
                args.adb,
                args.serial,
                "shell",
                "settings",
                "get",
                "system",
                "accelerometer_rotation",
                required=False,
            ).strip(),
            "user_rotation": _run(
                args.adb,
                args.serial,
                "shell",
                "settings",
                "get",
                "system",
                "user_rotation",
                required=False,
            ).strip(),
        }
    evidence = {
        "schema": SCHEMA,
        "test_id": test_id,
        "status": "failed",
        "target": release_shell_target(package),
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
    if device_state is not None:
        evidence["original_device_state"] = device_state
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
        if orientation is not None:
            first_stage = orientation["stages"][0]
            display_width, display_height = orientation["display_size"]
            _run(
                args.adb,
                args.serial,
                "shell",
                "settings",
                "put",
                "system",
                "accelerometer_rotation",
                "0",
            )
            _run(
                args.adb,
                args.serial,
                "shell",
                "wm",
                "size",
                f"{display_width}x{display_height}",
            )
            _run(
                args.adb,
                args.serial,
                "shell",
                "settings",
                "put",
                "system",
                "user_rotation",
                str(first_stage["rotation"]),
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
        orientation_probes = []
        orientation_evidence = []
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
                if gesture.get("location") == "outside_letterbox":
                    start_x, start_y = outside_letterbox_point(
                        expectations["logical_size"],
                        (native_width, native_height),
                    )
                    end_x, end_y = start_x, start_y
                else:
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
        if orientation is not None:
            surfaces = {}
            for index, stage in enumerate(orientation["stages"]):
                if index > 0:
                    _run(
                        args.adb,
                        args.serial,
                        "shell",
                        "settings",
                        "put",
                        "system",
                        "user_rotation",
                        str(stage["rotation"]),
                    )
                stage_capture_path = args.output / f"{stage['name']}-frame.png"
                configured_width, configured_height = orientation["display_size"]
                expected_surface = (
                    (configured_width, configured_height)
                    if stage["orientation"] == "portrait"
                    else (configured_height, configured_width)
                )
                surface = wait_for_surface(
                    args.adb,
                    args.serial,
                    stage["orientation"],
                    expected_surface,
                    orientation["surface_tolerance"],
                    min(deadline, time.monotonic() + 15),
                    stage_capture_path,
                )
                surfaces[stage["name"]] = surface
                touch_x, touch_y = logical_to_native(
                    orientation["touch"], expectations["logical_size"], surface
                )
                _run(
                    args.adb,
                    args.serial,
                    "shell",
                    "input",
                    "touchscreen",
                    "swipe",
                    str(touch_x),
                    str(touch_y),
                    str(touch_x),
                    str(touch_y),
                    "250",
                )
                probe_deadline = min(deadline, time.monotonic() + 10)
                while time.monotonic() < probe_deadline:
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
                        item.get("probe_sequence") == stage["sequence"]
                        for item in markers
                    ):
                        break
                    time.sleep(0.25)
                else:
                    raise SeamError(
                        f"Android orientation stage {stage['name']} did not reach "
                        f"probe sequence {stage['sequence']}"
                    )
                stage_expectations = {
                    "logical_size": expectations["logical_size"],
                    "regions": stage["regions"],
                }
                stage_regions = capture_until_regions_match(
                    args.adb,
                    args.serial,
                    stage_capture_path,
                    stage_expectations,
                    min(deadline, time.monotonic() + 10),
                    package_id,
                    component,
                )
                orientation_evidence.append(
                    {
                        "name": stage["name"],
                        "rotation": stage["rotation"],
                        "surface": list(surface),
                        "touch": [touch_x, touch_y],
                        "capture": str(stage_capture_path),
                        "regions": stage_regions,
                    }
                )
            orientation_probes = validate_orientation_markers(
                markers, expectations, surfaces
            )
        log_path.write_text(log, encoding="utf-8")
        regions = capture_until_regions_match(
            args.adb,
            args.serial,
            capture_path,
            expectations,
            min(deadline, time.monotonic() + 10),
            package_id,
            component,
        )
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
        if orientation_probes:
            evidence["orientation_probes"] = orientation_probes
            evidence["orientation_stages"] = orientation_evidence
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
                device_state,
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
