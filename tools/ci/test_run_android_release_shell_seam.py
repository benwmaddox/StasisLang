import json
import struct
import tempfile
import unittest
import zlib
from pathlib import Path
from types import SimpleNamespace
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
    def test_optional_adb_command_timeout_returns_for_marker_polling(self):
        timeout = seam.subprocess.TimeoutExpired(
            ["adb", "shell", "am", "start"], 10
        )
        with mock.patch.object(seam.subprocess, "run", side_effect=timeout):
            self.assertEqual(
                "",
                seam._run(
                    Path("adb"),
                    "device",
                    "shell",
                    "am",
                    "start",
                    required=False,
                    timeout=10,
                ),
            )

    def test_install_policy_keeps_default_uninstall_and_allows_only_stateful_recovery(self):
        self.assertEqual(
            {
                "existing_installation": False,
                "replaced_existing_installation": False,
                "retained_installation": False,
            },
            seam.validate_install_policy(False, False, False, "IT-017", None),
        )
        self.assertEqual(
            {
                "existing_installation": False,
                "replaced_existing_installation": False,
                "retained_installation": True,
            },
            seam.validate_install_policy(False, True, False, "IT-022", "malformed-manifest"),
        )
        self.assertEqual(
            {
                "existing_installation": True,
                "replaced_existing_installation": True,
                "retained_installation": False,
            },
            seam.validate_install_policy(True, False, True, "IT-021", None),
        )
        with self.assertRaisesRegex(seam.SeamError, "existing test package"):
            seam.validate_install_policy(False, False, True, "IT-021", None)
        with self.assertRaisesRegex(seam.SeamError, "only allowed for the recovery"):
            seam.validate_install_policy(True, False, True, "IT-017", None)
        with self.assertRaisesRegex(seam.SeamError, "contradictory"):
            seam.validate_install_policy(False, True, True, "IT-022", "missing")
        with self.assertRaisesRegex(seam.SeamError, "only allowed for an IT-022"):
            seam.validate_install_policy(False, True, False, "IT-017", None)
        with self.assertRaisesRegex(seam.SeamError, "only allowed for an IT-022"):
            seam.validate_install_policy(False, True, False, "IT-022", "")

    def test_retention_requires_successful_rejection_evidence(self):
        self.assertTrue(seam.should_retain_installed_package(True, "passed"))
        self.assertFalse(seam.should_retain_installed_package(True, "failed"))
        self.assertFalse(seam.should_retain_installed_package(False, "passed"))

    def test_terminal_event_stops_rejection_polling_before_stable(self):
        self.assertEqual(
            "asset_rejected",
            seam.terminal_event({"asset_rejection": {"code": "missing_asset"}}),
        )
        self.assertEqual("stable", seam.terminal_event({"stable_frame": 30}))

    def test_it022_storage_probe_requires_absent_staging_and_root(self):
        self.assertEqual(
            {"staging_absent": True, "root_unpublished": True},
            seam.validate_rejection_storage_state("absent\n", "absent\n"),
        )
        with self.assertRaisesRegex(seam.SeamError, "published extraction state"):
            seam.validate_rejection_storage_state("present", "absent")

    def test_it022_process_identity_accepts_original_pid_after_overlay(self):
        with mock.patch.object(seam, "_run", return_value="1234\n") as run:
            self.assertEqual(
                "1234",
                seam.validate_rejection_process_identity(
                    Path("adb"), "emulator-5554", "com.example.seam", "1234"
                ),
            )
        run.assert_called_once_with(
            Path("adb"),
            "emulator-5554",
            "shell",
            "pidof",
            "com.example.seam",
            required=False,
        )

    def test_it022_process_identity_rejects_restart_after_overlay(self):
        with mock.patch.object(seam, "_run", return_value="5678\n"):
            with self.assertRaisesRegex(seam.SeamError, "crash loop"):
                seam.validate_rejection_process_identity(
                    Path("adb"), "emulator-5554", "com.example.seam", "1234"
                )

    def test_it022_storage_probe_classifies_present_path_and_uses_direct_run_as(self):
        result = SimpleNamespace(returncode=0, stdout="files/stasis_game\n", stderr="")
        with mock.patch.object(seam, "_run_result", return_value=result) as run_result:
            self.assertEqual(
                "present",
                seam.probe_rejection_storage_path(
                    Path("adb"), "emulator-5554", "com.example.seam", "files/stasis_game"
                ),
            )
        run_result.assert_called_once_with(
            Path("adb"),
            "emulator-5554",
            "shell",
            "run-as",
            "com.example.seam",
            "ls",
            "-d",
            "files/stasis_game",
        )

    def test_it022_storage_probe_parses_no_such_file_as_absent(self):
        self.assertEqual(
            "absent",
            seam.classify_rejection_storage_probe(
                "files/.stasis_game.staging",
                1,
                "",
                "ls: files/.stasis_game.staging: No such file or directory\n",
            ),
        )

    def test_it022_overlay_probe_accepts_structured_hierarchy_and_direct_command(self):
        diagnostic = "code=missing_asset path=assets/token.bin detail=asset is missing"
        ui_xml = (
            "UI hierarchy dumped to: /dev/tty\n"
            "<?xml version='1.0'?><hierarchy><node "
            "content-desc='Stasis runtime error&#10;Release runtime error&#10;"
            "Asset verification failed: code=missing_asset path=assets/token.bin "
            "detail=asset is missing' "
            "bounds='[0,0][4,4]' "
            "text=''/>"
            "</hierarchy>"
        )
        result = SimpleNamespace(returncode=0, stdout=ui_xml, stderr="")
        with tempfile.TemporaryDirectory() as temporary:
            capture = Path(temporary) / "stable-frame.png"
            ui_hierarchy = Path(temporary) / "ui-hierarchy.xml"
            write_rgb_png(capture, 4, 4, [[(80, 0, 0)] * 4 for _ in range(4)])
            screenshot = capture.read_bytes()
            with (
                mock.patch.object(seam, "_run_result", return_value=result) as run_result,
                mock.patch.object(seam, "_run", return_value=screenshot) as run,
            ):
                evidence = seam.capture_it022_error_overlay(
                    Path("adb"),
                    "emulator-5554",
                    diagnostic,
                    capture,
                    ui_hierarchy,
                    deadline_seconds=1,
                )
            self.assertEqual(ui_xml, ui_hierarchy.read_text(encoding="utf-8"))
        self.assertEqual(
            {
                "java_error_visible": True,
                "overlay_bounds": [0, 0, 4, 4],
                "overlay_red_pixels": 16,
                "attempts": 1,
            },
            evidence,
        )
        run_result.assert_called_once_with(
            Path("adb"),
            "emulator-5554",
            "exec-out",
            "uiautomator",
            "dump",
            "--compressed",
            "/dev/tty",
        )
        run.assert_called_once_with(
            Path("adb"), "emulator-5554", "exec-out", "screencap", "-p", text=False
        )

    def test_it022_overlay_probe_retries_until_hierarchy_is_ready(self):
        diagnostic = "code=missing_asset path=assets/token.bin detail=asset is missing"
        valid_xml = (
            "<?xml version='1.0'?><hierarchy><node "
            "content-desc='Stasis runtime error Release runtime error "
            "Asset verification failed code=missing_asset path=assets/token.bin "
            "detail=asset is missing' "
            "bounds='[0,0][4,4]' "
            "text=''/>"
            "</hierarchy>"
        )
        results = [
            SimpleNamespace(returncode=0, stdout="<?xml bad", stderr=""),
            SimpleNamespace(returncode=0, stdout=valid_xml, stderr=""),
        ]
        with tempfile.TemporaryDirectory() as temporary:
            capture = Path(temporary) / "stable-frame.png"
            ui_hierarchy = Path(temporary) / "ui-hierarchy.xml"
            write_rgb_png(capture, 4, 4, [[(80, 0, 0)] * 4 for _ in range(4)])
            screenshot = capture.read_bytes()
            with (
                mock.patch.object(seam, "_run_result", side_effect=results) as run_result,
                mock.patch.object(seam, "_run", return_value=screenshot),
                mock.patch.object(seam.time, "monotonic", side_effect=[0.0, 0.1]),
                mock.patch.object(seam.time, "sleep") as sleep,
            ):
                evidence = seam.capture_it022_error_overlay(
                    Path("adb"),
                    None,
                    diagnostic,
                    capture,
                    ui_hierarchy,
                    deadline_seconds=1,
                    retry_interval_seconds=0.2,
                )
            self.assertEqual(valid_xml, ui_hierarchy.read_text(encoding="utf-8"))
        self.assertEqual(2, evidence["attempts"])
        sleep.assert_called_once_with(0.2)
        self.assertEqual(2, run_result.call_count)

    def test_it022_overlay_probe_rejects_text_only_details(self):
        diagnostic = "code=missing_asset path=assets/token.bin detail=asset is missing"
        ui_xml = (
            "<hierarchy><node content-desc='Stasis runtime error' "
            "bounds='[0,0][4,4]' text='Release runtime error Asset verification failed "
            "code=missing_asset path=assets/token.bin detail=asset is missing'/></hierarchy>"
        )
        with self.assertRaisesRegex(seam.SeamError, "missing required text"):
            seam.validate_it022_error_overlay(ui_xml, diagnostic)

    def test_it022_overlay_probe_reports_command_failure_diagnostics(self):
        result = SimpleNamespace(
            returncode=1,
            stdout="",
            stderr="uiautomator: permission denied\n",
        )
        with mock.patch.object(seam, "_run_result", return_value=result):
            with tempfile.TemporaryDirectory() as temporary:
                with self.assertRaisesRegex(
                    seam.SeamError, "after 1 attempts.*permission denied"
                ):
                    seam.capture_it022_error_overlay(
                        Path("adb"),
                        None,
                        "native diagnostic",
                        Path(temporary) / "capture.png",
                        Path(temporary) / "ui-hierarchy.xml",
                        deadline_seconds=0,
                    )

    def test_it022_overlay_probe_rejects_malformed_or_unrelated_hierarchy(self):
        with self.assertRaisesRegex(seam.SeamError, "missing or incomplete"):
            seam.validate_it022_error_overlay("UI hierarchy dumped to: /dev/tty\n", "diag")
        with self.assertRaisesRegex(seam.SeamError, "malformed"):
            seam.validate_it022_error_overlay(
                "<hierarchy><node></hierarchy>", "diag"
            )
        with self.assertRaisesRegex(seam.SeamError, "no Stasis runtime error node"):
            seam.validate_it022_error_overlay(
                "<hierarchy><node content-desc='Other' text='diag'/></hierarchy>", "diag"
            )
        with self.assertRaisesRegex(seam.SeamError, "missing required text"):
            seam.validate_it022_error_overlay(
                "<hierarchy><node content-desc='Stasis runtime error' "
                "text='Release runtime error'/></hierarchy>",
                "diag",
            )

    def test_it022_overlay_capture_requires_visible_red_pixels(self):
        with tempfile.TemporaryDirectory() as temporary:
            capture = Path(temporary) / "capture.png"
            write_rgb_png(capture, 4, 4, [[(80, 0, 0)] * 4 for _ in range(4)])
            evidence = seam.validate_it022_overlay_capture(capture, [0, 0, 4, 4])
            self.assertEqual(
                {"overlay_bounds": [0, 0, 4, 4], "overlay_red_pixels": 16}, evidence
            )
            write_rgb_png(capture, 4, 4, [[(30, 30, 30)] * 4 for _ in range(4)])
            with self.assertRaisesRegex(seam.SeamError, "red-dominant"):
                seam.validate_it022_overlay_capture(capture, [0, 0, 4, 4])

    def test_it022_overlay_capture_rejects_invalid_bounds(self):
        with tempfile.TemporaryDirectory() as temporary:
            capture = Path(temporary) / "capture.png"
            write_rgb_png(capture, 4, 4, [[(80, 0, 0)] * 4 for _ in range(4)])
            with self.assertRaisesRegex(seam.SeamError, "outside captured PNG"):
                seam.validate_it022_overlay_capture(capture, [0, 0, 5, 4])

    def test_it022_storage_probe_rejects_run_as_failure(self):
        with self.assertRaisesRegex(seam.SeamError, "storage probe.*failed"):
            seam.classify_rejection_storage_probe(
                "files/stasis_game", 1, "", "run-as: package is not debuggable\n"
            )

    def test_it022_storage_probe_rejects_mixed_missing_path_diagnostics(self):
        with self.assertRaisesRegex(seam.SeamError, "storage probe.*failed"):
            seam.classify_rejection_storage_probe(
                "files/stasis_game",
                1,
                "",
                "ls: files/stasis_game: No such file or directory\n"
                "run-as: package is not debuggable\n",
            )

    def test_it022_storage_probe_rejects_unexpected_success_output_or_diagnostics(self):
        with self.assertRaisesRegex(seam.SeamError, "unexpected success output"):
            seam.classify_rejection_storage_probe(
                "files/stasis_game", 0, "files/other\n", ""
            )
        with self.assertRaisesRegex(seam.SeamError, "unexpected success output"):
            seam.classify_rejection_storage_probe(
                "files/stasis_game", 0, "files/stasis_game\n", "warning\n"
            )
        with self.assertRaisesRegex(seam.SeamError, "unexpected missing-path output"):
            seam.classify_rejection_storage_probe(
                "files/stasis_game",
                1,
                "unexpected\n",
                "ls: files/stasis_game: No such file or directory\n",
            )

    def test_it022_rejection_requires_native_diagnostic_and_no_game_markers(self):
        diagnostic = (
            "code=tampered_asset path=assets/token.bin detail=asset hash does not match the manifest"
        )
        markers = [
            {
                "schema": seam.SCHEMA,
                "test_id": "IT-022",
                "event": "asset_rejected",
                "frame": 0,
                "initialized": 0,
                "accepted": 0,
                "presented": 0,
                "asset_error": diagnostic,
            }
        ]
        result = seam.validate_asset_rejection_markers(
            markers,
            "I/Stasis: Stasis IT-022 asset verification rejected package: " + diagnostic,
            {
                "asset_rejection": {
                    "variant": "tampered",
                    "code": "tampered_asset",
                    "path": "assets/token.bin",
                }
            },
        )
        self.assertEqual("tampered_asset", result["code"])

    def test_it022_diagnostic_parser_preserves_paths_with_spaces(self):
        diagnostic = "code=missing_asset path=assets/dir with space.bin detail=asset is missing"
        result = seam.validate_asset_rejection_markers(
            [{"event": "asset_rejected", "accepted": 0, "presented": 0, "asset_error": diagnostic}],
            "Stasis IT-022 asset verification rejected package: " + diagnostic,
            {"asset_rejection": {"code": "missing_asset", "path": "assets/dir with space.bin"}},
        )
        self.assertEqual("assets/dir with space.bin", result["path"])

    def test_it022_rejection_rejects_initialized_marker(self):
        with self.assertRaisesRegex(seam.SeamError, "initialization/frame"):
            seam.validate_asset_rejection_markers(
                [{"event": "initialized", "frame": 0}],
                "",
                {"asset_rejection": {"code": "missing_asset"}},
            )

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

    def test_it023_storage_path_and_marker_validation(self):
        storage = {
            "exact_value": 230023,
            "corrupt_fallback": -2303,
            "unrelated_scope_fallback": -2311,
            "unrelated_key_fallback": -2312,
        }
        marker = {
            "storage_phase": 2,
            "storage_loaded_value": 230023,
            "storage_unrelated_scope": -2311,
            "storage_unrelated_key": -2312,
            "storage_traversal_rejected": 1,
            "storage_checksum": 2230023,
        }
        self.assertEqual(
            "files/it023_target/durable_value.i32",
            seam.validate_storage_relative_path("files/it023_target/durable_value.i32"),
        )
        self.assertEqual(
            2, seam.validate_storage_marker(marker, storage, 2)["storage_phase"]
        )
        for invalid in (
            "../escape.i32",
            "files/../escape.i32",
            "files/a/b.txt",
            "/data/local/tmp/x.i32",
        ):
            with self.subTest(path=invalid), self.assertRaisesRegex(
                seam.SeamError, "invalid app-private"
            ):
                seam.validate_storage_relative_path(invalid)
        marker["storage_loaded_value"] = -2303
        with self.assertRaisesRegex(seam.SeamError, "storage_loaded_value"):
            seam.validate_storage_marker(marker, storage, 2)

    def test_it023_requires_new_pid_and_exact_corruption_bytes(self):
        self.assertEqual("202", seam.validate_fresh_process_pid("101", "202"))
        with self.assertRaisesRegex(seam.SeamError, "fresh process PID"):
            seam.validate_fresh_process_pid("101", "101")
        self.assertEqual(
            "corrupt\n",
            seam.validate_storage_file_text("corrupt\n", "corrupt\n", "corrupt target"),
        )
        with self.assertRaisesRegex(seam.SeamError, "corrupt target storage bytes"):
            seam.validate_storage_file_text("230023\n", "corrupt\n", "corrupt target")
        self.assertIsNone(
            seam.validate_storage_write_result(
                "files/it023_target/durable_value.i32",
                0,
                b"corrupt\n",
                b"",
                b"corrupt\n",
            )
        )
        with self.assertRaisesRegex(seam.SeamError, "storage write failed"):
            seam.validate_storage_write_result(
                "files/it023_target/durable_value.i32",
                1,
                b"",
                b"permission denied",
                b"corrupt\n",
            )
        self.assertIsNone(
            seam.require_storage_file_absent(
                "files/escape.i32", 1, "", "cat: No such file or directory"
            )
        )
        with self.assertRaisesRegex(seam.SeamError, "traversal escaped"):
            seam.require_storage_file_absent("files/escape.i32", 0, "9\n", "")
        with self.assertRaisesRegex(seam.SeamError, "escape probe failed"):
            seam.require_storage_file_absent("files/escape.i32", 1, "", "permission denied")

    def test_it023_corruption_uses_one_bounded_remote_run_as_command(self):
        completed = SimpleNamespace(returncode=0, stdout=b"", stderr=b"")
        with mock.patch.object(
            seam.subprocess, "run", return_value=completed
        ) as run:
            seam.corrupt_storage_file(
                Path("adb"),
                "device",
                "com.example.seam",
                "files/it023_target/durable_value.i32",
            )

        command = run.call_args.args[0]
        self.assertEqual(["adb", "-s", "device", "shell"], command[:4])
        self.assertIn("run-as com.example.seam sh -c", command[4])
        self.assertIn(
            "> files/it023_target/durable_value.i32", command[4]
        )
        self.assertEqual(10, run.call_args.kwargs["timeout"])

    def test_it023_relaunch_restores_foreground_before_polling_markers(self):
        calls = []
        logs = iter(
            [
                "",
                (
                    'I/Stasis: Stasis seam: {"schema":"stasis.seam_test.v1",'
                    '"test_id":"IT-023","event":"stable"}'
                ),
            ]
        )

        def fake_run(_adb, _serial, *arguments, **_options):
            calls.append(arguments)
            if arguments[:3] == ("shell", "pidof", "com.example.seam"):
                return "222"
            if arguments[:2] == ("logcat", "-d"):
                return next(logs)
            return ""

        with (
            mock.patch.object(seam, "_run", side_effect=fake_run),
            mock.patch.object(
                seam, "ensure_test_activity_foreground", return_value=True
            ) as foreground,
            mock.patch.object(seam.time, "sleep") as sleep,
        ):
            pid, marker, _log = seam.wait_for_storage_launch(
                Path("adb"),
                "device",
                "com.example.seam",
                "com.example.seam/.MainActivity",
                "IT-023",
                {},
                "111",
                seam.time.monotonic() + 10,
            )

        self.assertEqual("222", pid)
        self.assertEqual("stable", marker["event"])
        foreground.assert_called_once_with(
            Path("adb"),
            "device",
            "com.example.seam",
            "com.example.seam/.MainActivity",
            wait_for_launch=False,
            intent_arguments=("--es", "stasis.seam_test_id", "IT-023"),
        )
        self.assertEqual([mock.call(0.25), mock.call(0.25)], sleep.call_args_list)

    def test_it023_relaunch_accepts_initialized_guest_storage_marker(self):
        initialized = (
            'I/Stasis: Stasis seam: {"schema":"stasis.seam_test.v1",'
            '"test_id":"IT-023","event":"initialized"}'
        )

        def fake_run(_adb, _serial, *arguments, **_options):
            if arguments[:3] == ("shell", "pidof", "com.example.seam"):
                return "222"
            if arguments[:2] == ("logcat", "-d"):
                return initialized
            return ""

        with (
            mock.patch.object(seam, "_run", side_effect=fake_run),
            mock.patch.object(seam, "ensure_test_activity_foreground") as foreground,
        ):
            pid, marker, _log = seam.wait_for_storage_launch(
                Path("adb"),
                "device",
                "com.example.seam",
                "com.example.seam/.MainActivity",
                "IT-023",
                {},
                "111",
                seam.time.monotonic() + 10,
            )

        self.assertEqual("222", pid)
        self.assertEqual("initialized", marker["event"])
        foreground.assert_not_called()

    def test_stable_marker_requires_a_positive_command_trace_without_fixed_oracle(self):
        for trace in (None, 0):
            stable = {
                "event": "stable",
                "frame": 30,
                "state_checksum": 1210,
                "accepted": 30,
                "presented": 30,
                "rejected": 0,
                "validation": 0,
            }
            if trace is not None:
                stable["command_trace"] = trace
            markers = [
                {"event": "initialized", "frame": 0},
                {"event": "frame", "frame": 1},
                stable,
            ]
            with self.subTest(trace=trace), self.assertRaisesRegex(
                seam.SeamError, "positive integer"
            ):
                seam.validate_markers(
                    markers,
                    {
                        "stable_frame": 30,
                        "state_checksum": 1210,
                        "require_command_trace": True,
                    },
                )

    def test_it021_validates_packaged_identity_and_offline_audio(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifest = root / "assets/manifest.json"
            manifest.parent.mkdir()
            manifest.write_text('{"schema":"stasis-assets","version":1,"assets":[]}', encoding="utf-8")
            package_manifest = root / "stasis_mobile_package.json"
            package_manifest.write_text("{}", encoding="utf-8")
            manifest_hash = __import__("hashlib").sha256(manifest.read_bytes()).hexdigest()
            stable = {
                "event": "stable", "frame": 30, "state_checksum": 2310,
                "accepted": 30, "presented": 30, "rejected": 0, "validation": 0,
                "asset_root": "/data/user/0/com.example.seam/files/stasis_game",
                "asset_manifest_sha256": manifest_hash,
                "sprite_handle": 1, "font_handle": 2, "cached_text_handle": 3,
                "audio_handle": 4, "voice_handle": 5,
                "direct_text_width": 90.0, "cached_text_width": 88.0,
                "audio_queued_before": 4, "audio_queued_after": 0,
                "audio_frames_mixed": 32, "audio_nonzero_after_prefix": 12,
                "audio_voice_state": 1, "audio_sample_checksum": 17,
                "audio_replay_checksum": 17, "audio_replay_matches": 1,
            }
            markers = [
                {"event": "initialized", "frame": 0},
                {"event": "frame", "frame": 1},
                stable,
            ]
            result = seam.validate_asset_audio_markers(
                markers,
                {
                    "stable_frame": 30,
                    "state_checksum": 2310,
                    "assets": {
                        "manifest_sha256": "computed_from_packaged_manifest",
                        "handles": {"sprite": "sprite_handle", "font": "font_handle"},
                        "minimum_text_width": 1,
                        "audio": {
                            "queued_frames_before": 4,
                            "queued_frames_after": 0,
                            "minimum_frames_mixed": 32,
                            "minimum_nonzero_samples_after_prefix": 2,
                            "voice_state": 1,
                            "sample_checksum": "nonzero",
                            "replay_matches": 1,
                        },
                    },
                },
                {"package_id": "com.example.seam", "assets": "."},
                package_manifest,
            )
            self.assertEqual(manifest_hash, result["manifest_sha256"])
            self.assertEqual("/data/user/0/com.example.seam/files/stasis_game", result["asset_root"])
            self.assertEqual(1, result["identities"]["sprite"]["handle"])
            stable["asset_root"] = "/data/data/com.example.seam/files/stasis_game"
            alias_result = seam.validate_asset_audio_markers(
                markers,
                {"stable_frame": 30, "state_checksum": 2310, "assets": {}},
                {"package_id": "com.example.seam", "assets": "."},
                package_manifest,
            )
            self.assertEqual(stable["asset_root"], alias_result["asset_root"])
            for malicious_root in (
                "/data/data/com.other.seam/files/stasis_game",
                "/data/user/0/com.example.seam/files/stasis_game/extra",
                "/data/user/0/com.example.seam/files/../other",
                "",
            ):
                stable["asset_root"] = malicious_root
                with self.assertRaisesRegex(
                    seam.SeamError, "asset_root expected one of.*actual"
                ):
                    seam.validate_asset_audio_markers(
                        markers,
                        {"stable_frame": 30, "state_checksum": 2310, "assets": {}},
                        {"package_id": "com.example.seam", "assets": "."},
                        package_manifest,
                    )
            stable["asset_root"] = "/data/user/0/com.example.seam/files/stasis_game"
            stable["audio_replay_matches"] = 0
            with self.assertRaisesRegex(seam.SeamError, "field audio_replay_matches"):
                seam.validate_asset_audio_markers(
                    markers,
                    {
                        "stable_frame": 30,
                        "state_checksum": 2310,
                        "assets": {
                            "handles": {"sprite": "sprite_handle"},
                            "minimum_text_width": 1,
                            "audio": {
                                "queued_frames_before": 4,
                                "queued_frames_after": 0,
                                "minimum_frames_mixed": 32,
                                "minimum_nonzero_samples_after_prefix": 2,
                                "voice_state": 1,
                                "sample_checksum": "nonzero",
                                "replay_matches": 1,
                            },
                        },
                    },
                    {"package_id": "com.example.seam", "assets": "."},
                    package_manifest,
                )

    def test_it021_failure_names_field_and_evidence_path(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifest = root / "assets/manifest.json"
            manifest.parent.mkdir()
            manifest.write_text('{"schema":"stasis-assets","version":1,"assets":[]}', encoding="utf-8")
            package_manifest = root / "stasis_mobile_package.json"
            package_manifest.write_text("{}", encoding="utf-8")
            stable = {
                "event": "stable", "frame": 30, "state_checksum": 2310,
                "accepted": 30, "presented": 30, "rejected": 0, "validation": 0,
                "asset_root": "/tmp/escape", "asset_manifest_sha256": "bad",
            }
            with self.assertRaisesRegex(seam.SeamError, "IT-021 field asset_manifest_sha256.*evidence path"):
                seam.validate_asset_audio_markers(
                    [{"event": "initialized", "frame": 0}, {"event": "frame", "frame": 1}, stable],
                    {"stable_frame": 30, "state_checksum": 2310, "assets": {}},
                    {"package_id": "com.example.seam", "assets": "."},
                    package_manifest,
                )

    def test_resource_lifecycle_requires_ready_generations_and_zero_failures(self):
        marker = {
            "event": "lifecycle",
            "resource_state": 1,
            "surface_generation": 2,
            "renderer_generation": 3,
            "restore_attempts": 1,
            "restore_failures": 0,
            "restore_reason": 5,
            "accepted": 1,
            "presented": 1,
        }
        expectations = {
            "lifecycle": {
                "stages": [
                    {"name": "initial", "min_renderer_generation": 1},
                    {"name": "resume", "min_renderer_generation": 2},
                ]
            }
        }
        observed = seam.validate_resource_lifecycle_markers(
            [marker], expectations, {"initial": marker, "resume": marker}
        )
        self.assertEqual(3, observed["resume"]["renderer_generation"])
        failing = dict(marker, restore_failures=1)
        with self.assertRaisesRegex(seam.SeamError, "restore failures"):
            seam.validate_resource_lifecycle_markers(
                [failing], expectations, {"initial": failing, "resume": failing}
            )

    def test_resource_diagnostic_scan_names_stale_restore_events(self):
        expectations = {
            "lifecycle": {
                "diagnostic_forbidden": [
                    "rejected stale sprite",
                    "renderer restore failed",
                    "rejected frame",
                ]
            }
        }
        seam.validate_resource_diagnostics("Stasis renderer ready", expectations)
        for diagnostic in expectations["lifecycle"]["diagnostic_forbidden"]:
            with self.assertRaisesRegex(seam.SeamError, diagnostic):
                seam.validate_resource_diagnostics(diagnostic, expectations)

    def test_transition_selection_ignores_history_paused_and_requires_counter_advance(self):
        baseline = {
            "resource_state": 1,
            "accepted": 30,
            "presented": 30,
            "rejected": 0,
            "validation": 0,
        }
        paused_history = dict(baseline, event="lifecycle", restore_reason=4)
        with self.assertRaisesRegex(seam.SeamError, "no new ready marker"):
            seam.select_post_transition_marker([paused_history], baseline, "background_resume")
        advanced = dict(
            baseline,
            event="lifecycle",
            accepted=31,
            presented=31,
            restore_reason=5,
        )
        self.assertIs(advanced, seam.select_post_transition_marker([advanced], baseline, "background_resume"))
        rejected = dict(advanced, rejected=1)
        with self.assertRaisesRegex(seam.SeamError, "no new ready marker"):
            seam.select_post_transition_marker([rejected], baseline, "background_resume")

    def test_recreation_selection_requires_new_epoch_initialized_marker(self):
        ready = {
            "event": "lifecycle",
            "resource_state": 1,
            "accepted": 1,
            "presented": 1,
            "rejected": 0,
            "validation": 0,
        }
        with self.assertRaisesRegex(seam.SeamError, "initialized"):
            seam.select_post_transition_marker([ready], {}, "force_activity_restart")
        initialized = {"event": "initialized"}
        self.assertIs(
            ready,
            seam.select_post_transition_marker([initialized, ready], {}, "force_activity_restart"),
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

    def test_resource_pixel_oracle_rejects_lane_background_only_pass(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "capture.png"
            rows = [[(24, 33, 48)] * 5 + [(242, 199, 51)] * 5 for _ in range(6)]
            rows[2][2] = (49, 209, 124)
            rows[2][7] = (13, 20, 31)
            write_rgb_png(path, 10, 6, rows)
            expectations = {
                "logical_size": [10, 6],
                "resource_regions": [
                    {
                        "name": "sprite",
                        "rect": [0, 0, 5, 6],
                        "target_rgb": [49, 209, 124],
                        "tolerance": 0,
                        "minimum_target_pixels": 1,
                    },
                    {
                        "name": "text",
                        "rect": [5, 0, 5, 6],
                        "target_rgb": [13, 20, 31],
                        "tolerance": 0,
                        "minimum_target_pixels": 1,
                    },
                ],
            }
            observed = seam.validate_resource_regions(path, expectations)
            self.assertEqual([1, 1], [item["target_pixels"] for item in observed])
            expectations["resource_regions"][0]["minimum_target_pixels"] = 2
            with self.assertRaisesRegex(seam.SeamError, "target pixels"):
                seam.validate_resource_regions(path, expectations)
            expectations["resource_regions"][0]["minimum_target_pixels"] = 1
            expectations["resource_regions"][0]["target_rgb"] = [250, 250, 250]
            with self.assertRaisesRegex(seam.SeamError, "target pixels"):
                seam.validate_resource_regions(path, expectations)

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

    def test_foreground_check_leaves_product_anr_visible(self):
        calls = []

        def fake_run(_adb, _serial, *arguments, **_options):
            calls.append(arguments)
            if arguments == ("shell", "dumpsys", "window", "windows"):
                return "mCurrentFocus=Window{456 u0 android/.AppNotRespondingDialog}"
            if arguments == (
                "exec-out",
                "uiautomator",
                "dump",
                "--compressed",
                "/dev/tty",
            ):
                return (
                    "<?xml version='1.0' encoding='UTF-8'?><hierarchy>"
                    "<node resource-id=\"android:id/alertTitle\" "
                    "text=\"Stasis Android Seam isn't responding\" />"
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

        self.assertFalse(changed)
        self.assertEqual(
            calls,
            [
                ("shell", "dumpsys", "window", "windows"),
                (
                    "exec-out",
                    "uiautomator",
                    "dump",
                    "--compressed",
                    "/dev/tty",
                ),
            ],
        )

    def test_foreground_check_taps_system_dialog_action_before_restarting(self):
        calls = []

        def fake_run(_adb, _serial, *arguments, **_options):
            calls.append(arguments)
            if arguments == ("shell", "dumpsys", "window", "windows"):
                return "mCurrentFocus=Window{456 u0 android/.AppNotRespondingDialog}"
            if arguments == (
                "exec-out",
                "uiautomator",
                "dump",
                "--compressed",
                "/dev/tty",
            ):
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
            if arguments == (
                "exec-out",
                "uiautomator",
                "dump",
                "--compressed",
                "/dev/tty",
            ):
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
            if arguments == (
                "exec-out",
                "uiautomator",
                "dump",
                "--compressed",
                "/dev/tty",
            ):
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
            if arguments == (
                "exec-out",
                "uiautomator",
                "dump",
                "--compressed",
                "/dev/tty",
            ):
                return (
                    "<?xml version='1.0' encoding='UTF-8'?><hierarchy>"
                    "<node resource-id=\"android:id/alertTitle\" "
                    "text=\"Stasis Android Seam isn't responding\" />"
                    "<node text=\"Wait\" bounds=\"[120,720][420,840]\" />"
                    "</hierarchy>"
                )
            return ""

        with mock.patch.object(seam, "_run", side_effect=fake_run) as run:
            dismissed = seam.dismiss_system_dialog_action(Path("adb"), "device")

        self.assertFalse(dismissed)
        self.assertNotIn(("shell", "input", "tap", "270", "780"), calls)
        run.assert_called_once_with(
            Path("adb"),
            "device",
            "exec-out",
            "uiautomator",
            "dump",
            "--compressed",
            "/dev/tty",
            required=False,
        )

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
        markers = [
            {
                "event": "stable",
                "frame": 30,
                "command_trace": 71,
            }
        ]
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
                "command_trace": 71 if index < 3 else 70 + index,
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
                "stable_frame": 30,
                "touch": {
                    "coordinate_tolerance": 16.0,
                    "require_command_trace": True,
                    "probes": expected_probes,
                }
            },
        )
        self.assertEqual(
            [item["probe_sequence"] for item in observed], [1, 2, 3, 4, 5]
        )

        markers[-1]["command_trace"] = 71
        with self.assertRaisesRegex(seam.SeamError, "stable/outside state"):
            seam.validate_touch_markers(
                markers,
                {
                    "stable_frame": 30,
                    "touch": {
                        "coordinate_tolerance": 16.0,
                        "require_command_trace": True,
                        "probes": expected_probes,
                    },
                },
            )

        markers[-1]["command_trace"] = 0
        with self.assertRaisesRegex(seam.SeamError, "positive integer"):
            seam.validate_touch_markers(
                markers,
                {
                    "stable_frame": 30,
                    "touch": {
                        "coordinate_tolerance": 16.0,
                        "require_command_trace": True,
                        "probes": expected_probes,
                    },
                },
            )

        del markers[-1]["command_trace"]
        with self.assertRaisesRegex(seam.SeamError, "positive integer"):
            seam.validate_touch_markers(
                markers,
                {
                    "stable_frame": 30,
                    "touch": {
                        "coordinate_tolerance": 16.0,
                        "require_command_trace": True,
                        "probes": expected_probes,
                    },
                },
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
            fitted_scale = min(width / 360, height / 720)
            fitted_width = max(1, int(360 * fitted_scale + 0.5))
            fitted_height = max(1, int(720 * fitted_scale + 0.5))
            scale = min(fitted_width / 360, fitted_height / 720)
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
                    "drawable_w": width,
                    "drawable_h": height,
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
        self.assertEqual([801, 1601], observed[0]["fitted_content_extent"])
        fitted_drawable_markers = [dict(item) for item in markers]
        fitted_drawable_markers[0]["drawable_w"] = 801
        with self.assertRaisesRegex(seam.SeamError, "drawable size mismatch"):
            seam.validate_orientation_markers(
                fitted_drawable_markers,
                expectations,
                {
                    "portrait": (1001, 1601),
                    "landscape": (1601, 1001),
                    "restored_portrait": (1001, 1601),
                },
            )
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
            "drawable_w": 1001,
            "drawable_h": 1601,
            "display_generation": 2,
            "density_generation": 1,
            "frame_display_generation": 2,
            "frame_density_generation": 1,
            "content_scale": 1601 / 720,
            "raster_scale": 1601 / 720,
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

    def test_cleanup_retains_install_when_recovery_needs_existing_data(self):
        calls = []

        def fake_run(_adb, _serial, *arguments, **_kwargs):
            calls.append(arguments)
            return ""

        with mock.patch.object(seam, "_run", side_effect=fake_run):
            errors = seam.restore_device_state(
                Path("adb"),
                "device",
                "com.example.seam",
                True,
                "null",
                None,
                retain_installed_package=True,
            )

        self.assertEqual([], errors)
        self.assertIn(("shell", "am", "force-stop", "com.example.seam"), calls)
        self.assertNotIn(("uninstall", "com.example.seam"), calls)

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
