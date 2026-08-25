#!/usr/bin/env python3
"""Capture the visible Android frame and bind it to current Stasis generations."""

from __future__ import annotations

import argparse
import re
import subprocess
from pathlib import Path


STAGES = {
    "initial_launch",
    "second_frame",
    "resize_or_density_change",
    "resource_restore",
}


def adb(*args: str, text: bool = True) -> subprocess.CompletedProcess:
    return subprocess.run(["adb", *args], check=True, capture_output=True, text=text)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--stage", choices=sorted(STAGES), required=True)
    parser.add_argument("--capture", type=Path, required=True)
    parser.add_argument("--runtime-log", type=Path, required=True)
    args = parser.parse_args()

    args.capture.parent.mkdir(parents=True, exist_ok=True)
    args.runtime_log.parent.mkdir(parents=True, exist_ok=True)
    remote = f"/sdcard/stasis-render-parity-{args.stage}.png"
    adb("shell", "screencap", "-p", remote)
    adb("pull", remote, str(args.capture))
    log = adb("logcat", "-d", "-v", "brief", "Stasis:I", "*:S").stdout
    events = re.findall(
        r"Stasis renderer resources restored: backend=(\w+) surface_generation=(\d+) "
        r"renderer_generation=(\d+) reason=(\w+) sprites=(\d+)",
        log,
    )
    if not events:
        raise SystemExit("no Stasis resource restoration event is available")
    backend, surface, renderer, _, _ = events[-1]
    producer_event = (
        f"Stasis parity capture: stage={args.stage} path={args.capture.resolve()} frame=0 "
        f"backend={backend} surface_generation={surface} renderer_generation={renderer}"
    )
    args.runtime_log.write_text(log.rstrip() + "\n" + producer_event + "\n", encoding="utf-8")
    print(f"captured Android render parity stage {args.stage}: {args.capture}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
