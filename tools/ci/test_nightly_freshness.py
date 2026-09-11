import pathlib
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]


class NightlyFreshnessContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.freshness = (
            ROOT / ".github/workflows/nightly-freshness.yml"
        ).read_text(encoding="utf-8")
        cls.release = (
            ROOT / ".github/workflows/nightly-release.yml"
        ).read_text(encoding="utf-8")

    def test_independent_schedule_detects_stale_changed_main(self):
        self.assertIn('cron: "15 15 * * *"', self.freshness)
        self.assertIn("workflow_dispatch:", self.freshness)
        self.assertIn("git tag --merged HEAD --list 'nightly-*'", self.freshness)
        self.assertIn('gh release view "${last_tag}"', self.freshness)
        self.assertIn("stasis-nightly-win-x64.zip", self.freshness)
        self.assertIn("stasis-editor-release-win32-x64.zip", self.freshness)
        self.assertIn('git rev-list "${last_tag}..HEAD" --count', self.freshness)
        self.assertIn("36 * 60 * 60", self.freshness)
        self.assertIn("::error::main has remained ahead", self.freshness)

    def test_release_smokes_editor_command_on_every_desktop_archive(self):
        self.assertEqual(1, self.release.count(".\\stasis.exe editor --help"))
        self.assertEqual(1, self.release.count("./bin/stasis editor --help"))

    def test_windows_archive_is_extracted_and_graphical_windows_are_required(self):
        self.assertIn("Verify extracted Windows editor toolchain", self.release)
        for relative in (
            "stasis.exe",
            "stasis_graphics.dll",
            "stasis_dynload.dll",
            "src/stdlib",
            "stasis_release_provenance.json",
        ):
            self.assertIn(f'$validationRoot/{relative}', self.release)
        self.assertIn("extracted editor fingerprint mismatch", self.release)
        self.assertIn("extracted Authenticode verification failed", self.release)
        self.assertIn("tools/ci/test_editor_windows.ps1", self.release)
        smoke = (ROOT / "tools/ci/test_editor_windows.ps1").read_text(
            encoding="utf-8"
        )
        self.assertIn('titles -contains "Stasis Editor"', smoke)
        self.assertIn("titles -contains $GameWindowTitle", smoke)
        self.assertIn("$process.Kill()", smoke)

    def test_vsix_secret_scan_skips_only_provenance_bound_native_binaries(self):
        package = (ROOT / "vscode-stasis/package.json").read_text(encoding="utf-8")
        self.assertIn("npm run scan:package-secrets && vsce package", package)
        self.assertIn("--allow-package-all-secrets --allow-package-env-file", package)
        ignored = (ROOT / "vscode-stasis/.secretlintignore").read_text(
            encoding="utf-8"
        )
        self.assertIn("dist/toolchain/**/*.exe", ignored)
        self.assertIn("dist/toolchain/**/stasis", ignored)
        self.assertNotIn("dist/toolchain/**\n", ignored)
        config = (ROOT / "vscode-stasis/.secretlintrc.json").read_text(
            encoding="utf-8"
        )
        self.assertIn("@secretlint/secretlint-rule-preset-recommend", config)
        self.assertIn("@secretlint/secretlint-rule-no-dotenv", config)


if __name__ == "__main__":
    unittest.main()
