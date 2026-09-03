#!/usr/bin/env python3
"""Run Windows platform seam suites with bounded, retained case logs."""

from __future__ import annotations

import argparse
import os
import queue
import signal
import subprocess
import sys
import threading
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence


CASE_TIMEOUT_SECONDS = 900
SUITES = {
    "DesktopSdl": (
        "desktop_input_frame_seam",
        "desktop_display_metrics_seam",
        "desktop_manifest_assets_seam",
        "desktop_asset_load_stress",
        "desktop_render_recovery_seam",
        "desktop_hot_swap_generation_seam",
    ),
    "MobileRuntime": (
        "generated_mobile_aot_runtime_seam",
        "mobile_packaged_assets_seam",
    ),
}


@dataclass(frozen=True)
class CaseResult:
    exit_code: int
    timed_out: bool


def _terminate_process_tree(process: subprocess.Popen[str]) -> None:
    if process.poll() is not None:
        return
    if os.name == "nt":
        termination = subprocess.run(
            ["taskkill", "/PID", str(process.pid), "/T", "/F"],
            check=False,
            capture_output=True,
            text=True,
        )
        if termination.returncode != 0 and process.poll() is None:
            process.kill()
    else:
        os.killpg(os.getpgid(process.pid), signal.SIGKILL)
    try:
        process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=10)


def run_command(
    command: Sequence[str], log_path: Path, timeout_seconds: float
) -> CaseResult:
    log_path.parent.mkdir(parents=True, exist_ok=True)
    popen_options: dict[str, object] = {}
    if os.name == "nt":
        popen_options["creationflags"] = subprocess.CREATE_NEW_PROCESS_GROUP
    else:
        popen_options["start_new_session"] = True
    process = subprocess.Popen(
        command,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
        **popen_options,
    )
    assert process.stdout is not None
    output: queue.Queue[str | None] = queue.Queue()

    def read_output() -> None:
        try:
            for line in process.stdout:
                output.put(line)
        finally:
            output.put(None)

    reader = threading.Thread(target=read_output, daemon=True)
    reader.start()
    deadline = time.monotonic() + timeout_seconds
    drain_deadline: float | None = None
    timed_out = False
    reader_done = False

    with log_path.open("w", encoding="utf-8", newline="") as log:
        while not reader_done or process.poll() is None:
            remaining = deadline - time.monotonic()
            if not timed_out and remaining <= 0 and process.poll() is None:
                timed_out = True
                _terminate_process_tree(process)
                drain_deadline = time.monotonic() + 2
            elif process.poll() is not None and drain_deadline is None:
                drain_deadline = time.monotonic() + 2
            if drain_deadline is not None and time.monotonic() >= drain_deadline:
                break
            try:
                line = output.get(timeout=0.1)
            except queue.Empty:
                continue
            if line is None:
                reader_done = True
                continue
            sys.stdout.write(line)
            sys.stdout.flush()
            log.write(line)
            log.flush()

    reader.join(timeout=10)
    if reader.is_alive():
        raise RuntimeError(f"output reader did not stop for PID {process.pid}")
    process.stdout.close()
    return CaseResult(exit_code=process.wait(timeout=10), timed_out=timed_out)


def _windows_lingering_pids(target: str) -> list[int]:
    if os.name != "nt":
        return []
    prefix = f"{target}-"
    script = (
        f"$prefix = '{prefix}'; "
        "Get-Process | ForEach-Object { try { "
        "$base = [System.IO.Path]::GetFileNameWithoutExtension($_.Path); "
        "if ($base.StartsWith($prefix) -and "
        "$_.Path -like '*\\build\\codex-cargo-target\\*') { $_.Id } "
        "} catch {} }"
    )
    result = subprocess.run(
        ["powershell.exe", "-NoProfile", "-Command", script],
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"could not inspect lingering {target} processes: {result.stderr.strip()}"
        )
    return [int(line) for line in result.stdout.splitlines() if line.strip()]


def remove_lingering_case_processes(target: str) -> list[str]:
    failures = []
    for pid in _windows_lingering_pids(target):
        message = f"{target} left process {pid} running"
        print(f"::error::{message}; terminating it.")
        result = subprocess.run(
            ["taskkill", "/PID", str(pid), "/T", "/F"],
            check=False,
            capture_output=True,
            text=True,
        )
        failures.append(message)
        if result.returncode != 0:
            failures.append(
                f"could not terminate {target} process {pid}: {result.stderr.strip()}"
            )
    return failures


def cargo_command(root: Path, target: str) -> list[str]:
    return [
        sys.executable,
        str(root / "tools/cargo_cache.py"),
        "run",
        "--",
        "cargo",
        "test",
        "-p",
        "stasis",
        "--test",
        target,
        "--",
        "--test-threads=1",
        "--nocapture",
    ]


def run_suite(root: Path, suite: str) -> int:
    failures = []
    log_dir = root / "target/windows-platform-seams" / suite
    for target in SUITES[suite]:
        log_path = log_dir / f"{target}.log"
        print(f"::group::{suite} - {target}")
        try:
            result = run_command(
                cargo_command(root, target), log_path, CASE_TIMEOUT_SECONDS
            )
            if result.timed_out:
                failures.append(
                    f"{target} timed out after {CASE_TIMEOUT_SECONDS} seconds "
                    f"(log: {log_path})"
                )
            elif result.exit_code != 0:
                failures.append(
                    f"{target} exited with code {result.exit_code} (log: {log_path})"
                )
        except (OSError, RuntimeError, subprocess.SubprocessError) as error:
            failures.append(f"{target} could not run: {error} (log: {log_path})")
        finally:
            try:
                failures.extend(remove_lingering_case_processes(target))
            except RuntimeError as error:
                failures.append(str(error))
            print("::endgroup::")

    if failures:
        print(f"::error::{suite} seam suite failed:", file=sys.stderr)
        for failure in failures:
            print(f" - {failure}", file=sys.stderr)
        return 1
    print(f"{suite} seam suite passed ({len(SUITES[suite])} cases).")
    return 0


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--suite", choices=tuple(SUITES), required=True)
    args = parser.parse_args(argv)
    root = Path(__file__).resolve().parents[2]
    return run_suite(root, args.suite)


if __name__ == "__main__":
    raise SystemExit(main())
