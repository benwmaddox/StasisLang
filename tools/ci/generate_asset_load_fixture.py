"""Generate the deterministic asset-load stress fixture.

The fixture is intentionally ephemeral: callers provide an empty output
directory, and the generator writes only beneath that directory. The generated
manifest is the same v2 manifest consumed by ``stasis_assets``.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import struct
import zlib
from pathlib import Path


SMALL_PNG_COUNT = 256
SVG_COUNT = 256
LARGE_PNG_COUNT = 24
FONT_COUNT = 10
PHRASE_COUNT = 600
MANIFEST_ENTRY_COUNT = FONT_COUNT + SMALL_PNG_COUNT + SVG_COUNT + LARGE_PNG_COUNT


def _png(width: int, height: int, seed: int) -> bytes:
    """Return a tiny deterministic RGBA PNG without a third-party dependency."""

    rows = bytearray()
    for y in range(height):
        rows.append(0)  # filter byte
        for x in range(width):
            value = (seed * 29 + x * 17 + y * 31) & 0xFF
            rows.extend((value, (value + seed) & 0xFF, (value + x + y) & 0xFF, 255))

    def chunk(kind: bytes, payload: bytes) -> bytes:
        return (
            struct.pack(">I", len(payload))
            + kind
            + payload
            + struct.pack(">I", zlib.crc32(kind + payload) & 0xFFFFFFFF)
        )

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(bytes(rows), level=9))
        + chunk(b"IEND", b"")
    )


def _write(path: Path, data: bytes) -> str:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    return hashlib.sha256(data).hexdigest()


def _entry(asset_id: str, path: str, digest: str, fmt: dict) -> dict:
    return {"id": asset_id, "path": path, "content_sha256": digest, "format": fmt}


def generate_fixture(output: Path, font_source: Path | None = None) -> Path:
    """Create the fixture at *output* and return its project root."""

    output = output.resolve()
    if output.exists():
        if any(output.iterdir()):
            raise ValueError(f"output directory must be empty: {output}")
    else:
        output.mkdir(parents=True)

    source = font_source or Path(__file__).resolve().parents[2] / "apps/stasis/assets/gauntlet-font/Basic-Regular.ttf"
    if not source.is_file():
        raise FileNotFoundError(f"canonical test font not found: {source}")

    entries: list[dict] = []
    assets = output / "assets"
    for index in range(FONT_COUNT):
        relative = f"fonts/font_{index:03d}.ttf"
        target = assets / relative.removeprefix("assets/")
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target)
        entries.append(
            _entry(
                f"font_{index:03d}",
                f"assets/{relative}",
                hashlib.sha256(target.read_bytes()).hexdigest(),
                {"kind": "font", "encoding": "ttf"},
            )
        )

    for index in range(SMALL_PNG_COUNT):
        relative = f"sprites/small_{index:03d}.png"
        digest = _write(assets / relative, _png(8, 8, index))
        entries.append(
            _entry(
                f"small_{index:03d}",
                f"assets/{relative}",
                digest,
                {"kind": "sprite", "encoding": "png", "width": 8, "height": 8},
            )
        )

    for index in range(SVG_COUNT):
        relative = f"sprites/vector_{index:03d}.svg"
        svg = (
            '<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" '
            f'viewBox="0 0 16 16"><rect width="16" height="16" fill="#{index:06x}"/>'
            f'<circle cx="{index % 16}" cy="{(index // 16) % 16}" r="2" fill="#ffffff"/></svg>\n'
        ).encode("ascii")
        digest = _write(assets / relative, svg)
        entries.append(
            _entry(
                f"vector_{index:03d}",
                f"assets/{relative}",
                digest,
                {"kind": "sprite", "encoding": "svg", "width": 16, "height": 16},
            )
        )

    for index in range(LARGE_PNG_COUNT):
        relative = f"sprites/large_{index:03d}.png"
        digest = _write(assets / relative, _png(256, 256, 1000 + index))
        entries.append(
            _entry(
                f"large_{index:03d}",
                f"assets/{relative}",
                digest,
                {"kind": "sprite", "encoding": "png", "width": 256, "height": 256},
            )
        )

    phrases = [f"deterministic asset-load phrase {index:03d}" for index in range(PHRASE_COUNT)]
    (output / "phrases.json").write_text(
        json.dumps({"version": 1, "phrases": phrases}, indent=2) + "\n", encoding="utf-8"
    )
    manifest = {"schema": "stasis-assets", "version": 2, "assets": entries}
    if len(entries) != MANIFEST_ENTRY_COUNT:
        raise AssertionError(f"generated {len(entries)} entries, expected {MANIFEST_ENTRY_COUNT}")
    (assets / "manifest.json").parent.mkdir(parents=True, exist_ok=True)
    (assets / "manifest.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=False) + "\n", encoding="utf-8"
    )
    return output


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path, help="empty directory to populate")
    parser.add_argument("--font", type=Path, help="override canonical test font")
    args = parser.parse_args()
    generate_fixture(args.output, args.font)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
