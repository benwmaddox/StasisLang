#!/usr/bin/env python3
"""Validate a generic Stasis Android release APK or app bundle."""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
import zipfile
from pathlib import Path


MAX_PACKAGE_BYTES = 150 * 1024 * 1024
MAX_MANIFEST_BYTES = 1024 * 1024
MAX_MANIFEST_ASSETS = 4096
REQUIRED_NATIVE_LIBRARIES = {"libSDL2.so", "libSDL2_image.so", "libmain.so"}
FORBIDDEN_SUFFIXES = {
    "libstasis_android_bridge.so",
    "libstasis_codex_android.so",
    ".stasis",
    ".test.stasis",
    ".stub",
}


def _manifest_assets(archive: zipfile.ZipFile, prefix: str) -> list[dict[str, object]]:
    manifest_entry = f"{prefix}assets/stasis_game/assets/manifest.json"
    info = archive.getinfo(manifest_entry)
    if info.file_size > MAX_MANIFEST_BYTES:
        raise ValueError("release asset manifest exceeds the byte limit")
    try:
        manifest = json.loads(archive.read(info))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValueError("release asset manifest is invalid JSON") from error
    if manifest.get("schema") != "stasis-assets" or manifest.get("version") != 1:
        raise ValueError("release asset manifest has an unsupported schema")
    assets = manifest.get("assets")
    if not isinstance(assets, list) or len(assets) > MAX_MANIFEST_ASSETS:
        raise ValueError("release asset manifest has an invalid asset list")
    return assets


def _verify_asset_hashes(
    archive: zipfile.ZipFile, entries: set[str], prefix: str
) -> int:
    ids: set[str] = set()
    paths: set[str] = set()
    assets = _manifest_assets(archive, prefix)
    for asset in assets:
        if not isinstance(asset, dict):
            raise ValueError("release asset manifest contains an invalid entry")
        asset_id = asset.get("id")
        path = asset.get("path")
        expected = asset.get("content_sha256")
        if not isinstance(asset_id, str) or not asset_id or asset_id in ids:
            raise ValueError("release asset manifest contains an invalid or duplicate id")
        if (
            not isinstance(path, str)
            or not path.startswith("assets/")
            or path.endswith("/")
            or "\\" in path
            or "//" in path
            or any(part in {"", ".", ".."} for part in path.split("/"))
            or path in paths
        ):
            raise ValueError("release asset manifest contains an unsafe or duplicate path")
        if not isinstance(expected, str) or len(expected) != 64 or any(
            char not in "0123456789abcdef" for char in expected
        ):
            raise ValueError("release asset manifest contains an invalid SHA-256 value")
        entry = f"{prefix}assets/stasis_game/{path}"
        if entry not in entries:
            raise ValueError(f"release package is missing declared asset: {path}")
        actual = hashlib.sha256(archive.read(entry)).hexdigest()
        if actual != expected:
            raise ValueError(f"release package asset hash mismatch: {path}")
        ids.add(asset_id)
        paths.add(path)
    return len(assets)


def validate(
    package: Path,
    abi: str = "arm64-v8a",
    required_asset: str = "assets/ball.svg",
) -> dict[str, object]:
    if not package.is_file():
        raise ValueError(f"release package was not found: {package}")
    if package.stat().st_size > MAX_PACKAGE_BYTES:
        raise ValueError(
            f"release package exceeds {MAX_PACKAGE_BYTES} bytes: {package.stat().st_size}"
        )
    is_bundle = package.suffix.lower() == ".aab"
    prefix = "base/" if is_bundle else ""
    with zipfile.ZipFile(package) as archive:
        entries = set(archive.namelist())
        required_entries = {
            f"{prefix}manifest/AndroidManifest.xml" if is_bundle else "AndroidManifest.xml",
            f"{prefix}assets/stasis_game/assets/manifest.json",
            *(f"{prefix}lib/{abi}/{library}" for library in REQUIRED_NATIVE_LIBRARIES),
        }
        if required_asset:
            required_entries.add(f"{prefix}assets/stasis_game/{required_asset}")
        missing = sorted(required_entries - entries)
        if missing:
            raise ValueError(f"release package is missing required entries: {missing}")
        forbidden = sorted(
            entry
            for entry in entries
            if any(entry.endswith(suffix) for suffix in FORBIDDEN_SUFFIXES)
            or ("/assets/" in entry and "/build/" in entry)
        )
        if forbidden:
            raise ValueError(f"release package contains development files: {forbidden[:10]}")
        native_prefix = f"{prefix}lib/"
        native_libraries = sorted(entry for entry in entries if entry.startswith(native_prefix))
        wrong_abis = [
            entry for entry in native_libraries if not entry.startswith(f"{native_prefix}{abi}/")
        ]
        if wrong_abis:
            raise ValueError(f"release package contains unsupported ABIs: {wrong_abis}")
        verified_asset_count = _verify_asset_hashes(archive, entries, prefix)
    return {
        "package": str(package.resolve()),
        "format": "aab" if is_bundle else "apk",
        "bytes": package.stat().st_size,
        "entry_count": len(entries),
        "native_libraries": native_libraries,
        "abi": abi,
        "runtime_only": True,
        "verified_asset_count": verified_asset_count,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("package", type=Path)
    parser.add_argument("--abi", default="arm64-v8a")
    parser.add_argument("--required-asset", default="assets/ball.svg")
    args = parser.parse_args()
    try:
        summary = validate(args.package, args.abi, args.required_asset)
    except (OSError, ValueError, zipfile.BadZipFile) as error:
        print(f"release package validation failed: {error}", file=sys.stderr)
        return 1
    print(json.dumps(summary, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
