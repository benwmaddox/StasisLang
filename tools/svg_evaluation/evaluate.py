"""Complete pixel gates against the real runtime bridge; no asset promotion."""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import subprocess
import xml.etree.ElementTree as ET

import numpy as np
from PIL import Image, ImageDraw

TARGETS = {
    "character": [(64, 64), (128, 128), (256, 256), (512, 512), (1024, 1024),
                  (1080, 2400), (2160, 4800), (3840, 2160)],
    "screen": [(360, 800), (720, 1600), (1080, 2400), (2160, 4800),
               (1920, 1080), (3840, 2160)],
}


def audit(file):
    root = ET.parse(file).getroot()
    allowed = {"svg", "g", "path", "defs", "style", "use"}
    tags = {}
    for node in root.iter():
        tag = node.tag.split("}")[-1]
        if tag not in allowed:
            raise ValueError(f"unsupported research construct: {tag}")
        if tag == "style" and any(token in (node.text or "").lower() for token in ("url(", "@import")):
            raise ValueError("external or unvalidated CSS reference")
        tags[tag] = tags.get(tag, 0) + 1
        for key, value in node.attrib.items():
            if key.split("}")[-1] in {"href"} and not value.startswith("#"):
                raise ValueError("external reference")
            if key.startswith("on") or "url(" in value or "data:" in value:
                raise ValueError("unvalidated active/reference construct")
    return {"tags": tags, "width": root.get("width"), "height": root.get("height"),
            "viewBox": root.get("viewBox")}


def compare(a, b):
    # Transparent gate includes alpha and RGB; composites test both matte extremes.
    results = {}
    for bg in ("transparent", "black", "white"):
        if bg == "transparent":
            x, y = a, b
        else:
            color = 0 if bg == "black" else 255
            def composite(p):
                q = p.astype(np.uint16)
                return ((q[..., :3] * q[..., 3:] + color * (255 - q[..., 3:]) + 127) // 255).astype(np.uint8)
            x, y = composite(a), composite(b)
        diff = np.abs(x.astype(np.int16) - y.astype(np.int16))
        results[bg] = {"max_channel_delta": int(diff.max()),
                       "changed_pixels": int(np.count_nonzero(np.any(diff, axis=2))),
                       "mean_absolute_delta": float(diff.mean())}
    return results


def main():
    parser = argparse.ArgumentParser(__doc__)
    parser.add_argument("--probe", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    out = args.output
    sizes = json.loads((out / "sizes.json").read_text())
    report = {"platform": platform.platform(), "gate": "exact RGBA and black/white composite equality",
              "timing": "fresh process per unique SVG/target; first cold then two warm total load+raster calls; byte-identical candidates share measurements",
              "assets": {}}
    previews = []
    for asset, records in sizes["assets"].items():
        audits = {name: audit(out / asset / f"{name}.svg") for name in records if name != "master"}
        master = audit(Path(__file__).parent / "corpus" / f"{asset}.svg")
        assert master["tags"]["path"] == (300 if asset == "character" else 1500)
        for value in audits.values():
            assert all(value[k] == master[k] for k in ("width", "height", "viewBox"))
        result = {"master_audit": master, "audits": audits, "targets": {}}
        for width, height in TARGETS[asset]:
            label = f"{width}x{height}"
            cache = {}
            comparisons = {}
            target = {}
            for name, record in records.items():
                if name == "master":
                    continue
                digest = record["sha256"]
                if digest not in cache:
                    rgba = out / "frame.rgba"
                    proc = subprocess.run([str(args.probe.resolve()), str((out / asset / f"{name}.svg").resolve()),
                                           str(width), str(height), str(rgba.resolve()), "3"],
                                          check=True, capture_output=True, text=True, timeout=120)
                    pixels = np.frombuffer(rgba.read_bytes(), dtype=np.uint8).reshape(height, width, 4).copy()
                    rgba.unlink()
                    cache[digest] = (pixels, json.loads(proc.stdout))
                pixels, timing = cache[digest]
                baseline = cache[records["baseline"]["sha256"]][0]
                if digest not in comparisons:
                    comparisons[digest] = compare(baseline, pixels)
                comparison = comparisons[digest]
                target[name] = {"timing": timing, "render_sha256": hashlib.sha256(pixels.tobytes()).hexdigest(),
                                "comparison": comparison, "pass": all(v["changed_pixels"] == 0 for v in comparison.values())}
                if (asset, width, height) in (("character", 256, 256), ("screen", 1080, 2400)) and name in ("baseline", "numeric", "precision1"):
                    png = out / asset / f"{name}-{label}.png"
                    Image.fromarray(pixels).save(png)
                    previews.append((f"{asset}/{name} {label}", png))
            result["targets"][label] = target
            print(f"{asset} {label}: " + ", ".join(n for n, r in target.items() if not r["pass"]) + " rejected", flush=True)
        report["assets"][asset] = result
    (out / "renders.json").write_text(json.dumps(report, indent=2) + "\n")
    sheet = Image.new("RGB", (900, 660), "#888888")
    draw = ImageDraw.Draw(sheet)
    for i, (title, file) in enumerate(previews):
        image = Image.open(file).convert("RGBA")
        image.thumbnail((290, 300))
        x, y = (i % 3) * 300, (i // 3) * 330
        sheet.paste(image, (x, y + 25), image)
        draw.text((x + 5, y + 5), title, fill="black")
    sheet.save(out / "review.png")


if __name__ == "__main__":
    main()
