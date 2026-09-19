import importlib.util
import json
import pathlib
import tempfile
import unittest
import zipfile


ROOT = pathlib.Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tools/audit_release_bundle.py"
SPEC = importlib.util.spec_from_file_location("audit_release_bundle", SCRIPT)
assert SPEC and SPEC.loader
AUDIT = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(AUDIT)


class ReleaseBundleSizeTests(unittest.TestCase):
    def _fixture(self, root):
        for name in AUDIT.required_files("windows"):
            path = root / pathlib.Path(*name.split("/"))
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(name.encode("ascii"))
        for name in AUDIT.REQUIRED_DIRECTORIES:
            directory = root / pathlib.Path(*name.split("/"))
            directory.mkdir(parents=True, exist_ok=True)
            (directory / "fixture.txt").write_text("fixture\n", encoding="ascii")

    def test_windows_archive_report_is_deterministic_and_tracks_deflate_sizes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory) / "stasis-nightly-win-x64"
            root.mkdir()
            self._fixture(root)
            archive = pathlib.Path(directory) / "stasis-nightly-win-x64.zip"
            with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as output:
                for path in sorted(root.rglob("*")):
                    if path.is_file():
                        output.write(path, path.relative_to(root).as_posix())

            first = AUDIT.build_report(root, "windows", archive)
            second = AUDIT.build_report(root, "windows", archive)
            self.assertEqual(json.dumps(first, sort_keys=True), json.dumps(second, sort_keys=True))
            self.assertEqual(first["archive"]["file_count"], first["bundle"]["file_count"])
            self.assertEqual(first["archive"]["compression"], "zip member compressed sizes")
            self.assertTrue(first["size_breakdown"]["files"][-1]["path"])

    def test_workflow_runs_the_audit_with_a_regression_budget(self):
        workflow = (ROOT / ".github/workflows/nightly-release.yml").read_text(encoding="utf-8")
        self.assertEqual(workflow.count("tools/audit_release_bundle.py"), 3)
        self.assertEqual(workflow.count("--max-archive-bytes 125829120"), 3)
        self.assertIn("bundle-audit", workflow)


if __name__ == "__main__":
    unittest.main()
