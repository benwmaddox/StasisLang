#!/usr/bin/env python3
"""Export a native atlas snapshot using the production Rust affinity planner.

This tool validates compiler v4 transition evidence, writes a TSV input for
`stasis_dynload::atlas_placement::plan_atlas_affinity`, and renders only the
placements returned by that API. It does not assign sprites to pages.
"""

from __future__ import annotations

import argparse
import collections
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
from typing import Any

from PIL import Image


SNAPSHOT_SCHEMA = "atlas-affinity-preview/input-v1"
PLAN_SCHEMA = "atlas-affinity-plan/v1"
CONTACT_SIZE = (2000, 900)
REQUIRED_PAGE_SIZE = (512, 512)
MAX_PLANNER_SECONDS = 14 * 60
AOT_PAIR_LIMIT = 4096
AOT_MAX_PAIR_WEIGHT = 1_000_000_000
NATIVE_PAIR_VALID = 1 << 0
NATIVE_PAIR_OVERFLOW = 1 << 1
NATIVE_INVENTORY_OVERFLOW = 1 << 2


def safe_name(value: str) -> str:
    return re.sub(r"[^A-Za-z0-9_.-]+", "_", value).strip("._") or "asset"


def read_json(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"expected JSON object in {path}")
    return value


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def require_int(value: Any, field: str, *, minimum: int = 0) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < minimum:
        raise ValueError(f"{field} must be an integer >= {minimum}; got {value!r}")
    return value


def resolve_path(value: str, base: Path) -> Path:
    path = Path(value)
    return path.resolve() if path.is_absolute() else (base / path).resolve()


def compatibility_fields(value: Any, owner: str) -> list[str]:
    if not isinstance(value, dict):
        raise ValueError(f"{owner}.compatibility must be an object")
    fields: list[str] = []
    for key in ("group_id", "format", "sampler", "color_space", "backend"):
        item = value.get(key)
        if item is None:
            fields.append("-")
        else:
            fields.append(str(require_int(item, f"{owner}.compatibility.{key}")))
    return fields


def _normalized_logical_path(value: str) -> str:
    return normalize_asset_path(value)


def resolve_aot_identity_to_sprite(
    snapshot: dict[str, Any], image_rows: list[dict[str, Any]]
) -> tuple[dict[str, int], dict[str, list[int]]]:
    """Join v4 receiver identities to native resident sprite ids.

    The host bridge may emit `aot_identity` on a resident sprite. If omitted,
    the only accepted fallback is a unique logical-path and logical-size match.
    """
    sprites = snapshot.get("sprites")
    if not isinstance(sprites, list):
        raise ValueError("snapshot.sprites must be an array")
    identity_rows: dict[str, dict[str, Any]] = {}
    for row in image_rows:
        identity = row.get("identity")
        if not isinstance(identity, str) or not identity:
            raise ValueError("every hot_render_images row must have an identity")
        if identity in identity_rows:
            raise ValueError(f"duplicate AOT image identity {identity!r}")
        identity_rows[identity] = row

    by_identity: dict[str, list[int]] = collections.defaultdict(list)
    sprite_rows_by_id: dict[int, dict[str, Any]] = {}
    for sprite in sprites:
        if not isinstance(sprite, dict):
            raise ValueError("snapshot.sprites entries must be objects")
        sprite_id = require_int(sprite.get("id"), "sprite.id", minimum=1)
        if sprite_id in sprite_rows_by_id:
            raise ValueError(f"duplicate resident sprite id {sprite_id}")
        sprite_rows_by_id[sprite_id] = sprite
        identity = sprite.get("aot_identity")
        if identity is None:
            identity = sprite.get("identity")
        if isinstance(identity, str) and identity:
            by_identity[identity].append(sprite_id)

    identity_to_sprite: dict[str, int] = {}
    ambiguous: dict[str, list[int]] = {}
    for identity, image in identity_rows.items():
        candidates = list(by_identity.get(identity, []))
        if not candidates:
            logical_path = image.get("logical_path")
            logical_width = image.get("logical_width")
            logical_height = image.get("logical_height")
            if isinstance(logical_path, str):
                normalized = _normalized_logical_path(logical_path)
                for sprite_id, sprite in sprite_rows_by_id.items():
                    candidate_path = sprite.get("logical_path")
                    if not isinstance(candidate_path, str):
                        continue
                    if _normalized_logical_path(candidate_path) != normalized:
                        continue
                    if sprite.get("logical_width", logical_width) != logical_width:
                        continue
                    if sprite.get("logical_height", logical_height) != logical_height:
                        continue
                    candidates.append(sprite_id)
        candidates = sorted(set(candidates))
        if len(candidates) == 1:
            sprite = sprite_rows_by_id[candidates[0]]
            logical_path = image.get("logical_path")
            if isinstance(logical_path, str) and isinstance(sprite.get("logical_path"), str):
                if _normalized_logical_path(sprite["logical_path"]) != _normalized_logical_path(logical_path):
                    raise ValueError(f"AOT identity {identity!r} matched a sprite with a different logical path")
            identity_to_sprite[identity] = candidates[0]
        elif candidates:
            ambiguous[identity] = candidates

    if ambiguous:
        detail = ", ".join(f"{identity}: {ids}" for identity, ids in sorted(ambiguous.items()))
        raise ValueError(f"ambiguous AOT receiver to resident sprite mapping: {detail}")
    return identity_to_sprite, by_identity


def _metadata_analysis(manifest: dict[str, Any]) -> dict[str, Any]:
    analysis = manifest.get("hot_render_transition_analysis")
    if not isinstance(analysis, dict):
        raise ValueError("AOT manifest v4 lacks hot_render_transition_analysis")
    return analysis


