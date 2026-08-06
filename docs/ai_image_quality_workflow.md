# AI image quality workflow

Workshop offers explicit ImageGen profiles instead of treating every generated image as the same operation:

| Profile | Quality | Size | Estimated image output cost |
| --- | --- | --- | ---: |
| Draft square | low | 1024 x 1024 | $0.006 |
| Final square | high | 1024 x 1024 | $0.211 |
| Final landscape | high | 1536 x 1024 | $0.165 |
| Final portrait | high | 1024 x 1536 | $0.165 |

The selector defaults to no generated image. Its profile is captured in the durable queue, included in the request fingerprint, reserved against the device spending limit, and reset after submission. The costs are conservative image-output estimates; normal model input and output usage remains separate.

Keep accepted ImageGen PNGs as lossless masters. If a v2 manifest opts a PNG into build preparation, Stasis downsizes it with Lanczos3 in linear-light, premultiplied-alpha space. The master remains unchanged.

## Vector reconstruction

SVG works best for deliberate silhouettes, icons, UI ornaments, flat-shaded props, and backgrounds built from clean layers. It is not a substitute for painterly texture. Use a high-quality ImageGen result as visual direction, then ask the coding model to reconstruct the important geometry instead of mechanically tracing every raster detail.

Use this brief with an attached reference:

```text
Reconstruct the attached reference as an original, compact SVG game asset.

Preserve the silhouette, proportions, major color relationships, recognizable
internal features, and visual weight at the intended in-game size.

Optimize for clean Bezier paths, balanced negative space, a small palette,
consistent stroke widths, and crisp rendering at 64, 128, 256, and 1024 pixels.
Use only paths, rectangles, circles, and simple linear or radial gradients.
Do not embed raster images, fonts, filters, masks, scripts, CSS, or external
resources. Remove details that disappear at the smallest intended size.
```

Render and inspect the result at its smallest gameplay size as well as a large review size. Correct silhouette, gaps, overlaps, stroke weight, and gradient behavior before accepting it.

## Reviewable demo

The demo below is a compact vector scene made from the same bounded primitives recommended for Stasis assets. Zoom it or open the source to review the paths and gradients; it remains resolution-independent.

![Moonlit sentinel vector demo](demos/ai_svg_quality_demo.svg)
