import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock

from tools.ci import run_architecture_characterization as runner


class ArchitectureCharacterizationManifestTests(unittest.TestCase):
    def test_checked_in_manifest_is_valid_and_covers_all_lanes(self):
        manifest = runner.validate_manifest()
        self.assertGreaterEqual(len(manifest["rows"]), 20)
        self.assertEqual(
            {row["lane"] for row in manifest["rows"]},
            {"fast-hermetic", "platform-host", "device-browser"},
        )
        self.assertEqual(
            {row["evidence"] for row in manifest["rows"]},
            {"behavioral", "structural-lint"},
        )
        self.assertEqual(
            {
                row["id"]
                for row in manifest["rows"]
                if row["default_gate"]
            },
            {
                "compiler.pipeline",
                "compiler.failed-publication-rollback",
                "runtime.storage",
                "runtime.network",
                "live.protocol-rust",
                "vscode.protocol",
            },
        )

    def test_rejects_duplicate_ids(self):
        manifest = runner.validate_manifest()
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "manifest.json"
            duplicate = dict(manifest)
            duplicate["rows"] = [manifest["rows"][0], manifest["rows"][0]]
            path.write_text(json.dumps(duplicate), encoding="utf-8")
            with self.assertRaisesRegex(runner.ManifestError, "duplicate"):
                runner.validate_manifest(path, runner.ROOT)

    def test_rejects_missing_or_escaping_fixture(self):
        manifest = runner.validate_manifest()
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "manifest.json"
            changed = json.loads(json.dumps(manifest))
            changed["rows"][0]["fixture"] = ["../outside.json"]
            path.write_text(json.dumps(changed), encoding="utf-8")
            with self.assertRaises(runner.ManifestError):
                runner.validate_manifest(path, runner.ROOT)

    def test_rejects_unknown_lane_and_evidence(self):
        manifest = runner.validate_manifest()
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "manifest.json"
            for field, value in (("lane", "unknown"), ("evidence", "guess")):
                changed = json.loads(json.dumps(manifest))
                changed["rows"][0][field] = value
                path.write_text(json.dumps(changed), encoding="utf-8")
                with self.subTest(field=field):
                    with self.assertRaises(runner.ManifestError):
                        runner.validate_manifest(path, runner.ROOT)

    def test_rejects_non_boolean_or_non_fast_default_gate(self):
        manifest = runner.validate_manifest()
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "manifest.json"
            for value in (1, "true", None):
                changed = json.loads(json.dumps(manifest))
                changed["rows"][0]["default_gate"] = value
                path.write_text(json.dumps(changed), encoding="utf-8")
                with self.subTest(value=value):
                    with self.assertRaises(runner.ManifestError):
                        runner.validate_manifest(path, runner.ROOT)
            changed = json.loads(json.dumps(manifest))
            changed["rows"][7]["default_gate"] = True
            path.write_text(json.dumps(changed), encoding="utf-8")
            with self.assertRaisesRegex(runner.ManifestError, "only valid"):
                runner.validate_manifest(path, runner.ROOT)

    @mock.patch("tools.ci.run_architecture_characterization.subprocess.run")
    def test_fast_lane_deduplicates_commands_and_bounds_execution(self, run):
        run.return_value = mock.Mock(returncode=0)
        manifest = {
            "rows": [
                {
                    "id": "a",
                    "lane": "fast-hermetic",
                    "default_gate": True,
                    "command": "one",
                },
                {
                    "id": "b",
                    "lane": "fast-hermetic",
                    "default_gate": False,
                    "command": "two",
                },
                {
                    "id": "c",
                    "lane": "platform-host",
                    "default_gate": False,
                    "command": "three",
                },
            ]
        }
        self.assertEqual(runner.run_fast_lane(manifest, Path("."), 17), 0)
        run.assert_called_once_with(
            "one",
            cwd=Path("."),
            shell=True,
            check=False,
            timeout=17,
            env=mock.ANY,
        )

    @mock.patch("tools.ci.run_architecture_characterization.subprocess.run")
    def test_full_lane_runs_rows_not_selected_for_default_gate(self, run):
        run.return_value = mock.Mock(returncode=0)
        manifest = {
            "rows": [
                {
                    "id": "a",
                    "lane": "fast-hermetic",
                    "default_gate": True,
                    "command": "one",
                },
                {
                    "id": "b",
                    "lane": "fast-hermetic",
                    "default_gate": False,
                    "command": "two",
                },
            ]
        }
        self.assertEqual(runner.run_lane(manifest, "fast-hermetic", Path("."), 17), 0)
        self.assertEqual([call.args[0] for call in run.call_args_list], ["one", "two"])


if __name__ == "__main__":
    unittest.main()