def load_aot_pair_weights(
    snapshot: dict[str, Any],
    snapshot_path: Path,
    aot_override: Path | None,
) -> tuple[list[dict[str, int]], dict[str, Any]]:
    source = snapshot.get("source")
    if not isinstance(source, dict):
        raise ValueError("snapshot.source must be an object")
    raw_path = aot_override or source.get("aot_manifest_path")
    if not raw_path:
        raise ValueError("snapshot.source.aot_manifest_path is required for AOT evidence")
    manifest_path = resolve_path(str(raw_path), snapshot_path.parent)
    manifest = read_json(manifest_path)
    if manifest.get("hot_render_metadata_version") != 4:
        raise ValueError("accepted evidence requires hot_render_metadata_version 4")
    analysis = _metadata_analysis(manifest)
    images = manifest.get("hot_render_images")
    transitions = manifest.get("hot_render_transitions")
    if not isinstance(images, list) or not isinstance(transitions, list):
        raise ValueError("AOT v4 image and transition rows must be arrays")

    validity = analysis.get("validity")
    pair_count = analysis.get("pair_count")
    published_count = require_int(analysis.get("published_pair_count"), "published_pair_count")
    omitted_count = analysis.get("omitted_pair_count")
    pair_limit = require_int(analysis.get("pair_limit"), "pair_limit", minimum=1)
    max_pair_weight = require_int(analysis.get("max_pair_weight"), "max_pair_weight", minimum=1)
    if pair_limit != AOT_PAIR_LIMIT:
        raise ValueError(f"AOT pair_limit must equal the production cap {AOT_PAIR_LIMIT}")
    if max_pair_weight != AOT_MAX_PAIR_WEIGHT:
        raise ValueError(f"AOT max_pair_weight must equal the production cap {AOT_MAX_PAIR_WEIGHT}")
    unknown_causes = analysis.get("unknown_causes")
    if validity != "complete":
        if isinstance(unknown_causes, list) and unknown_causes:
            causes = "; ".join(str(cause) for cause in unknown_causes)
            raise ValueError(f"AOT analysis is {validity!r}; unknown causes: {causes}")
        raise ValueError(f"AOT analysis is {validity!r}; accepted mode requires complete")
    if not isinstance(unknown_causes, list) or unknown_causes:
        raise ValueError(f"AOT analysis has unknown causes: {unknown_causes!r}")
    if pair_count is None or require_int(pair_count, "pair_count") != published_count:
        raise ValueError("AOT analysis pair_count must equal published_pair_count")
    if omitted_count is None or require_int(omitted_count, "omitted_pair_count") != 0:
        raise ValueError("AOT analysis must prove zero omitted transition rows")
    if published_count != len(transitions):
        raise ValueError("published transition count does not match the AOT rows")
    if published_count > pair_limit:
        raise ValueError("published transition count exceeds pair_limit")

    image_rows = [row for row in images if isinstance(row, dict)]
    if len(image_rows) != len(images):
        raise ValueError("hot_render_images contains a non-object row")
    image_by_identity = {row.get("identity"): row for row in image_rows}
    if len(image_by_identity) != len(image_rows):
        raise ValueError("hot_render_images contains duplicate identities")
    identity_to_sprite, _identity_presence = resolve_aot_identity_to_sprite(snapshot, image_rows)

    mapped: list[dict[str, int]] = []
    unmapped_count = 0
    unmapped_weight = 0
    seen: set[tuple[int, int]] = set()
    for index, edge in enumerate(transitions):
        if not isinstance(edge, dict):
            raise ValueError(f"hot_render_transitions[{index}] must be an object")
        from_identity = edge.get("from_identity")
        to_identity = edge.get("to_identity")
        if not isinstance(from_identity, str) or not isinstance(to_identity, str):
            raise ValueError(f"transition {index} has invalid endpoint identities")
        weight = edge.get("max_transitions_per_render")
        if edge.get("validity") != "finite" or weight is None:
            raise ValueError(f"transition {from_identity!r} → {to_identity!r} is not finite")
        weight = require_int(weight, f"transition {index} weight", minimum=1)
        if weight > max_pair_weight:
            raise ValueError(f"transition {index} exceeds max_pair_weight")
        if from_identity == to_identity:
            raise ValueError(f"self transition {from_identity!r} cannot guide atlas placement")
        if from_identity not in image_by_identity or to_identity not in image_by_identity:
            raise ValueError(f"transition {index} references an identity absent from hot_render_images")
        if not image_by_identity[from_identity].get("atlas_eligible") or not image_by_identity[to_identity].get("atlas_eligible"):
            raise ValueError(f"transition {index} references an image that is not atlas eligible")
        if from_identity not in identity_to_sprite or to_identity not in identity_to_sprite:
            unmapped_count += 1
            unmapped_weight += weight
            continue
        from_id = identity_to_sprite[from_identity]
        to_id = identity_to_sprite[to_identity]
        key = (from_id, to_id)
        if key in seen:
            raise ValueError(f"duplicate directed transition for resident sprites {key}")
        seen.add(key)
        mapped.append({"from_sprite_id": from_id, "to_sprite_id": to_id, "weight": weight})

    bridge_counts = snapshot.get("evidence", {})
    if not isinstance(bridge_counts, dict):
        raise ValueError("snapshot.evidence must be an object")
    declared_unmapped_count = bridge_counts.get("unmapped_pair_count", unmapped_count)
    declared_unmapped_weight = bridge_counts.get("unmapped_pair_weight", unmapped_weight)
    if require_int(declared_unmapped_count, "unmapped_pair_count") != unmapped_count:
        raise ValueError("bridge unmapped_pair_count disagrees with the exporter mapping")
    if require_int(declared_unmapped_weight, "unmapped_pair_weight") != unmapped_weight:
        raise ValueError("bridge unmapped_pair_weight disagrees with the exporter mapping")
    if unmapped_count or unmapped_weight:
        raise ValueError(
            f"accepted AOT evidence requires every transition to map; found {unmapped_count} "
            f"unmapped rows weighing {unmapped_weight}"
        )

    declared_pairs = bridge_counts.get("pair_weights")
    if declared_pairs is not None:
        if not isinstance(declared_pairs, list):
            raise ValueError("snapshot.evidence.pair_weights must be an array")
        expected = sorted((row["from_sprite_id"], row["to_sprite_id"], row["weight"]) for row in mapped)
        provided = sorted(
            (
                require_int(row.get("from_sprite_id"), "pair.from_sprite_id", minimum=1),
                require_int(row.get("to_sprite_id"), "pair.to_sprite_id", minimum=1),
                require_int(row.get("weight"), "pair.weight", minimum=1),
            )
            for row in declared_pairs
            if isinstance(row, dict)
        )
        if provided != expected:
            raise ValueError("bridge pair_weights disagree with AOT v4 transitions")

    report = {
        "mode": "accepted-aot-v4",
        "manifest_path": str(manifest_path),
        "manifest_sha256": sha256_file(manifest_path),
        "metadata_version": 4,
        "analysis": analysis,
        "mapped_pair_count": len(mapped),
        "unmapped_pair_count": unmapped_count,
        "unmapped_pair_weight": unmapped_weight,
        "weight_direction": "directed AOT from_identity → to_identity rows are preserved separately",
    }
    return mapped, report


def load_illustrative_pair_weights(snapshot: dict[str, Any]) -> tuple[list[dict[str, int]], dict[str, Any]]:
    evidence = snapshot.get("evidence")
    if not isinstance(evidence, dict) or evidence.get("kind") != "curated-illustrative":
        raise ValueError("without an AOT manifest, evidence.kind must be curated-illustrative")
    rows = evidence.get("pair_weights")
    if not isinstance(rows, list) or not rows:
        raise ValueError("curated-illustrative evidence requires pair_weights")
    result: list[dict[str, int]] = []
    for index, row in enumerate(rows):
        if not isinstance(row, dict):
            raise ValueError(f"illustrative pair_weights[{index}] must be an object")
        result.append(
            {
                "from_sprite_id": require_int(row.get("from_sprite_id"), "pair.from_sprite_id", minimum=1),
                "to_sprite_id": require_int(row.get("to_sprite_id"), "pair.to_sprite_id", minimum=1),
                "weight": require_int(row.get("weight"), "pair.weight", minimum=1),
            }
        )
    return result, {
        "mode": "curated-illustrative",
        "description": str(evidence.get("description", "Curated example; not compiler or runtime telemetry.")),
        "pair_count": len(result),
        "weight_direction": "directed illustrative rows",
    }


def planner_input_tsv(snapshot: dict[str, Any], pair_weights: list[dict[str, int]]) -> str:
    current_generation = require_int(snapshot.get("current_generation"), "current_generation")
    baseline_generation = require_int(snapshot.get("baseline_generation"), "baseline_generation")
    budget = snapshot.get("budget")
    if not isinstance(budget, dict):
        raise ValueError("snapshot.budget must be an object from the same allocation domain")
    budget_values = [
        require_int(budget.get("current_device_bytes"), "budget.current_device_bytes"),
        require_int(budget.get("max_final_bytes"), "budget.max_final_bytes"),
        require_int(budget.get("max_peak_bytes"), "budget.max_peak_bytes"),
    ]
    lines = ["ATLAS_AFFINITY_PREVIEW_V1"]
    lines.append("meta\t" + "\t".join(map(str, [current_generation, baseline_generation, *budget_values])))

    pages = snapshot.get("pages")
    sprites = snapshot.get("sprites")
    placements = snapshot.get("baseline_placements")
    if not isinstance(pages, list) or not isinstance(sprites, list) or not isinstance(placements, list):
        raise ValueError("snapshot pages, sprites, and baseline_placements must be arrays")

    page_ids: set[int] = set()
    for page in pages:
        page_id = require_int(page.get("id"), "page.id")
        if page_id in page_ids:
            raise ValueError(f"duplicate page id {page_id}")
        page_ids.add(page_id)
        width = require_int(page.get("width"), f"page {page_id}.width", minimum=1)
        height = require_int(page.get("height"), f"page {page_id}.height", minimum=1)
        values = [
            page_id,
            width,
            height,
            require_int(page.get("usable_origin_x"), f"page {page_id}.usable_origin_x"),
            require_int(page.get("usable_origin_y"), f"page {page_id}.usable_origin_y"),
            require_int(page.get("allocation_bytes"), f"page {page_id}.allocation_bytes", minimum=1),
            *compatibility_fields(page.get("compatibility"), f"page {page_id}"),
        ]
        lines.append("page\t" + "\t".join(map(str, values)))

    sprite_ids: set[int] = set()
    for sprite in sprites:
        sprite_id = require_int(sprite.get("id"), "sprite.id", minimum=1)
        if sprite_id in sprite_ids:
            raise ValueError(f"duplicate sprite id {sprite_id}")
        sprite_ids.add(sprite_id)
        values = [
            sprite_id,
            require_int(sprite.get("width"), f"sprite {sprite_id}.width", minimum=1),
            require_int(sprite.get("height"), f"sprite {sprite_id}.height", minimum=1),
            require_int(sprite.get("padding"), f"sprite {sprite_id}.padding"),
            *compatibility_fields(sprite.get("compatibility"), f"sprite {sprite_id}"),
        ]
        lines.append("sprite\t" + "\t".join(map(str, values)))

    placement_ids: set[int] = set()
    for placement in placements:
        sprite_id = require_int(placement.get("sprite_id"), "baseline.sprite_id", minimum=1)
        if sprite_id in placement_ids:
            raise ValueError(f"duplicate baseline placement for sprite {sprite_id}")
        placement_ids.add(sprite_id)
        if sprite_id not in sprite_ids:
            raise ValueError(f"baseline placement references unknown sprite {sprite_id}")
        page_id = require_int(placement.get("page_id"), f"baseline {sprite_id}.page_id")
        if page_id not in page_ids:
            raise ValueError(f"baseline placement references unknown page {page_id}")
        values = [
            sprite_id,
            page_id,
            require_int(placement.get("x"), f"baseline {sprite_id}.x"),
            require_int(placement.get("y"), f"baseline {sprite_id}.y"),
            require_int(placement.get("width"), f"baseline {sprite_id}.width", minimum=1),
            require_int(placement.get("height"), f"baseline {sprite_id}.height", minimum=1),
            require_int(placement.get("padding"), f"baseline {sprite_id}.padding"),
        ]
        lines.append("baseline\t" + "\t".join(map(str, values)))
    if placement_ids != sprite_ids:
        missing = sorted(sprite_ids - placement_ids)
        raise ValueError(f"native baseline is missing placements for sprite ids {missing}")

    for edge in pair_weights:
        from_id = require_int(edge["from_sprite_id"], "pair.from_sprite_id", minimum=1)
        to_id = require_int(edge["to_sprite_id"], "pair.to_sprite_id", minimum=1)
        if from_id not in sprite_ids or to_id not in sprite_ids:
            raise ValueError(f"pair weight references a nonresident sprite ({from_id}, {to_id})")
        if from_id == to_id:
            raise ValueError("self-pair weight is invalid")
        lines.append(
            "edge\t" + "\t".join(
                map(
                    str,
                    [from_id, to_id, require_int(edge["weight"], "pair.weight", minimum=1)],
                )
            )
        )
    return "\n".join(lines) + "\n"


