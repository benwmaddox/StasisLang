#!/usr/bin/env python3
"""Build deterministic launcher icon derivatives from permanent recovered art."""

from __future__ import annotations

import argparse
from hashlib import sha256
import io
import json
from pathlib import Path
import sys

from PIL import Image, ImageColor


SAMPLE_ROOT = Path(__file__).resolve().parents[1]
ASSET_ROOT = SAMPLE_ROOT / "assets" / "original"
LINEAGE_PATH = ASSET_ROOT / "asset_lineage.json"
RESOURCE_ROOT = SAMPLE_ROOT / "branding" / "android" / "res"
BACKGROUND_PATH = "background.png"
TOWER_PATH = "generated_4x/animations/towers/standard/frame_00.png"
BALL_PATH = "generated_4x/animations/balls/standard/frame_00.png"
MASTER_PATHS = (BACKGROUND_PATH, TOWER_PATH, BALL_PATH)
ADAPTIVE_SIZE = 432
ADAPTIVE_SAFE_DP = 66
ADAPTIVE_CONTAINER_DP = 108
ADAPTIVE_SAFE_SIZE = ADAPTIVE_SIZE * ADAPTIVE_SAFE_DP // ADAPTIVE_CONTAINER_DP
ADAPTIVE_SAFE_OFFSET = (ADAPTIVE_SIZE - ADAPTIVE_SAFE_SIZE) // 2
ADAPTIVE_VIEWPORT_DP = 72
ADAPTIVE_VIEWPORT_RADIUS = ADAPTIVE_SIZE * ADAPTIVE_VIEWPORT_DP // ADAPTIVE_CONTAINER_DP // 2
ADAPTIVE_LOGO_SIZE = 200
ICON_SIZES = {"mdpi": 48, "hdpi": 72, "xhdpi": 96, "xxhdpi": 144, "xxxhdpi": 192}
ADAPTIVE_BACKGROUND = "#17233B"
PREVIEW_BACKDROP = (234, 237, 243, 255)
ADAPTIVE_XML = '''<?xml version="1.0" encoding="utf-8"?>
<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">
    <background android:drawable="@color/brickout_icon_background" />
    <foreground android:drawable="@drawable/brickout_icon_foreground" />
</adaptive-icon>
'''
COLORS_XML = '''<?xml version="1.0" encoding="utf-8"?>
<resources>
    <color name="brickout_icon_background">#17233B</color>
</resources>
'''


def png_bytes(image: Image.Image) -> bytes:
    buffer = io.BytesIO()
    image.save(buffer, format="PNG", optimize=False, compress_level=9)
    return buffer.getvalue()


def adaptive_background_rgba() -> tuple[int, int, int, int]:
    return (*ImageColor.getrgb(ADAPTIVE_BACKGROUND), 255)


def make_adaptive_foreground(tower_master: Image.Image, ball_master: Image.Image) -> tuple[Image.Image, dict[str, object]]:
    if ADAPTIVE_LOGO_SIZE < 48 * ADAPTIVE_SIZE // ADAPTIVE_CONTAINER_DP:
        raise ValueError("adaptive logo is smaller than Android's 48dp minimum")
    if ADAPTIVE_LOGO_SIZE > ADAPTIVE_SAFE_SIZE:
        raise ValueError("adaptive logo exceeds Android's 66dp safe-zone limit")

    offset = (ADAPTIVE_SIZE - ADAPTIVE_LOGO_SIZE) // 2
    tower = tower_master.resize((ADAPTIVE_LOGO_SIZE, ADAPTIVE_LOGO_SIZE), Image.Resampling.LANCZOS)
    ball_size = ADAPTIVE_LOGO_SIZE * 36 // 100
    ball = ball_master.resize((ball_size, ball_size), Image.Resampling.LANCZOS)
    output = Image.new("RGBA", (ADAPTIVE_SIZE, ADAPTIVE_SIZE), (0, 0, 0, 0))
    output.alpha_composite(tower, (offset, offset))
    # The ball sits over the tile's upper-right corner so the mark remains square
    # while keeping the recovered tower/ball relationship recognizable.
    output.alpha_composite(ball, (offset + ADAPTIVE_LOGO_SIZE - ball_size, offset))

    output_alpha = output.getchannel("A")
    output_bounds = output_alpha.getbbox()
    if output_bounds is None:
        raise ValueError("safe-zone fitting removed the adaptive foreground")
    safe_left = ADAPTIVE_SAFE_OFFSET
    safe_top = ADAPTIVE_SAFE_OFFSET
    safe_right = safe_left + ADAPTIVE_SAFE_SIZE
    safe_bottom = safe_top + ADAPTIVE_SAFE_SIZE
    if not (
        output_bounds[0] >= safe_left
        and output_bounds[1] >= safe_top
        and output_bounds[2] <= safe_right
        and output_bounds[3] <= safe_bottom
    ):
        raise ValueError(f"adaptive foreground alpha escapes the centered safe square: {output_bounds}")

    alpha_bytes = output_alpha.tobytes()
    outside_square = 0
    outside_viewport_circle = 0
    radius = ADAPTIVE_VIEWPORT_RADIUS
    center = ADAPTIVE_SIZE / 2
    for y in range(ADAPTIVE_SIZE):
        for x in range(ADAPTIVE_SIZE):
            if alpha_bytes[y * ADAPTIVE_SIZE + x] == 0:
                continue
            pixel_x = x + 0.5
            pixel_y = y + 0.5
            if not (safe_left <= pixel_x < safe_right and safe_top <= pixel_y < safe_bottom):
                outside_square += 1
            dx = pixel_x - center
            dy = pixel_y - center
            if dx * dx + dy * dy > radius * radius:
                outside_viewport_circle += 1
    if outside_square or outside_viewport_circle:
        raise ValueError(
            "adaptive foreground alpha escapes Android's safe area "
            f"(outside-square={outside_square}, outside-{ADAPTIVE_VIEWPORT_DP}dp-circle={outside_viewport_circle})"
        )

    return output, {
        "safe_zone_bounds_px": [safe_left, safe_top, safe_right, safe_bottom],
        "safe_zone_size_px": ADAPTIVE_SAFE_SIZE,
        "fitted_alpha_bounds_px": list(output_bounds),
        "fitted_visible_size_px": [output_bounds[2] - output_bounds[0], output_bounds[3] - output_bounds[1]],
        "logo_container_size_px": ADAPTIVE_LOGO_SIZE,
        "logo_container_size_dp": ADAPTIVE_LOGO_SIZE * ADAPTIVE_CONTAINER_DP // ADAPTIVE_SIZE,
        "ball_size_px": ball_size,
        "ball_position_px": [offset + ADAPTIVE_LOGO_SIZE - ball_size, offset],
        "viewport_circle_diameter_dp": ADAPTIVE_VIEWPORT_DP,
        "viewport_circle_radius_px": ADAPTIVE_VIEWPORT_RADIUS,
        "outside_safe_square_alpha_pixels": outside_square,
        "outside_viewport_circle_alpha_pixels": outside_viewport_circle,
    }


