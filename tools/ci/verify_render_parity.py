#!/usr/bin/env python3
"""Validate the render-parity fixture and optional PNG/BMP captures."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import struct
import zlib
from collections import Counter
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
DEFAULT_MANIFEST = ROOT / "samples" / "render_parity" / "capture_manifest.json"
ALLOWED_STAGES = {
    "initial_launch",
    "second_frame",
    "resize_or_density_change",
    "resource_restore",
}
INITIAL_STAGE_FRAMES = {"initial_launch": 1, "second_frame": 2}


def _read_bmp(path: Path) -> tuple[int, int, bytes]:
    data = path.read_bytes()
    if len(data) < 54 or data[:2] != b"BM":
        raise ValueError("capture is not a BMP file")
    pixel_offset = struct.unpack_from("<I", data, 10)[0]
    dib_size, width, signed_height, planes, bits = struct.unpack_from("<IiiHH", data, 14)
    if dib_size < 40 or width <= 0 or signed_height == 0 or planes != 1 or bits not in (24, 32):
        raise ValueError("capture must be an uncompressed 24-bit or 32-bit BMP")
    compression = struct.unpack_from("<I", data, 30)[0]
    if compression != 0:
        raise ValueError("compressed BMP captures are not supported")
    height = abs(signed_height)
    stride = ((width * bits + 31) // 32) * 4
    if pixel_offset + stride * height > len(data):
        raise ValueError("BMP pixel payload is truncated")
    rgba = bytearray(width * height * 4)
    for output_y in range(height):
        source_y = output_y if signed_height < 0 else height - 1 - output_y
        row = pixel_offset + source_y * stride
        for x in range(width):
            source = row + x * (bits // 8)
            target = (output_y * width + x) * 4
            b, g, r = data[source : source + 3]
            a = data[source + 3] if bits == 32 else 255
            rgba[target : target + 4] = bytes((r, g, b, a))
    return width, height, bytes(rgba)


def _paeth(a: int, b: int, c: int) -> int:
    prediction = a + b - c
    pa = abs(prediction - a)
    pb = abs(prediction - b)
    pc = abs(prediction - c)
    return a if pa <= pb and pa <= pc else b if pb <= pc else c


def _read_png(path: Path) -> tuple[int, int, bytes]:
    data = path.read_bytes()
    if not data.startswith(b"\x89PNG\r\n\x1a\n"):
        raise ValueError("capture is not a PNG file")
    position = 8
    width = height = color_type = bit_depth = interlace = None
    compressed = bytearray()
    while position + 12 <= len(data):
        length = struct.unpack_from(">I", data, position)[0]
        kind = data[position + 4 : position + 8]
        payload = data[position + 8 : position + 8 + length]
        position += 12 + length
        if kind == b"IHDR":
            width, height, bit_depth, color_type, _, _, interlace = struct.unpack(">IIBBBBB", payload)
        elif kind == b"IDAT":
            compressed.extend(payload)
        elif kind == b"IEND":
            break
    if not width or not height or bit_depth != 8 or color_type not in (2, 6) or interlace != 0:
        raise ValueError("PNG capture must be non-interlaced 8-bit RGB or RGBA")
    channels = 4 if color_type == 6 else 3
    row_bytes = width * channels
    raw = zlib.decompress(bytes(compressed))
    if len(raw) != height * (row_bytes + 1):
        raise ValueError("PNG pixel payload has an unexpected length")
    rows: list[bytearray] = []
    position = 0
    for _ in range(height):
        filter_kind = raw[position]
        source = raw[position + 1 : position + 1 + row_bytes]
        position += row_bytes + 1
        previous = rows[-1] if rows else bytearray(row_bytes)
        row = bytearray(row_bytes)
        for index, value in enumerate(source):
            left = row[index - channels] if index >= channels else 0
            above = previous[index]
            upper_left = previous[index - channels] if index >= channels else 0
            if filter_kind == 0:
                decoded = value
            elif filter_kind == 1:
                decoded = value + left
            elif filter_kind == 2:
                decoded = value + above
            elif filter_kind == 3:
                decoded = value + ((left + above) // 2)
            elif filter_kind == 4:
                decoded = value + _paeth(left, above, upper_left)
            else:
                raise ValueError(f"PNG uses unsupported filter {filter_kind}")
            row[index] = decoded & 0xFF
        rows.append(row)
    rgba = bytearray(width * height * 4)
    for y, row in enumerate(rows):
        for x in range(width):
            source = x * channels
            target = (y * width + x) * 4
            rgba[target : target + 3] = row[source : source + 3]
            rgba[target + 3] = row[source + 3] if channels == 4 else 255
    return width, height, bytes(rgba)


def read_capture(path: Path) -> tuple[int, int, bytes]:
    suffix = path.suffix.lower()
    if suffix == ".bmp":
        return _read_bmp(path)
    if suffix == ".png":
        return _read_png(path)
    raise ValueError("capture must use .bmp or .png")


def validate_fixture(manifest_path: Path) -> dict:
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    if manifest.get("schema_version") != 1:
        raise ValueError("render parity manifest schema_version must be 1")
    fixture = ROOT / manifest["fixture"]
    if not fixture.is_file():
        raise ValueError(f"render parity fixture is missing: {fixture}")
    source = fixture.read_text(encoding="utf-8")
    frame_source = (fixture.parent / "frame.stasis").read_text(encoding="utf-8")
    actual_commands = {
        "clear": int("PARITY_GFX_FLAG_CLEAR + PARITY_GFX_FLAG_PRESENT" in frame_source),
        "lines": 2 if "cmd_i32[3] = 2;" in frame_source else 0,
        "filled_rectangles": 1 if "cmd_i32[24] = 1;" in frame_source else 0,
        "sprites": frame_source.count("\n    parity_add_sprite("),
        "direct_text": frame_source.count("\n    parity_add_direct_label("),
        "cached_text": frame_source.count("\n    parity_add_cached_label("),
        "present": int("PARITY_GFX_FLAG_CLEAR + PARITY_GFX_FLAG_PRESENT" in frame_source),
    }
    for command, actual in actual_commands.items():
        expected = int(manifest["required_commands"][command])
        if actual != expected:
            raise ValueError(f"fixture has {actual} {command} commands; expected {expected}")
    if "parity_add_sprite(cmd_i32, 0," not in frame_source:
        raise ValueError("fixture does not exercise the procedural fallback sprite")
    if "platform" in frame_source.lower():
        raise ValueError("render parity fixture must not branch on platform")
    if 'import "frame.stasis";' not in source or "build_parity_frame(" not in source:
        raise ValueError("runnable fixture does not use the canonical parity frame builder")

    trace_fixture = ROOT / manifest["trace_fixture"]
    if not trace_fixture.is_file():
        raise ValueError(f"render parity trace fixture is missing: {trace_fixture}")
    trace_source = trace_fixture.read_text(encoding="utf-8")
    if "native_render_trace" not in trace_source or "build_parity_frame(" not in trace_source:
        raise ValueError("trace fixture does not use the canonical parity frame builder")
    if int(manifest["command_trace"]) <= 0:
        raise ValueError("command_trace must be a nonzero i32 value")

    fixture_root = fixture.parent
    while fixture_root != fixture_root.parent and not (fixture_root / "stasis.json").is_file():
        fixture_root = fixture_root.parent
    if not (fixture_root / "stasis.json").is_file():
        raise ValueError("render parity fixture is not inside a Stasis project")
    for relative in manifest["required_resources"]:
        resource = fixture_root / relative
        if not resource.is_file() or resource.stat().st_size == 0:
            raise ValueError(f"fixture resource is missing or empty: {resource}")
    asset_manifest_path = fixture_root / "assets" / "manifest.json"
    asset_manifest = json.loads(asset_manifest_path.read_text(encoding="utf-8"))
    declared_assets = {entry["path"]: entry for entry in asset_manifest["assets"]}
    for relative in manifest["required_resources"]:
        if not relative.endswith(".svg"):
            continue
        entry = declared_assets.get(relative)
        if entry is None:
            raise ValueError(f"fixture resource is absent from assets/manifest.json: {relative}")
        actual_hash = hashlib.sha256((fixture_root / relative).read_bytes()).hexdigest()
        if entry.get("content_sha256") != actual_hash:
            raise ValueError(f"fixture resource hash is stale: {relative}")
    font = fixture_root / "assets" / "parity.ttf"
    if font.read_bytes()[:4] not in (b"\x00\x01\x00\x00", b"OTTO"):
        raise ValueError("parity.ttf is not a TrueType/OpenType font")
    if hashlib.sha256(font.read_bytes()).hexdigest() != manifest["font_sha256"]:
        raise ValueError("parity.ttf hash is stale; regenerate the deterministic font")
    if set(manifest["stages"]) != ALLOWED_STAGES:
        raise ValueError("fixture matrix must cover launch, second frame, resize/density, and restore")
    return manifest


def _region_pixels(rgba: bytes, width: int, height: int, rect: list[int]) -> list[tuple[int, int, int, int]]:
    x, y, region_w, region_h = rect
    if x < 0 or y < 0 or region_w <= 0 or region_h <= 0 or x + region_w > width or y + region_h > height:
        raise ValueError(f"capture region is out of bounds: {rect}")
    return [
        tuple(rgba[(row * width + column) * 4 : (row * width + column) * 4 + 4])
        for row in range(y, y + region_h)
        for column in range(x, x + region_w)
    ]


def _normalize_viewport(
    rgba: bytes,
    capture_width: int,
    capture_height: int,
    viewport: list[int],
    output_width: int,
    output_height: int,
) -> bytes:
    x, y, width, height = viewport
    if x < 0 or y < 0 or width <= 0 or height <= 0 or x + width > capture_width or y + height > capture_height:
        raise ValueError(f"capture viewport is out of bounds: {viewport}")
    output = bytearray(output_width * output_height * 4)
    for output_y in range(output_height):
        source_y = y + min(height - 1, output_y * height // output_height)
        for output_x in range(output_width):
            source_x = x + min(width - 1, output_x * width // output_width)
            source = (source_y * capture_width + source_x) * 4
            target = (output_y * output_width + output_x) * 4
            output[target : target + 4] = rgba[source : source + 4]
    return bytes(output)


def verify_capture(
    manifest: dict,
    capture_path: Path,
    profile_name: str,
    viewport: list[int] | None = None,
) -> str:
    width, height, rgba = read_capture(capture_path)
    logical_width, logical_height = manifest["logical_size"]
    if viewport is not None:
        rgba = _normalize_viewport(
            rgba, width, height, viewport, logical_width, logical_height
        )
        width, height = logical_width, logical_height
    if [width, height] != [logical_width, logical_height]:
        raise ValueError(
            f"capture dimensions are {width}x{height}; expected "
            f"{logical_width}x{logical_height}; pass --viewport for a letterboxed device capture"
        )
    profile = manifest["capture_profiles"].get(profile_name)
    if profile is None:
        raise ValueError(f"unknown capture profile: {profile_name}")
    digest = hashlib.sha256(rgba).hexdigest()
    if profile["comparison"] == "exact":
        if digest != profile["sha256_rgba"]:
            raise ValueError(f"capture hash {digest} != expected {profile['sha256_rgba']}")
        return digest
    if profile["comparison"] != "regions":
        raise ValueError(f"unsupported comparison mode: {profile['comparison']}")

    background = Counter(tuple(rgba[index : index + 4]) for index in range(0, len(rgba), 4)).most_common(1)[0][0]
    for region in profile["regions"]:
        pixels = _region_pixels(rgba, width, height, region["rect"])
        if "rgba" in region:
            expected = tuple(region["rgba"])
            maximum = int(region["max_channel_delta"])
            matching = sum(max(abs(pixel[channel] - expected[channel]) for channel in range(4)) <= maximum for pixel in pixels)
            coverage = matching / len(pixels)
            if coverage < float(region["min_coverage"]):
                raise ValueError(
                    f"region {region['name']} matched {coverage:.3f}; expected "
                    f"at least {region['min_coverage']:.3f}"
                )
        elif region.get("predicate") == "neutral_bright":
            matching = sum(
                min(pixel[:3]) >= 90 and max(pixel[:3]) - min(pixel[:3]) <= 28
                for pixel in pixels
            )
            coverage = matching / len(pixels)
            if coverage < float(region["min_coverage"]):
                raise ValueError(
                    f"region {region['name']} neutral-bright coverage {coverage:.3f}; expected "
                    f"at least {region['min_coverage']:.3f}"
                )
        elif region.get("predicate") == "gold_text":
            matching = sum(
                pixel[0] >= 110 and pixel[0] > pixel[1] > pixel[2] and pixel[0] - pixel[2] >= 55
                for pixel in pixels
            )
            coverage = matching / len(pixels)
            if coverage < float(region["min_coverage"]):
                raise ValueError(
                    f"region {region['name']} gold-text coverage {coverage:.3f}; expected "
                    f"at least {region['min_coverage']:.3f}"
                )
        elif region.get("predicate") == "green_sprite":
            matching = sum(
                pixel[1] >= 95 and pixel[1] >= pixel[0] + 30 and pixel[1] >= pixel[2] + 18
                for pixel in pixels
            )
            coverage = matching / len(pixels)
            if coverage < float(region["min_coverage"]):
                raise ValueError(
                    f"region {region['name']} green-sprite coverage {coverage:.3f}; expected "
                    f"at least {region['min_coverage']:.3f}"
                )
        elif region.get("predicate") == "crossing_lines":
            red_coverage = sum(
                pixel[0] >= 120 and pixel[0] >= pixel[1] + 45 and pixel[0] >= pixel[2] + 35
                for pixel in pixels
            ) / len(pixels)
            cyan_coverage = sum(
                pixel[2] >= 120 and pixel[1] >= 90 and pixel[2] >= pixel[0] + 45
                for pixel in pixels
            ) / len(pixels)
            if red_coverage < float(region["min_red_coverage"]) or cyan_coverage < float(region["min_cyan_coverage"]):
                raise ValueError(
                    f"region {region['name']} red/cyan coverage {red_coverage:.3f}/{cyan_coverage:.3f}; expected "
                    f"at least {region['min_red_coverage']:.3f}/{region['min_cyan_coverage']:.3f}"
                )
        elif "non_background_fraction" in region:
            changed = sum(pixel != background for pixel in pixels) / len(pixels)
            if changed < float(region["non_background_fraction"]):
                raise ValueError(
                    f"region {region['name']} non-background fraction {changed:.3f}; expected "
                    f"at least {region['non_background_fraction']:.3f}"
                )
        else:
            raise ValueError(f"region {region['name']} has no supported comparison")
    return digest


def _read_runtime_log(log_path: Path) -> str:
    raw_log = log_path.read_bytes()
    encoding = "utf-16" if raw_log.startswith((b"\xff\xfe", b"\xfe\xff")) else "utf-8"
    return raw_log.decode(encoding, errors="replace")


def _restoration_events(log: str) -> list[tuple[str, int, int, str, int]]:
    return [
        (backend, int(surface), int(renderer), reason, int(sprites))
        for backend, surface, renderer, reason, sprites in re.findall(
            r"Stasis renderer resources restored: backend=(\w+)\s+surface_generation=(\d+)\s+"
            r"renderer_generation=(\d+)\s+reason=(\w+)\s+sprites=(\d+)",
            log,
        )
    ]


def verify_runtime_evidence(
    manifest: dict,
    log_path: Path,
    capture_path: Path,
    evidence_path: Path,
    stage: str,
    require_load_details: bool = False,
) -> None:
    log = _read_runtime_log(log_path)
    trace_match = re.search(
        r"Stasis render contract v4 trace=(\d+)\s+flags=3\s+lines=2\s+rects=1\s+sprites=5\s+text=2",
        log,
    )
    if trace_match is None or int(trace_match.group(1)) != int(manifest["command_trace"]):
        raise ValueError(f"runtime evidence for {stage} lacks the exact command trace/counts")
    metrics = re.search(
        r"Stasis display metrics: logical=(\d+)x(\d+)\s+native=(\d+)x(\d+)\s+"
        r"drawable=(\d+)x(\d+)\s+scale=([0-9.]+)",
        log,
    )
    if metrics is None or any(int(value) <= 0 for value in metrics.groups()[:6]):
        raise ValueError(f"runtime evidence for {stage} lacks valid logical/raster dimensions")
    if "stasis_load_font: loaded " not in log or " handle=1" not in log:
        raise ValueError(f"runtime evidence for {stage} lacks successful font/atlas creation")
    if "assets/parity.ttf" not in log.replace("\\", "/"):
        raise ValueError(f"runtime evidence for {stage} lacks the resolved parity font path")
    if require_load_details:
        sprite_loads = re.findall(
            r"gfx_load_sprite:\s+(.+?\.svg)\s+\((\d+)x(\d+)\)\s+->\s+handle=(\d+)\s+"
            r"raster=(\d+)x(\d+)\s+backend=(\w+)",
            log,
        )
        actual_loads = {
            entry[0].replace("\\", "/").split("/assets/")[-1]: (
                int(entry[1]), int(entry[2]), int(entry[3]),
                int(entry[4]), int(entry[5]), entry[6],
            )
            for entry in sprite_loads
        }
        expected_loads = {
            "opaque.svg": (96, 72, 1),
            "translucent.svg": (96, 72, 2),
            "full_canvas.svg": (640, 360, 3),
        }
        if set(actual_loads) != set(expected_loads):
            raise ValueError(f"runtime evidence for {stage} lacks all resolved SVG paths")
        for path, (logical_w, logical_h, handle) in expected_loads.items():
            actual = actual_loads[path]
            if actual[:3] != (logical_w, logical_h, handle):
                raise ValueError(f"runtime evidence for {stage} has the wrong tuple for {path}")
            if actual[3] < logical_w or actual[4] < logical_h or actual[5] not in {"sdl", "gl"}:
                raise ValueError(f"runtime evidence for {stage} has invalid raster/backend for {path}")
    restores = _restoration_events(log)
    if any(backend not in {"sdl", "gl"} for backend, *_ in restores):
        raise ValueError(f"runtime evidence for {stage} lacks backend/resource restoration")
    if restores and max(entry[4] for entry in restores) < 3:
        raise ValueError(f"runtime evidence for {stage} restored fewer than three SVG resources")
    evidence = json.loads(evidence_path.read_text(encoding="utf-8"))
    capture_hash = hashlib.sha256(capture_path.read_bytes()).hexdigest()
    if evidence.get("stage") != stage or evidence.get("capture_sha256") != capture_hash:
        raise ValueError(f"stage evidence for {stage} is not bound to this capture")
    evidence_restore = (
        evidence.get("backend"),
        evidence.get("surface_generation"),
        evidence.get("renderer_generation"),
        evidence.get("reason"),
        evidence.get("sprites"),
    )
    initial_load = (
        stage in INITIAL_STAGE_FRAMES
        and require_load_details
        and not restores
        and evidence.get("frame") == INITIAL_STAGE_FRAMES[stage]
        and evidence_restore == (evidence.get("backend"), 1, 1, "initial", 3)
        and len(actual_loads) == 3
    )
    if evidence_restore not in restores and not initial_load:
        raise ValueError(f"stage evidence for {stage} does not match a runtime restoration event")
    producer_events = re.findall(
        r"Stasis parity capture: stage=(\w+)\s+path=(.+?)\s+frame=(\d+)\s+"
        r"backend=(\w+)\s+surface_generation=(\d+)\s+renderer_generation=(\d+)",
        log,
    )
    producer_match = any(
        event[0] == stage
        and Path(event[1].replace("\\", "/")).name == capture_path.name
        and int(event[2]) == evidence["frame"]
        and event[3] == evidence["backend"]
        and int(event[4]) == evidence["surface_generation"]
        and int(event[5]) == evidence["renderer_generation"]
        for event in producer_events
    )
    if not producer_match:
        raise ValueError(f"stage evidence for {stage} was not emitted by the capture producer")
    if require_load_details and {entry[5] for entry in actual_loads.values()} != {evidence["backend"]}:
        raise ValueError(f"runtime evidence for {stage} has inconsistent load/restore backends")
    if stage == "resize_or_density_change":
        surfaces = {entry[1] for entry in restores}
        if len(surfaces) < 2:
            raise ValueError("resize/density evidence did not advance surface generation")
    if stage == "resource_restore":
        initial_renderer = min(entry[2] for entry in restores)
        foreground = [entry for entry in restores if entry[3] == "foreground"]
        if not foreground or max(entry[2] for entry in foreground) <= initial_renderer:
            raise ValueError("lifecycle evidence did not recreate renderer resources in foreground")


def write_stage_evidence(capture_path: Path, log_path: Path, stage: str, output_path: Path) -> None:
    log = _read_runtime_log(log_path)
    capture_hash = hashlib.sha256(capture_path.read_bytes()).hexdigest()
    capture_events = re.findall(
        r"Stasis parity capture: stage=(\w+)\s+path=(.+?)\s+frame=(\d+)\s+"
        r"backend=(\w+)\s+surface_generation=(\d+)\s+renderer_generation=(\d+)",
        log,
    )
    candidates = [entry for entry in capture_events if entry[0] == stage]
    if not candidates:
        raise ValueError(f"capture producer did not emit stage evidence for {stage}")
    _, logged_path, frame, backend, surface, renderer = candidates[-1]
    if Path(logged_path.replace("\\", "/")).name != capture_path.name:
        raise ValueError(f"capture producer evidence for {stage} names a different file")
    if stage in INITIAL_STAGE_FRAMES and int(frame) != INITIAL_STAGE_FRAMES[stage]:
        raise ValueError(f"capture producer evidence for {stage} has the wrong frame")
    restores = _restoration_events(log)
    matching_restores = [entry for entry in restores if entry[:3] == (backend, int(surface), int(renderer))]
    initial_load = (
        stage in INITIAL_STAGE_FRAMES
        and not restores
        and (int(surface), int(renderer)) == (1, 1)
        and len(re.findall(r"gfx_load_sprite: .*? backend=" + re.escape(backend), log)) == 3
    )
    if not matching_restores and not initial_load:
        raise ValueError(f"capture producer evidence for {stage} has unknown generations")
    reason = matching_restores[-1][3] if matching_restores else "initial"
    sprites = matching_restores[-1][4] if matching_restores else 3
    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "stage": stage,
                "capture_sha256": capture_hash,
                "backend": backend,
                "surface_generation": int(surface),
                "renderer_generation": int(renderer),
                "frame": int(frame),
                "reason": reason,
                "sprites": sprites,
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument("--capture", type=Path)
    parser.add_argument(
        "--capture-only",
        action="store_true",
        help="verify capture regions without desktop runtime-log evidence",
    )
    parser.add_argument("--profile", default="portable")
    parser.add_argument("--stage", choices=sorted(ALLOWED_STAGES))
    parser.add_argument(
        "--viewport",
        help="physical x,y,width,height containing the logical scene",
    )
    parser.add_argument("--runtime-log", type=Path)
    parser.add_argument("--evidence", type=Path)
    parser.add_argument("--write-evidence", action="store_true")
    parser.add_argument("--require-load-details", action="store_true")
    args = parser.parse_args()

    try:
        manifest = validate_fixture(args.manifest.resolve())
        if args.capture:
            viewport = [int(value) for value in args.viewport.split(",")] if args.viewport else None
            if viewport is not None and len(viewport) != 4:
                raise ValueError("--viewport requires x,y,width,height")
            if args.capture_only:
                digest = verify_capture(
                    manifest, args.capture.resolve(), args.profile, viewport
                )
                print(f"render parity capture passed: sha256_rgba={digest}")
                return 0
            if not args.stage:
                raise ValueError("--capture requires --stage")
            if not args.runtime_log:
                raise ValueError("--capture requires --runtime-log lifecycle evidence")
            if not args.evidence:
                raise ValueError("--capture requires --evidence bound to the captured frame")
            if args.write_evidence:
                write_stage_evidence(
                    args.capture.resolve(),
                    args.runtime_log.resolve(),
                    args.stage,
                    args.evidence.resolve(),
                )
            verify_runtime_evidence(
                manifest,
                args.runtime_log.resolve(),
                args.capture.resolve(),
                args.evidence.resolve(),
                args.stage,
                args.require_load_details,
            )
            digest = verify_capture(
                manifest, args.capture.resolve(), args.profile, viewport
            )
            print(f"render parity capture passed: stage={args.stage} sha256_rgba={digest}")
        else:
            print("render parity fixture passed")
        return 0
    except (OSError, KeyError, ValueError, json.JSONDecodeError, zlib.error) as error:
        print(f"render parity gate failed: {error}")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
