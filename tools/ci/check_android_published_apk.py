#!/usr/bin/env python3
"""Validate that an Android published APK is runtime-only and arm64 AOT-backed."""

from __future__ import annotations

import json
import sys
import zipfile
from pathlib import Path


MAX_APK_BYTES = 50 * 1024 * 1024
REQUIRED_ENTRIES = {
    "AndroidManifest.xml",
    "classes.dex",
    "lib/arm64-v8a/libstasis_mobile_smoke.so",
    "assets/stasis_game/assets/manifest.json",
    "assets/stasis_game/assets/ball.svg",
}
FORBIDDEN_SUFFIXES = {
    "libstasis_android_bridge.so",
    ".stasis",
    ".test.stasis",
    ".stub",
}


def validate(apk: Path) -> dict[str, object]:
    if not apk.is_file():
        raise ValueError(f"published APK was not found: {apk}")
    if apk.stat().st_size > MAX_APK_BYTES:
        raise ValueError(f"published APK exceeds {MAX_APK_BYTES} bytes: {apk.stat().st_size}")
    with zipfile.ZipFile(apk) as archive:
        entries = set(archive.namelist())
        missing = sorted(REQUIRED_ENTRIES - entries)
        if missing:
            raise ValueError(f"published APK is missing required entries: {missing}")
        forbidden = sorted(
            entry
            for entry in entries
            if any(entry.endswith(suffix) for suffix in FORBIDDEN_SUFFIXES)
            or (entry.startswith("assets/") and "/build/" in entry)
        )
        if forbidden:
            raise ValueError(f"published APK contains workshop-only files: {forbidden[:10]}")
        native_libraries = sorted(entry for entry in entries if entry.startswith("lib/"))
        wrong_abis = [entry for entry in native_libraries if not entry.startswith("lib/arm64-v8a/")]
        if wrong_abis:
            raise ValueError(f"published APK contains unsupported ABIs: {wrong_abis}")
    return {
        "apk": str(apk.resolve()),
        "bytes": apk.stat().st_size,
        "entry_count": len(entries),
        "native_libraries": native_libraries,
        "runtime_only": True,
    }


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: check_android_published_apk.py <apk>", file=sys.stderr)
        return 2
    try:
        summary = validate(Path(sys.argv[1]))
    except (OSError, ValueError, zipfile.BadZipFile) as error:
        print(f"published APK validation failed: {error}", file=sys.stderr)
        return 1
    print(json.dumps(summary, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