def parse_production_planner_output(stdout: str) -> dict[str, Any]:
    """Ignore harmless signing-wrapper output and accept one planner JSON row."""
    results = []
    for line in stdout.splitlines():
        try:
            candidate = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(candidate, dict) and candidate.get("schema") == PLAN_SCHEMA:
            results.append(candidate)
    if len(results) != 1:
        raise RuntimeError(f"Rust planner returned {len(results)} valid JSON result rows")
    return results[0]


def run_production_planner(repo_root: Path, cargo: str, input_text: str) -> dict[str, Any]:
    command = [cargo, "run", "--quiet", "-p", "stasis_dynload", "--example", "atlas_affinity_preview"]
    try:
        process = subprocess.run(
            command,
            cwd=repo_root,
            input=input_text,
            text=True,
            capture_output=True,
            timeout=MAX_PLANNER_SECONDS,
            check=False,
        )
    except subprocess.TimeoutExpired as error:
        raise RuntimeError(f"Rust planner did not finish within {MAX_PLANNER_SECONDS} seconds") from error
    if process.returncode != 0:
        raise RuntimeError(
            "Rust planner failed with exit code "
            f"{process.returncode}:\n{process.stderr.strip() or process.stdout.strip()}"
        )
    return parse_production_planner_output(process.stdout)


def source_image_path(sprite: dict[str, Any], project_root: Path) -> Path:
    value = sprite.get("source_path") or sprite.get("logical_path")
    if not isinstance(value, str) or not value:
        raise ValueError(f"sprite {sprite.get('id')} has no source_path/logical_path")
    path = Path(value)
    return path.resolve() if path.is_absolute() else (project_root / path).resolve()


def render_svg(source: Path, output: Path, width: int, height: int) -> None:
    try:
        import cairosvg
    except ImportError as error:
        raise RuntimeError("SVG preview requires Python package CairoSVG") from error
    try:
        cairosvg.svg2png(
            url=str(source),
            write_to=str(output),
            output_width=width,
            output_height=height,
        )
    except Exception as error:
        raise RuntimeError(f"SVG render failed for {source}: {error}") from error

def prepare_assets(
    snapshot: dict[str, Any], project_root: Path, output_root: Path
) -> dict[int, dict[str, Any]]:
    assets_root = output_root / "assets"
    assets_root.mkdir(parents=True, exist_ok=True)
    result: dict[int, dict[str, Any]] = {}
    for sprite in snapshot["sprites"]:
        sprite_id = require_int(sprite.get("id"), "sprite.id", minimum=1)
        width = require_int(sprite.get("width"), f"sprite {sprite_id}.width", minimum=1)
        height = require_int(sprite.get("height"), f"sprite {sprite_id}.height", minimum=1)
        source = source_image_path(sprite, project_root)
        if not source.is_file():
            raise FileNotFoundError(f"sprite {sprite_id} source does not exist: {source}")
        name = safe_name(str(sprite.get("aot_identity") or sprite.get("identity") or source.stem))
        derivative = assets_root / f"sprite-{sprite_id:016x}-{name}.png"
        if source.suffix.lower() == ".svg":
            render_svg(source, derivative, width, height)
            with Image.open(derivative) as opened:
                rgba = opened.convert("RGBA")
        else:
            with Image.open(source) as opened:
                rgba = opened.convert("RGBA")
            if rgba.size != (width, height):
                rgba = rgba.resize((width, height), Image.Resampling.LANCZOS)
            rgba.save(derivative, format="PNG", optimize=False)
        if rgba.size != (width, height):
            raise ValueError(f"sprite {sprite_id} rendered at {rgba.size}, expected {(width, height)}")
        # Transparent pixels carry no hidden RGB values into the exported page.
        rgba.putdata([(0, 0, 0, 0) if pixel[3] == 0 else pixel for pixel in rgba.getdata()])
        rgba.save(derivative, format="PNG", optimize=False)
        result[sprite_id] = {
            "id": sprite_id,
            "identity": sprite.get("aot_identity") or sprite.get("identity"),
            "logical_path": sprite.get("logical_path"),
            "source_path": str(source),
            "source_sha256": sha256_file(source),
            "source_type": source.suffix.lower().lstrip("."),
            "width": width,
            "height": height,
            "padding": require_int(sprite.get("padding"), f"sprite {sprite_id}.padding"),
            "derivative_path": str(derivative),
            "derivative_sha256": sha256_file(derivative),
        }
    return result


def write_contact_sheets(
    game: str,
    assets: dict[int, dict[str, Any]],
    output_root: Path,
    *,
    max_enlargement: float = 1.0,
) -> list[dict[str, Any]]:
    """Make unlabeled 2000×900 asset contacts; these do not affect atlas layout."""
    if max_enlargement <= 0:
        raise ValueError("max_enlargement must be positive")
    width, height = CONTACT_SIZE
    background = {
        "RootbeerMaze3": (38, 28, 29, 255),
        "SheepHerder": (20, 33, 27, 255),
        "HamsterHavenTycoon": (28, 45, 52, 255),
    }.get(game, (24, 24, 24, 255))
    contacts_root = output_root / "contacts"
    contacts_root.mkdir(parents=True, exist_ok=True)
    pages: list[Image.Image] = []
    canvas = Image.new("RGBA", (width, height), background)
    x, y, row_height = 24, 24, 0

    def flush() -> None:
        nonlocal canvas, x, y, row_height
        pages.append(canvas)
        canvas = Image.new("RGBA", (width, height), background)
        x, y, row_height = 24, 24, 0

    for sprite_id in assets:
        asset = assets[sprite_id]
        with Image.open(asset["derivative_path"]) as opened:
            image = opened.convert("RGBA")
        scale = min(max_enlargement, (width - 48) / image.width, (height - 48) / image.height)
        if scale != 1.0:
            image = image.resize((max(1, round(image.width * scale)), max(1, round(image.height * scale))), Image.Resampling.LANCZOS)
        if x + image.width > width - 24:
            y += row_height + 18
            x, row_height = 24, 0
        if y + image.height > height - 24 and y > 24:
            flush()
        canvas.alpha_composite(image, (x, y))
        x += image.width + 18
        row_height = max(row_height, image.height)
    if x != 24 or y != 24 or not pages:
        pages.append(canvas)

    outputs: list[dict[str, Any]] = []
    for index, page in enumerate(pages):
        target = contacts_root / f"{safe_name(game)}-contact-{index:02d}.png"
        page.save(target, format="PNG", optimize=False)
        outputs.append({"path": str(target.relative_to(output_root)), "width": width, "height": height, "sha256": sha256_file(target)})
    return outputs


