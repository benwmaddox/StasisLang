#!/usr/bin/env python3
"""Generate the small deterministic digit font bundled with Pointer Pong."""

from pathlib import Path

from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen


ROOT = Path(__file__).resolve().parents[2]
OUTPUT = ROOT / "samples" / "pointer_pong" / "assets" / "score.ttf"

DIGITS = (
    ("01110", "10001", "10011", "10101", "11001", "10001", "01110"),
    ("00100", "01100", "00100", "00100", "00100", "00100", "01110"),
    ("01110", "10001", "00001", "00010", "00100", "01000", "11111"),
    ("11110", "00001", "00001", "01110", "00001", "00001", "11110"),
    ("00010", "00110", "01010", "10010", "11111", "00010", "00010"),
    ("11111", "10000", "10000", "11110", "00001", "00001", "11110"),
    ("00110", "01000", "10000", "11110", "10001", "10001", "01110"),
    ("11111", "00001", "00010", "00100", "01000", "01000", "01000"),
    ("01110", "10001", "10001", "01110", "10001", "10001", "01110"),
    ("01110", "10001", "10001", "01111", "00001", "00010", "11100"),
)


def glyph_for(rows: tuple[str, ...]) -> object:
    pen = TTGlyphPen(None)
    for row, bits in enumerate(rows):
        for column, bit in enumerate(bits):
            if bit == "0":
                continue
            x = 80 + column * 100
            y = 100 + (6 - row) * 100
            pen.moveTo((x, y))
            pen.lineTo((x + 90, y))
            pen.lineTo((x + 90, y + 90))
            pen.lineTo((x, y + 90))
            pen.closePath()
    return pen.glyph()


def main() -> None:
    glyph_order = [".notdef"] + [f"u{ord('0') + digit:04X}" for digit in range(10)]
    glyphs = {".notdef": TTGlyphPen(None).glyph()}
    glyphs.update(
        {
            f"u{ord('0') + digit:04X}": glyph_for(rows)
            for digit, rows in enumerate(DIGITS)
        }
    )

    builder = FontBuilder(1000, isTTF=True)
    builder.setupGlyphOrder(glyph_order)
    builder.setupCharacterMap({ord("0") + digit: f"u{ord('0') + digit:04X}" for digit in range(10)})
    builder.setupGlyf(glyphs)
    builder.setupHorizontalMetrics({name: (600, 40) for name in glyph_order})
    builder.setupHorizontalHeader(ascent=820, descent=-180)
    builder.setupHead(created=2082844800, modified=2082844800)
    builder.setupNameTable(
        {
            "familyName": "Stasis Pong Digits",
            "styleName": "Regular",
            "uniqueFontIdentifier": "Stasis-Pong-Digits-1",
            "fullName": "Stasis Pong Digits Regular",
            "psName": "StasisPongDigits-Regular",
            "version": "Version 1.000",
        }
    )
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
