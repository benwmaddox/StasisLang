#!/usr/bin/env python3
"""Audit an extracted Stasis release bundle and its archive."""

from __future__ import annotations

import argparse
import collections
import hashlib
import json
import pathlib
import sys
import tarfile
import zipfile

try:
    from generate_release_provenance import RUNTIME_DIRS, RUNTIME_FILES
except ModuleNotFoundError:  # pragma: no cover - supports importing as tools.*
    from tools.generate_release_provenance import RUNTIME_DIRS, RUNTIME_FILES


MAX_ARCHIVE_BYTES = 120 * 1024 * 1024
ARCHIVE_REQUIRED_FILES = (
    "README.md",
    "LICENSE",
    "stasis_release_provenance.json",
    "docs/agent_workflow.md",
    "docs/mobile_packaging.md",
    "docs/mobile_packaging_abi.md",
    "docs/mobile_aot_artifacts.md",
    "docs/release_provenance.md",
    "docs/sdl3_migration.md",
    "src/stdlib/stdlib.stasis",
    "mobile/network/include/stasis_network.h",
    "mobile/network/android-arm64/libstasis_network.a",
    "mobile/network/android-x86_64/libstasis_network.a",
    "mobile/network/ios-arm64/libstasis_network.a",
)
REQUIRED_DIRECTORIES = ("docs/knowledge", "mobile/shells") + tuple(
    f"runtime/{directory}" for directory in RUNTIME_DIRS
)
PLATFORM_REQUIRED_FILES = {
    "windows": (
        "stasis.exe",
        "stasis_dynload.dll",
        "stasis_dynload.dll.lib",
        "stasis_graphics.dll",
        "stasis_runner.exe",
        "lld-link.exe",
        "clang-cl.exe",
        "RUST-LLVM-COPYRIGHT.html",
        "LLVM-THIRD-PARTY-NOTICES.txt",
        "THIRD_PARTY_NOTICES.md",
        "tools/windows/stasis-signing.ps1",
        "tools/diagnose_desktop_network.ps1",
        "desktop/network/windows-x86_64/stasis_network.lib",
        "desktop/network/include/stasis_network.h",
    ),
    "linux": (
        "bin/stasis",
        "bin/libstasis_dynload.a",
        "bin/libstasis_graphics.so",
        "bin/stasis_runner",
    ),
    "macos": (
        "bin/stasis",
        "bin/libstasis_dynload.a",
        "bin/libstasis_graphics.dylib",
        "bin/stasis_runner.app/Contents/MacOS/stasis_runner",
    ),
}
RETAINED_LARGE_FILE_RATIONALE = {
    "clang-cl.exe": "Required to compile per-project Windows AOT runtime bridges offline.",
    "lld-link.exe": "Required to link per-project Windows AOT outputs offline.",
    "RUST-LLVM-COPYRIGHT.html": "License notice shipped with the bundled Rust LLVM linker.",
    "LLVM-THIRD-PARTY-NOTICES.txt": "License notices for the bundled LLVM compiler tool.",
}


class BundleAuditError(ValueError):
    """Raised when a release bundle violates its layout contract."""


def required_files(platform: str) -> tuple[str, ...]:
    """Return the sorted file contract for one release platform."""
    if platform not in PLATFORM_REQUIRED_FILES:
        raise BundleAuditError(f"unsupported platform: {platform}")
    return tuple(
        sorted(
            set(ARCHIVE_REQUIRED_FILES)
            | set(f"runtime/{name}" for name in RUNTIME_FILES)
            | set(PLATFORM_REQUIRED_FILES[platform])
        )
    )


def _sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _relative(path: pathlib.Path, root: pathlib.Path) -> str:
    return path.relative_to(root).as_posix()


def _bundle_files(root: pathlib.Path) -> list[pathlib.Path]:
    return sorted(
        (path for path in root.rglob("*") if path.is_file()),
        key=lambda path: _relative(path, root),
    )


