"""Assemble the deterministic review artifacts from completed CPU LIVE runs.

This script intentionally does not run LIVE.  It only copies the canonical
LIVE SVG checkpoints, runs the one shared SVGO binary, rasterizes those SVGs,
and calculates review metrics.
"""
from __future__ import annotations

import argparse
import gzip
import hashlib
import html
import json
import re
import shutil
import subprocess
from pathlib import Path
from xml.etree import ElementTree

from PIL import Image, ImageChops, ImageStat


ROOT = Path(__file__).resolve().parent
LIVE_ROOT = Path(r"D:\code\external\LIVE-Layerwise-Image-Vectorization")
SVGO = Path(r"D:\code\SiteRefresh\node_modules\.bin\svgo.cmd")
CHECKPOINTS = [45, 90, 150, 300, 600, 900, 1500]
SCREENS = {
    "puddle-pop": "Puddle Pop",
    "honey-hop": "Honey Hop",
    "doodle-drift": "Doodle Drift",
}
ORIGINALS = {
    "puddle-pop": r"../puddle-pop/imagegen-vector-reference.png",
    "honey-hop": r"../honey-hop/imagegen-vector-reference.png",
    "doodle-drift": r"../../doodle-drift/concept.png",
}
RUN_NAMES = {"puddle-pop": "puddle", "honey-hop": "honey", "doodle-drift": "doodle"}


def run_dir_for(screen: str, expanded: bool) -> Path:
    needle = f"live-screen-review-{RUN_NAMES[screen]}{'-1500' if expanded else ''}"
    candidates = sorted(
        (p for p in (LIVE_ROOT / "local_output" / "live-runs").glob(f"*_{needle}") if p.is_dir()),
        key=lambda p: p.stat().st_mtime,
    )
    if not candidates:
        raise FileNotFoundError(f"No LIVE output found for {needle}")
    return candidates[-1]


def checkpoint_svg(run_dir: Path, checkpoint: int) -> Path:
    additions = [45, 45, 60, 150, 300, 300, 600]
    cumulative = 0
    parts: list[str] = []
    for add in additions:
        cumulative += add
        parts.append(str(add))
        if cumulative == checkpoint:
            path = run_dir / "output-svg" / ("-".join(parts) + ".svg")
            if path.exists():
                return path
    raise FileNotFoundError(f"Could not map checkpoint {checkpoint} in {run_dir / 'output-svg'}")


def optimize(raw: Path, optimized: Path) -> None:
    optimized.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        [str(SVGO), str(raw), "-o", str(optimized), "--multipass"],
        check=True,
        capture_output=True,
        text=True,
    )


def render(svg: Path, png: Path, width: int, height: int) -> None:
    subprocess.run(
        ["magick", str(svg), "-background", "white", "-resize", f"{width}x{height}!", str(png)],
        check=True,
        capture_output=True,
        text=True,
    )


def image_metrics(source: Path, render_path: Path) -> tuple[float, float]:
    with Image.open(source).convert("RGB") as a, Image.open(render_path).convert("RGB") as b:
        if a.size != b.size:
            b = b.resize(a.size)
        diff = ImageChops.difference(a, b)
        stat = ImageStat.Stat(diff)
        means = stat.mean
        mae = sum(means) / (3 * 255)
        sq = sum(v * v for v in means) / 3
        rmse = (sq**0.5) / 255
        return mae, rmse


def count_paths(svg: Path) -> int:
    text = svg.read_text(encoding="utf-8")
    root = ElementTree.fromstring(text)
    return sum(1 for e in root.iter() if e.tag.rsplit("}", 1)[-1] == "path")


def has_raster(svg: Path) -> bool:
    text = svg.read_text(encoding="utf-8").lower()
    return "<image" in text or "data:image" in text or "href=\"data:" in text


def deterministic_gzip(path: Path) -> int:
    data = path.read_bytes()
    return len(gzip.compress(data, compresslevel=9, mtime=0))