def _paint_extrusion(page: Image.Image, image: Image.Image, placement: dict[str, int], padding: int) -> None:
    x, y = placement["x"], placement["y"]
    width, height = placement["width"], placement["height"]
    if image.size != (width, height):
        raise ValueError(f"placement dimensions {(width, height)} do not match rendered image {image.size}")
    if x < padding or y < padding or x + width + padding > page.width or y + height + padding > page.height:
        raise ValueError(f"sprite {placement['sprite_id']} padding allocation falls outside page {placement['page_id']}")
    page.alpha_composite(image, (x, y))
    for distance in range(1, padding + 1):
        for column in range(width):
            page.putpixel((x + column, y - distance), image.getpixel((column, 0)))
            page.putpixel((x + column, y + height - 1 + distance), image.getpixel((column, height - 1)))
        for row in range(height):
            page.putpixel((x - distance, y + row), image.getpixel((0, row)))
            page.putpixel((x + width - 1 + distance, y + row), image.getpixel((width - 1, row)))
        for px, py, pixel in (
            (x - distance, y - distance, image.getpixel((0, 0))),
            (x + width - 1 + distance, y - distance, image.getpixel((width - 1, 0))),
            (x - distance, y + height - 1 + distance, image.getpixel((0, height - 1))),
            (x + width - 1 + distance, y + height - 1 + distance, image.getpixel((width - 1, height - 1))),
        ):
            page.putpixel((px, py), pixel)


def verify_allocations(layout_name: str, placements: list[dict[str, Any]]) -> None:
    by_page: dict[int, list[dict[str, Any]]] = collections.defaultdict(list)
    for placement in placements:
        by_page[placement["page_id"]].append(placement)
    for page_id, page_placements in by_page.items():
        for index, left in enumerate(page_placements):
            ax = left["allocation_x"]
            ay = left["allocation_y"]
            aw = left["allocation_width"]
            ah = left["allocation_height"]
            for right in page_placements[index + 1 :]:
                bx = right["allocation_x"]
                by = right["allocation_y"]
                bw = right["allocation_width"]
                bh = right["allocation_height"]
                if not (ax + aw <= bx or bx + bw <= ax or ay + ah <= by or by + bh <= ay):
                    raise ValueError(f"{layout_name} allocation overlap on page {page_id}: {left['sprite_id']} and {right['sprite_id']}")


def baseline_render_placements(snapshot: dict[str, Any]) -> list[dict[str, Any]]:
    render_sprites = snapshot.get("render_sprites", snapshot["sprites"])
    render_rows = snapshot.get("render_baseline_placements", snapshot["baseline_placements"])
    padding_by_id = {sprite["id"]: sprite["padding"] for sprite in render_sprites}
    result = []
    for row in render_rows:
        sprite_id = require_int(row.get("sprite_id"), "baseline.sprite_id", minimum=1)
        padding = require_int(padding_by_id[sprite_id], f"sprite {sprite_id}.padding")
        x = require_int(row.get("x"), f"baseline {sprite_id}.x")
        y = require_int(row.get("y"), f"baseline {sprite_id}.y")
        width = require_int(row.get("width"), f"baseline {sprite_id}.width", minimum=1)
        height = require_int(row.get("height"), f"baseline {sprite_id}.height", minimum=1)
        result.append(
            {
                "sprite_id": sprite_id,
                "page_id": require_int(row.get("page_id"), f"baseline {sprite_id}.page_id"),
                "x": x,
                "y": y,
                "width": width,
                "height": height,
                "allocation_x": require_int(row.get("allocation_x", x - padding), f"baseline {sprite_id}.allocation_x"),
                "allocation_y": require_int(row.get("allocation_y", y - padding), f"baseline {sprite_id}.allocation_y"),
                "allocation_width": require_int(row.get("allocation_width", width + padding * 2), f"baseline {sprite_id}.allocation_width", minimum=1),
                "allocation_height": require_int(row.get("allocation_height", height + padding * 2), f"baseline {sprite_id}.allocation_height", minimum=1),
            }
        )
    return result


def render_layout(
    layout_name: str,
    placements: list[dict[str, Any]],
    page_rows: list[dict[str, Any]],
    assets: dict[int, dict[str, Any]],
    output_root: Path,
) -> list[dict[str, Any]]:
    verify_allocations(layout_name, placements)
    page_by_id = {row["id"]: row for row in page_rows}
    by_page: dict[int, list[dict[str, Any]]] = collections.defaultdict(list)
    for placement in placements:
        by_page[placement["page_id"]].append(placement)
    output_pages: list[dict[str, Any]] = []
    atlases_root = output_root / "atlases"
    atlases_root.mkdir(parents=True, exist_ok=True)
    for page_id in sorted(page_by_id):
        page_info = page_by_id[page_id]
        page = Image.new("RGBA", (page_info["width"], page_info["height"]), (0, 0, 0, 0))
        for marker in page_info.get("header_markers", []):
            x = require_int(marker.get("x"), f"page {page_id}.marker.x")
            y = require_int(marker.get("y"), f"page {page_id}.marker.y")
            width = require_int(marker.get("width"), f"page {page_id}.marker.width", minimum=1)
            height = require_int(marker.get("height"), f"page {page_id}.marker.height", minimum=1)
            color = marker.get("rgba")
            if not isinstance(color, list) or len(color) != 4 or any(not isinstance(channel, int) or channel < 0 or channel > 255 for channel in color):
                raise ValueError(f"page {page_id}.header marker has invalid rgba")
            page.paste(tuple(color), (x, y, x + width, y + height))
        for placement in sorted(by_page.get(page_id, []), key=lambda item: (item["y"], item["x"], item["sprite_id"])):
            asset = assets[placement["sprite_id"]]
            with Image.open(asset["derivative_path"]) as opened:
                image = opened.convert("RGBA")
            _paint_extrusion(page, image, placement, asset["padding"])
        target = atlases_root / f"{safe_name(layout_name)}-page-{page_id:03d}.png"
        page.save(target, format="PNG", optimize=False)
        output_pages.append(
            {
                "id": page_id,
                "path": str(target.relative_to(output_root)),
                "width": page.width,
                "height": page.height,
                "allocation_bytes": require_int(page_info.get("allocation_bytes"), f"page {page_id}.allocation_bytes", minimum=1),
                "sprite_count": len(by_page.get(page_id, [])),
                "usable_origin_x": page_info.get("usable_origin_x"),
                "usable_origin_y": page_info.get("usable_origin_y"),
                "reserved_header_height": page_info.get("reserved_header_height"),
                "native_flags": page_info.get("native_flags"),
                "native_compatibility_flags": page_info.get("native_compatibility_flags"),
                "group_id": page_info.get("group_id"),
                "planner_eligible": page_info.get("planner_eligible"),
                "planner_exclusion_reason": page_info.get("planner_exclusion_reason"),
                "sha256": sha256_file(target),
            }
        )
    return output_pages


def normalize_asset_path(value: str) -> str:
    """Mirror stasis_asset_normalize_relative_path for logical asset names."""
    if not isinstance(value, str) or not value:
        raise ValueError("logical path must be a non-empty string")
    if value.startswith("\\") or re.match(r"^[A-Za-z]:", value) or "://" in value:
        raise ValueError(f"logical path is not a relative asset path: {value!r}")
    rooted = value == "/assets" or value.startswith("/assets/")
    if value.startswith("/") and not rooted:
        raise ValueError(f"absolute logical path is not supported: {value!r}")
    cursor = value[1:] if rooted else value
    parts: list[str] = []
    for segment in re.split(r"[/\\]+", cursor):
        if not segment or segment == ".":
            continue
        if segment == "..":
            if parts:
                parts.pop()
            continue
        parts.append(segment)
    normalized = "/".join(parts)
    if not normalized:
        raise ValueError(f"logical path normalized to empty: {value!r}")
    if rooted and normalized != "assets" and not normalized.startswith("assets/"):
        raise ValueError(f"virtual asset path escaped assets/: {value!r}")
    return normalized


