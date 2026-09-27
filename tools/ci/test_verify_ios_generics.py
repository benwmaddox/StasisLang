import json
import struct
import tempfile
import unittest
import zlib
from pathlib import Path

from tools.ci.verify_ios_generics import EvidenceError, build_evidence, main


WIDTH = 1179
HEIGHT = 2556
BACKGROUND = (10, 20, 41, 255)
TEAL = (41, 184, 133, 255)
RED = (220, 30, 40, 255)
TRANSPARENT_TEAL = (41, 184, 133, 0)


def _png_chunk(chunk_type: bytes, payload: bytes) -> bytes:
    return (
        struct.pack(">I", len(payload))
        + chunk_type
        + payload
        + struct.pack(">I", zlib.crc32(chunk_type + payload) & 0xFFFFFFFF)
    )


def write_png(
    path: Path,
    accent: tuple[int, int, int, int],
    failure_pixels: int = 0,
) -> None:
    rows = bytearray()
    accent_rows = 32
    for y in range(HEIGHT):
        rows.append(0)
        if y < accent_rows:
            rows.extend(bytes(accent) * WIDTH)
            continue
        red_count = min(WIDTH, failure_pixels)
        rows.extend(bytes(RED) * red_count)
        rows.extend(bytes(BACKGROUND) * (WIDTH - red_count))
        failure_pixels -= red_count
    ihdr = struct.pack(">IIBBBBB", WIDTH, HEIGHT, 8, 6, 0, 0, 0)
    path.write_bytes(
        b"\x89PNG\r\n\x1a\n"
        + _png_chunk(b"IHDR", ihdr)
        + _png_chunk(b"IDAT", zlib.compress(bytes(rows)))
        + _png_chunk(b"IEND", b"")
    )


def write_json(path: Path, value: dict) -> None:
    path.write_text(json.dumps(value), encoding="utf-8")


class IosGenericsVerifierTest(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.receipt = self.root / "receipt.json"
        self.frame = self.root / "frame.png"
        self.log = self.root / "simulator.log"
        self.bounds_low = self.root / "bounds-low.json"
        self.bounds_high = self.root / "bounds-high.json"
        self.crash_low = self.root / "bounds-low.ips"
        self.crash_high = self.root / "bounds-high.ips"
        self.output = self.root / "evidence.json"
        write_json(
            self.receipt,
            {
                "schema": "stasis.ios.generics.v1",
                "main_result": 0,
                "tick_result": 0,
                "render_result": 0,
                "digest": 507,
                "frame": 1,
            },
        )
        write_png(self.frame, TEAL)
        self.log.write_text(
            "Stasis provenance: generics tag=v0 commit=abc renderer=gfx_cmd schema=7\n"
            "Stasis iOS generics acceptance digest=507 frame=1 receipt=/tmp/result.json\n",
            encoding="utf-8",
        )
        self.crash_low.write_text(
            "Exception Type: EXC_BREAKPOINT (SIGTRAP)\n", encoding="utf-8"
        )
        self.crash_high.write_text(
            "Exception Type: EXC_BREAKPOINT (SIGTRAP)\n", encoding="utf-8"
        )
        write_json(
            self.bounds_low,
            {
                "schema": "stasis.ios.generics.bounds.v1",
                "label": "low",
                "index": -1,
                "fatal": True,
                "signal": "SIGTRAP",
                "exception": "EXC_BREAKPOINT",
                "crash_report": str(self.crash_low),
            },
        )
        write_json(
            self.bounds_high,
            {
                "schema": "stasis.ios.generics.bounds.v1",
                "label": "high",
                "index": 2,
                "fatal": True,
                "signal": "SIGTRAP",
                "exception": "EXC_BREAKPOINT",
                "crash_report": str(self.crash_high),
            },
        )

    def evidence(self) -> dict:
        return build_evidence(
            self.receipt,
            self.frame,
            [self.log],
            self.bounds_low,
            self.bounds_high,
        )

    def test_success_writes_aggregate_evidence(self) -> None:
        result = main([
            "--receipt", str(self.receipt),
            "--frame", str(self.frame),
            "--log", str(self.log),
            "--bounds-low", str(self.bounds_low),
            "--bounds-high", str(self.bounds_high),
            "--output", str(self.output),
        ])
        self.assertEqual(result, 0)
        evidence = json.loads(self.output.read_text(encoding="utf-8"))
        self.assertEqual(evidence["schema"], "stasis.ios.generics.evidence.v1")
        self.assertEqual(evidence["receipt"]["digest"], 507)
        self.assertGreater(evidence["frame"]["teal_pixels"], WIDTH * HEIGHT // 100)
        self.assertGreater(evidence["frame"]["background_pixels"], WIDTH * HEIGHT // 2)
        self.assertLess(evidence["frame"]["failure_pixels"], 100)
        self.assertIn("renderer=gfx_cmd", evidence["log_markers"]["provenance"])
        self.assertEqual(evidence["bounds"]["low"]["index"], -1)
        self.assertEqual(evidence["bounds"]["high"]["index"], 2)
        self.assertEqual(
            len(evidence["bounds"]["low"]["crash_report_evidence"]["sha256"]),
            64,
        )

    def test_wrong_digest_is_rejected(self) -> None:
        value = json.loads(self.receipt.read_text(encoding="utf-8"))
        value["digest"] = 506
        write_json(self.receipt, value)
        with self.assertRaisesRegex(EvidenceError, "expected digest=507"):
            self.evidence()

    def test_red_frame_is_rejected(self) -> None:
        write_png(self.frame, TEAL, failure_pixels=100)
        with self.assertRaisesRegex(EvidenceError, "digest-failure red rectangle"):
            self.evidence()

    def test_error_log_is_rejected(self) -> None:
        self.log.write_text(
            "dyld: Library not loaded: @rpath/libstasis.dylib\n",
            encoding="utf-8",
        )
        with self.assertRaisesRegex(EvidenceError, "link evidence"):
            self.evidence()

    def test_nonfatal_bounds_evidence_is_rejected(self) -> None:
        value = json.loads(self.bounds_high.read_text(encoding="utf-8"))
        value["fatal"] = False
        write_json(self.bounds_high, value)
        with self.assertRaisesRegex(EvidenceError, "bounds evidence is not fatal"):
            self.evidence()

    def test_transparent_teal_is_rejected(self) -> None:
        write_png(self.frame, TRANSPARENT_TEAL)
        with self.assertRaisesRegex(EvidenceError, "digest-success teal rectangle"):
            self.evidence()

    def test_missing_positive_log_marker_is_rejected(self) -> None:
        self.log.write_text(
            "Stasis provenance: generics tag=v0 commit=abc renderer=gfx_cmd schema=7\n",
            encoding="utf-8",
        )
        with self.assertRaisesRegex(EvidenceError, "digest/frame marker"):
            self.evidence()


if __name__ == "__main__":
    unittest.main()
