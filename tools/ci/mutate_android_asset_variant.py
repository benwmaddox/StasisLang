#!/usr/bin/env python3
"""Create one bounded malformed release-asset tree for Android IT-022."""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
from pathlib import Path


VARIANTS = (
    "missing",
    "tampered",
    "traversal",
    "duplicate",
    "oversized",
    "malformed-manifest",
)


def _asset_path(root: Path, relative: str) -> Path:
    path = Path(relative)
    if path.is_absolute() or ".." in path.parts or path.parts[:1] != ("assets",):
        raise ValueError(f"manifest asset path is unsafe: {relative}")
    return root.joinpath(*path.parts)


def mutate_asset_tree(root: Path, variant: str) -> dict[str, str]:
    """Mutate ``root/stasis_game`` (or a supplied stasis_game root) in place."""
    if variant not in VARIANTS:
        raise ValueError(f"unknown IT-022 asset variant: {variant}")
    game_root = root / "stasis_game" if (root / "stasis_game").is_dir() else root
    manifest_path = game_root / "assets" / "manifest.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    entries = manifest.get("assets")
    if not isinstance(entries, list) or not entries:
        raise ValueError("IT-022 requires at least one declared asset")
    entry = next((item for item in entries if item.get("path") != "assets/manifest.json"), entries[0])
    relative = entry.get("path")
    if not isinstance(relative, str):
        raise ValueError("IT-022 fixture has no usable asset path")
    selected = _asset_path(game_root, relative)

    if variant == "missing":
        selected.unlink()
        code = "missing_asset"
        path = relative
    elif variant == "tampered":
        selected.write_bytes(selected.read_bytes() + b"\nIT-022 tampered\n")
        code = "tampered_asset"
        path = relative
    elif variant == "traversal":
        entry["path"] = "assets/../it022-escape.bin"
        code = "traversal_path"
        path = entry["path"]
    elif variant == "duplicate":
        manifest["assets"].append(copy.deepcopy(entry))
        code = "duplicate_asset"
        path = relative
    elif variant == "oversized":
        # Keep one real asset over the seam-only one-byte bound.  Shrinking
        # every other declared asset makes the result independent of APK
        # enumeration order while leaving production limits untouched.
        for other in entries:
            other_path = other.get("path")
            if other is entry or not isinstance(other_path, str):
                continue
            other_file = _asset_path(game_root, other_path)
            other_file.write_bytes(b"x")
            other["content_sha256"] = hashlib.sha256(b"x").hexdigest()
        code = "oversized_asset"
        path = relative
    else:
        manifest_path.write_text("{\n", encoding="utf-8")
        return {"variant": variant, "code": "malformed_manifest", "path": "assets/manifest.json"}

    manifest_path.write_text(json.dumps(manifest, separators=(",", ":")) + "\n", encoding="utf-8")
    return {"variant": variant, "code": code, "path": path}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--variant", choices=VARIANTS, required=True)
    parser.add_argument("--expectations", type=Path)
    args = parser.parse_args()
    result = mutate_asset_tree(args.root, args.variant)
    if args.expectations:
        expectations = json.loads(args.expectations.read_text(encoding="utf-8"))
        expectations["asset_rejection"] = {
            "variant": result["variant"],
            "code": result["code"],
            "path": result["path"],
        }
        args.expectations.write_text(
            json.dumps(expectations, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