def _normalize_archive_name(name: str, root_name: str) -> str:
    parts = [
        part
        for part in pathlib.PurePosixPath(name.replace("\\", "/")).parts
        if part not in ("", ".")
    ]
    if parts and parts[0] == root_name:
        parts = parts[1:]
    return "/".join(parts)


def _archive_entries(
    archive: pathlib.Path, root: pathlib.Path
) -> tuple[dict[str, tuple[int, int | None]], str]:
    """Return member sizes and the archive compression model."""
    entries: dict[str, tuple[int, int | None]] = {}
    if archive.suffix.lower() == ".zip":
        compression = "zip member compressed sizes"
        with zipfile.ZipFile(archive) as source:
            for info in source.infolist():
                if info.is_dir():
                    continue
                name = _normalize_archive_name(info.filename, root.name)
                if name in entries:
                    raise BundleAuditError(f"archive contains duplicate file: {name}")
                entries[name] = (info.file_size, info.compress_size)
        return entries, compression

    if archive.name.lower().endswith((".tar.gz", ".tgz")):
        compression = "tar member sizes; one shared gzip stream has no per-file compressed size"
        with tarfile.open(archive, "r:gz") as source:
            for info in source.getmembers():
                if not info.isfile():
                    continue
                name = _normalize_archive_name(info.name, root.name)
                if name in entries:
                    raise BundleAuditError(f"archive contains duplicate file: {name}")
                entries[name] = (info.size, None)
        return entries, compression

    raise BundleAuditError(f"unsupported archive format: {archive.name}")


def _validate_layout(
    root: pathlib.Path, platform: str, files: list[pathlib.Path]
) -> tuple[str, ...]:
    present = {_relative(path, root) for path in files}
    missing = [name for name in required_files(platform) if name not in present]
    missing_directories = [
        name
        for name in REQUIRED_DIRECTORIES
        if not (root / pathlib.Path(*name.split("/"))).is_dir()
        or not any(
            path.is_file()
            for path in (root / pathlib.Path(*name.split("/"))).rglob("*")
        )
    ]
    if missing or missing_directories:
        details = []
        if missing:
            details.append(f"missing files: {', '.join(missing)}")
        if missing_directories:
            details.append(f"missing directories: {', '.join(missing_directories)}")
        raise BundleAuditError("; ".join(details))
    return tuple(sorted(present))


def _directory_breakdown(
    files: list[pathlib.Path],
    root: pathlib.Path,
    archive_entries: dict[str, tuple[int, int | None]],
) -> list[dict[str, int | None | str]]:
    groups: dict[str, dict[str, int | None | str]] = {}
    for path in files:
        name = _relative(path, root)
        top_level = name.split("/", 1)[0]
        entry = groups.setdefault(
            top_level,
            {
                "path": top_level,
                "file_count": 0,
                "uncompressed_bytes": 0,
                "archive_uncompressed_bytes": 0,
                "compressed_bytes": 0,
                "compressed_bytes_known": True,
            },
        )
        size, compressed = archive_entries.get(name, (path.stat().st_size, None))
        entry["file_count"] = int(entry["file_count"]) + 1
        entry["uncompressed_bytes"] = int(entry["uncompressed_bytes"]) + path.stat().st_size
        entry["archive_uncompressed_bytes"] = int(entry["archive_uncompressed_bytes"]) + size
        if compressed is None:
            entry["compressed_bytes_known"] = False
        else:
            entry["compressed_bytes"] = int(entry["compressed_bytes"]) + compressed
    for entry in groups.values():
        if not entry["compressed_bytes_known"]:
            entry["compressed_bytes"] = None
        del entry["compressed_bytes_known"]
    return sorted(groups.values(), key=lambda entry: str(entry["path"]))


def _duplicate_files(files: list[pathlib.Path], root: pathlib.Path) -> list[dict[str, object]]:
    by_hash: dict[str, list[str]] = collections.defaultdict(list)
    sizes: dict[str, int] = {}
    for path in files:
        digest = _sha256(path)
        by_hash[digest].append(_relative(path, root))
        sizes[digest] = path.stat().st_size
    return [
        {"sha256": digest, "bytes": sizes[digest], "paths": sorted(paths)}
        for digest, paths in sorted(by_hash.items())
        if len(paths) > 1
    ]


