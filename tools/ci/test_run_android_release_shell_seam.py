import json
import struct
import tempfile
import unittest
import zlib
from pathlib import Path

from tools.ci import run_android_release_shell_seam as seam


def write_rgb_png(
    path: Path, width: int, height: int, rows: list[list[tuple[int, int, int]]]
):
    def chunk(kind: bytes, payload: bytes) -> bytes:
        return (
            struct.pack(">I", len(payload))
            + kind
            + payload
            + struct.pack(">I", zlib.crc32(kind + payload))
        )

    raw = b"".join(b"\0" + b"".join(bytes(pixel) for pixel in row) for row in rows)
    header = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    path.write_bytes(
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(raw))
        + chunk(b"IEND", b"")
    )


class AndroidReleaseShellSeamTests(unittest.TestCase):
    def test_parses_and_validates_stable_marker(self):
        values = [
            {
                "schema": seam.SCHEMA,
                "test_id": "IT-017",
                "event": "initialized",
                "frame": 0,
            },
            {
                "schema": seam.SCHEMA,
                "test_id": "IT-017",
                "event": "frame",
                "frame": 1,
            },
            {
                "schema": seam.SCHEMA,
                "test_id": "IT-017",
                "event": "stable",
                "frame": 30,
                "state_checksum": 1210,
                "command_trace": 77,
                "accepted": 30,
                "presented": 30,
                "rejected": 0,
                "validation": 0,
            },
        ]
        log = "\n".join(f"I/Stasis: Stasis seam: {json.dumps(value)}" for value in values)
        markers = seam.parse_markers(log, "IT-017")
        stable = seam.validate_markers(
            markers,
            {"stable_frame": 30, "state_checksum": 1210, "command_trace": 77},
        )
        self.assertEqual(stable["presented"], 30)

    def test_marker_mismatch_names_the_field(self):
        markers = [
            {"event": "initialized", "frame": 0},
            {"event": "frame", "frame": 1},
            {
                "event": "stable",
                "frame": 30,
                "state_checksum": 5,
                "command_trace": 77,
                "accepted": 30,
                "presented": 30,
                "rejected": 0,
                "validation": 0,
            },
        ]
        with self.assertRaisesRegex(seam.SeamError, "state_checksum"):
            seam.validate_markers(
                markers,
                {"stable_frame": 30, "state_checksum": 1210, "command_trace": 77},
            )

    def test_validates_named_capture_regions(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "capture.png"
            rows = [[(230, 31, 41)] * 5 + [(26, 199, 184)] * 5 for _ in range(6)]
            write_rgb_png(path, 10, 6, rows)
            observed = seam.validate_regions(
                path,
                {
                    "logical_size": [10, 6],
                    "regions": [
                        {
                            "name": "red",
                            "center": [1, 3],
                            "rgb": [230, 31, 41],
                            "tolerance": 0,
                        },
                        {
                            "name": "teal",
                            "center": [8, 3],
                            "rgb": [26, 199, 184],
                            "tolerance": 0,
                        },
                    ],
                },
            )
            self.assertEqual([item["name"] for item in observed], ["red", "teal"])


if __name__ == "__main__":
    unittest.main()