def fnv1a64(text: str) -> int:
    value = 14695981039346656037
    for byte in text.encode("utf-8"):
        value ^= byte
        value = (value * 1099511628211) & 0xFFFFFFFFFFFFFFFF
    return value


def path_hash(value: str) -> int:
    return fnv1a64(normalize_asset_path(value))


def native_sprite_path_hash(value: str) -> int:
    """Mirror stasis_sprite_normalized_path_hash, including its absolute-path fast path."""
    if not isinstance(value, str) or not value:
        raise ValueError("logical path must be a non-empty string")
    # Virtual /assets/... names are absolute to the C runtime, so it hashes
    # their original spelling without calling the relative-path normalizer.
    if value.startswith(("/", "\\")) or re.match(r"^[A-Za-z]:[/\\]", value):
        return fnv1a64(value)
    return path_hash(value)


def index_project_images(project_root: Path) -> dict[int, list[Path]]:
    extensions = {".png", ".svg", ".jpg", ".jpeg", ".webp", ".bmp"}
    by_hash: dict[int, list[Path]] = collections.defaultdict(list)
    if not project_root.is_dir():
        raise FileNotFoundError(f"project root does not exist: {project_root}")
    for path in project_root.rglob("*"):
        if not path.is_file() or path.suffix.lower() not in extensions:
            continue
        resolved = path.resolve()
        try:
            relative = resolved.relative_to(project_root.resolve()).as_posix()
            by_hash[path_hash(relative)].append(resolved)
            normalized_relative = normalize_asset_path(relative)
            if normalized_relative == "assets" or normalized_relative.startswith("assets/"):
                by_hash[native_sprite_path_hash("/" + normalized_relative)].append(resolved)
        except ValueError:
            continue
        # Windows native builds can hash the canonical absolute path when a
        # logical asset path was absent or malformed.
        by_hash[fnv1a64(str(resolved).replace("\\", "/"))].append(resolved)
    for key, paths in list(by_hash.items()):
        by_hash[key] = sorted(set(paths), key=lambda item: str(item).casefold())
    return by_hash


def read_aot_for_mapping(path: Path | None) -> tuple[dict[str, Any] | None, Path | None]:
    if path is None:
        return None, None
    resolved = path.resolve()
    manifest = read_json(resolved)
    if manifest.get("hot_render_metadata_version") != 4:
        raise ValueError("AOT mapping requires hot_render_metadata_version 4")
    images = manifest.get("hot_render_images")
    if not isinstance(images, list) or any(not isinstance(row, dict) for row in images):
        raise ValueError("AOT v4 hot_render_images must be an array of objects")
    identities: set[str] = set()
    for index, row in enumerate(images):
        identity = row.get("identity")
        logical_path = row.get("logical_path")
        if not isinstance(identity, str) or not identity or identity in identities:
            raise ValueError(f"AOT image row {index} has a missing or duplicate identity")
        identities.add(identity)
        if not isinstance(logical_path, str) or not logical_path:
            raise ValueError(f"AOT image {identity!r} has no logical_path")
        path_hash(logical_path)
        eligible = row.get("atlas_eligible")
        if not isinstance(eligible, bool):
            raise ValueError(f"AOT image {identity}.atlas_eligible must be boolean")
        logical_width = require_int(row.get("logical_width"), f"AOT image {identity}.logical_width")
        logical_height = require_int(row.get("logical_height"), f"AOT image {identity}.logical_height")
        if eligible and (logical_width == 0 or logical_height == 0):
            raise ValueError(f"eligible AOT image {identity} must have positive logical dimensions")
    return manifest, resolved


