#!/usr/bin/env python3
"""Install and verify a generated Android AOT shell through structured markers."""

from __future__ import annotations

import argparse
import hashlib
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
ASSET_DIAGNOSTIC = re.compile(r"code=([^ ]+) path=(.*?) detail=(.+)")


class SeamError(RuntimeError):
    pass


def _run_result(
    adb: Path,
    serial: str | None,
    *arguments: str,
):
    command = [str(adb)]
    if serial:
        command.extend(("-s", serial))
    command.extend(arguments)
    return subprocess.run(command, capture_output=True, text=True, check=False)


def _run(
    adb: Path,
    serial: str | None,
    *arguments: str,
    text: bool = True,
    required: bool = True,
    timeout: float | None = None,
):
    try:
        if not text:
            command = [str(adb)]
            if serial:
                command.extend(("-s", serial))
            command.extend(arguments)
            result = subprocess.run(
                command,
                capture_output=True,
                text=False,
                check=False,
                timeout=timeout,
            )
        elif timeout is None:
            result = _run_result(adb, serial, *arguments)
        else:
            command = [str(adb)]
            if serial:
                command.extend(("-s", serial))
            command.extend(arguments)
            result = subprocess.run(
                command,
                capture_output=True,
                text=True,
                check=False,
                timeout=timeout,
            )
    except subprocess.TimeoutExpired as error:
        if required:
            raise SeamError(
                f"adb {' '.join(arguments)} timed out after {timeout} seconds"
            ) from error
        return "" if text else b""
    if required and result.returncode != 0:
        stderr = (
            result.stderr.strip()
            if text
            else result.stderr.decode(errors="replace").strip()
        )
        raise SeamError(f"adb {' '.join(arguments)} failed: {stderr}")
    return result.stdout


def classify_rejection_storage_probe(
    relative_path: str,
    returncode: int,
    stdout: str,
    stderr: str,
) -> str:
    """Classify one direct run-as path probe without hiding adb diagnostics."""
    output = stdout.strip()
    diagnostic = stderr.strip()
    if returncode == 0:
        if output != relative_path or diagnostic:
            raise SeamError(
                "IT-022 storage probe for "
                f"{relative_path} returned unexpected success output: "
                f"stdout={output!r} stderr={diagnostic!r}"
            )
        return "present"
    diagnostic_lines = [line.strip().lower() for line in diagnostic.splitlines() if line.strip()]
    if diagnostic_lines and all(
        "no such file or directory" in line for line in diagnostic_lines
    ):
        if output:
            raise SeamError(
                "IT-022 storage probe for "
                f"{relative_path} returned unexpected missing-path output: "
                f"stdout={output!r} stderr={diagnostic!r}"
            )
        return "absent"
    raise SeamError(
        f"IT-022 storage probe for {relative_path} failed: "
        f"exit={returncode} stdout={output!r} stderr={diagnostic!r}"
    )


def probe_rejection_storage_path(
    adb: Path,
    serial: str | None,
    package_id: str,
    relative_path: str,
) -> str:
    """Probe an app-private path through run-as while preserving result details."""
    result = _run_result(
        adb,
        serial,
        "shell",
        "run-as",
        package_id,
        "ls",
        "-d",
        relative_path,
    )
    return classify_rejection_storage_probe(
        relative_path,
        result.returncode,
        result.stdout,
        result.stderr,
    )


def _clip_probe_text(value: str, limit: int = 320) -> str:
    value = value.strip()
    return value if len(value) <= limit else value[: limit - 3] + "..."


def validate_runtime_error_overlay(
    ui_xml: str, required_text: tuple[str, ...], test_id: str
) -> dict[str, bool | list[int]]:
    """Require all expected runtime-error content on one accessibility node."""
    start = ui_xml.find("<?xml")
    if start < 0:
        start = ui_xml.find("<hierarchy")
    end = ui_xml.rfind("</hierarchy>")
    if start < 0 or end < start:
        raise SeamError(f"{test_id} UI hierarchy XML is missing or incomplete")
    xml = ui_xml[start : end + len("</hierarchy>")]
    try:
        root = ET.fromstring(xml)
    except ET.ParseError as error:
        raise SeamError(f"{test_id} UI hierarchy XML is malformed: {error}") from error
    if root.tag != "hierarchy":
        raise SeamError(f"{test_id} UI hierarchy XML has no hierarchy root")
    overlay_nodes = [
        node
        for node in root.iter("node")
        if node.attrib.get("content-desc", "").startswith("Stasis runtime error")
    ]
    if not overlay_nodes:
        raise SeamError(f"{test_id} UI hierarchy has no Stasis runtime error node")
    matching_nodes = [
        node
        for node in overlay_nodes
        if all(
            value in node.attrib.get("content-desc", "")
            for value in required_text
        )
    ]
    if not matching_nodes:
        raise SeamError(
            f"{test_id} error overlay node is missing required text: "
            + ", ".join(required_text)
        )
    for node in matching_nodes:
        bounds_text = node.attrib.get("bounds", "")
        bounds_match = re.fullmatch(
            r"\[(\d+),(\d+)\]\[(\d+),(\d+)\]", bounds_text
        )
        if bounds_match is None:
            continue
        bounds = [int(value) for value in bounds_match.groups()]
        if bounds[2] <= bounds[0] or bounds[3] <= bounds[1]:
            continue
        return {"java_error_visible": True, "overlay_bounds": bounds}
    raise SeamError(f"{test_id} error overlay node has malformed bounds")


def validate_it022_error_overlay(
    ui_xml: str, diagnostic: str
) -> dict[str, bool | list[int]]:
    """Require the expected IT-022 error content on one XML node."""
    return validate_runtime_error_overlay(
        ui_xml,
        ("Release runtime error", "Asset verification failed", diagnostic),
        "IT-022",
    )


