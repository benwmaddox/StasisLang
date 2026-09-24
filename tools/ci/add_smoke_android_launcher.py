#!/usr/bin/env python3
"""Add deterministic project-owned launcher artwork to a generated CLI smoke game."""

from __future__ import annotations

import argparse
import json
import struct
import zlib
from pathlib import Path


def _png(size: int) -> bytes:
    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(
            ">I", zlib.crc32(kind + data) & 0xFFFFFFFF
        )

    # A plain blue-green CI marker, not a default shipped game icon.
    pixel = bytes((27, 129, 138, 255))
    rows = b"".join(b"\0" + pixel * size for _ in range(size))
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(rows, 9))
        + chunk(b"IEND", b"")
    )


def add_smoke_launcher(project: Path) -> None:
    project = project.resolve()
    manifest_path = project / "stasis.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    android = manifest.setdefault("android", {})
    android.update({
        "application_id": "org.stasislang.ci_smoke",
        "label": "Stasis CI Smoke",
        "orientation": "sensorLandscape",
        "version_code": 1,
        "version_name": "1.0.0",
        "launcher_resources": "branding/android/res",
    })
    resources = project / android["launcher_resources"]
    for density, size in (("mdpi", 48), ("hdpi", 72), ("xhdpi", 96), ("xxhdpi", 144), ("xxxhdpi", 192)):
        destination = resources / f"mipmap-{density}" / "ic_launcher.png"
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(_png(size))
    foreground = resources / "drawable" / "ic_launcher_foreground.png"
    foreground.parent.mkdir(parents=True, exist_ok=True)
    foreground.write_bytes(_png(432))
    adaptive = resources / "mipmap-anydpi-v26" / "ic_launcher.xml"
    adaptive.parent.mkdir(parents=True, exist_ok=True)
    adaptive.write_text(
        '<?xml version="1.0" encoding="utf-8"?>\n'
        '<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">\n'
        '  <background android:drawable="@android:color/black" />\n'
        '  <foreground android:drawable="@drawable/ic_launcher_foreground" />\n'
        '</adaptive-icon>\n',
        encoding="utf-8",
    )
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("project", type=Path)
    add_smoke_launcher(parser.parse_args().project)


if __name__ == "__main__":
    main()