def normalize_native_snapshot(
    raw: dict[str, Any],
    snapshot_path: Path,
    project: str,
    project_root: Path,
    aot_manifest_path: Path | None,
    aot_manifest: dict[str, Any] | None,
) -> dict[str, Any]:
    if raw.get("schema") != "atlas-affinity-native-query/v1":
        raise ValueError("expected native snapshot schema 'atlas-affinity-native-query/v1'")
    token = require_int(raw.get("snapshot_token"), "snapshot_token", minimum=1)
    stage_cap = require_int(raw.get("stage_peak_cap_bytes"), "stage_peak_cap_bytes", minimum=1)
    raw_pages = raw.get("pages")
    raw_sprites = raw.get("sprites")
    raw_pairs = raw.get("pairs")
    if not isinstance(raw_pages, list) or not isinstance(raw_sprites, list) or not isinstance(raw_pairs, list):
        raise ValueError("native snapshot pages, sprites, and pairs must be arrays")

    pages_by_id: dict[int, dict[str, Any]] = {}
    for row in raw_pages:
        if not isinstance(row, dict):
            raise ValueError("native snapshot page row must be an object")
        page_id = require_int(row.get("page_index"), "page.page_index")
        if page_id in pages_by_id:
            raise ValueError(f"duplicate native page index {page_id}")
        require_int(row.get("width"), f"page {page_id}.width", minimum=1)
        require_int(row.get("height"), f"page {page_id}.height", minimum=1)
        require_int(row.get("allocation_bytes"), f"page {page_id}.allocation_bytes", minimum=1)
        pages_by_id[page_id] = row
    if not pages_by_id:
        raise ValueError("native query returned no pages")

    eligible_page_flag = 1 << 4
    protected_page_flags = (1 << 0) | (1 << 1) | (1 << 2) | (1 << 3)
    blocked_pages: set[int] = set()
    for row in raw_sprites:
        if not isinstance(row, dict):
            raise ValueError("native snapshot sprite row must be an object")
        page_id = require_int(row.get("page_index"), "sprite.page_index")
        page = pages_by_id.get(page_id)
        if page is None:
            raise ValueError(f"sprite references unknown page {page_id}")
        handle = row.get("handle")
        if isinstance(handle, bool) or not isinstance(handle, int):
            raise ValueError("sprite.handle must be an integer")
        group_id = require_int(row.get("group_id"), "sprite.group_id")
        flags = require_int(row.get("flags"), "sprite.flags")
        padding = require_int(row.get("padding"), "sprite.padding")
        page_flags = require_int(page.get("flags"), f"page {page_id}.flags")
        if (
            handle <= 0
            or flags & 1 == 0
            or group_id == 0
            or group_id != require_int(page.get("group_id"), f"page {page_id}.group_id")
            or page_flags & protected_page_flags != 0
            or page_flags & eligible_page_flag == 0
            or require_int(page.get("compatibility_flags"), f"page {page_id}.compatibility_flags") != 1
            or padding != require_int(page.get("padding"), f"page {page_id}.padding")
        ):
            blocked_pages.add(page_id)

    native_pages: list[dict[str, Any]] = []
    planner_pages: list[dict[str, Any]] = []
    for page_id, row in sorted(pages_by_id.items()):
        width = require_int(row.get("width"), f"page {page_id}.width", minimum=1)
        height = require_int(row.get("height"), f"page {page_id}.height", minimum=1)
        group_id = require_int(row.get("group_id"), f"page {page_id}.group_id")
        flags = require_int(row.get("flags"), f"page {page_id}.flags")
        compatibility_flags = require_int(row.get("compatibility_flags"), f"page {page_id}.compatibility_flags")
        reason = None
        if flags & protected_page_flags:
            reason = "protected_or_dedicated_page_flags"
        elif flags & eligible_page_flag == 0:
            reason = "native_page_not_marked_plan_eligible"
        elif compatibility_flags != 1:
            reason = "unsupported_native_compatibility"
        elif group_id == 0:
            reason = "unknown_compatibility_group"
        elif page_id in blocked_pages:
            reason = "contains_resident_blocking_page_repacking"
        page = {
            "id": page_id,
            "width": width,
            "height": height,
            "usable_origin_x": require_int(row.get("usable_x"), f"page {page_id}.usable_x"),
            "usable_origin_y": require_int(row.get("usable_y"), f"page {page_id}.usable_y"),
            "allocation_bytes": require_int(row.get("allocation_bytes"), f"page {page_id}.allocation_bytes", minimum=1),
            "reserved_header_height": require_int(row.get("reserved_header_height"), f"page {page_id}.reserved_header_height"),
            "native_padding": require_int(row.get("padding"), f"page {page_id}.padding"),
            "native_flags": flags,
            "native_compatibility_flags": compatibility_flags,
            "group_id": group_id,
            "planner_eligible": reason is None,
            "planner_exclusion_reason": reason,
            "compatibility": {
                "group_id": group_id if reason is None else None,
                "format": 1 if reason is None else None,
                "sampler": 1 if reason is None else None,
                "color_space": 1 if reason is None else None,
                "backend": 1 if reason is None else None,
            },
        }
        native_pages.append(page)
        if reason is None:
            planner_pages.append(page)

    planner_page_ids = {page["id"] for page in planner_pages}
    all_residents: list[dict[str, Any]] = []
    sprite_ids: set[int] = set()
    for row in raw_sprites:
        handle = row["handle"]
        if handle <= 0:
            continue
        sprite_id = require_int(handle, "sprite.handle", minimum=1)
        if sprite_id in sprite_ids:
            raise ValueError(f"duplicate native sprite handle {sprite_id}")
        sprite_ids.add(sprite_id)
        page_id = require_int(row.get("page_index"), f"sprite {sprite_id}.page_index")
        width = require_int(row.get("width"), f"sprite {sprite_id}.width", minimum=1)
        height = require_int(row.get("height"), f"sprite {sprite_id}.height", minimum=1)
        logical_width = require_int(row.get("logical_width"), f"sprite {sprite_id}.logical_width", minimum=1)
        logical_height = require_int(row.get("logical_height"), f"sprite {sprite_id}.logical_height", minimum=1)
        padding = require_int(row.get("padding"), f"sprite {sprite_id}.padding")
        allocation_width = require_int(row.get("allocation_width"), f"sprite {sprite_id}.allocation_width", minimum=1)
        allocation_height = require_int(row.get("allocation_height"), f"sprite {sprite_id}.allocation_height", minimum=1)
        if allocation_width != width + padding * 2 or allocation_height != height + padding * 2:
            raise ValueError(f"native sprite {sprite_id} padded allocation does not match its extent and padding")
        group_id = require_int(row.get("group_id"), f"sprite {sprite_id}.group_id")
        normalized_hash = require_int(row.get("normalized_path_hash"), f"sprite {sprite_id}.normalized_path_hash")
        sprite = {
            "id": sprite_id,
            "width": width,
            "height": height,
            "logical_width": logical_width,
            "logical_height": logical_height,
            "padding": padding,
            "compatibility": {
                "group_id": group_id if page_id in planner_page_ids else None,
                "format": 1 if page_id in planner_page_ids else None,
                "sampler": 1 if page_id in planner_page_ids else None,
                "color_space": 1 if page_id in planner_page_ids else None,
                "backend": 1 if page_id in planner_page_ids else None,
            },
            "page_index": page_id,
            "x": require_int(row.get("x"), f"sprite {sprite_id}.x"),
            "y": require_int(row.get("y"), f"sprite {sprite_id}.y"),
            "logical_path_hash": normalized_hash,
            "native_flags": require_int(row.get("flags"), f"sprite {sprite_id}.flags"),
            "source_path": None,
            "logical_path": None,
            "aot_identity": None,
            "identity": None,
            "planner_eligible": page_id in planner_page_ids,
        }
        all_residents.append(sprite)

    image_rows: list[dict[str, Any]] = []
    image_by_key: dict[tuple[int, int, int], list[dict[str, Any]]] = collections.defaultdict(list)
    if aot_manifest is not None:
        image_rows = aot_manifest["hot_render_images"]
        for image in image_rows:
            identity = image["identity"]
            logical_width = require_int(image.get("logical_width"), f"AOT image {identity}.logical_width")
            logical_height = require_int(image.get("logical_height"), f"AOT image {identity}.logical_height")
            if logical_width == 0 or logical_height == 0:
                if image.get("atlas_eligible"):
                    raise ValueError(f"eligible AOT image {identity} must have positive logical dimensions")
                continue
            image_key = (
                native_sprite_path_hash(image["logical_path"]),
                logical_width,
                logical_height,
            )
            image_by_key[image_key].append(image)

    for sprite in all_residents:
        key = (sprite["logical_path_hash"], sprite["logical_width"], sprite["logical_height"])
        candidates = image_by_key.get(key, [])
        if len(candidates) == 1:
            image = candidates[0]
            sprite["aot_identity"] = image["identity"]
            sprite["identity"] = image["identity"]
            sprite["logical_path"] = image["logical_path"]
        elif len(candidates) > 1:
            # Keep collisions unresolved so the AOT transition join rejects them.
            sprite["logical_path"] = None

    source_index = index_project_images(project_root)
    for sprite in all_residents:
        if sprite["logical_path"]:
            source = resolve_path(sprite["logical_path"], project_root)
            if source.is_file():
                sprite["source_path"] = str(source)
        if sprite["source_path"] is None:
            candidates = source_index.get(sprite["logical_path_hash"], [])
            if len(candidates) == 1:
                sprite["source_path"] = str(candidates[0])
                if aot_manifest is None:
                    try:
                        sprite["logical_path"] = candidates[0].relative_to(project_root.resolve()).as_posix()
                    except ValueError:
                        pass
            elif len(candidates) > 1:
                raise ValueError(
                    f"native path hash {sprite['logical_path_hash']} matches multiple project images: "
                    + ", ".join(str(path) for path in candidates)
                )

    residents = [sprite for sprite in all_residents if sprite["planner_eligible"]]
    planner_ids = {sprite["id"] for sprite in residents}
    current_device_bytes = sum(
        require_int(row.get("allocation_bytes"), "page.allocation_bytes", minimum=1)
        for row in raw_pages
    )
    all_placements = [
        {
            "sprite_id": sprite["id"],
            "page_id": sprite["page_index"],
            "x": sprite["x"],
            "y": sprite["y"],
            "width": sprite["width"],
            "height": sprite["height"],
            "padding": sprite["padding"],
        }
        for sprite in all_residents
    ]
    planner_placements = [row for row in all_placements if row["sprite_id"] in planner_ids]

    return {
        "schema": SNAPSHOT_SCHEMA,
        "source": {
            "project": project,
            "project_root": str(project_root.resolve()),
            "aot_manifest_path": str(aot_manifest_path.resolve()) if aot_manifest_path else None,
            "native_query": "stasis_gfx_sprite_atlas_query_v1",
            "evidence_provenance": raw.get("evidence_provenance"),
            "snapshot_token": token,
            "renderer_generation": raw.get("renderer_generation"),
            "asset_generation": raw.get("asset_generation"),
            "native_flags": raw.get("flags"),
            "stage_peak_cap_bytes": stage_cap,
            "unmapped_pair_count": 0,
            "unmapped_pair_weight": 0,
            "aot_identity_join_ambiguities": [
                {"path_hash": key[0], "logical_width": key[1], "logical_height": key[2], "identities": [row.get("identity") for row in rows]}
                for key, rows in image_by_key.items()
                if len(rows) > 1
            ],
        },
        "current_generation": token,
        "baseline_generation": token,
        "budget": {
            "current_device_bytes": current_device_bytes,
            "max_final_bytes": current_device_bytes,
            "max_peak_bytes": stage_cap,
        },
        "pages": planner_pages,
        "native_pages": native_pages,
        "sprites": residents,
        "render_sprites": all_residents,
        "baseline_placements": planner_placements,
        "render_baseline_placements": all_placements,
        "raw_pairs": raw_pairs,
        "evidence": {},
        "raw_snapshot_path": str(snapshot_path.resolve()),
    }