def build_report(
    root: pathlib.Path,
    platform: str,
    archive: pathlib.Path | None = None,
    max_archive_bytes: int = MAX_ARCHIVE_BYTES,
) -> dict[str, object]:
    root = root.resolve()
    if not root.is_dir():
        raise BundleAuditError(f"bundle root is not a directory: {root}")
    files = _bundle_files(root)
    present = _validate_layout(root, platform, files)

    archive_entries: dict[str, tuple[int, int | None]] = {}
    compression = "not provided"
    archive_report: dict[str, object] | None = None
    if archive is not None:
        archive = archive.resolve()
        if not archive.is_file():
            raise BundleAuditError(f"archive is not a file: {archive}")
        archive_entries, compression = _archive_entries(archive, root)
        archive_names = set(archive_entries)
        missing_from_archive = sorted(set(present) - archive_names)
        unexpected_in_archive = sorted(archive_names - set(present))
        if missing_from_archive or unexpected_in_archive:
            details = []
            if missing_from_archive:
                details.append(
                    f"files missing from archive: {', '.join(missing_from_archive)}"
                )
            if unexpected_in_archive:
                details.append(
                    f"unexpected archive files: {', '.join(unexpected_in_archive)}"
                )
            raise BundleAuditError("; ".join(details))
        archive_bytes = archive.stat().st_size
        if archive_bytes > max_archive_bytes:
            raise BundleAuditError(
                f"archive is {archive_bytes} bytes, above the {max_archive_bytes}-byte budget"
            )
        archive_report = {
            "path": archive.name,
            "bytes": archive_bytes,
            "sha256": _sha256(archive),
            "file_count": len(archive_entries),
            "compression": compression,
            "budget_bytes": max_archive_bytes,
        }

    file_report = []
    required = set(required_files(platform))
    for path in files:
        name = _relative(path, root)
        archive_size = archive_entries.get(name, (path.stat().st_size, None))[1]
        file_report.append(
            {
                "path": name,
                "required_by_contract": name in required,
                "uncompressed_bytes": path.stat().st_size,
                "compressed_bytes": archive_size,
                "sha256": _sha256(path),
            }
        )
    file_report.sort(
        key=lambda entry: (-int(entry["uncompressed_bytes"]), str(entry["path"]))
    )

    retained_large = []
    for entry in file_report:
        name = pathlib.PurePosixPath(str(entry["path"])).name
        rationale = RETAINED_LARGE_FILE_RATIONALE.get(name)
        if rationale:
            retained_large.append(
                {
                    "path": entry["path"],
                    "uncompressed_bytes": entry["uncompressed_bytes"],
                    "compressed_bytes": entry["compressed_bytes"],
                    "rationale": rationale,
                }
            )

    return {
        "schema": "stasis.release_bundle_size.v1",
        "platform": platform,
        "bundle": {
            "file_count": len(files),
            "uncompressed_bytes": sum(path.stat().st_size for path in files),
        },
        "archive": archive_report,
        "size_breakdown": {
            "compression": compression,
            "directories": _directory_breakdown(files, root, archive_entries),
            "files": file_report,
        },
        "duplicate_files": _duplicate_files(files, root),
        "retained_large_files": retained_large,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", required=True, type=pathlib.Path)
    parser.add_argument("--platform", required=True, choices=sorted(PLATFORM_REQUIRED_FILES))
    parser.add_argument("--archive", type=pathlib.Path)
    parser.add_argument("--output", type=pathlib.Path)
    parser.add_argument("--max-archive-bytes", type=int, default=MAX_ARCHIVE_BYTES)
    args = parser.parse_args(argv)
    try:
        report = build_report(
            args.root,
            args.platform,
            archive=args.archive,
            max_archive_bytes=args.max_archive_bytes,
        )
    except BundleAuditError as error:
        print(f"release bundle audit failed: {error}", file=sys.stderr)
        return 1
    serialized = json.dumps(report, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(serialized, encoding="utf-8")
    print(serialized, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
