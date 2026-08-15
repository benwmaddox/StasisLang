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
    def test_evidence_target_matches_packaged_android_abi(self):
        self.assertEqual(
            seam.release_shell_target({"target": "android-arm64"}),
            "android-arm64-release-shell",
        )
        self.assertEqual(
            seam.release_shell_target({"target": "android-x86_64"}),
            "android-x86_64-release-shell",
        )
        for invalid in ("android-foo", "ios-arm64", None):
            with self.assertRaisesRegex(seam.SeamError, "invalid Android package target"):
                seam.release_shell_target({"target": invalid})

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

    def test_foreground_check_leaves_test_activity_untouched(self):
        calls = []

        def fake_run(_adb, _serial, *arguments, **_options):
            calls.append(arguments)
            return "mCurrentFocus=Window{123 u0 com.example.seam/.MainActivity}"

        with mock.patch.object(seam, "_run", side_effect=fake_run):
            changed = seam.ensure_test_activity_foreground(
                Path("adb"),
                "device",
                "com.example.seam",
                "com.example.seam/.MainActivity",
            )

        self.assertFalse(changed)
        self.assertEqual(
            calls,
            [("shell", "dumpsys", "window", "windows")],
        )

    def test_foreground_check_dismisses_system_dialog_and_restarts(self):
        calls = []

        def fake_run(_adb, _serial, *arguments, **_options):
            calls.append(arguments)
            if arguments == ("shell", "dumpsys", "window", "windows"):
                return "mCurrentFocus=Window{456 u0 android/.AppNotRespondingDialog}"
            return ""

        with mock.patch.object(seam, "_run", side_effect=fake_run):
            changed = seam.ensure_test_activity_foreground(
                Path("adb"),
                "device",
                "com.example.seam",
                "com.example.seam/.MainActivity",
            )

        self.assertTrue(changed)
        self.assertIn(
            ("shell", "input", "keyevent", "KEYCODE_BACK"),
            calls,
        )

    def test_foreground_check_taps_system_dialog_action_before_restarting(self):
        calls = []

        def fake_run(_adb, _serial, *arguments, **_options):
            calls.append(arguments)
            if arguments == ("shell", "dumpsys", "window", "windows"):
                return "mCurrentFocus=Window{456 u0 android/.AppNotRespondingDialog}"
            if arguments == ("shell", "uiautomator", "dump", "/dev/tty"):
                return (
                    "UI hierarchy dumped\n<?xml version='1.0' encoding='UTF-8' "
                    "standalone='yes'?><hierarchy><node "
                    "text=\"Pixel Launcher isn't responding\" />"
                    "<node text=\"Close app\" "
                    "bounds=\"[120,600][420,720]\" /></hierarchy>"
                )
            return ""

        with mock.patch.object(seam, "_run", side_effect=fake_run):
            changed = seam.ensure_test_activity_foreground(
                Path("adb"),
                "device",
                "com.example.seam",
                "com.example.seam/.MainActivity",
            )

        self.assertTrue(changed)
        self.assertIn(("shell", "input", "tap", "270", "660"), calls)
        self.assertNotIn(("shell", "input", "keyevent", "KEYCODE_BACK"), calls)
        self.assertIn(
            (
                "shell",
                "am",
                "start",
                "-W",
                "-n",
                "com.example.seam/.MainActivity",
            ),
            calls,
        )

    def test_foreground_check_prefers_wait_over_closing_system_component(self):
        calls = []

        def fake_run(_adb, _serial, *arguments, **_options):
            calls.append(arguments)
            if arguments == ("shell", "dumpsys", "window", "windows"):
                return "mCurrentFocus=Window{456 u0 android/.AppNotRespondingDialog}"
            if arguments == ("shell", "uiautomator", "dump", "/dev/tty"):
                return (
                    "<?xml version='1.0' encoding='UTF-8'?><hierarchy>"
                    "<node resource-id=\"android:id/alertTitle\" "
                    "text=\"System UI isn't responding\" />"
                    "<node text=\"Close app\" bounds=\"[120,600][420,720]\" />"
                    "<node text=\"Wait\" bounds=\"[120,720][420,840]\" />"
                    "</hierarchy>"
                )
            return ""

        with mock.patch.object(seam, "_run", side_effect=fake_run):
            changed = seam.ensure_test_activity_foreground(
                Path("adb"),
                "device",
                "com.example.seam",
                "com.example.seam/.MainActivity",
            )

        self.assertTrue(changed)
        self.assertIn(("shell", "input", "tap", "270", "780"), calls)
        self.assertNotIn(("shell", "input", "tap", "270", "660"), calls)

    def test_foreground_check_dismisses_layered_anr_over_focused_activity(self):
        calls = []

        def fake_run(_adb, _serial, *arguments, **_options):
            calls.append(arguments)
            if arguments == ("shell", "dumpsys", "window", "windows"):
                return (
                    "mCurrentFocus=Window{123 u0 com.example.seam/.MainActivity}\n"
                    "Window #2 Window{456 u0 android/.AppNotRespondingDialog}"
                )
            if arguments == ("shell", "uiautomator", "dump", "/dev/tty"):
                return (
                    "<?xml version='1.0' encoding='UTF-8'?><hierarchy>"
                    "<node resource-id=\"android:id/alertTitle\" "
                    "text=\"Pixel Launcher isn't responding\" />"
                    "<node text=\"Close app\" bounds=\"[120,600][420,720]\" />"
                    "</hierarchy>"
                )
            return ""

        with mock.patch.object(seam, "_run", side_effect=fake_run):
            changed = seam.ensure_test_activity_foreground(
                Path("adb"),
                "device",
                "com.example.seam",
                "com.example.seam/.MainActivity",
            )

        self.assertTrue(changed)
        self.assertIn(("shell", "input", "tap", "270", "660"), calls)
        self.assertIn(
            (
                "shell",
                "am",
                "start",
                "-W",
                "-n",
                "com.example.seam/.MainActivity",
            ),
            calls,
        )
        self.assertIn(
            (
                "shell",
                "am",
                "start",
                "-W",
                "-n",
                "com.example.seam/.MainActivity",
            ),
            calls,
        )

    def test_dialog_action_does_not_hide_product_anr(self):
        calls = []

        def fake_run(_adb, _serial, *arguments, **_options):
            calls.append(arguments)
            if arguments == ("shell", "uiautomator", "dump", "/dev/tty"):
                return (
                    "<?xml version='1.0' encoding='UTF-8'?><hierarchy>"
                    "<node resource-id=\"android:id/alertTitle\" "
                    "text=\"Stasis Android Seam isn't responding\" />"
                    "<node text=\"Wait\" bounds=\"[120,720][420,840]\" />"
                    "</hierarchy>"
                )
            return ""

        with mock.patch.object(seam, "_run", side_effect=fake_run):
            dismissed = seam.dismiss_system_dialog_action(Path("adb"), "device")

        self.assertFalse(dismissed)
        self.assertNotIn(("shell", "input", "tap", "270", "780"), calls)

    def test_capture_mismatch_dismisses_undetected_system_dialog(self):
        calls = []

        def fake_run(_adb, _serial, *arguments, **_options):
            calls.append(arguments)
            if arguments == ("exec-out", "screencap", "-p"):
                return b"capture"
            return ""

        with tempfile.TemporaryDirectory() as temporary:
            capture = Path(temporary) / "frame.png"
            with (
                mock.patch.object(seam, "_run", side_effect=fake_run),
                mock.patch.object(
                    seam, "ensure_test_activity_foreground", return_value=False
                ),
                mock.patch.object(
                    seam, "validate_regions",
                    side_effect=[seam.SeamError("covered"), [{"name": "red"}]],
                ),
                mock.patch.object(
                    seam, "dismiss_system_dialog_action", return_value=True
                ) as dismiss,
                mock.patch.object(seam.time, "sleep"),
            ):
                observed = seam.capture_until_regions_match(
                    Path("adb"),
                    "device",
                    capture,
                    {"logical_size": [640, 360], "regions": []},
                    seam.time.monotonic() + 5,
                    "com.example.seam",
                    "com.example.seam/.MainActivity",
                )

        self.assertEqual(observed, [{"name": "red"}])
        dismiss.assert_called_once_with(Path("adb"), "device")
        self.assertIn(
            (
                "shell",
                "am",
                "start",
                "-W",
                "-n",
                "com.example.seam/.MainActivity",
            ),
            calls,
        )

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
            if index == 5:
                marker.update({"x": 259.183, "y": 518.118, "x_n": 0.72, "y_n": 0.7196})
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
                expected.update(
                    {"x_min": 240, "y_min": 480, "x_n": 0.75, "y_n": 0.75}
                )
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

    def test_validates_ordered_orientation_metrics_and_pointer_mapping(self):
        stages = [
            (1, 1, 40, 1, 1001, 1601, 0x101),
            (2, 2, 80, 2, 1601, 1001, 0x202),
            (3, 1, 120, 3, 1001, 1601, 0x303),
        ]
        markers = []
        for sequence, kind, tick, generation, width, height, trace in stages:
            native_scale = min(width / 360, height / 720)
            drawable_width = int(360 * native_scale)
            drawable_height = int(720 * native_scale)
            scale = min(drawable_width / 360, drawable_height / 720)
            markers.append(
                {
                    "event": "probe",
                    "probe_sequence": sequence,
                    "probe_kind": kind,
                    "probe_tick": tick,
                    "pointer_id": 1,
                    "pointer_count": 2,
                    "went_up": 1,
                    "x": 270.0,
                    "y": 540.0,
                    "x_n": 0.75,
                    "y_n": 0.75,
                    "safe_x": 0.0,
                    "safe_y": 0.0,
                    "safe_w": 360.0,
                    "safe_h": 720.0,
                    "logical_w": 360.0,
                    "logical_h": 720.0,
                    "native_w": width,
                    "native_h": height,
                    "drawable_w": drawable_width,
                    "drawable_h": drawable_height,
                    "display_generation": generation,
                    "density_generation": generation,
                    "frame_display_generation": generation,
                    "frame_density_generation": generation,
                    "content_scale": scale,
                    "raster_scale": max(1.0, min(8.0, scale)),
                    "state_checksum": 4000 + sequence * 100 + kind * 10 + generation,
                    "command_trace": trace,
                }
            )
        expectations = {
            "logical_size": [360, 720],
            "orientation": {
                "coordinate_tolerance": 1.0,
                "surface_tolerance": 0,
                "display_size": [1001, 1601],
                "safe_viewport": [0, 0, 360, 720],
                "touch": [270, 540],
                "stages": [
                    {"name": "portrait", "sequence": 1, "kind": 1, "orientation": "portrait"},
                    {"name": "landscape", "sequence": 2, "kind": 2, "orientation": "landscape"},
                    {"name": "restored_portrait", "sequence": 3, "kind": 1, "orientation": "portrait"},
                ],
            },
        }
        observed = seam.validate_orientation_markers(
            markers,
            expectations,
            {
                "portrait": (1001, 1601),
                "landscape": (1601, 1001),
                "restored_portrait": (1001, 1601),
            },
        )
        self.assertEqual([1, 2, 3], [item["probe_sequence"] for item in observed])
        with self.assertRaisesRegex(seam.SeamError, "configured size mismatch"):
            seam.validate_orientation_markers(
                markers,
                expectations,
                {
                    "portrait": (999, 1599),
                    "landscape": (1599, 999),
                    "restored_portrait": (999, 1599),
                },
            )
        markers[1]["frame_display_generation"] = 1
        with self.assertRaisesRegex(seam.SeamError, "frame_display_generation"):
            seam.validate_orientation_markers(
                markers,
                expectations,
                {
                    "portrait": (1001, 1601),
                    "landscape": (1601, 1001),
                    "restored_portrait": (1001, 1601),
                },
            )

    def test_rejects_regressing_orientation_generation(self):
        expectations = {
            "logical_size": [360, 720],
            "orientation": {
                "coordinate_tolerance": 1.0,
                "surface_tolerance": 0,
                "display_size": [1001, 1601],
                "safe_viewport": [0, 0, 360, 720],
                "touch": [270, 540],
                "stages": [
                    {"name": "portrait", "sequence": 1, "kind": 1, "orientation": "portrait"},
                    {"name": "restored", "sequence": 2, "kind": 1, "orientation": "portrait"},
                ],
            },
        }
        marker = {
            "event": "probe",
            "probe_kind": 1,
            "pointer_id": 1,
            "pointer_count": 2,
            "went_up": 1,
            "x": 270.0,
            "y": 540.0,
            "x_n": 0.75,
            "y_n": 0.75,
            "safe_x": 0.0,
            "safe_y": 0.0,
            "safe_w": 360.0,
            "safe_h": 720.0,
            "logical_w": 360.0,
            "logical_h": 720.0,
            "native_w": 1001,
            "native_h": 1601,
            "drawable_w": 800,
            "drawable_h": 1601,
            "display_generation": 2,
            "density_generation": 1,
            "frame_display_generation": 2,
            "frame_density_generation": 1,
            "content_scale": 800 / 360,
            "raster_scale": 800 / 360,
            "command_trace": 1,
        }
        markers = []
        for sequence, tick in ((1, 10), (2, 20)):
            value = dict(marker)
            value.update(
                probe_sequence=sequence,
                probe_tick=tick,
                state_checksum=4000 + sequence * 100 + 10 + 2,
                command_trace=sequence,
            )
            markers.append(value)
        with self.assertRaisesRegex(seam.SeamError, "not strictly ordered"):
            seam.validate_orientation_markers(
                markers,
                expectations,
                {"portrait": (1001, 1601), "restored": (1001, 1601)},
            )

    def test_parses_only_an_explicit_wm_size_override(self):
        self.assertEqual(
            "1001x1601",
            seam.parse_wm_size_override(
                "Physical size: 1080x2400\nOverride size: 1001x1601\n"
            ),
        )
        self.assertIsNone(seam.parse_wm_size_override("Physical size: 1080x2400\n"))

    def test_cleanup_continues_after_force_stop_failure(self):
        calls = []

        def fake_run(_adb, _serial, *arguments, **_options):
            calls.append(arguments)
            if arguments[:3] == ("shell", "am", "force-stop"):
                raise seam.SeamError("injected force-stop failure")
            return ""

        with mock.patch.object(seam, "_run", side_effect=fake_run):
            errors = seam.restore_device_state(
                Path("adb"),
                "device",
                "com.example.seam",
                True,
                "null",
                {
                    "wm_size_override": None,
                    "user_rotation": "3",
                    "accelerometer_rotation": "1",
                },
            )

        self.assertEqual(len(errors), 1)
        self.assertIn("force-stop", errors[0])
        self.assertIn(("uninstall", "com.example.seam"), calls)
        self.assertIn(("shell", "wm", "size", "reset"), calls)
        self.assertIn(
            ("shell", "settings", "put", "system", "user_rotation", "3"),
            calls,
        )

    def test_cleanup_restores_orientation_and_prior_display_override(self):
        calls = []

        def fake_run(_adb, _serial, *arguments, **_kwargs):
            calls.append(arguments)
            return ""

        with mock.patch.object(seam, "_run", side_effect=fake_run):
            errors = seam.restore_device_state(
                Path("adb"),
                "device",
                "com.example.seam",
                False,
                "null",
                {
                    "wm_size_override": "901x1501",
                    "user_rotation": "3",
                    "accelerometer_rotation": "1",
                },
            )
        self.assertEqual([], errors)
        self.assertIn(("shell", "wm", "size", "901x1501"), calls)
        self.assertIn(
            ("shell", "settings", "put", "system", "user_rotation", "3"),
            calls,
        )
        self.assertIn(
            (
                "shell",
                "settings",
                "put",
                "system",
                "accelerometer_rotation",
                "1",
            ),
            calls,
        )
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