def validate_it022_overlay_capture(
    capture: Path, bounds: list[int]
) -> dict[str, int | list[int]]:
    """Require red-dominant pixels inside the reported overlay bounds."""
    if len(bounds) != 4 or not all(isinstance(value, int) for value in bounds):
        raise SeamError(f"IT-022 error overlay bounds are malformed: {bounds!r}")
    left, top, right, bottom = bounds
    width, height, pixels = read_png_rgb(capture)
    if left < 0 or top < 0 or right > width or bottom > height:
        raise SeamError(
            f"IT-022 error overlay bounds are outside captured PNG: "
            f"bounds={bounds!r} size={[width, height]}"
        )
    if right <= left or bottom <= top:
        raise SeamError(f"IT-022 error overlay bounds are empty: {bounds!r}")
    samples = [
        pixels[row * width + column]
        for row in range(top, bottom)
        for column in range(left, right)
    ]
    red_pixels = sum(
        1
        for red, green, blue in samples
        if red >= 40 and red >= green + 20 and red >= blue + 20
    )
    minimum_red_pixels = max(4, min(2000, len(samples) // 100))
    if red_pixels < minimum_red_pixels:
        raise SeamError(
            "IT-022 error overlay has no meaningful red-dominant pixels: "
            f"observed={red_pixels} minimum={minimum_red_pixels} bounds={bounds!r}"
        )
    return {"overlay_bounds": bounds, "overlay_red_pixels": red_pixels}


def capture_runtime_error_overlay(
    adb: Path,
    serial: str | None,
    diagnostic: str,
    capture: Path,
    ui_hierarchy: Path,
    validator,
    test_id: str,
    deadline_seconds: float = 10.0,
    retry_interval_seconds: float = 0.25,
) -> dict[str, bool | int | list[int]]:
    """Poll a direct UI XML dump until the expected error overlay is ready."""
    deadline = time.monotonic() + deadline_seconds
    attempts = 0
    last_error = ""
    last_result = None
    while True:
        result = _run_result(
            adb,
            serial,
            "exec-out",
            "uiautomator",
            "dump",
            "--compressed",
            "/dev/tty",
        )
        attempts += 1
        last_result = result
        ui_hierarchy.write_text(result.stdout, encoding="utf-8")
        if result.returncode == 0:
            try:
                evidence = validator(result.stdout, diagnostic)
                capture.write_bytes(
                    _run(adb, serial, "exec-out", "screencap", "-p", text=False)
                )
                evidence.update(
                    validate_it022_overlay_capture(capture, evidence["overlay_bounds"])
                )
            except SeamError as error:
                last_error = str(error)
            else:
                return {**evidence, "attempts": attempts}
        else:
            last_error = (
                f"adb UI dump failed with exit={result.returncode}: "
                f"{_clip_probe_text(result.stderr)}"
            )
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            break
        time.sleep(min(max(retry_interval_seconds, 0.0), remaining))
    assert last_result is not None
    raise SeamError(
        f"{test_id} Java error overlay was not visible after {attempts} attempts: "
        f"{last_error}; final_exit={last_result.returncode} "
        f"final_stdout={_clip_probe_text(last_result.stdout)!r} "
        f"final_stderr={_clip_probe_text(last_result.stderr)!r}"
    )


def capture_it022_error_overlay(
    adb: Path,
    serial: str | None,
    diagnostic: str,
    capture: Path,
    ui_hierarchy: Path,
    deadline_seconds: float = 10.0,
    retry_interval_seconds: float = 0.25,
) -> dict[str, bool | int | list[int]]:
    """Poll a direct UI XML dump until the IT-022 rejection overlay is ready."""
    return capture_runtime_error_overlay(
        adb,
        serial,
        diagnostic,
        capture,
        ui_hierarchy,
        validate_it022_error_overlay,
        "IT-022",
        deadline_seconds,
        retry_interval_seconds,
    )


def validate_entry_failure_error_overlay(
    ui_xml: str, diagnostic: str
) -> dict[str, bool | list[int]]:
    """Require the exact native entry failure on the Java accessibility node."""
    return validate_runtime_error_overlay(
        ui_xml, ("Release runtime error", diagnostic), "IT-024"
    )


def capture_entry_failure_error_overlay(
    adb: Path,
    serial: str | None,
    diagnostic: str,
    capture: Path,
    ui_hierarchy: Path,
    deadline_seconds: float = 10.0,
) -> dict[str, bool | int | list[int]]:
    """Poll accessibility until the IT-024 runtime error overlay is visible."""
    return capture_runtime_error_overlay(
        adb,
        serial,
        diagnostic,
        capture,
        ui_hierarchy,
        validate_entry_failure_error_overlay,
        "IT-024",
        deadline_seconds,
    )


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


def terminal_event(expectations: dict) -> str:
    """Return the marker that ends polling for this shell lane."""
    if expectations.get("asset_rejection") is not None:
        return "asset_rejected"
    if expectations.get("entry_failure") is not None:
        return "entry_failure"
    return "stable"


def validate_markers(markers: list[dict], expectations: dict) -> dict:
    stable_frame = expectations["stable_frame"]
    events = {(item.get("event"), item.get("frame")): item for item in markers}
    if ("initialized", 0) not in events or ("frame", 1) not in events:
        raise SeamError("Android shell did not emit initialized and first-frame markers")
    stable = events.get(("stable", stable_frame))
    if stable is None:
        raise SeamError(f"Android shell did not reach stable frame {stable_frame}")
    if "command_trace" in expectations or expectations.get("require_command_trace") is True:
        stable_trace = stable.get("command_trace")
        if (
            not isinstance(stable_trace, int)
            or isinstance(stable_trace, bool)
            or stable_trace <= 0
        ):
            raise SeamError(
                "Android stable-frame marker command_trace must be a positive integer: "
                f"actual={stable_trace}"
            )
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


def validate_storage_relative_path(relative_path: str) -> str:
    """Accept only fixed app-private storage paths used by IT-023."""
    if not isinstance(relative_path, str) or re.fullmatch(
        r"files/[A-Za-z0-9_-]{1,63}/[A-Za-z0-9_-]{1,63}\.i32", relative_path
    ) is None:
        raise SeamError(f"IT-023 invalid app-private storage path: {relative_path!r}")
    return relative_path


def probe_storage_file(
    adb: Path, serial: str | None, package_id: str, relative_path: str
) -> str | None:
    relative_path = validate_storage_relative_path(relative_path)
    result = _run_result(
        adb, serial, "shell", "run-as", package_id, "cat", relative_path
    )
    if result.returncode == 0:
        return result.stdout
    diagnostic = (result.stdout + result.stderr).lower()
    if "no such file" in diagnostic or "not found" in diagnostic:
        return None
    raise SeamError(
        f"IT-023 storage probe failed for {relative_path}: "
        f"stdout={result.stdout!r} stderr={result.stderr!r}"
    )


def corrupt_storage_file(
    adb: Path, serial: str | None, package_id: str, relative_path: str
) -> None:
    relative_path = validate_storage_relative_path(relative_path)
    command = [str(adb)]
    if serial:
        command.extend(("-s", serial))
    remote_command = (
        f"run-as {package_id} sh -c 'printf \"corrupt\\n\" > {relative_path}'"
    )
    command.extend(("shell", remote_command))
    result = subprocess.run(
        command, capture_output=True, text=False, check=False, timeout=10
    )
    validate_storage_write_result(
        relative_path, result.returncode, result.stdout, result.stderr, b""
    )


def validate_storage_write_result(
    relative_path: str,
    returncode: int,
    stdout: bytes,
    stderr: bytes,
    expected: bytes,
) -> None:
    if returncode != 0 or stdout != expected or stderr:
        raise SeamError(
            f"IT-023 storage write failed for {relative_path}: exit={returncode} "
            f"stdout={stdout!r} stderr={stderr!r}"
        )


def validate_storage_marker(marker: dict, storage: dict, phase: int) -> dict:
    expected_value = storage["exact_value"] if phase < 3 else storage["corrupt_fallback"]
    expected = {
        "storage_phase": phase,
        "storage_loaded_value": expected_value,
        "storage_unrelated_scope": storage["unrelated_scope_fallback"],
        "storage_unrelated_key": storage["unrelated_key_fallback"],
        "storage_traversal_rejected": 1,
        "storage_checksum": phase * 1000000 + expected_value,
    }
    mismatches = {
        key: {"expected": value, "actual": marker.get(key)}
        for key, value in expected.items()
        if marker.get(key) != value
    }
    if mismatches:
        raise SeamError(f"IT-023 storage marker mismatch: {mismatches}")
    return {key: marker[key] for key in expected}


def validate_fresh_process_pid(previous_pid: str, current_pid: str) -> str:
    if not current_pid or current_pid == previous_pid:
        raise SeamError(
            f"IT-023 fresh process PID expected after {previous_pid!r}, got {current_pid!r}"
        )
    return current_pid


def validate_storage_file_text(actual: str | None, expected: str, stage: str) -> str:
    if actual != expected:
        raise SeamError(
            f"IT-023 {stage} storage bytes expected {expected!r}, got {actual!r}"
        )
    return actual


def require_storage_file_absent(
    relative_path: str, returncode: int, stdout: str, stderr: str
) -> None:
    if returncode == 0:
        raise SeamError(f"IT-023 traversal escaped app storage: {relative_path}")
    diagnostic = (stdout + stderr).lower()
    if "no such file" not in diagnostic and "not found" not in diagnostic:
        raise SeamError(
            f"IT-023 escape probe failed for {relative_path}: "
            f"stdout={stdout!r} stderr={stderr!r}"
        )


def wait_for_storage_launch(
    adb: Path,
    serial: str | None,
    package_id: str,
    component: str,
    test_id: str,
    expectations: dict,
    previous_pid: str,
    deadline: float,
) -> tuple[str, dict, str]:
    _run(adb, serial, "shell", "am", "force-stop", package_id)
    _run(adb, serial, "logcat", "-c")
    _run(
        adb, serial, "shell", "am", "start", "-n", component,
        "--es", "stasis.seam_test_id", test_id,
        required=False,
        timeout=10,
    )
    log = ""
    foreground_checked = False
    while time.monotonic() < deadline:
        log = _run(
            adb, serial, "logcat", "-d", "-v", "brief", "Stasis:I", "*:S"
        )
        markers = parse_markers(log, test_id)
        marker = next(
            (
                item
                for item in reversed(markers)
                if item.get("event") in {"stable", "initialized"}
            ),
            None,
        )
        if marker is not None:
            pid = _run(
                adb, serial, "shell", "pidof", package_id, required=False
            ).strip()
            return validate_fresh_process_pid(previous_pid, pid), marker, log
        if not markers and not foreground_checked:
            foreground_checked = True
            if ensure_test_activity_foreground(
                adb,
                serial,
                package_id,
                component,
                wait_for_launch=False,
                intent_arguments=("--es", "stasis.seam_test_id", test_id),
            ):
                time.sleep(0.25)
        time.sleep(0.25)
    raise SeamError("IT-023 relaunched process did not reach its stable marker")


def run_it023_storage_lifecycle(
    adb: Path,
    serial: str | None,
    package_id: str,
    component: str,
    test_id: str,
    expectations: dict,
    first_pid: str,
    initial_marker: dict,
    deadline: float,
) -> dict:
    storage = expectations["storage"]
    target_path = validate_storage_relative_path(storage["target_path"])
    control_path = validate_storage_relative_path(storage["control_path"])
    absent_paths = [
        validate_storage_relative_path(storage["unrelated_scope_path"]),
        validate_storage_relative_path(storage["unrelated_key_path"]),
    ]
    validate_storage_marker(initial_marker, storage, 1)
    exact_text = f"{storage['exact_value']}\n"
    validate_storage_file_text(
        probe_storage_file(adb, serial, package_id, target_path),
        exact_text,
        "initial target",
    )
    validate_storage_file_text(
        probe_storage_file(adb, serial, package_id, control_path),
        "1\n",
        "initial control",
    )
    if any(
        probe_storage_file(adb, serial, package_id, path) is not None
        for path in absent_paths
    ):
        raise SeamError("IT-023 unrelated scope or key unexpectedly created storage")
    second_pid, second_marker, second_log = wait_for_storage_launch(
        adb, serial, package_id, component, test_id, expectations, first_pid,
        min(deadline, time.monotonic() + 45),
    )
    validate_storage_marker(second_marker, storage, 2)
    validate_storage_file_text(
        probe_storage_file(adb, serial, package_id, target_path),
        exact_text,
        "persisted target",
    )
    corrupt_storage_file(adb, serial, package_id, target_path)
    validate_storage_file_text(
        probe_storage_file(adb, serial, package_id, target_path),
        "corrupt\n",
        "corrupt target",
    )
    third_pid, third_marker, third_log = wait_for_storage_launch(
        adb, serial, package_id, component, test_id, expectations, second_pid,
        min(deadline, time.monotonic() + 45),
    )
    validate_storage_marker(third_marker, storage, 3)
    for path in absent_paths:
        if probe_storage_file(adb, serial, package_id, path) is not None:
            raise SeamError(f"IT-023 unrelated storage path exists: {path}")
    for escape_path in storage["escape_paths"]:
        if not isinstance(escape_path, str) or not re.fullmatch(
            r"(?:files/)?[A-Za-z0-9_-]{1,63}\.i32", escape_path
        ):
            raise SeamError(f"IT-023 invalid escape probe path: {escape_path!r}")
        result = _run_result(
            adb, serial, "shell", "run-as", package_id, "cat", escape_path
        )
        require_storage_file_absent(
            escape_path, result.returncode, result.stdout, result.stderr
        )
    return {
        "process_epochs": [first_pid, second_pid, third_pid],
        "markers": [initial_marker, second_marker, third_marker],
        "target_path": target_path,
        "target_value": storage["exact_value"],
        "corrupt_fallback": storage["corrupt_fallback"],
        "scope_and_key_isolated": True,
        "traversal_escape_absent": True,
        "logs": [second_log, third_log],
    }


def validate_asset_rejection_markers(
    markers: list[dict], log: str, expectations: dict
) -> dict:
    """Validate IT-022 rejection before any guest initialization or frame."""
    rejection = expectations.get("asset_rejection")
    if not isinstance(rejection, dict):
        raise SeamError("IT-022 expectations are missing asset_rejection")
    if any(item.get("event") in ("initialized", "frame", "stable") for item in markers):
        raise SeamError("IT-022 rejected package emitted a game initialization/frame marker")
    marker = next((item for item in markers if item.get("event") == "asset_rejected"), None)
    if marker is None:
        raise SeamError("IT-022 package rejection marker is missing")
    diagnostic = marker.get("asset_error")
    if not isinstance(diagnostic, str):
        raise SeamError("IT-022 rejection marker has no asset_error diagnostic")
    parsed = ASSET_DIAGNOSTIC.fullmatch(diagnostic)
    if parsed is None:
        raise SeamError(f"IT-022 rejection diagnostic is not stable: {diagnostic}")
    code, path, detail = parsed.groups()
    for key, actual in (("code", code), ("path", path)):
        expected = rejection.get(key)
        if expected is not None and expected != actual:
            raise SeamError(
                f"IT-022 {key} expected {expected} actual {actual}; diagnostic={diagnostic}"
            )
    if marker.get("accepted") != 0 or marker.get("presented") != 0:
        raise SeamError("IT-022 rejected package reported accepted or presented work")
    if "Stasis IT-022 asset verification rejected package: " + diagnostic not in log:
        raise SeamError("IT-022 native rejection log does not preserve Java diagnostic")
    if rejection.get("detail_contains") and rejection["detail_contains"] not in detail:
        raise SeamError(
            f"IT-022 detail expected {rejection['detail_contains']} actual {detail}"
        )
    return {"code": code, "path": path, "detail": detail, "diagnostic": diagnostic}


def validate_entry_failure_markers(
    markers: list[dict], log: str, expectations: dict
) -> dict:
    """Validate exact IT-024 entry identity, code, call boundary, and no submit."""
    expected = expectations.get("entry_failure")
    if not isinstance(expected, dict):
        raise SeamError("IT-024 expectations are missing entry_failure")
    failures = [item for item in markers if item.get("event") == "entry_failure"]
    if len(failures) != 1:
        raise SeamError(f"IT-024 expected one entry_failure marker, observed {len(failures)}")
    marker = failures[0]
    failure_index = markers.index(marker)
    if failure_index != len(markers) - 1:
        later = [item.get("event") for item in markers[failure_index + 1 :]]
        raise SeamError(f"IT-024 emitted lifecycle markers after entry failure: {later}")
    for key in ("entry", "code", "main_calls", "tick_calls", "render_calls"):
        if marker.get(key) != expected.get(key):
            raise SeamError(
                f"IT-024 {key} expected {expected.get(key)!r} actual {marker.get(key)!r}"
            )
    for key in ("accepted", "rejected", "presented", "validation"):
        if marker.get(key) != 0:
            raise SeamError(f"IT-024 entry failure reported nonzero {key}: {marker.get(key)!r}")
    diagnostic = f"Stasis {expected['entry']} entry failed with code {expected['code']}"
    if log.count(diagnostic) != 1:
        raise SeamError("IT-024 native log must contain exactly one entry/code diagnostic")
    return {**expected, "diagnostic": diagnostic}


def validate_entry_failure_process_identity(
    adb: Path,
    serial: str | None,
    package_id: str,
    expected_pid: str,
) -> str:
    """Require the IT-024 error surface to stay in its original process."""
    actual = _run(
        adb, serial, "shell", "pidof", package_id, required=False
    ).strip()
    if actual != expected_pid:
        raise SeamError(
            f"IT-024 process identity changed: expected {expected_pid!r} actual {actual!r}"
        )
    return actual


def validate_no_android_fatal_evidence(log: str) -> None:
    """Reject Android runtime/native crash evidence after a graceful entry stop."""
    fatal = re.search(r"FATAL EXCEPTION|Fatal signal|Abort message|has died", log, re.I)
    if fatal is not None:
        raise SeamError(f"IT-024 fatal/crash evidence observed: {fatal.group(0)}")


def validate_rejection_storage_state(staging_probe: str, root_probe: str) -> dict[str, bool]:
    """Require a fresh rejected install to publish neither staging nor root."""
    staging = staging_probe.strip().lower()
    root = root_probe.strip().lower()
    if staging != "absent" or root != "absent":
        raise SeamError(
            f"IT-022 rejected package published extraction state: staging={staging!r} root={root!r}"
        )
    return {"staging_absent": True, "root_unpublished": True}


def validate_rejection_process_identity(
    adb: Path,
    serial: str | None,
    package_id: str,
    expected_pid: str,
) -> str:
    """Require the rejected package to keep the original process alive."""
    actual_pid = _run(
        adb, serial, "shell", "pidof", package_id, required=False
    ).strip()
    if actual_pid != expected_pid:
        raise SeamError("IT-022 rejected package entered a crash loop")
    return actual_pid


def validate_install_policy(
    preinstalled: bool,
    retain_installed_package: bool,
    replace_existing_package: bool,
    test_id: str,
    asset_variant: str | None,
) -> dict[str, bool]:
    """Validate the narrowly scoped install behavior used by the seam runner."""
    rejection = test_id == "IT-022" and bool(asset_variant)
    recovery = test_id == "IT-021" and not asset_variant
    if retain_installed_package and replace_existing_package:
        raise SeamError(
            "retaining an install and replacing an existing package are contradictory"
        )
    if retain_installed_package and not rejection:
        raise SeamError(
            "retaining an install is only allowed for an IT-022 rejection variant"
        )
    if retain_installed_package and preinstalled:
        raise SeamError(
            "refusing to retain a malformed run over a preinstalled test package"
        )
    if replace_existing_package and not recovery:
        raise SeamError(
            "replacing an existing package is only allowed for the recovery invocation"
        )
    if replace_existing_package and not preinstalled:
        raise SeamError(
            "recovery requires an existing test package to replace"
        )
    if preinstalled and not replace_existing_package:
        raise SeamError(
            "refusing to replace preinstalled test package; remove it explicitly"
        )
    return {
        "existing_installation": preinstalled,
        "replaced_existing_installation": replace_existing_package,
        "retained_installation": retain_installed_package,
    }


def should_retain_installed_package(requested: bool, evidence_status: str) -> bool:
    """Retain a rejected install only after its complete seam evidence passes."""
    return requested and evidence_status == "passed"


def packaged_asset_manifest(package_manifest: Path, package: dict) -> tuple[Path, str]:
    """Resolve and hash the manifest that was copied into the generated package."""
    candidates = []
    asset_root = package.get("assets")
    if isinstance(asset_root, str):
        candidates.append(package_manifest.parent / asset_root / "assets/manifest.json")
    candidates.extend(
        (
            package_manifest.parent / "android/app/src/main/assets/stasis_game/assets/manifest.json",
            package_manifest.parent / "aot/apk_assets/stasis_game/assets/manifest.json",
        )
    )
    for candidate in candidates:
        if candidate.is_file():
            data = candidate.read_bytes()
            return candidate, hashlib.sha256(data).hexdigest()
    rendered = ", ".join(str(path) for path in candidates)
    raise SeamError(f"IT-021 packaged manifest missing; checked {rendered}")


def validate_asset_audio_markers(
    markers: list[dict], expectations: dict, package: dict, package_manifest: Path
) -> dict:
    """Validate IT-021 identities and offline mixer evidence from the stable marker."""
    assets = expectations.get("assets")
    if not isinstance(assets, dict):
        return {}
    stable = validate_markers(markers, expectations)
    manifest_path, manifest_hash = packaged_asset_manifest(package_manifest, package)
    expected_hash = assets.get("manifest_sha256")
    if expected_hash not in (None, "computed_from_packaged_manifest", manifest_hash):
        raise SeamError(
            f"IT-021 field manifest_sha256 expected {expected_hash} actual {manifest_hash}; "
            f"evidence path {manifest_path}"
        )
    actual_hash = stable.get("asset_manifest_sha256")
    if actual_hash != manifest_hash:
        raise SeamError(
            f"IT-021 field asset_manifest_sha256 expected {manifest_hash} actual {actual_hash}; "
            f"evidence path {manifest_path}"
        )
    package_id = package["package_id"]
    expected_roots = (
        f"/data/user/0/{package_id}/files/stasis_game",
        f"/data/data/{package_id}/files/stasis_game",
    )
    actual_root = stable.get("asset_root")
    if actual_root not in expected_roots:
        raise SeamError(
            f"IT-021 field asset_root expected one of {expected_roots} actual {actual_root}; "
            f"evidence path {package_manifest}"
        )
    handles = assets.get("handles", {})
    for marker_field in handles.values():
        actual = stable.get(marker_field)
        if not isinstance(actual, int) or actual <= 0:
            raise SeamError(
                f"IT-021 field {marker_field} expected positive identity actual {actual}; "
                f"evidence path {package_manifest}"
            )
    minimum_width = float(assets.get("minimum_text_width", 0.0))
    for field in ("direct_text_width", "cached_text_width"):
        actual = stable.get(field)
        if not isinstance(actual, (int, float)) or actual < minimum_width:
            raise SeamError(
                f"IT-021 field {field} expected >= {minimum_width} actual {actual}; "
                f"evidence path {package_manifest}"
            )
    audio = assets.get("audio", {})
    exact = {
        "audio_queued_before": audio.get("queued_frames_before"),
        "audio_queued_after": audio.get("queued_frames_after"),
    }
    for field, expected in exact.items():
        if expected is not None and stable.get(field) != expected:
            raise SeamError(
                f"IT-021 field {field} expected {expected} actual {stable.get(field)}; "
                f"evidence path {package_manifest}"
            )
    minimums = {
        "audio_frames_mixed": audio.get("minimum_frames_mixed", 1),
        "audio_nonzero_after_prefix": audio.get("minimum_nonzero_samples_after_prefix", 1),
    }
    for field, minimum in minimums.items():
        actual = stable.get(field)
        if not isinstance(actual, int) or actual < minimum:
            raise SeamError(
                f"IT-021 field {field} expected >= {minimum} actual {actual}; "
                f"evidence path {package_manifest}"
            )
    expected_voice = audio.get("voice_state", 1)
    if stable.get("audio_voice_state") != expected_voice:
        raise SeamError(
            f"IT-021 field audio_voice_state expected {expected_voice} actual "
            f"{stable.get('audio_voice_state')}; evidence path {package_manifest}"
        )
    checksum = stable.get("audio_sample_checksum")
    if audio.get("sample_checksum") == "nonzero" and (not isinstance(checksum, int) or checksum == 0):
        raise SeamError(
            f"IT-021 field audio_sample_checksum expected nonzero actual {checksum}; "
            f"evidence path {package_manifest}"
        )
    if stable.get("audio_replay_matches") != audio.get("replay_matches", 1):
        raise SeamError(
            f"IT-021 field audio_replay_matches expected {audio.get('replay_matches', 1)} "
            f"actual {stable.get('audio_replay_matches')}; evidence path {package_manifest}"
        )
    if stable.get("audio_replay_checksum") != checksum:
        raise SeamError(
            f"IT-021 field audio_replay_checksum expected {checksum} actual "
            f"{stable.get('audio_replay_checksum')}; evidence path {package_manifest}"
        )
    checksum_values = {
        marker.get("audio_sample_checksum")
        for marker in markers
        if "audio_sample_checksum" in marker
    }
    if len(checksum_values) > 1:
        raise SeamError(
            f"IT-021 field audio_sample_checksum expected stable value actual {sorted(checksum_values, key=str)}; "
            f"evidence path {package_manifest}"
        )
    return {
        "manifest": str(manifest_path),
        "manifest_sha256": manifest_hash,
        "asset_root": actual_root,
        "identities": {
            name: {"marker_field": marker_field, "handle": stable.get(marker_field)}
            for name, marker_field in handles.items()
        },
        "marker": {
            key: stable.get(key)
            for key in (
                "sprite_handle", "font_handle", "cached_text_handle", "audio_handle",
                "voice_handle", "direct_text_width", "cached_text_width",
                "audio_queued_before", "audio_queued_after", "audio_frames_mixed",
                "audio_nonzero_after_prefix", "audio_voice_state", "audio_sample_checksum",
                "audio_replay_checksum", "audio_replay_matches",
            )
        },
    }


def forbidden_resource_diagnostics(expectations: dict) -> tuple[str, ...]:
    """Return diagnostics that invalidate a resource restoration seam."""
    lifecycle = expectations.get("lifecycle", {})
    values = lifecycle.get("diagnostic_forbidden", ())
    return tuple(str(value).lower() for value in values)


def validate_resource_lifecycle_markers(
    markers: list[dict], expectations: dict, stage_markers: dict[str, dict] | None = None
) -> dict[str, dict]:
    """Validate generation/counter invariants for every requested lifecycle stage."""
    lifecycle = expectations.get("lifecycle")
    if not lifecycle:
        return {}
    observed = stage_markers if stage_markers is not None else {}
    candidates = [item for item in markers if item.get("event") in ("lifecycle", "stable", "initialized")]
    for stage in lifecycle.get("stages", ()):
        name = stage["name"]
        marker = observed.get(name)
        if marker is None:
            marker = next((item for item in reversed(candidates) if item.get("stage") == name), None)
        if marker is None:
            raise SeamError(f"Android resource lifecycle is missing stage {name}")
        generation = marker.get("renderer_generation", 0)
        surface_generation = marker.get("surface_generation", 0)
        if not isinstance(generation, int) or generation < stage.get("min_renderer_generation", 1):
            raise SeamError(f"Android resource lifecycle stage {name} has invalid renderer generation {generation}")
        if not isinstance(surface_generation, int) or surface_generation <= 0:
            raise SeamError(f"Android resource lifecycle stage {name} has invalid surface generation {surface_generation}")
        if marker.get("resource_state") not in (1,):
            raise SeamError(f"Android resource lifecycle stage {name} is not ready: state={marker.get('resource_state')}")
        if marker.get("restore_failures", 0) != 0:
            raise SeamError(f"Android resource lifecycle stage {name} reported restore failures: {marker.get('restore_failures')}")
        # These counters prove render progress; the compositor capture below is
        # the presentation oracle because end_frame may suppress presentation.
        if marker.get("accepted", 0) <= 0 or marker.get("presented", 0) <= 0:
            raise SeamError(f"Android resource lifecycle stage {name} has no accepted presented frame")
        if marker.get("rejected", 0) != 0 or marker.get("validation", 0) != 0:
            raise SeamError(
                f"Android resource lifecycle stage {name} rejected or validation counters are nonzero: "
                f"rejected={marker.get('rejected')} validation={marker.get('validation')}"
            )
        observed[name] = marker
    return observed


def select_post_transition_marker(
    markers: list[dict], baseline: dict, action: str
) -> dict:
    """Select a ready marker emitted after an action, never from log history."""
    ready = [
        item
        for item in markers
        if item.get("event") in ("lifecycle", "stable")
        and item.get("resource_state") == 1
        and item.get("accepted", 0) > 0
        and item.get("presented", 0) > 0
        and item.get("rejected", 0) == 0
        and item.get("validation", 0) == 0
    ]
    if action == "background_resume":
        ready = [
            item
            for item in ready
            if item.get("accepted", 0) > baseline.get("accepted", 0)
            and item.get("presented", 0) > baseline.get("presented", 0)
        ]
    elif action == "force_activity_restart":
        initialized = next(
            (index for index, item in enumerate(markers) if item.get("event") == "initialized"),
            None,
        )
        if initialized is None:
            raise SeamError("Android resource recreation emitted no new initialized marker")
        ready = [
            item
            for index, item in enumerate(markers)
            if index > initialized
            and item.get("event") in ("lifecycle", "stable")
            and item.get("resource_state") == 1
            and item.get("accepted", 0) > 0
            and item.get("presented", 0) > 0
            and item.get("rejected", 0) == 0
            and item.get("validation", 0) == 0
        ]
    if not ready:
        raise SeamError(
            f"Android resource transition {action} emitted no new ready marker with advancing counters"
        )
    return ready[-1]


def validate_resource_diagnostics(log: str, expectations: dict) -> None:
    lowered = log.lower()
    for diagnostic in forbidden_resource_diagnostics(expectations):
        if diagnostic in lowered:
            raise SeamError(f"Android resource lifecycle diagnostic contains forbidden {diagnostic!r}")


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
    require_trace = (
        "final_command_trace" in touch or touch.get("require_command_trace") is True
    )
    if require_trace:
        final_trace = final.get("command_trace")
        if (
            not isinstance(final_trace, int)
            or isinstance(final_trace, bool)
            or final_trace <= 0
        ):
            raise SeamError(
                "Android touch final command_trace must be a positive integer: "
                f"actual={final_trace}"
            )
        if (
            "final_command_trace" in touch
            and final_trace != touch["final_command_trace"]
        ):
            raise SeamError(
                "Android touch final command trace mismatch: "
                f"expected={touch['final_command_trace']} actual={final_trace}"
            )
        stable = next(
            (
                item
                for item in markers
                if item.get("event") == "stable"
                and item.get("frame") == expectations.get("stable_frame")
            ),
            None,
        )
        unchanged = [
            marker
            for marker, expected in zip(observed, touch["probes"])
            if expected.get("state_transitions") == 0
        ]
        baseline_traces = [item.get("command_trace") for item in unchanged]
        if stable is not None:
            baseline_traces.append(stable.get("command_trace"))
        if any(final_trace == trace for trace in baseline_traces):
            raise SeamError(
                "Android touch final command_trace did not change from the "
                "stable/outside state: "
                f"final={final_trace} baseline={baseline_traces}"
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
        expected_sizes = {
            "native": (surface_width, surface_height),
            "drawable": (surface_width, surface_height),
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
        fitted_scale = min(
            surface_width / logical_width, surface_height / logical_height
        )
        fitted_extent = (
            max(1, int(logical_width * fitted_scale + 0.5)),
            max(1, int(logical_height * fitted_scale + 0.5)),
        )
        expected_scale = min(
            fitted_extent[0] / logical_width,
            fitted_extent[1] / logical_height,
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
        observed_marker = dict(marker)
        observed_marker["fitted_content_extent"] = list(fitted_extent)
        observed.append(observed_marker)
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


def validate_resource_regions(capture: Path, expectations: dict) -> list[dict]:
    """Prove resource pixels, rather than merely sampling their lane backgrounds."""
    regions = expectations.get("resource_regions", ())
    if not regions:
        return []
    width, height, pixels = read_png_rgb(capture)
    logical_width, logical_height = expectations["logical_size"]
    scale = min(width / logical_width, height / logical_height)
    offset_x = (width - logical_width * scale) / 2.0
    offset_y = (height - logical_height * scale) / 2.0
    observed = []
    for region in regions:
        logical_x, logical_y, logical_w, logical_h = region["rect"]
        left = max(0, round(offset_x + logical_x * scale))
        top = max(0, round(offset_y + logical_y * scale))
        right = min(width, round(offset_x + (logical_x + logical_w) * scale))
        bottom = min(height, round(offset_y + (logical_y + logical_h) * scale))
        samples = [
            pixels[row * width + column]
            for row in range(top, bottom)
            for column in range(left, right)
        ]
        if not samples:
            raise SeamError(f"resource region {region['name']} has no captured pixels")
        target_count = 0
        target = region.get("target_rgb")
        if target is None:
            raise SeamError(f"resource region {region['name']} is missing target_rgb")
        tolerance = region.get("tolerance", 0)
        for pixel in samples:
            if all(
                abs(pixel[index] - target[index]) <= tolerance for index in range(3)
            ):
                target_count += 1
        minimum_target = region.get("minimum_target_pixels", 0)
        if minimum_target <= 0:
            raise SeamError(f"resource region {region['name']} is missing minimum_target_pixels")
        if target_count < minimum_target:
            raise SeamError(
                f"resource region {region['name']} did not contain enough target pixels: "
                f"expected>={minimum_target} actual={target_count}"
            )
        observed.append(
            {
                "name": region["name"],
                "target_pixels": target_count,
                "bounds": [left, top, right, bottom],
            }
        )
    return observed


def capture_until_resource_regions_match(
    adb: Path,
    serial: str | None,
    capture: Path,
    expectations: dict,
    deadline: float,
    package_id: str,
    component: str,
) -> tuple[list[dict], list[dict]]:
    """Use the Android compositor capture as the actual presentation oracle."""
    last_error: SeamError | None = None
    while time.monotonic() < deadline:
        if ensure_test_activity_foreground(adb, serial, package_id, component):
            time.sleep(0.25)
        capture.write_bytes(
            _run(adb, serial, "exec-out", "screencap", "-p", text=False)
        )
        try:
            return validate_regions(capture, expectations), validate_resource_regions(
                capture, expectations
            )
        except SeamError as error:
            last_error = error
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
    raise SeamError("resource capture deadline expired before both pixel oracles passed")


def dismiss_system_dialog_action(adb: Path, serial: str | None) -> bool:
    hierarchy = _run(
        adb,
        serial,
        "exec-out",
        "uiautomator",
        "dump",
        "--compressed",
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
    *,
    wait_for_launch: bool = True,
    intent_arguments: tuple[str, ...] = (),
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
    # Fail closed for product and unrecognized ANRs: keep the dialog visible so
    # the bounded capture oracle reports the failure instead of masking it.
    if system_dialog_present and not dismissed:
        return False
    _run(
        adb,
        serial,
        "shell",
        "am",
        "start",
        *(("-W",) if wait_for_launch else ()),
        "-n",
        component,
        *intent_arguments,
        required=False,
        timeout=10,
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
    retain_installed_package: bool = False,
) -> list[str]:
    errors = []
    operations = []
    if installed:
        operations.append(("force-stop", ("shell", "am", "force-stop", package_id)))
        if not retain_installed_package:
            operations.append(("uninstall", ("uninstall", package_id)))
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
    parser.add_argument("--asset-variant")
    parser.add_argument(
        "--retain-installed-package",
        action="store_true",
        help="retain the newly installed package after this run for recovery",
    )
    parser.add_argument(
        "--replace-existing-package",
        action="store_true",
        help="allow this recovery run to replace the retained test package",
    )
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
    ui_hierarchy_path = args.output / "ui-hierarchy.xml"
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
    install_policy = validate_install_policy(
        preinstalled,
        args.retain_installed_package,
        args.replace_existing_package,
        test_id,
        args.asset_variant,
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
        "install_policy": install_policy,
    }
    if device_state is not None:
        evidence["original_device_state"] = device_state
    if test_id == "IT-023":
        evidence["artifacts"].pop("capture")
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
            *(("-W",) if test_id != "IT-023" else ()),
            "-n",
            component,
            "--es",
            "stasis.seam_test_id",
            test_id,
            *( (
                "--es",
                "stasis.asset_variant",
                args.asset_variant,
            ) if args.asset_variant else () ),
            required=test_id != "IT-023",
            timeout=10 if test_id == "IT-023" else None,
        )
        deadline = time.monotonic() + args.timeout_seconds
        log = ""
        markers: list[dict] = []
        expected_terminal_event = terminal_event(expectations)
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
            if any(item.get("event") == expected_terminal_event for item in markers):
                break
            time.sleep(1)
        asset_rejection = expectations.get("asset_rejection")
        if asset_rejection is not None:
            rejection = validate_asset_rejection_markers(markers, log, expectations)
            log_path.write_text(log, encoding="utf-8")
            first_pid = _run(
                args.adb, args.serial, "shell", "pidof", package_id, required=False
            ).strip()
            if not first_pid:
                raise SeamError("IT-022 rejected package process exited unexpectedly")
            staging_probe = probe_rejection_storage_path(
                args.adb,
                args.serial,
                package_id,
                "files/.stasis_game.staging",
            )
            root_probe = probe_rejection_storage_path(
                args.adb,
                args.serial,
                package_id,
                "files/stasis_game",
            )
            storage = validate_rejection_storage_state(staging_probe, root_probe)
            foreground_restored = ensure_test_activity_foreground(
                args.adb,
                args.serial,
                package_id,
                component,
            )
            if foreground_restored:
                time.sleep(0.25)
            evidence["artifacts"]["ui_hierarchy"] = str(ui_hierarchy_path)
            overlay = capture_it022_error_overlay(
                args.adb,
                args.serial,
                rejection["diagnostic"],
                capture_path,
                ui_hierarchy_path,
            )
            time.sleep(1.0)
            second_pid = validate_rejection_process_identity(
                args.adb,
                args.serial,
                package_id,
                first_pid,
            )
            evidence.update(
                {
                    "status": "passed",
                    "asset_rejection": rejection,
                    "process_id": first_pid,
                    "foreground_restored": foreground_restored,
                    **overlay,
                    **storage,
                    "lifecycle_events": [item["event"] for item in markers],
                    "presentation_oracle": "native_rejection_before_game_runtime",
                }
            )
            return 0
        entry_failure = expectations.get("entry_failure")
        if entry_failure is not None:
            failure = validate_entry_failure_markers(markers, log, expectations)
            log_path.write_text(log, encoding="utf-8")
            first_pid = _run(
                args.adb, args.serial, "shell", "pidof", package_id, required=False
            ).strip()
            if not first_pid:
                raise SeamError("IT-024 failed package process exited unexpectedly")
            foreground_restored = ensure_test_activity_foreground(
                args.adb, args.serial, package_id, component
            )
            evidence["artifacts"]["ui_hierarchy"] = str(ui_hierarchy_path)
            overlay = capture_entry_failure_error_overlay(
                args.adb,
                args.serial,
                failure["diagnostic"],
                capture_path,
                ui_hierarchy_path,
            )
            time.sleep(2.0)
            second_pid = validate_entry_failure_process_identity(
                args.adb, args.serial, package_id, first_pid
            )
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
            failure = validate_entry_failure_markers(markers, log, expectations)
            log_path.write_text(log, encoding="utf-8")
            fatal_log = _run(
                args.adb,
                args.serial,
                "logcat",
                "-d",
                "-v",
                "brief",
                "AndroidRuntime:E",
                "libc:F",
                "DEBUG:F",
                "*:S",
            )
            validate_no_android_fatal_evidence(fatal_log)
            fatal_log_path = args.output / "fatal-logcat.txt"
            fatal_log_path.write_text(fatal_log, encoding="utf-8")
            evidence["artifacts"]["fatal_log"] = str(fatal_log_path)
            evidence.update(
                {
                    "status": "passed",
                    "entry_failure": failure,
                    "process_ids": [first_pid, second_pid],
                    "foreground_restored": foreground_restored,
                    **overlay,
                    "lifecycle_events": [item["event"] for item in markers],
                    "presentation_oracle": "entry_failure_before_native_submission",
                    "fatal_evidence": False,
                }
            )
            return 0
        stable = validate_markers(markers, expectations)
        if test_id == "IT-021" or "assets" in expectations:
            evidence["assets"] = validate_asset_audio_markers(
                markers, expectations, package, args.package_manifest
            )
        log_history = [log]
        initial_log_path = args.output / "initial-logcat.txt"
        initial_log_path.write_text(log, encoding="utf-8")
        first_pid = _run(
            args.adb, args.serial, "shell", "pidof", package_id, required=False
        ).strip()
        if not first_pid:
            raise SeamError("generated Android shell exited before capture")
        if test_id == "IT-023":
            storage_evidence = run_it023_storage_lifecycle(
                args.adb, args.serial, package_id, component, test_id,
                expectations, first_pid, stable, deadline,
            )
            storage_logs = storage_evidence.pop("logs")
            log = "\n".join([log, *storage_logs])
            stable = storage_evidence["markers"][-1]
            first_pid = storage_evidence["process_epochs"][-1]
            evidence["storage_lifecycle"] = storage_evidence
        lifecycle_evidence = []
        lifecycle_markers = {"initial": stable}
        lifecycle = expectations.get("lifecycle")
        if lifecycle:
            initial_stage = next(
                stage for stage in lifecycle["stages"] if stage["name"] == "initial"
            )
            initial_stage_capture = args.output / "initial-resource-frame.png"
            initial_regions, initial_resource_regions = capture_until_resource_regions_match(
                args.adb,
                args.serial,
                initial_stage_capture,
                expectations,
                min(deadline, time.monotonic() + 10),
                package_id,
                component,
            )
            lifecycle_evidence.append(
                {
                    "name": initial_stage["name"],
                    "action": initial_stage["action"],
                    "pid": first_pid,
                    "process_epoch": first_pid,
                    "same_pid": None,
                    "capture": str(initial_stage_capture),
                    "log": str(initial_log_path),
                    "regions": initial_regions,
                    "resource_regions": initial_resource_regions,
                    "presentation_oracle": "android_compositor_capture_target_pixels",
                    "marker": stable,
                }
            )
            for stage in lifecycle["stages"]:
                if stage["name"] == "initial":
                    continue
                before_pid = first_pid
                baseline_marker = next(reversed(lifecycle_markers.values()), stable)
                _run(args.adb, args.serial, "logcat", "-c")
                if stage["action"] == "background_resume":
                    _run(args.adb, args.serial, "shell", "input", "keyevent", "KEYCODE_HOME")
                    resume_deadline = min(deadline, time.monotonic() + 10)
                    while time.monotonic() < resume_deadline:
                        current_focus = _run(
                            args.adb, args.serial, "shell", "dumpsys", "window", "windows", required=False
                        )
                        if package_id not in current_focus:
                            break
                        time.sleep(0.25)
                    _run(
                        args.adb, args.serial, "shell", "am", "start", "-W", "-n", component,
                        "--es", "stasis.seam_test_id", test_id,
                    )
                elif stage["action"] == "force_activity_restart":
                    _run(
                        args.adb, args.serial, "shell", "am", "start", "-S", "-W", "-n", component,
                        "--es", "stasis.seam_test_id", test_id,
                    )
                else:
                    raise SeamError(f"unknown Android resource lifecycle action {stage['action']}")
                stage_deadline = min(deadline, time.monotonic() + 15)
                stage_log = ""
                stage_markers_list: list[dict] = []
                stage_pid = ""
                marker = None
                while time.monotonic() < stage_deadline:
                    stage_log = _run(
                        args.adb, args.serial, "logcat", "-d", "-v", "brief", "Stasis:I", "*:S"
                    )
                    stage_markers_list = parse_markers(stage_log, test_id)
                    stage_pid = _run(
                        args.adb, args.serial, "shell", "pidof", package_id, required=False
                    ).strip()
                    if stage["action"] != "force_activity_restart" or (
                        stage_pid and stage_pid != before_pid
                    ):
                        try:
                            marker = select_post_transition_marker(
                                stage_markers_list, baseline_marker, stage["action"]
                            )
                        except SeamError:
                            marker = None
                        if marker is not None:
                            break
                    time.sleep(0.25)
                if marker is None:
                    raise SeamError(
                        f"Android resource lifecycle stage {stage['name']} did not reach a new ready marker"
                    )
                validate_resource_diagnostics(stage_log, expectations)
                stage_log_path = args.output / f"{stage['name']}-logcat.txt"
                stage_log_path.write_text(stage_log, encoding="utf-8")
                log_history.append(stage_log)
                markers = parse_markers("\n".join(log_history), test_id)
                if not stage_pid:
                    raise SeamError(f"Android resource lifecycle stage {stage['name']} exited")
                if stage.get("same_pid") is True and stage_pid != before_pid:
                    raise SeamError(f"Android resource lifecycle stage {stage['name']} changed PID: {before_pid} -> {stage_pid}")
                if stage.get("same_pid") is False and stage_pid == before_pid:
                    raise SeamError(f"Android resource lifecycle stage {stage['name']} reused PID {stage_pid}")
                stage_capture = args.output / f"{stage['name']}-resource-frame.png"
                stage_regions, stage_resource_regions = capture_until_resource_regions_match(
                    args.adb, args.serial, stage_capture, expectations,
                    min(deadline, time.monotonic() + 10), package_id, component,
                )
                if stage.get("same_pid") is True and marker.get("renderer_generation", 0) < baseline_marker.get("renderer_generation", 0):
                    raise SeamError(f"Android resource lifecycle stage {stage['name']} regressed renderer generation")
                if stage.get("same_pid") is True and marker.get("restore_reason") in (1, 2, 3) and marker.get("renderer_generation", 0) <= baseline_marker.get("renderer_generation", 0):
                    raise SeamError(f"Android resource lifecycle stage {stage['name']} reset without advancing renderer generation")
                lifecycle_markers[stage["name"]] = marker
                lifecycle_evidence.append(
                    {
                        "name": stage["name"],
                        "action": stage["action"],
                        "pid": stage_pid,
                        "previous_pid": before_pid,
                        "process_epoch": stage_pid,
                        "same_pid": stage_pid == before_pid,
                        "capture": str(stage_capture),
                        "log": str(stage_log_path),
                        "regions": stage_regions,
                        "resource_regions": stage_resource_regions,
                        "presentation_oracle": "android_compositor_capture_target_pixels",
                        "marker": marker,
                    }
                )
                first_pid = stage_pid
            validate_resource_lifecycle_markers(markers, expectations, lifecycle_markers)
            evidence["resource_lifecycle"] = lifecycle_evidence
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
        if lifecycle:
            log = "\n".join(log_history)
        log_path.write_text(log, encoding="utf-8")
        validate_resource_diagnostics(log, expectations)
        if test_id == "IT-023":
            regions = []
            resource_regions = []
        elif expectations.get("resource_regions"):
            regions, resource_regions = capture_until_resource_regions_match(
                args.adb,
                args.serial,
                capture_path,
                expectations,
                min(deadline, time.monotonic() + 10),
                package_id,
                component,
            )
        else:
            regions = capture_until_regions_match(
                args.adb,
                args.serial,
                capture_path,
                expectations,
                min(deadline, time.monotonic() + 10),
                package_id,
                component,
            )
            resource_regions = []
        if lifecycle:
            final_stage_log = _run(
                args.adb, args.serial, "logcat", "-d", "-v", "brief", "Stasis:I", "*:S"
            )
            log_history[-1] = final_stage_log
            if lifecycle_evidence:
                final_stage_log_path = Path(lifecycle_evidence[-1]["log"])
                final_stage_log_path.write_text(final_stage_log, encoding="utf-8")
            log = "\n".join(log_history)
            log_path.write_text(log, encoding="utf-8")
            validate_resource_diagnostics(log, expectations)
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
                "resource_regions": resource_regions,
                "presentation_oracle": (
                    "guest_storage_markers_and_app_private_bytes"
                    if test_id == "IT-023"
                    else "android_compositor_capture_target_pixels"
                ),
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
                should_retain_installed_package(
                    args.retain_installed_package,
                    evidence["status"],
                ),
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
