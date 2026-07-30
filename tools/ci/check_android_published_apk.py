#!/usr/bin/env python3
"""Validate that an Android published APK is runtime-only and AOT-backed."""

from __future__ import annotations

import argparse
import json
import zipfile
from pathlib import Path


MAX_APK_BYTES = 50 * 1024 * 1024
BASE_REQUIRED_ENTRIES = {
    "AndroidManifest.xml",
    "classes.dex",
    "assets/stasis_game/assets/manifest.json",
}
FORBIDDEN_SUFFIXES = {
    "libstasis_android_bridge.so",
    ".stasis",
    ".test.stasis",
    ".stub",
}


def validate(apk: Path, abi: str = "arm64-v8a", required_asset: str = "assets/ball.svg") -> dict[str, object]:
    if not apk.is_file():
        raise ValueError(f"published APK was not found: {apk}")
    if apk.stat().st_size > MAX_APK_BYTES:
        raise ValueError(f"published APK exceeds {MAX_APK_BYTES} bytes: {apk.stat().st_size}")
    with zipfile.ZipFile(apk) as archive:
        entries = set(archive.namelist())
        required_entries = set(BASE_REQUIRED_ENTRIES)
        required_entries.add(f"lib/{abi}/libstasis_mobile_smoke.so")
        if required_asset:
            required_entries.add(f"assets/stasis_game/{required_asset}")
        missing = sorted(required_entries - entries)
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
        wrong_abis = [entry for entry in native_libraries if not entry.startswith(f"lib/{abi}/")]
        if wrong_abis:
            raise ValueError(f"published APK contains unsupported ABIs: {wrong_abis}")
    return {
        "apk": str(apk.resolve()),
        "bytes": apk.stat().st_size,
        "entry_count": len(entries),
        "native_libraries": native_libraries,
        "abi": abi,
        "runtime_only": True,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("apk", type=Path)
    parser.add_argument("--abi", default="arm64-v8a")
    parser.add_argument("--required-asset", default="assets/ball.svg")
    args = parser.parse_args()
    try:
        summary = validate(args.apk, args.abi, args.required_asset)
    except (OSError, ValueError, zipfile.BadZipFile) as error:
        print(f"published APK validation failed: {error}", file=sys.stderr)
        return 1
    print(json.dumps(summary, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
