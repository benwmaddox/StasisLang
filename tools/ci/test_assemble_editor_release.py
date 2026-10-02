import hashlib
import json
import subprocess
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
ASSEMBLER = REPO_ROOT / "tools" / "assemble_editor_release.mjs"


class AssembleEditorReleaseTests(unittest.TestCase):
    def run_assembler(self, vsix, output):
        return subprocess.run(
            [
                "node",
                str(ASSEMBLER),
                "--vsix",
                str(vsix),
                "--out",
                str(output),
                "--release-id",
                "nightly-test",
                "--platform",
                "win32-x64",
            ],
            cwd=REPO_ROOT,
            capture_output=True,
            text=True,
        )

    def test_release_contains_vsix_once_and_records_its_size_and_hash(self):
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            vsix = root / "stasislang.stasis.vsix"
            output = root / "release"
            vsix_bytes = b"packaged VSIX with bundled toolchain\x00\x01"
            vsix.write_bytes(vsix_bytes)

            result = self.run_assembler(vsix, output)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(
                {path.name for path in output.iterdir()},
                {vsix.name, "stasis-editor-release.json"},
            )
            self.assertEqual((output / vsix.name).read_bytes(), vsix_bytes)

            manifest = json.loads((output / "stasis-editor-release.json").read_text())
            self.assertEqual(manifest["schema"], 2)
            self.assertEqual(manifest["release_id"], "nightly-test")
            self.assertEqual(manifest["platform"], "win32-x64")
            self.assertEqual(
                manifest["files"],
                [
                    {
                        "role": "vscode_extension",
                        "name": vsix.name,
                        "bytes": len(vsix_bytes),
                        "sha256": hashlib.sha256(vsix_bytes).hexdigest(),
                    }
                ],
            )

    def test_missing_vsix_fails_before_creating_release_output(self):
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            output = root / "release"

            result = self.run_assembler(root / "missing.vsix", output)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("Release input is not a file", result.stderr)
            self.assertFalse(output.exists())

    def test_oversized_vsix_fails_before_creating_release_output(self):
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            vsix = root / "oversized.vsix"
            output = root / "release"
            with vsix.open("wb") as oversized_file:
                oversized_file.truncate(40 * 1024 * 1024 + 1)

            result = self.run_assembler(vsix, output)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("above the 41943040-byte budget", result.stderr)
            self.assertFalse(output.exists())

    def test_stale_toolchain_archive_is_rejected_without_deleting_it(self):
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            vsix = root / "stasislang.stasis.vsix"
            output = root / "release"
            stale_archive = output / "stasis-nightly-win-x64.zip"
            vsix.write_bytes(b"VSIX")
            output.mkdir()
            stale_archive.write_bytes(b"old duplicate toolchain")

            result = self.run_assembler(vsix, output)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("Release output directory contains stale files", result.stderr)
            self.assertEqual(list(output.iterdir()), [stale_archive])


if __name__ == "__main__":
    unittest.main()
