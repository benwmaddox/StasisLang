#!/usr/bin/env python3
"""Render curated named-game asset contacts; no atlas pages or packing are produced."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys
from typing import Any

from PIL import Image

REPO_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(REPO_ROOT))
from tools import atlas_affinity_preview as preview  # noqa: E402


def render_source(source: Path, target: Path, width: int, height: int) -> None:
    target.parent.mkdir(parents=True, exist_ok=True)
    if source.suffix.lower() == ".svg":
        preview.render_svg(source, target, width, height)
        return
    with Image.open(source) as opened:
        image = opened.convert("RGBA")
    if image.size != (width, height):
        image = image.resize((width, height), Image.Resampling.LANCZOS)
    image.putdata([(0, 0, 0, 0) if pixel[3] == 0 else pixel for pixel in image.get_flattened_data()])
    image.save(target, format="PNG", optimize=False)


def export_catalog(catalog_path: Path, output_root: Path) -> dict[str, Any]:
    catalog_path = catalog_path.resolve()
    catalog = preview.read_json(catalog_path)
    if catalog.get("schema") != "sprite-sheet-preview/catalog-v1":
        raise ValueError("expected sprite-sheet-preview/catalog-v1")
    games = catalog.get("games")
    if not isinstance(games, list) or len(games) != 3:
        raise ValueError("curated example catalog must contain all three named games")
    output_root.mkdir(parents=True, exist_ok=True)
    game_reports = []
    for game in games:
        name = game.get("name")
        project_root = Path(game.get("project_root", "")).resolve()
        assets = game.get("assets")
        trace = game.get("trace")
        if not isinstance(name, str) or not isinstance(assets, list) or not isinstance(trace, dict):
            raise ValueError("catalog game is missing its name, assets, or trace")
        if trace.get("source") != "curated-illustrative":
            raise ValueError(f"{name} catalog trace must remain labeled curated-illustrative")
        game_output = output_root / preview.safe_name(name)
        derivatives = game_output / "assets"
        rendered: dict[int, dict[str, Any]] = {}
        asset_rows = []
        excluded_assets = []
        for index, asset in enumerate(assets):
            if not isinstance(asset, dict):
                raise ValueError(f"{name} asset {index} must be an object")
            asset_id = asset.get("id")
            source_value = asset.get("source")
            logical_size = asset.get("logical_size")
            if (
                not isinstance(asset_id, str)
                or not isinstance(source_value, str)
                or not isinstance(logical_size, list)
                or len(logical_size) != 2
            ):
                raise ValueError(f"{name} asset {index} is missing id/source/logical_size")
            width = preview.require_int(logical_size[0], f"{name}.{asset_id}.width", minimum=1)
            height = preview.require_int(logical_size[1], f"{name}.{asset_id}.height", minimum=1)
            source = (project_root / source_value).resolve()
            if not source.is_file():
                excluded_assets.append(
                    {
                        "id": asset_id,
                        "catalog_source": source_value,
                        "source_variant": asset.get("source_variant"),
                        "reason": "curated source variant is not present in the current project checkout",
                    }
                )
                continue
            derivative = derivatives / f"{index:03d}-{preview.safe_name(asset_id)}.png"
            render_source(source, derivative, width, height)
            rendered[index] = {
                "id": index,
                "derivative_path": str(derivative),
                "width": width,
                "height": height,
                "source_path": str(source),
                "source_sha256": preview.sha256_file(source),
                "derivative_sha256": preview.sha256_file(derivative),
            }
            asset_rows.append(
                {
                    "id": asset_id,
                    "source": str(source),
                    "source_sha256": preview.sha256_file(source),
                    "source_variant": asset.get("source_variant"),
                    "role": asset.get("role"),
                    "logical_width": width,
                    "logical_height": height,
                    "rendered_path": str(derivative),
                    "rendered_sha256": preview.sha256_file(derivative),
                }
            )
        contacts = preview.write_contact_sheets(name, rendered, game_output, max_enlargement=2.0)
        game_reports.append(
            {
                "name": name,
                "project_root": str(project_root),
                "asset_count": len(asset_rows),
                "catalog_asset_count": len(assets),
                "excluded_assets": excluded_assets,
                "trace": {
                    "source": "curated-illustrative",
                    "description": trace.get("evidence"),
                    "event_count": len(trace.get("events", [])),
                    "events": trace.get("events", []),
                },
                "assets": asset_rows,
                "contacts": [
                    {
                        **contact,
                        "path": f"{preview.safe_name(name)}/{contact['path']}",
                    }
                    for contact in contacts
                ],
                "atlas_pages": "not-generated; awaiting a native query snapshot",
                "planner_metrics": "not-reported for curated contact sheets",
            }
        )
    report = {
        "schema": "atlas-affinity-curated-contacts/v1",
        "status": "illustrative-only",
        "catalog_path": str(catalog_path),
        "catalog_sha256": preview.sha256_file(catalog_path),
        "contract": {
            "contact_width": 2000,
            "contact_height": 900,
            "atlas_pages_generated": False,
            "native_placements_used": False,
        },
        "gpu_timing": {
            "status": "not-measured",
            "reason": "These curated contacts do not run in a supported GPU host.",
        },
        "note": (
            "Catalog assets and ordered traces are curated illustrations. They are not compiler output, "
            "runtime observations, native baseline placements, planner evidence, or performance results."
        ),
        "games": game_reports,
    }
    manifest = output_root / "curated-illustrative-contacts.json"
    manifest.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    return {
        "status": report["status"],
        "manifest": str(manifest),
        "games": [
            {
                "name": game["name"],
                "assets": game["asset_count"],
                "contacts": [row["path"] for row in game["contacts"]],
            }
            for game in game_reports
        ],
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--catalog",
        type=Path,
        default=REPO_ROOT.parent / "sprite-sheet-preview" / "asset_catalog.json",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=REPO_ROOT / "docs" / "evidence" / "task-623-atlas-affinity" / "illustrative",
    )
    args = parser.parse_args(argv)
    try:
        result = export_catalog(args.catalog, args.output.resolve())
    except Exception as error:
        print(f"curated-atlas-contacts: {error}", file=sys.stderr)
        return 1
    print(json.dumps(result, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
