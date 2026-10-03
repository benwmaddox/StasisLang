#!/usr/bin/env python3
"""Generate and verify the explicit package manifest for recovered game assets."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import sys


SAMPLE_ROOT = Path(__file__).resolve().parents[1]
ASSETS_ROOT = SAMPLE_ROOT / "assets"
SOURCE_ROOT = ASSETS_ROOT / "original"
CATALOG_PATH = ASSETS_ROOT / "asset_catalog.json"
LINEAGE_PATH = SOURCE_ROOT / "asset_lineage.json"
MANIFEST_PATH = ASSETS_ROOT / "manifest.json"

FONT_PATHS = (
    "assets/original/estudio.ttf",
    "assets/original/galaxy.ttf",
    "assets/original/system.ttf",
)
AUDIO_METADATA = (
    ("background_music.mp3", 44_100, 2, 15_428_298),
    ("beep.mp3", 44_100, 2, 4_606),
    ("curve_collision.mp3", 44_100, 1, 10_366),
    ("flixel.mp3", 44_100, 2, 109_440),
    ("lost_life.mp3", 44_100, 2, 40_318),
    ("rocket_explosion.mp3", 44_100, 2, 9_214),
)


def read_json(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def build_manifest(catalog: dict, lineage: dict) -> dict:
    lineage_rows = {row["path"]: row for row in lineage["assets"]}
    entries: list[dict] = []

    for row in catalog["assets"]:
        extension = Path(row["path"]).suffix.lower()
        encodings = {".png": "png", ".svg": "svg"}
        try:
            encoding = encodings[extension]
        except KeyError as error:
            raise ValueError(f"unsupported catalog image encoding: {row['path']}") from error
        entries.append(
            {
                "id": f"brickout-image-{row['id']:03d}",
                "path": row["path"],
                "content_sha256": row["sha256"],
                "format": {
                    "kind": "sprite",
                    "encoding": encoding,
                    "width": row["width"],
                    "height": row["height"],
                },
                "dependencies": [],
            }
        )

    for path in FONT_PATHS:
        source = path.removeprefix("assets/original/")
        row = lineage_rows.get(source)
        if row is None:
            raise ValueError(f"font is absent from source lineage: {path}")
        entries.append(
            {
                "id": f"brickout-font-{Path(path).stem}",
                "path": path,
                "content_sha256": row["sha256"],
                "format": {"kind": "font", "encoding": "ttf"},
                "dependencies": [],
            }
        )

    for name, sample_rate, channels, duration_frames in AUDIO_METADATA:
        path = f"assets/original/audio/{name}"
        source = path.removeprefix("assets/original/")
        row = lineage_rows.get(source)
        if row is None:
            raise ValueError(f"audio asset is absent from source lineage: {path}")
        entries.append(
            {
                "id": f"brickout-audio-{Path(name).stem}",
                "path": path,
                "content_sha256": row["sha256"],
                "format": {
                    "kind": "audio",
                    "encoding": "mp3",
                    "sample_rate": sample_rate,
                    "channels": channels,
                    "duration_frames": duration_frames,
                },
                "dependencies": [],
            }
        )

    entries.sort(key=lambda entry: entry["id"])
    paths = [entry["path"] for entry in entries]
    if len(paths) != len(set(paths)):
        raise ValueError("asset manifest paths must be unique")

    return {"schema": "stasis-assets", "version": 2, "assets": entries}


def verify_source_hashes(manifest: dict, lineage: dict) -> None:
    lineage_rows = {row["path"]: row for row in lineage["assets"]}
    for entry in manifest["assets"]:
        source = entry["path"].removeprefix("assets/original/")
        row = lineage_rows.get(source)
        if row is None:
            raise ValueError(f"package asset is absent from source lineage: {entry['path']}")
        file_path = SOURCE_ROOT / source
        if sha256(file_path) != row["sha256"] or row["sha256"] != entry["content_sha256"]:
            raise ValueError(f"package asset hash differs from recovered source: {entry['path']}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write", action="store_true", help="write the current explicit package manifest")
    args = parser.parse_args()

    catalog = read_json(CATALOG_PATH)
    lineage = read_json(LINEAGE_PATH)
    manifest = build_manifest(catalog, lineage)
    verify_source_hashes(manifest, lineage)
    expected_text = json.dumps(manifest, indent=2) + "\n"
    if args.write:
        MANIFEST_PATH.write_text(expected_text, encoding="utf-8", newline="\n")
        print(f"wrote {len(manifest['assets'])} production asset declarations")
        return 0
    if not MANIFEST_PATH.is_file() or MANIFEST_PATH.read_text(encoding="utf-8") != expected_text:
        print("source package manifest is stale; run python tools/build_asset_manifest.py --write", file=sys.stderr)
        return 1
    print(
        f"verified {len(catalog['assets'])} images, {len(FONT_PATHS)} preserved font masters, "
        f"and {len(AUDIO_METADATA)} audio assets ({len(manifest['assets'])} manifest entries)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