def runtime_pair_weights(snapshot: dict[str, Any]) -> tuple[list[dict[str, int]], dict[str, Any]]:
    source = snapshot.get("source")
    if not isinstance(source, dict):
        raise ValueError("snapshot.source must be an object")
    flags = require_int(source.get("native_flags"), "native flags")
    if flags & NATIVE_PAIR_VALID == 0:
        raise ValueError("native runtime histogram requires PAIR_VALID flag bit 0")
    if flags & NATIVE_PAIR_OVERFLOW:
        raise ValueError("native runtime histogram is invalid: PAIR_OVERFLOW flag bit 1 is set")
    if flags & NATIVE_INVENTORY_OVERFLOW:
        raise ValueError("native runtime histogram is invalid: INVENTORY_OVERFLOW flag bit 2 is set")
    resident_ids = {sprite["id"] for sprite in snapshot["sprites"]}
    rows = snapshot.get("raw_pairs")
    if not isinstance(rows, list):
        raise ValueError("native pairs must be an array")
    mapped: list[dict[str, int]] = []
    seen: set[tuple[int, int]] = set()
    omitted_count = 0
    omitted_weight = 0
    for index, row in enumerate(rows):
        if not isinstance(row, dict):
            raise ValueError(f"native pairs[{index}] must be an object")
        from_id = require_int(row.get("from_handle"), f"native pair {index}.from_handle", minimum=1)
        to_id = require_int(row.get("to_handle"), f"native pair {index}.to_handle", minimum=1)
        weight = require_int(row.get("weight"), f"native pair {index}.weight", minimum=1)
        if from_id not in resident_ids or to_id not in resident_ids:
            omitted_count += 1
            omitted_weight += weight
            continue
        if from_id == to_id:
            raise ValueError(f"native runtime histogram contains self-pair for handle {from_id}")
        key = (from_id, to_id)
        if key in seen:
            raise ValueError(f"native runtime histogram contains duplicate directed pair {key}")
        seen.add(key)
        mapped.append({"from_sprite_id": from_id, "to_sprite_id": to_id, "weight": weight})
    return mapped, {
        "mode": "bounded-runtime-histogram",
        "provenance": "production stasis_gfx_sprite_atlas_query_v1 pair rows",
        "mapped_pair_count": len(mapped),
        "unmapped_pair_count": omitted_count,
        "unmapped_pair_weight": omitted_weight,
        "weight_direction": "directed native from_handle → to_handle rows are preserved separately",
        "status": "mapped-edges" if mapped else "no-edges-between-planner-eligible-residents",
    }


def load_curated_pair_weights(path: Path, snapshot: dict[str, Any]) -> tuple[list[dict[str, int]], dict[str, Any]]:
    rows = read_json(path).get("pair_weights")
    if not isinstance(rows, list) or not rows:
        raise ValueError("curated pair file must contain a non-empty pair_weights array")
    resident_ids = {sprite["id"] for sprite in snapshot["sprites"]}
    mapped: list[dict[str, int]] = []
    for index, row in enumerate(rows):
        if not isinstance(row, dict):
            raise ValueError(f"curated pair_weights[{index}] must be an object")
        from_id = require_int(row.get("from_handle"), f"curated pair {index}.from_handle", minimum=1)
        to_id = require_int(row.get("to_handle"), f"curated pair {index}.to_handle", minimum=1)
        weight = require_int(row.get("weight"), f"curated pair {index}.weight", minimum=1)
        if from_id not in resident_ids or to_id not in resident_ids:
            raise ValueError(f"curated pair {index} references a nonresident native handle")
        mapped.append({"from_sprite_id": from_id, "to_sprite_id": to_id, "weight": weight})
    return mapped, {
        "mode": "curated-illustrative",
        "source_path": str(path.resolve()),
        "source_sha256": sha256_file(path),
        "description": "Curated input. It is an illustrative example, not compiler or runtime telemetry.",
        "mapped_pair_count": len(mapped),
        "weight_direction": "directed curated rows",
    }


