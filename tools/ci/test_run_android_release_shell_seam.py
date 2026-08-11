import json
import struct
import tempfile
import unittest
import zlib
from pathlib import Path
from unittest import mock

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

    def test_selects_real_letterbox_for_each_surface_orientation(self):
        self.assertEqual(
            seam.outside_letterbox_point([360, 720], (1080, 2400)),
            (540, 60),
        )
        self.assertEqual(
            seam.outside_letterbox_point([360, 720], (1920, 1080)),
            (345, 540),
        )
        self.assertEqual(
            seam.logical_to_native([270, 540], [360, 720], (1080, 2400)),
            (810, 1740),
        )

    def test_validates_ordered_android_touch_probes(self):
        markers = []
        expected_probes = []
        kinds = [1, 2, 3, 4, 5]
        counts = [(1, 0, 0), (1, 0, 1), (2, 0, 1), (2, 1, 1), (2, 1, 2)]
        for index, (kind, count) in enumerate(zip(kinds, counts), start=1):
            marker = {
                "event": "probe",
                "probe_sequence": index,
                "probe_kind": kind,
                "probe_tick": 40 + index * 5,
                "pointer_id": 1,
                "pointer_count": 2,
                "down_count": count[0],
                "move_count": count[1],
                "up_count": count[2],
                "state_transitions": int(index >= 3),
                "is_down": int(index in (1, 3, 4)),
                "went_down": int(index in (1, 3)),
                "went_up": int(index in (2, 5)),
                "input_phase": max(0, index - 2),
                "x": 0.0 if index < 3 else 90.0 * (index - 2),
                "y": 360.0 if index < 3 else 180.0 * (index - 2),
                "x_n": 0.0 if index < 3 else 0.25 * (index - 2),
                "y_n": 0.5 if index < 3 else 0.25 * (index - 2),
                "state_checksum": 3215 if index == 5 else 0,
                "command_trace": 77,
            }
            markers.append(marker)
            expected = {
                "sequence": index,
                "kind": kind,
                "down_count": count[0],
                "move_count": count[1],
                "up_count": count[2],
                "state_transitions": int(index >= 3),
                "is_down": marker["is_down"],
                "went_down": marker["went_down"],
                "went_up": marker["went_up"],
            }
            if index == 5:
                expected["state_checksum"] = 3215
            expected_probes.append(expected)
        observed = seam.validate_touch_markers(
            markers,
            {
                "touch": {
                    "coordinate_tolerance": 16.0,
                    "final_command_trace": 77,
                    "probes": expected_probes,
                }
            },
        )
        self.assertEqual(
            [item["probe_sequence"] for item in observed], [1, 2, 3, 4, 5]
        )

    def test_rejects_touch_probes_on_the_same_tick(self):
        markers = [
            {
                "event": "probe",
                "probe_sequence": sequence,
                "probe_kind": sequence,
                "probe_tick": 42,
                "pointer_id": 1,
                "pointer_count": 2,
            }
            for sequence in (1, 2)
        ]
        with self.assertRaisesRegex(seam.SeamError, "strictly ordered"):
            seam.validate_touch_markers(
                markers,
                {
                    "touch": {
                        "coordinate_tolerance": 1.0,
                        "probes": [
                            {"sequence": 1, "kind": 1},
                            {"sequence": 2, "kind": 2},
                        ],
                    }
                },
            )

    def test_cleanup_continues_after_force_stop_failure(self):
        calls = []

        def fake_run(_adb, _serial, *arguments, **_options):
            calls.append(arguments)
            if arguments[:3] == ("shell", "am", "force-stop"):
                raise seam.SeamError("injected force-stop failure")
            return ""

        with mock.patch.object(seam, "_run", side_effect=fake_run):
            errors = seam.restore_device_state(
                Path("adb"), "device", "com.example.seam", True, "null"
            )

        self.assertEqual(len(errors), 1)
        self.assertIn("force-stop", errors[0])
        self.assertIn(("uninstall", "com.example.seam"), calls)
        self.assertIn(
            (
                "shell",
                "settings",
                "delete",
                "secure",
                "immersive_mode_confirmations",
            ),
            calls,
        )


if __name__ == "__main__":
    unittest.main()
