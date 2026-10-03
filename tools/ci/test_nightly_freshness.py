import pathlib
import re
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
        self.assertIn("name: Nightly Publication Freshness", self.freshness)
        self.assertIn('cron: "15 15 * * *"', self.freshness)
        self.assertIn("workflow_dispatch:", self.freshness)
        self.assertIn("git tag --merged HEAD --list 'nightly-*'", self.freshness)
        self.assertIn('gh release view "${last_tag}"', self.freshness)
        self.assertIn("stasis-nightly-win-x64.zip", self.freshness)
        self.assertIn("stasis-editor-release-win32-x64.zip", self.freshness)
        self.assertIn('git rev-list "${last_tag}..HEAD" --count', self.freshness)
        self.assertIn(
            "git log --format='%ct' \"${last_tag}..HEAD\" | tail -n1",
            self.freshness,
        )
        self.assertNotIn(
            "git log --reverse --format='%ct' \"${last_tag}..HEAD\" | head -n1",
            self.freshness,
        )
        self.assertIn("36 * 60 * 60", self.freshness)
        self.assertIn("::error::main has remained ahead", self.freshness)

    def test_release_smokes_live_command_on_every_desktop_archive(self):
        self.assertEqual(1, self.release.count(".\\stasis.exe live --help"))
        self.assertEqual(1, self.release.count("./bin/stasis live --help"))

    def test_windows_archive_stage_runs_both_packaged_web_consumers_in_chrome(self):
        step = self.release.split(
            "- name: Extract and qualify the exact staged archive", 1
        )[1].split("- name: Upload desktop staged archive receipt", 1)[0]
        self.assertIn('"--browser"', step)
        self.assertIn('$env:STASIS_BROWSER_EXECUTABLE = $chrome', step)
        self.assertLess(step.index('$env:STASIS_BROWSER_EXECUTABLE = $chrome'), step.index("python @arguments"))
        self.assertLess(step.index('$arguments += "--browser"'), step.index("python @arguments"))

    def test_mobile_archive_lanes_extract_exact_archive_roots(self):
        self.assertIn(
            "tar -xf $archiveFile -C $archiveRoot --strip-components=1",
            self.release,
        )
        android = (ROOT / ".github/workflows/staged-android-archive-acceptance.yml").read_text(encoding="utf-8")
        ios = (ROOT / ".github/workflows/staged-ios-archive-acceptance.yml").read_text(encoding="utf-8")
        self.assertIn(
            'tar -xf "$STASIS_ARTIFACT_FILE" -C target/staged-linux --strip-components=1',
            android,
        )
        self.assertIn(
            'tar -xf "$STASIS_ARTIFACT_FILE" -C target/staged-macos --strip-components=1',
            ios,
        )

    def test_expensive_seams_wait_for_release_detection(self):
        for job in (
            "integration_seams",
            "performance_benchmarks",
            "network_browser_acceptance",
            "android_device_seams",
            "android_runtime_support",
        ):
            with self.subTest(job=job):
                self.assertRegex(
                    self.release,
                    rf"(?ms)^  {job}:\n    needs: detect\n    if: needs\.detect\.outputs\.should_release == 'true'",
                )
        release = self.release.split("  release:\n", 1)[1].split("  no_changes:\n", 1)[0]
        self.assertIn(
            "if: ${{ always() && needs.detect.outputs.should_release == 'true' && github.ref == 'refs/heads/main' }}",
            release,
        )
        for lane in (
            "build",
            "windows_signing",
            "android_prebuilt_acceptance",
            "vscode_extension",
            "integration_seams",
            "android_device_seams",
            "performance_benchmarks",
            "network_browser_acceptance",
            "staged_archive_desktop_acceptance",
            "staged_android_archive_acceptance",
            "staged_ios_archive_acceptance",
        ):
            self.assertIn(f"{lane}=${{{{ needs.{lane}.result }}}}", release)
        self.assertIn("--require-signed-windows", release)

    def test_nightly_calls_performance_and_network_validation(self):
        for job, workflow in (
            ("performance_benchmarks", "perf-ci.yml"),
            ("network_browser_acceptance", "network-browser-acceptance.yml"),
        ):
            with self.subTest(job=job):
                self.assertRegex(
                    self.release,
                    rf"(?ms)^  {job}:\n    needs: detect\n    if: needs\.detect\.outputs\.should_release == 'true'\n    uses: \.\/\.github\/workflows\/{re.escape(workflow)}",
                )

    def test_windows_archive_is_extracted_and_graphical_windows_are_required(self):
        self.assertIn("Verify extracted Windows editor toolchain", self.release)
        for relative in (
            "stasis.exe",
            "stasis_graphics.dll",
            "stasis_dynload.dll",
            "src/stdlib",
            "stasis_release_provenance.json",
            "stasis_windows_signing.json",
        ):
            self.assertIn(f'$validationRoot/{relative}', self.release)
        self.assertIn("extracted editor fingerprint mismatch", self.release)
        self.assertIn("Sign trusted Windows release files", self.release)
        self.assertIn("Windows signing receipt generation failed", self.release)
        self.assertIn("tools/windows/stasis-signing.ps1 verify", self.release)
        self.assertIn("Extracted Windows signature verification failed", self.release)
        self.assertIn("windows_signing_manifest.py verify-files", self.release)
        self.assertIn("stasis.exe live --help", self.release)

    def test_extracted_windows_toolchain_uses_supported_editor_info_probe(self):
        extracted = self.release.split(
            "Verify extracted Windows editor toolchain", 1
        )[1].split("Smoke test bundled graphics runtime (windows)", 1)[0]
        self.assertIn(
            '$editorInfoJson = & "$validationRoot/stasis.exe" --json editor-info',
            extracted,
        )
        self.assertIn(
            'if ($LASTEXITCODE -ne 0) { throw "extracted editor-info probe failed" }',
            extracted,
        )
        self.assertIn("$editorInfo = $editorInfoJson | ConvertFrom-Json", extracted)
        self.assertIn("$editorInfo.result.release_id", extracted)
        self.assertNotIn('"$validationRoot/stasis.exe" editor --help', extracted)

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