def run(
    snapshot_path: Path,
    output_root: Path,
    repo_root: Path,
    cargo: str,
    aot_override: Path | None,
    allow_illustrative: bool,
    game: str,
    project_root_override: Path | None,
    curated_pairs: Path | None,
) -> dict[str, Any]:
    snapshot_path = snapshot_path.resolve()
    raw = read_json(snapshot_path)
    if raw.get("schema") != "atlas-affinity-native-query/v1":
        raise ValueError("input must be an atlas-affinity-native-query/v1 JSON snapshot")
    project_root = (
        project_root_override.resolve()
        if project_root_override is not None
        else (repo_root.parent / game).resolve()
    )
    mapping_path = aot_override.resolve() if aot_override is not None else None
    mapping_error: str | None = None
    try:
        manifest, manifest_path = read_aot_for_mapping(mapping_path) if mapping_path else (None, None)
    except Exception as error:
        manifest, manifest_path = None, None
        mapping_error = str(error)
    snapshot = normalize_native_snapshot(
        raw, snapshot_path, game, project_root, manifest_path, manifest
    )
    aot_unmapped_count = 0
    aot_unmapped_weight = 0
    if manifest is not None:
        resident_identities = {
            sprite["aot_identity"]
            for sprite in snapshot["sprites"]
            if isinstance(sprite.get("aot_identity"), str)
        }
        for edge in manifest.get("hot_render_transitions", []) or []:
            if not isinstance(edge, dict):
                continue
            from_identity = edge.get("from_identity")
            to_identity = edge.get("to_identity")
            weight = edge.get("max_transitions_per_render")
            if (
                from_identity not in resident_identities
                or to_identity not in resident_identities
            ) and isinstance(weight, int) and not isinstance(weight, bool) and weight > 0:
                aot_unmapped_count += 1
                aot_unmapped_weight += weight
    snapshot["source"]["unmapped_pair_count"] = aot_unmapped_count
    snapshot["source"]["unmapped_pair_weight"] = aot_unmapped_weight
    aot_rejection: str | None = mapping_error
    pair_weights: list[dict[str, int]]
    evidence: dict[str, Any]
    if manifest_path is not None:
        try:
            pair_weights, evidence = load_aot_pair_weights(snapshot, snapshot_path, manifest_path)
        except Exception as error:
            aot_rejection = str(error)
            try:
                pair_weights, evidence = runtime_pair_weights(snapshot)
            except ValueError:
                if not allow_illustrative or curated_pairs is None:
                    raise
                pair_weights, evidence = load_curated_pair_weights(curated_pairs, snapshot)
            evidence["aot_manifest_path"] = str(manifest_path)
            evidence["aot_manifest_sha256"] = sha256_file(manifest_path)
            evidence["aot_fallback_reason"] = aot_rejection
            evidence["aot_unmapped_pair_count"] = snapshot["source"].get("unmapped_pair_count", 0)
            evidence["aot_unmapped_pair_weight"] = snapshot["source"].get("unmapped_pair_weight", 0)
    else:
        try:
            pair_weights, evidence = runtime_pair_weights(snapshot)
        except ValueError:
            if not allow_illustrative or curated_pairs is None:
                raise
            pair_weights, evidence = load_curated_pair_weights(curated_pairs, snapshot)
    if evidence.get("mode") == "accepted-aot-v4":
        evidence["runtime_pair_provenance"] = raw.get("evidence_provenance")
    if evidence.get("mode") == "bounded-runtime-histogram" and aot_rejection is not None:
        evidence["aot_manifest_validation"] = "rejected; runtime histogram used"
        evidence["aot_fallback_reason"] = aot_rejection
        evidence["aot_unmapped_pair_count"] = aot_unmapped_count
        evidence["aot_unmapped_pair_weight"] = aot_unmapped_weight
        if mapping_path is not None and mapping_path.is_file():
            evidence["aot_manifest_path"] = str(mapping_path)
            evidence["aot_manifest_sha256"] = sha256_file(mapping_path)
    if evidence.get("mode") == "curated-illustrative" and not allow_illustrative:
        raise ValueError("curated examples require --allow-illustrative")

    output_root.mkdir(parents=True, exist_ok=True)
    render_snapshot = {**snapshot, "sprites": snapshot["render_sprites"]}
    assets = prepare_assets(render_snapshot, project_root, output_root)
    plan = run_production_planner(repo_root, cargo, planner_input_tsv(snapshot, pair_weights))
    contacts = write_contact_sheets(game, assets, output_root)
    baseline_placements = baseline_render_placements(snapshot)
    baseline_pages = render_layout(
        f"{safe_name(game)}-baseline",
        baseline_placements,
        snapshot["native_pages"],
        assets,
        output_root,
    )

    base_report = {
        "schema": "atlas-affinity-preview/report-v1",
        "project": game,
        "source": {
            "snapshot_path": str(snapshot_path),
            "snapshot_sha256": sha256_file(snapshot_path),
            "project_root": str(project_root),
            "native_query": "stasis_gfx_sprite_atlas_query_v1",
            "snapshot_token": snapshot["source"]["snapshot_token"],
            "renderer_generation": snapshot["source"]["renderer_generation"],
            "asset_generation": snapshot["source"]["asset_generation"],
            "evidence_provenance": snapshot["source"]["evidence_provenance"],
            "native_flags": snapshot["source"]["native_flags"],
            "stage_peak_cap_bytes": snapshot["source"]["stage_peak_cap_bytes"],
            "unmapped_pair_count": snapshot["source"]["unmapped_pair_count"],
            "unmapped_pair_weight": snapshot["source"]["unmapped_pair_weight"],
            "aot_identity_join_ambiguities": snapshot["source"]["aot_identity_join_ambiguities"],
        },
        "evidence": evidence,
        "contract": {
            "contact_width": CONTACT_SIZE[0],
            "contact_height": CONTACT_SIZE[1],
            "default_affinity_page_extent": list(REQUIRED_PAGE_SIZE),
            "native_page_extents_preserved": True,
            "page_extent_policy": "Use exact native page extents; protected or dedicated pages stay frozen.",
            "page_format": "RGBA8 PNG",
            "atlas_pngs_labeled": False,
        },
        "memory_budget": snapshot["budget"],
        "planner_schema": plan["schema"],
        "baseline_metrics": plan.get("baseline_metrics"),
        "optimized_metrics": plan.get("candidate_metrics"),
        "contacts": contacts,
        "page_inventory": [
            {
                "id": page["id"],
                "width": page["width"],
                "height": page["height"],
                "allocation_bytes": page["allocation_bytes"],
                "planner_eligible": page["planner_eligible"],
                "planner_exclusion_reason": page["planner_exclusion_reason"],
            }
            for page in snapshot["native_pages"]
        ],
        "planner_scope": {
            "page_count": len(snapshot["pages"]),
            "sprite_count": len(snapshot["sprites"]),
            "metrics_cover_only_pages_marked_plan_eligible_by_the_native_bridge": True,
            "frozen_pages_and_sprites_remain_at_native_baseline_positions": True,
        },
        "baseline": {"pages": baseline_pages, "placements": baseline_placements},
        "optimized": None,
        "sprites": [
            {
                "id": sprite["id"],
                "identity": sprite.get("aot_identity"),
                "logical_path": sprite.get("logical_path"),
                "source_path": assets[sprite["id"]]["source_path"],
                "source_sha256": assets[sprite["id"]]["source_sha256"],
                "source_type": assets[sprite["id"]]["source_type"],
                "logical_width": sprite["logical_width"],
                "logical_height": sprite["logical_height"],
                "width": sprite["width"],
                "height": sprite["height"],
                "padding": sprite["padding"],
                "compatibility": sprite["compatibility"],
                "native_page_id": sprite["page_index"],
                "planner_eligible": sprite["planner_eligible"],
            }
            for sprite in snapshot["render_sprites"]
        ],
        "gpu_timing": {
            "status": "not-measured",
            "reason": "No supported GPU timing data was supplied by this run.",
        },
        "interpretation": (
            "Planner metrics are placement, directed cut-weight, and byte accounting from the production Rust API. "
            "They do not estimate native page-run counts, GPU time, or frame-time speedup."
        ),
    }
    if not plan.get("accepted") or not isinstance(plan.get("plan"), dict):
        base_report["status"] = "planner-fallback"
        base_report["planner"] = plan
        report_path = output_root / f"{safe_name(game)}-report.json"
        manifest_path_out = output_root / f"{safe_name(game)}-atlas-manifest.json"
        text = json.dumps(base_report, indent=2) + "\n"
        report_path.write_text(text, encoding="utf-8")
        manifest_path_out.write_text(text, encoding="utf-8")
        return {
            "project": game,
            "status": "planner-fallback",
            "manifest": str(manifest_path_out),
            "report": str(report_path),
            "baseline_pages": len(baseline_pages),
            "optimized_pages": 0,
            "baseline_metrics": plan.get("baseline_metrics"),
            "optimized_metrics": plan.get("candidate_metrics"),
            "evidence_mode": evidence["mode"],
            "fallback_reason": plan.get("fallback_reason"),
        }

    plan_data = plan["plan"]
    optimized_placements = plan_data.get("placements")
    if not isinstance(optimized_placements, list):
        raise ValueError("planner plan has no placements array")
    expected_sprite_ids = {sprite["id"] for sprite in snapshot["sprites"]}
    if {row["sprite_id"] for row in optimized_placements} != expected_sprite_ids:
        raise ValueError("optimized placement set does not cover every planner-eligible resident sprite")
    plan_pages = plan_data.get("pages")
    if not isinstance(plan_pages, list):
        raise ValueError("planner plan has no pages array")
    native_page_by_id = {row["id"]: row for row in snapshot["native_pages"]}
    planner_page_by_id = {row["id"]: row for row in snapshot["pages"]}
    for page in plan_pages:
        source_page = planner_page_by_id.get(page.get("id"))
        if source_page is None:
            raise ValueError(f"planner returned unknown or frozen native page {page.get('id')}")
        if (page.get("width"), page.get("height")) != (source_page["width"], source_page["height"]):
            raise ValueError("planner changed an exact native page extent")
    optimized_page_rows = []
    for page in plan_pages:
        native_page = native_page_by_id[page["id"]]
        optimized_page_rows.append({**native_page, **page})
    frozen_pages = [page for page in snapshot["native_pages"] if not page["planner_eligible"]]
    frozen_page_ids = {page["id"] for page in frozen_pages}
    frozen_sprite_ids = {
        sprite["id"] for sprite in snapshot["render_sprites"] if sprite["page_index"] in frozen_page_ids
    }
    frozen_placements = [
        row for row in baseline_placements if row["sprite_id"] in frozen_sprite_ids
    ]
    complete_optimized_placements = [*optimized_placements, *frozen_placements]
    for page in frozen_pages:
        page["sprite_count"] = sum(1 for row in frozen_placements if row["page_id"] == page["id"])
    optimized_page_rows.extend(frozen_pages)
    optimized_pages = render_layout(
        f"{safe_name(game)}-optimized",
        complete_optimized_placements,
        optimized_page_rows,
        assets,
        output_root,
    )
    base_report["status"] = "accepted-plan"
    base_report["optimized"] = {
        "pages": optimized_pages,
        "placements": complete_optimized_placements,
        "frozen_page_ids": sorted(frozen_page_ids),
        "frozen_sprite_ids": sorted(frozen_sprite_ids),
    }
    base_report["planner"] = {
        "schema": plan["schema"],
        "fallback_reason": plan.get("fallback_reason"),
    }
    report_path = output_root / f"{safe_name(game)}-report.json"
    manifest_path_out = output_root / f"{safe_name(game)}-atlas-manifest.json"
    text = json.dumps(base_report, indent=2) + "\n"
    report_path.write_text(text, encoding="utf-8")
    manifest_path_out.write_text(text, encoding="utf-8")
    return {
        "project": game,
        "status": "accepted-plan",
        "manifest": str(manifest_path_out),
        "report": str(report_path),
        "baseline_pages": len(baseline_pages),
        "optimized_pages": len(optimized_pages),
        "baseline_metrics": plan.get("baseline_metrics"),
        "optimized_metrics": plan.get("candidate_metrics"),
        "evidence_mode": evidence["mode"],
    }

def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, required=True, help="JSON native atlas query snapshot v1")
    parser.add_argument(
        "--game",
        required=True,
        choices=("RootbeerMaze3", "SheepHerder", "HamsterHavenTycoon"),
        help="named source project used for path resolution and contact-sheet styling",
    )
    parser.add_argument("--project-root", type=Path, help="override the sibling game project directory")
    parser.add_argument("--output", type=Path, default=Path("docs/evidence/task-623-atlas-affinity"))
    parser.add_argument("--repo-root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--cargo", default=os.environ.get("CARGO", "cargo"))

    parser.add_argument("--aot-manifest", type=Path, help="AOT engine bundle manifest v4 JSON")
    parser.add_argument("--curated-pairs", type=Path, help="optional explicit illustrative handle-pair JSON")
    parser.add_argument(
        "--allow-illustrative",
        action="store_true",
        help="permit the explicit --curated-pairs input when no usable telemetry exists",
    )
    args = parser.parse_args(argv)
    try:
        result = run(
            args.input,
            args.output.resolve(),
            args.repo_root.resolve(),
            args.cargo,
            args.aot_manifest,
            args.allow_illustrative,
            args.game,
            args.project_root,
            args.curated_pairs,
        )
    except Exception as error:
        print(f"atlas-affinity-preview: {error}", file=sys.stderr)
        return 1
    print(json.dumps(result, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())