def make_mask_preview(foreground: Image.Image, exponent: int) -> Image.Image:
    content = Image.new("RGBA", (ADAPTIVE_SIZE, ADAPTIVE_SIZE), adaptive_background_rgba())
    content.alpha_composite(foreground)

    scale = 2
    high_size = ADAPTIVE_SIZE * scale
    high_mask = Image.new("L", (high_size, high_size), 0)
    pixels = high_mask.load()
    center = high_size / 2
    radius = ADAPTIVE_VIEWPORT_RADIUS * scale
    for y in range(high_size):
        dy = abs((y + 0.5 - center) / radius)
        for x in range(high_size):
            dx = abs((x + 0.5 - center) / radius)
            if dx**exponent + dy**exponent <= 1.0:
                pixels[x, y] = 255
    mask = high_mask.resize((ADAPTIVE_SIZE, ADAPTIVE_SIZE), Image.Resampling.LANCZOS)
    content.putalpha(mask)

    preview = Image.new("RGBA", (ADAPTIVE_SIZE, ADAPTIVE_SIZE), PREVIEW_BACKDROP)
    preview.alpha_composite(content)
    return preview


def make_icons() -> dict[Path, bytes]:
    lineage = json.loads(LINEAGE_PATH.read_text(encoding="utf-8"))
    hashes = {row["path"]: row["sha256"] for row in lineage["assets"]}
    masters: dict[str, Image.Image] = {}
    master_info = []
    for relative in MASTER_PATHS:
        path = ASSET_ROOT / relative
        data = path.read_bytes()
        expected = hashes.get(relative)
        actual = sha256(data).hexdigest()
        if expected is None or actual != expected:
            raise ValueError(f"icon master is missing from lineage or has changed: {relative}")
        image = Image.open(io.BytesIO(data)).convert("RGBA")
        masters[relative] = image
        master_info.append(
            {
                "path": f"assets/original/{relative}",
                "sha256": actual,
                "dimensions": list(image.size),
                "alpha": "preserved source alpha",
            }
        )

    background = masters[BACKGROUND_PATH]
    # The recovered 4x logical playfield is 464x464 from its upper-left origin.
    crop = background.crop((0, 0, 464, 464))
    if crop.size != (464, 464):
        raise ValueError(f"background master is too small for the documented square crop: {background.size}")
    background_layer = crop.resize((ADAPTIVE_SIZE, ADAPTIVE_SIZE), Image.Resampling.LANCZOS)

    foreground = Image.new("RGBA", (ADAPTIVE_SIZE, ADAPTIVE_SIZE), (0, 0, 0, 0))
    tower = masters[TOWER_PATH].resize((220, 220), Image.Resampling.LANCZOS)
    ball = masters[BALL_PATH].resize((86, 86), Image.Resampling.LANCZOS)
    foreground.alpha_composite(tower, (106, 150))
    foreground.alpha_composite(ball, (253, 84))

    adaptive_foreground, safe_area = make_adaptive_foreground(masters[TOWER_PATH], masters[BALL_PATH])
    full_icon = Image.new("RGBA", (ADAPTIVE_SIZE, ADAPTIVE_SIZE), adaptive_background_rgba())
    full_icon.alpha_composite(background_layer)
    full_icon.alpha_composite(foreground)

    outputs: dict[Path, bytes] = {}
    for density, size in ICON_SIZES.items():
        resized = full_icon.resize((size, size), Image.Resampling.LANCZOS)
        outputs[RESOURCE_ROOT / f"mipmap-{density}" / "ic_launcher.png"] = png_bytes(resized)
    outputs[RESOURCE_ROOT / "drawable-xxxhdpi" / "brickout_icon_foreground.png"] = png_bytes(adaptive_foreground)
    outputs[RESOURCE_ROOT / "mipmap-anydpi-v26" / "ic_launcher.xml"] = ADAPTIVE_XML.encode("utf-8")
    outputs[RESOURCE_ROOT / "values" / "colors.xml"] = COLORS_XML.encode("utf-8")
    outputs[SAMPLE_ROOT / "branding" / "android" / "previews" / "adaptive-circle.png"] = png_bytes(
        make_mask_preview(adaptive_foreground, 2)
    )
    outputs[SAMPLE_ROOT / "branding" / "android" / "previews" / "adaptive-squircle.png"] = png_bytes(
        make_mask_preview(adaptive_foreground, 4)
    )

    provenance = {
        "contract_version": 2,
        "source_commit": lineage["upstream_commit"],
        "composition": {
            "canvas_px": [ADAPTIVE_SIZE, ADAPTIVE_SIZE],
            "resampling": "Pillow Image.Resampling.LANCZOS",
            "base_color": ADAPTIVE_BACKGROUND,
            "full_icon": "opaque color base, then alpha-composite recovered background crop and transparent foreground",
            "legacy_density_icons": "retain the larger recovered-art composition; not used as adaptive layers",
            "foreground": "transparent RGBA; source master alpha preserved; no new artwork",
            "background_master_crop_px": [0, 0, 464, 464],
            "tower_master_size_px": [220, 220],
            "tower_master_position_px": [106, 150],
            "ball_master_size_px": [86, 86],
            "ball_master_position_px": [253, 84],
            "adaptive_background": ADAPTIVE_BACKGROUND,
            "adaptive_foreground_resource": "@drawable/brickout_icon_foreground",
            "adaptive_safe_area": {
                "android_contract": "108dp layers, centered 66dp safe square, logo at least 48dp; tested against central 72dp viewport circle",
                "container_px": ADAPTIVE_SIZE,
                "container_dp": ADAPTIVE_CONTAINER_DP,
                "safe_size_dp": ADAPTIVE_SAFE_DP,
                "minimum_logo_dp": 48,
                "viewport_circle_diameter_dp": ADAPTIVE_VIEWPORT_DP,
                **safe_area,
            },
            "mask_previews": {
                "circle": "adaptive-circle.png; 2x supersampled central 72dp circular viewport preview",
                "squircle": "adaptive-squircle.png; 2x supersampled central 72dp p=4 superellipse viewport preview",
                "background_layer": f"{ADAPTIVE_BACKGROUND}; same @color resource referenced by ic_launcher.xml",
                "foreground_layer": "same generated PNG referenced by @drawable/brickout_icon_foreground",
                "outside_preview_backdrop": "#EAEDF3",
            },
            "density_outputs_px": ICON_SIZES,
        },
        "masters": master_info,
        "outputs": [
            {
                "path": path.relative_to(SAMPLE_ROOT).as_posix(),
                "bytes": len(data),
                "sha256": sha256(data).hexdigest(),
            }
            for path, data in sorted(outputs.items(), key=lambda item: item[0].as_posix())
        ],
    }
    outputs[SAMPLE_ROOT / "branding" / "launcher_icon_provenance.json"] = (
        json.dumps(provenance, indent=2, sort_keys=True) + "\n"
    ).encode("utf-8")
    return outputs


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write", action="store_true", help="write the derived PNG/XML resources")
    args = parser.parse_args()
    expected = make_icons()
    if args.write:
        for path, data in expected.items():
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        print(f"generated {len(ICON_SIZES)} density icons and adaptive icon from 3 recovered masters")
        return 0
    mismatches = [path for path, data in expected.items() if not path.is_file() or path.read_bytes() != data]
    if mismatches:
        for path in mismatches:
            print(f"launcher derivative is stale: {path.relative_to(SAMPLE_ROOT)}", file=sys.stderr)
        print("run python tools/build_launcher_icons.py --write to regenerate", file=sys.stderr)
        return 1
    print(f"verified {len(ICON_SIZES)} density icons and adaptive icon from 3 recovered masters")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
