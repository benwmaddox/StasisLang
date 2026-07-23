#!/usr/bin/env python3
"""Generate the tiny deterministic test-only font used by render_parity."""

from pathlib import Path

from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen


ROOT = Path(__file__).resolve().parents[2]
OUTPUT = ROOT / "samples" / "render_parity" / "assets" / "parity.ttf"


def glyph_for(codepoint: int | None):
    pen = TTGlyphPen(None)
    if codepoint not in (None, 32):
        for bit in range(7):
            if codepoint & (1 << bit):
                x = 80 + bit * 68
                pen.moveTo((x, 120))
                pen.lineTo((x + 42, 120))
                pen.lineTo((x + 42, 760))
                pen.lineTo((x, 760))
                pen.closePath()
    return pen.glyph()


def main() -> None:
    codepoints = list(range(32, 127))
    glyph_order = [".notdef"] + [f"u{value:04X}" for value in codepoints]
    glyphs = {".notdef": glyph_for(None)}
    glyphs.update({f"u{value:04X}": glyph_for(value) for value in codepoints})

    builder = FontBuilder(1000, isTTF=True)
    builder.setupGlyphOrder(glyph_order)
    builder.setupCharacterMap({value: f"u{value:04X}" for value in codepoints})
    builder.setupGlyf(glyphs)
    builder.setupHorizontalMetrics({name: (600, 40) for name in glyph_order})
    builder.setupHorizontalHeader(ascent=820, descent=-180)
    builder.setupHead(created=2082844800, modified=2082844800)
    builder.setupNameTable({
        "familyName": "Stasis Parity",
        "styleName": "Regular",
        "uniqueFontIdentifier": "Stasis-Parity-1",
        "fullName": "Stasis Parity Regular",
        "psName": "StasisParity-Regular",
        "version": "Version 1.000",
    })
    builder.setupOS2(
        sTypoAscender=820,
        sTypoDescender=-180,
        usWinAscent=820,
        usWinDescent=180,
    )
    builder.setupPost()
    builder.setupMaxp()
    builder.font.recalcTimestamp = False
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    builder.save(OUTPUT)


if __name__ == "__main__":
    main()
