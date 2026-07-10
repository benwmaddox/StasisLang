#!/usr/bin/env python3
"""Regenerate Brickout Revenge archival assets from the checked-in masters.

The game never runs this tool. Generated PNGs and the manifest are committed so
desktop and Android builds have no dependency on FFDec, Poppler, or local files.
"""

from __future__ import annotations

import argparse
import copy
import json
import math
import shutil
import subprocess
import tempfile
import xml.etree.ElementTree as ET
from pathlib import Path

from PIL import Image


BALLS = {
    "standard": {"box": (235, 0, 253, 18), "frames": 18, "rotations": 16},
    "curve": {"box": (260, 0, 278, 18), "frames": 34, "rotations": 16},
    "fast": {"box": (284, 0, 302, 18), "frames": 18, "rotations": 16},
    "fire": {"box": (309, 0, 327, 18), "frames": 40, "rotations": 1},
    "slow": {"box": (333, 0, 351, 18), "frames": 18, "rotations": 16},
    "splitter": {"box": (235, 30, 253, 48), "frames": 18, "rotations": 16},
    "swarm": {"box": (260, 30, 278, 48), "frames": 18, "rotations": 16},
    "mover": {"box": (309, 30, 327, 48), "frames": 18, "rotations": 16},
}

TOWERS = {
    "standard": {"frames": 61, "vector_frames": 65},
    "gold": {"frames": 60, "vector_frames": 60},
    "wall": {"frames": 61, "vector_frames": 65},
    "healer": {"frames": 53, "vector_frames": 53},
    "rocket": {"frames": 60, "vector_frames": 60},
    "rapid": {"frames": 60, "vector_frames": 60},
    "sniper": {"frames": 60, "vector_frames": 60},
}

VECTOR_INSTANCES = {
    "balls": {
        "standard": "ball_standard",
        "curve": "ball_curve",
        "fast": "ball_fast",
        "fire": "ball_fire",
        "slow": "ball_slow",
        "splitter": "ball_splitter",
        "swarm": "ball_swarm",
        "mover": "ball_mover",
    },
    "towers": {
        "standard": "tower_basic",
        "gold": "tower_goldminer",
        "wall": "tower_wall",
        "healer": "tower_healer",
        "rocket": "tower_rocket",
        "rapid": "tower_rapid",
        "sniper": "tower_sniper",
    },
}

SVG_NS = "http://www.w3.org/2000/svg"
XLINK_NS = "http://www.w3.org/1999/xlink"
FFDEC_NS = "https://www.free-decompiler.com/flash"
ET.register_namespace("", SVG_NS)
ET.register_namespace("xlink", XLINK_NS)
ET.register_namespace("ffdec", FFDEC_NS)


def run(command: list[str]) -> None:
    print("+", " ".join(command))
    subprocess.run(command, check=True)


def numbered_pngs(folder: Path) -> list[Path]:
    return sorted(folder.glob("*.png"), key=lambda path: int(path.stem))


def scaled_box(box: tuple[int, int, int, int], scale: int) -> tuple[int, int, int, int]:
    return tuple(value * scale for value in box)  # type: ignore[return-value]