def source_sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def selected(records: list[dict]) -> tuple[int, str, int]:
    # First identify the quality knee using the prescribed <5% relative
    # improvement signal across consecutive checkpoints.  Then report the
    # smallest checkpoint under the 500 KB full-screen artifact ceiling.
    knee = records[-1]["requested_paths"]
    for i in range(1, len(records) - 1):
        improvement = records[i]["rmse"] - records[i + 1]["rmse"]
        relative = improvement / max(records[i]["rmse"], 1e-12)
        if relative < 0.05 and all(
            (records[j]["rmse"] - records[j + 1]["rmse"])
            / max(records[j]["rmse"], 1e-12)
            < 0.05
            for j in range(i, len(records) - 1)
        ):
            knee = records[i]["requested_paths"]
            break
    under = [r for r in records if r["gzip_bytes"] <= 500 * 1024]
    budget = under[0]["requested_paths"] if under else None
    return knee, ("first sustained <5% RMSE improvement; no stable knee before 1500" if knee == 1500 else "first sustained <5% RMSE improvement"), budget


def html_page(manifest: dict) -> str:
    cards: list[str] = []
    for screen in manifest["screens"]:
        rows = [f"<h2>{html.escape(screen['title'])}</h2><p class=meta>Working source: {html.escape(screen['source'])} · <a href=\"{html.escape(screen['original_source'])}\">original full-resolution source</a></p>"]
        rows.append(
            '<div class="source-card"><img src="%s" alt="%s source"><div><b>Source raster</b><br>128 px working width, %d px high</div></div>'
            % (html.escape(screen["source"]), html.escape(screen["title"]), screen["height"])
        )
        rows.append('<div class="grid">')
        for r in screen["checkpoints"]:
            status = "within 500 KB" if r["within_budget"] else "over 500 KB"
            selected_badge = " · visual-knee candidate" if r["requested_paths"] == screen["selected_visual_knee"] else ""
            budget_badge = " · smallest within budget" if r["requested_paths"] == screen["selected_within_budget"] else ""
            rows.append(
                '<figure><img src="%s" alt="%s %d path render"><figcaption><b>%d requested paths</b><br>'
                'raw LIVE %d paths · SVG %s bytes / gzip %s bytes<br>optimized %d paths · SVG %s bytes / gzip %s bytes · MAE %.4f · RMSE %.4f<br><span class="%s">%s%s%s</span><br>'
                '<a href="%s">optimized SVG</a> · <a href="%s">raw SVG</a></figcaption></figure>'
                % (
                    html.escape(r["render"]), html.escape(screen["title"]), r["requested_paths"],
                    r["requested_paths"], r["raw_paths"], f'{r["raw_bytes"]:,}', f'{r["raw_gzip_bytes"]:,}', r["optimized_paths"], f'{r["svg_bytes"]:,}', f'{r["gzip_bytes"]:,}', r["mae"], r["rmse"],
                    "ok" if r["within_budget"] else "warn", status, selected_badge, budget_badge,
                    html.escape(r["svg"]), html.escape(r["raw_svg"]),
                )
            )
        rows.append('</div>')
        rows.append(
            '<p class="selection"><b>Selection:</b> visual knee: %s paths (%s). Smallest within the 500 KB full-screen SVG ceiling: %s.</p>'
            % (screen["selected_visual_knee"], html.escape(screen["selection_reason"]), screen["selected_within_budget"] or "none")
        )
        cards.append("\n".join(rows))
    return """<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>LIVE screen review</title>
<style>body{font:15px system-ui,sans-serif;max-width:1500px;margin:0 auto;padding:24px;background:#12151b;color:#e8edf5}h1{margin-bottom:4px}h2{margin-top:42px}.meta{color:#aeb8c8}.method{padding:16px 18px;background:#1e2632;border:1px solid #354052;border-radius:12px}.source-card{display:flex;gap:16px;align-items:center;margin:14px 0 22px;padding:12px;background:#1b2029;border-radius:10px}.source-card img{width:128px;height:auto;image-rendering:auto}.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(180px,1fr));gap:16px}figure{margin:0;background:#1b2029;border-radius:10px;padding:10px}figure img{display:block;width:100%%;height:auto;background:#fff;border-radius:6px}figcaption{line-height:1.5;margin-top:9px;color:#b9c3d1;font-size:12px}.ok{color:#73e2a0}.warn{color:#ffb866}.selection{padding:12px;background:#202936;border-radius:8px}a{color:#7ec8ff}</style></head><body><h1>LIVE layer-wise screen reconstruction review</h1>
<p>Expanded full-screen mockup pilot: seven cumulative checkpoints from 45 to 1500 paths, compared to the same 128 px-wide source raster.</p>
<div class="method"><b>LOW-RESOLUTION PATH-BUDGET STUDY — NOT A FULL-RESOLUTION FIDELITY PROOF</b><br>Source working dimensions are explicitly 128 × 277 px for all three screens. The original full-resolution source is linked in the manifest metadata but was not consumed by this experiment. All raster inputs were converted by the guarded CPU-only layer-wise LIVE runner with one shared 100-iteration config and no fallback tracer or direct redraw. Sources were aspect-preserving, opaque PNGs at 128 px working width; stock LIVE output is therefore reviewed as an intentional opaque full-screen mockup composition. Every checkpoint uses the same SVGO 3.3.2 command and is rasterized back at actual dimensions. The full-screen artifact ceiling is 500 KB deterministic gzip (the 50 KB ceiling applies to individual sprite/item SVGs, not these screens). Metrics and visual selections are conclusions only for this low-resolution path-budget study; visual fidelity may change at production dimensions.</div>
%s</body></html>""" % "\n".join(cards)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--include-baseline", action="store_true")
    args = parser.parse_args()
    (ROOT / "index.html").unlink(missing_ok=True)
    screens: list[dict] = []
    for slug, title in SCREENS.items():
        folder = ROOT / slug
        source = folder / "source.png"
        with Image.open(source) as im:
            width, height = im.size
        run_dir = run_dir_for(slug, expanded=True)
        records: list[dict] = []
        for checkpoint in CHECKPOINTS:
            raw = checkpoint_svg(run_dir, checkpoint)
            raw_dst = folder / f"live-{checkpoint:04d}-raw.svg"
            svg_dst = folder / f"live-{checkpoint:04d}.svg"
            render_dst = folder / f"live-{checkpoint:04d}.png"
            shutil.copy2(raw, raw_dst)
            optimize(raw_dst, svg_dst)
            render(svg_dst, render_dst, width, height)
            mae, rmse = image_metrics(source, render_dst)
            records.append({
                "requested_paths": checkpoint,
                "raw_paths": count_paths(raw_dst),
                "optimized_paths": count_paths(svg_dst),
                "raw_bytes": raw_dst.stat().st_size,
                "raw_gzip_bytes": deterministic_gzip(raw_dst),
                "svg_bytes": svg_dst.stat().st_size,
                "gzip_bytes": deterministic_gzip(svg_dst),
                "mae": mae,
                "rmse": rmse,
                "has_raster_payload": has_raster(svg_dst),
                "svg": f"{slug}/{svg_dst.name}",
                "raw_svg": f"{slug}/{raw_dst.name}",
                "render": f"{slug}/{render_dst.name}",
                "within_budget": deterministic_gzip(svg_dst) <= 500 * 1024,
            })
        knee, reason, budget = selected(records)
        screens.append({
            "slug": slug, "title": title, "source": f"{slug}/source.png", "original_source": ORIGINALS[slug], "width": width, "height": height,
            "run_dir": str(run_dir), "checkpoints": records, "selected_visual_knee": knee,
            "selection_reason": reason, "selected_within_budget": budget,
        })
    manifest = {
        "schema": "live-screen-review/v2", "generated_by": "build_report.py",
        "method": {"engine": "LIVE layer-wise image vectorization", "device": "cpu", "cuda_visible_devices": "-1", "num_iter": 100,
                    "requested_checkpoints": CHECKPOINTS, "working_width": 128, "optimizer": str(SVGO), "gzip_budget_bytes": 500 * 1024,
                    "no_fallback_tracer": True, "no_direct_redraw": True}, "screens": screens,
    }
    (ROOT / "metrics.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    (ROOT / "index.html").write_text(html_page(manifest), encoding="utf-8")


if __name__ == "__main__":
    main()
