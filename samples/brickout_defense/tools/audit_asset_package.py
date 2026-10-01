#!/usr/bin/env python3
"""Audit that a packaged build contains exactly its intended original assets."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path

import build_asset_manifest


SAMPLE_ROOT = Path(__file__).resolve().parents[1]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "asset_root",
        type=Path,
        help="path to the packaged assets directory that contains manifest.json",
    )
    args = parser.parse_args()

    catalog = build_asset_manifest.read_json(build_asset_manifest.CATALOG_PATH)
    lineage = build_asset_manifest.read_json(build_asset_manifest.LINEAGE_PATH)
    source_manifest = build_asset_manifest.build_manifest(catalog, lineage)
    source_entries = {entry["path"]: entry for entry in source_manifest["assets"]}
    expected_paths = {
        *(row["path"] for row in catalog["assets"]),
        "assets/original/estudio.ttf",
        *(f"assets/original/audio/{name}" for name, _, _, _ in build_asset_manifest.AUDIO_METADATA),
    }

    asset_root = args.asset_root.resolve()
    package_manifest_path = asset_root / "manifest.json"
    package_manifest = json.loads(package_manifest_path.read_text(encoding="utf-8"))
    packaged_entries = {entry["path"]: entry for entry in package_manifest["assets"]}
    packaged_paths = set(packaged_entries)
    if packaged_paths != expected_paths:
        missing = sorted(expected_paths - packaged_paths)
        extra = sorted(packaged_paths - expected_paths)
        raise SystemExit(f"package asset paths differ: missing={missing[:8]} extra={extra[:8]}")

    for path, entry in packaged_entries.items():
        expected = source_entries[path]
        if entry["content_sha256"] != expected["content_sha256"] or entry["format"] != expected["format"]:
            raise SystemExit(f"package manifest metadata differs from source: {path}")
        file_path = (asset_root.parent / Path(path)).resolve()
        if not file_path.is_relative_to(asset_root.parent.resolve()) or not file_path.is_file():
            raise SystemExit(f"packaged asset is missing or escaped its root: {path}")
        digest = hashlib.sha256(file_path.read_bytes()).hexdigest()
        if digest != entry["content_sha256"]:
            raise SystemExit(f"packaged asset hash differs from manifest: {path}")

    actual_files = {
        path.relative_to(asset_root.parent).as_posix()
        for path in asset_root.rglob("*")
        if path.is_file() and path != package_manifest_path
    }
    if actual_files != expected_paths:
        extra = sorted(actual_files - expected_paths)
        missing = sorted(expected_paths - actual_files)
        raise SystemExit(f"package asset files differ: missing={missing[:8]} extra={extra[:8]}")

    kinds: dict[str, int] = {}
    for entry in packaged_entries.values():
        kind = entry["format"]["kind"]
        kinds[kind] = kinds.get(kind, 0) + 1
    print(
        json.dumps(
            {
                "asset_count": len(packaged_entries),
                "counts_by_kind": kinds,
                "exact_source_paths": True,
                "hashes_match": True,
                "manifest_version": package_manifest["version"],
            },
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