def write_animation(
    source_frames: list[Path],
    output_root: Path,
    name: str,
    box: tuple[int, int, int, int],
    frame_count: int,
    scale: int,
    columns: int = 8,
) -> dict[str, int | str]:
    frame_dir = output_root / name
    frame_dir.mkdir(parents=True, exist_ok=True)
    frames: list[Image.Image] = []
    for index, source in enumerate(source_frames[:frame_count]):
        with Image.open(source) as image:
            frame = image.convert("RGBA").crop(scaled_box(box, scale))
        frame.save(frame_dir / f"frame_{index:02d}.png", optimize=True)
        frames.append(frame)

    width, height = frames[0].size
    rows = math.ceil(len(frames) / columns)
    sheet = Image.new("RGBA", (columns * width, rows * height), (0, 0, 0, 0))
    for index, frame in enumerate(frames):
        sheet.alpha_composite(frame, ((index % columns) * width, (index // columns) * height))
    sheet.save(output_root / f"{name}_4x.png", optimize=True)
    return {
        "sheet": f"{name}_4x.png",
        "frame_directory": name,
        "frame_count": len(frames),
        "frame_width": width,
        "frame_height": height,
        "columns": columns,
    }


def write_horizontal_sheet_animation(
    source: Path,
    output_root: Path,
    name: str,
    frame_count: int,
    scale: int,
    frame_size: int = 32,
    columns: int = 8,
) -> dict[str, int | str]:
    with Image.open(source) as image:
        sheet_source = image.convert("RGBA")
    available_frames = sheet_source.width // frame_size
    if sheet_source.height != frame_size or available_frames < frame_count:
        raise RuntimeError(
            f"expected at least {frame_count} {frame_size}x{frame_size} frames in {source}, "
            f"found {available_frames} in {sheet_source.size}"
        )

    frame_dir = output_root / name
    frame_dir.mkdir(parents=True, exist_ok=True)
    frames: list[Image.Image] = []
    published_size = frame_size * scale
    for index in range(frame_count):
        left = index * frame_size
        frame = sheet_source.crop((left, 0, left + frame_size, frame_size)).resize(
            (published_size, published_size), Image.Resampling.LANCZOS
        )
        frame.save(frame_dir / f"frame_{index:02d}.png", optimize=True)
        frames.append(frame)

    rows = math.ceil(len(frames) / columns)
    sheet = Image.new(
        "RGBA",
        (columns * published_size, rows * published_size),
        (0, 0, 0, 0),
    )
    for index, frame in enumerate(frames):
        sheet.alpha_composite(
            frame,
            ((index % columns) * published_size, (index // columns) * published_size),
        )
    sheet.save(output_root / f"{name}_4x.png", optimize=True)
    return {
        "sheet": f"{name}_4x.png",
        "frame_directory": name,
        "frame_count": len(frames),
        "frame_width": published_size,
        "frame_height": published_size,
        "columns": columns,
        "source": f"assets/original/sheets/tower_{name}.png",
    }


def render_ai_masters(source_root: Path, output_root: Path, pdftoppm: Path, scale: int) -> None:
    dpi = 72 * scale
    for stem in ("background", "PTDsource", "paddledef_02-0"):
        run(
            [
                str(pdftoppm),
                "-f",
                "1",
                "-singlefile",
                "-png",
                "-r",
                str(dpi),
                str(source_root / f"{stem}.ai"),
                str(output_root / f"{stem}_4x"),
            ]
        )


def write_stasis_loader(sample_root: Path, published_scale: int = 4) -> None:
    output = sample_root / "brickout_revenge_original_animations.stasis"
    lines = [
        "// Generated by tools/extract_original_assets.py. Do not edit by hand.",
        'import "../../src/stdlib/graphics.stasis";',
        "",
    ]
    for group_name, specs in (("ball", BALLS), ("tower", TOWERS)):
        for name, spec in specs.items():
            count = spec["frames"]
            lines.append(f"global original_{group_name}_{name}_frames: i32[{count}];")
    lines.extend(["", "function original_load_animation_assets(): void {"])
    for group_name, specs in (("ball", BALLS), ("tower", TOWERS)):
        logical_size = 18 if group_name == "ball" else 32
        bake_size = logical_size * published_scale
        for name, spec in specs.items():
            for frame in range(spec["frames"]):
                path = (
                    f"assets/original/generated_4x/animations/{group_name}s/"
                    f"{name}/frame_{frame:02d}.png"
                )
                lines.append(
                    f"    original_{group_name}_{name}_frames[{frame}] = "
                    f'gfx_load_sprite("{path}", {bake_size}, {bake_size});'
                )
    lines.extend(["}", ""])
    for group_name, specs in (("ball", BALLS), ("tower", TOWERS)):
        for name, spec in specs.items():
            count = spec["frames"]
            lines.extend(
                [
                    f"function original_{group_name}_{name}_frame(frame: i32): i32 {{",
                    f"    let index: i32 = frame % {count};",
                    f"    if (index < 0) {{ index += {count}; }}",
                    f"    return original_{group_name}_{name}_frames[index];",
                    "}",
                    "",
                ]
            )
    output.write_text("\n".join(lines), encoding="utf-8")


def href_of(element: ET.Element) -> str | None:
    return element.get(f"{{{XLINK_NS}}}href") or element.get("href")


def isolate_svg_instance(source: Path, instance_id: str, destination: Path, logical_size: int) -> None:
    tree = ET.parse(source)
    source_root = tree.getroot()
    instance = next((item for item in source_root.iter() if item.get("id") == instance_id), None)
    if instance is None:
        raise RuntimeError(f"missing SVG instance {instance_id} in {source}")
    root_href = href_of(instance)
    if not root_href or not root_href.startswith("#"):
        raise RuntimeError(f"SVG instance {instance_id} has no local definition in {source}")

    definitions = {
        item.get("id"): item
        for item in source_root.iter()
        if item.get("id")
    }
    required: list[str] = []
    pending = [root_href[1:]]
    seen: set[str] = set()
    while pending:
        definition_id = pending.pop()
        if definition_id in seen:
            continue
        seen.add(definition_id)
        definition = definitions.get(definition_id)
        if definition is None:
            raise RuntimeError(f"missing definition {definition_id} for {instance_id} in {source}")
        required.append(definition_id)
        for child in definition.iter():
            child_href = href_of(child)
            if child_href and child_href.startswith("#"):
                pending.append(child_href[1:])

    output_root = ET.Element(
        f"{{{SVG_NS}}}svg",
        {
            "width": f"{logical_size}px",
            "height": f"{logical_size}px",
            "viewBox": f"-1 -1 {logical_size} {logical_size}",
            f"{{{FFDEC_NS}}}sourceInstance": instance_id,
        },
    )
    output_defs = ET.SubElement(output_root, f"{{{SVG_NS}}}defs")
    for definition_id in required:
        output_defs.append(copy.deepcopy(definitions[definition_id]))
    ET.SubElement(
        output_root,
        f"{{{SVG_NS}}}use",
        {f"{{{XLINK_NS}}}href": root_href},
    )
    destination.parent.mkdir(parents=True, exist_ok=True)
    ET.ElementTree(output_root).write(destination, encoding="utf-8", xml_declaration=True)


def export_used_vectors(ffdec: Path, source_swf: Path, temp: Path, sample_root: Path) -> None:
    export_root = temp / "used_vectors"
    run(
        [
            str(ffdec),
            "-onerror",
            "abort",
            "-ignorebackground",
            "-sublength",
            "65",
            "-selectid",
            "226",
            "-format",
            "sprite:svg",
            "-export",
            "sprite",
            str(export_root),
            str(source_swf),
        ]
    )
    symbol_dirs = list(export_root.glob("DefineSprite_226*"))
    if len(symbol_dirs) != 1:
        raise RuntimeError(f"expected one SVG TDSymbols export, found {len(symbol_dirs)}")
    source_frames = sorted((symbol_dirs[0] / "1").glob("*.svg"), key=lambda path: int(path.stem))
    vector_root = sample_root / "assets" / "vectors" / "animations"
    if vector_root.exists():
        shutil.rmtree(vector_root)
    for group, instances in VECTOR_INSTANCES.items():
        specs = BALLS if group == "balls" else TOWERS
        logical_size = 18 if group == "balls" else 34
        for name, instance_id in instances.items():
            frame_count = specs[name].get("vector_frames", specs[name]["frames"])
            for frame, source_frame in enumerate(source_frames[:frame_count]):
                isolate_svg_instance(
                    source_frame,
                    instance_id,
                    vector_root / group / name / f"frame_{frame:02d}.svg",
                    logical_size,
                )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--ffdec", type=Path, required=True, help="Path to ffdec-cli or ffdec-cli.exe")
    parser.add_argument("--pdftoppm", type=Path, help="Optional Poppler pdftoppm executable")
    parser.add_argument("--scale", type=int, default=4)
    args = parser.parse_args()
    if args.scale < 1:
        parser.error("--scale must be at least 1")

    sample_root = Path(__file__).resolve().parents[1]
    source_root = sample_root / "assets" / "source"
    output_root = sample_root / "assets" / "original" / "generated_4x"
    animation_root = output_root / "animations"
    if output_root.exists():
        shutil.rmtree(output_root)
    animation_root.mkdir(parents=True)

    with tempfile.TemporaryDirectory(prefix="brickout-swf-") as temp_name:
        temp = Path(temp_name)
        run(
            [
                str(args.ffdec),
                "-onerror",
                "abort",
                "-ignorebackground",
                "-sublength",
                "65",
                "-zoom",
                str(args.scale),
                "-format",
                "sprite:png",
                "-export",
                "sprite",
                str(temp),
                str(source_root / "TowerDefense.swf"),
            ]
        )
        symbol_dirs = list(temp.glob("DefineSprite_226*"))
        if len(symbol_dirs) != 1:
            raise RuntimeError(f"expected one TDSymbols export, found {len(symbol_dirs)}")
        source_frames = numbered_pngs(symbol_dirs[0] / "1")
        if len(source_frames) < 65:
            raise RuntimeError(f"expected 65 TDSymbols subframes, found {len(source_frames)}")

        manifest: dict[str, object] = {
            "render_scale": args.scale,
            "source_canvas": [660, 550],
            "balls": {},
            "towers": {},
        }
        balls_manifest = manifest["balls"]
        assert isinstance(balls_manifest, dict)
        for name, spec in BALLS.items():
            entry = write_animation(
                source_frames,
                animation_root / "balls",
                name,
                spec["box"],
                spec["frames"],
                args.scale,
            )
            entry.update(
                {
                    "fps": 50,
                    "rotations": spec["rotations"],
                    "rotation_offset_degrees": 135 if spec["rotations"] > 1 else 0,
                }
            )
            balls_manifest[name] = entry

        towers_manifest = manifest["towers"]
        assert isinstance(towers_manifest, dict)
        for name, spec in TOWERS.items():
            entry = write_horizontal_sheet_animation(
                sample_root / "assets" / "original" / "sheets" / f"tower_{name}.png",
                animation_root / "towers",
                name,
                spec["frames"],
                args.scale,
            )
            entry["fps"] = 30
            towers_manifest[name] = entry

        if args.pdftoppm:
            render_ai_masters(source_root, output_root, args.pdftoppm, args.scale)

        export_used_vectors(args.ffdec, source_root / "TowerDefense.swf", temp, sample_root)

        (output_root / "animation_manifest.json").write_text(
            json.dumps(manifest, indent=2) + "\n", encoding="utf-8"
        )
        write_stasis_loader(sample_root)

    print(f"wrote {output_root}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
