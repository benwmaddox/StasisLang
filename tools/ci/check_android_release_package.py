#!/usr/bin/env python3
"""Validate a generic Stasis Android release APK or app bundle."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path


MAX_PACKAGE_BYTES = 150 * 1024 * 1024
MAX_MANIFEST_BYTES = 1024 * 1024
MAX_MANIFEST_ASSETS = 4096
REQUIRED_NATIVE_LIBRARIES = {"libmain.so"}
NETWORK_LIBRARY = "libstasis_network_v1.so"
ANDROID_PAGE_SIZE = 16 * 1024
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


def _zip_data_offset(package: Path, info: zipfile.ZipInfo) -> int:
    with package.open("rb") as source:
        source.seek(info.header_offset)
        header = source.read(30)
    if len(header) != 30 or header[:4] != b"PK\x03\x04":
        raise ValueError(f"release package has an invalid local header: {info.filename}")
    name_length, extra_length = struct.unpack_from("<HH", header, 26)
    return info.header_offset + 30 + name_length + extra_length


def _run_readelf(readelf: str, library: Path, option: str) -> str:
    try:
        result = subprocess.run(
            [readelf, option, str(library)],
            capture_output=True,
            text=True,
            check=False,
            timeout=60,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise ValueError(f"Android network ELF inspection failed: {error}") from error
    if result.returncode != 0:
        raise ValueError(
            "Android network ELF inspection failed: "
            + (result.stdout + result.stderr).strip()[-1000:]
        )
    return result.stdout


def _verify_network_library(
    archive: zipfile.ZipFile,
    info: zipfile.ZipInfo,
    abi: str,
    readelf: str,
) -> dict[str, object]:
    expected_machine = {
        "arm64-v8a": "AArch64",
        "x86_64": "Advanced Micro Devices X86-64",
    }.get(abi)
    if expected_machine is None:
        raise ValueError(f"network-enabled release package uses an unsupported ABI: {abi}")
    with tempfile.TemporaryDirectory() as directory:
        library = Path(directory) / NETWORK_LIBRARY
        library.write_bytes(archive.read(info))
        header = _run_readelf(readelf, library, "-h")
        machine = re.search(r"^\s*Machine:\s*(.+?)\s*$", header, re.MULTILINE)
        if machine is None or machine.group(1) != expected_machine:
            actual = machine.group(1) if machine else "missing"
            raise ValueError(
                f"Android network ELF machine differs from ABI {abi}: {actual}"
            )
        dynamic = _run_readelf(readelf, library, "-d")
        soname = re.search(r"\(SONAME\).*?\[([^]]+)\]", dynamic)
        if soname is None or soname.group(1) != NETWORK_LIBRARY:
            actual = soname.group(1) if soname else "missing"
            raise ValueError(
                f"Android network ELF SONAME must be {NETWORK_LIBRARY}, found {actual}"
            )
        program_headers = _run_readelf(readelf, library, "-lW")
        loads: list[tuple[int, int, int]] = []
        for line in program_headers.splitlines():
            fields = line.split()
            if not fields or fields[0] != "LOAD" or len(fields) < 7:
                continue
            try:
                offset = int(fields[1], 16)
                virtual_address = int(fields[2], 16)
                alignment = int(fields[-1], 16)
            except ValueError as error:
                raise ValueError("Android network ELF has malformed PT_LOAD headers") from error
            loads.append((offset, virtual_address, alignment))
        if not loads or any(
            alignment < ANDROID_PAGE_SIZE
            or (virtual_address - offset) % ANDROID_PAGE_SIZE != 0
            for offset, virtual_address, alignment in loads
        ):
            raise ValueError(
                "Android network ELF PT_LOAD segments are not 16 KiB page aligned"
            )
    return {
        "path": info.filename,
        "soname": NETWORK_LIBRARY,
        "machine": expected_machine,
        "pt_load_count": len(loads),
    }


def validate(
    package: Path,
    abi: str = "arm64-v8a",
    required_asset: str = "assets/ball.svg",
    network_enabled: bool = False,
    readelf: str | None = None,
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
        expected_libraries = set(REQUIRED_NATIVE_LIBRARIES)
        if network_enabled:
            expected_libraries.add(NETWORK_LIBRARY)
        required_entries = {
            f"{prefix}manifest/AndroidManifest.xml" if is_bundle else "AndroidManifest.xml",
            f"{prefix}assets/stasis_game/assets/manifest.json",
            *(f"{prefix}lib/{abi}/{library}" for library in expected_libraries),
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
        expected_native_entries = {
            f"{native_prefix}{abi}/{library}" for library in expected_libraries
        }
        unexpected_libraries = sorted(set(native_libraries) - expected_native_entries)
        if unexpected_libraries:
            raise ValueError(
                f"release package contains unexpected native libraries: {unexpected_libraries}"
            )
        network_summary = None
        if network_enabled:
            network_entry = f"{native_prefix}{abi}/{NETWORK_LIBRARY}"
            network_info = archive.getinfo(network_entry)
            if not is_bundle:
                if network_info.compress_type != zipfile.ZIP_STORED:
                    raise ValueError("Android network shared object must be uncompressed in APK")
                data_offset = _zip_data_offset(package, network_info)
                if data_offset % ANDROID_PAGE_SIZE != 0:
                    raise ValueError(
                        "Android network shared object is not 16 KiB aligned in APK"
                    )
            resolved_readelf = readelf or shutil.which("llvm-readelf") or shutil.which("readelf")
            if not resolved_readelf:
                raise ValueError(
                    "network-enabled package audit requires llvm-readelf or readelf"
                )
            network_summary = _verify_network_library(
                archive, network_info, abi, resolved_readelf
            )
        verified_asset_count = _verify_asset_hashes(archive, entries, prefix)
    return {
        "package": str(package.resolve()),
        "format": "aab" if is_bundle else "apk",
        "bytes": package.stat().st_size,
        "entry_count": len(entries),
        "native_libraries": native_libraries,
        "abi": abi,
        "runtime_only": True,
        "network_enabled": network_enabled,
        "network_library": network_summary,
        "verified_asset_count": verified_asset_count,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("package", type=Path)
    parser.add_argument("--abi", default="arm64-v8a")
    parser.add_argument("--required-asset", default="assets/ball.svg")
    parser.add_argument("--network-enabled", action="store_true")
    parser.add_argument("--readelf")
    args = parser.parse_args()
    try:
        summary = validate(
            args.package,
            args.abi,
            args.required_asset,
            args.network_enabled,
            args.readelf,
        )
    except (OSError, ValueError, zipfile.BadZipFile) as error:
        print(f"release package validation failed: {error}", file=sys.stderr)
        return 1
    print(json.dumps(summary, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
