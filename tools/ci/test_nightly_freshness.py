import pathlib
import re
import os
import shutil
import subprocess
import textwrap
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

    def test_every_desktop_archive_uses_a_verified_real_chrome_for_web_consumers(self):
        browser = self.release.split(
            "- name: Locate and verify hosted Chrome", 1
        )[1].split("- name: Setup MSVC dev environment", 1)[0]
        self.assertIn(
            "linux)\n              candidates=(/usr/bin/google-chrome /usr/bin/google-chrome-stable /opt/google/chrome/chrome)",
            browser,
        )
        for executable in (
            "/usr/bin/google-chrome",
            "/usr/bin/google-chrome-stable",
            "/opt/google/chrome/chrome",
            '"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"',
            '"C:/Program Files/Google/Chrome/Application/chrome.exe"',
        ):
            with self.subTest(executable=executable):
                self.assertIn(executable, browser)
        self.assertIn('command -v "$name" 2>/dev/null', browser)
        self.assertIn('if [[ ! -x "$candidate" ]]', browser)
        self.assertIn("version_status=$?", browser)
        self.assertIn(
            "Google Chrome version probe: path=%s exit=%s raw=%q", browser
        )
        self.assertIn("Google Chrome normalized version: %q", browser)
        self.assertIn('version="$version_normalized"', browser)
        self.assertIn(
            "::error::No executable branded Google Chrome was found", browser
        )
        self.assertIn("[System.Diagnostics.FileVersionInfo]::GetVersionInfo($chrome).ProductVersion", browser)
        self.assertIn("^\\d+(?:\\.\\d+){2,3}$", browser)
        self.assertIn("STASIS_BROWSER_EXECUTABLE=$chrome", browser)
        self.assertIn("target/staged-acceptance/hosted-chrome-version.txt", browser)

        qualify = self.release.split(
            "- name: Extract and qualify the exact staged archive", 1
        )[1].split("- name: Collect staged archive review evidence", 1)[0]
        self.assertIn('$arguments += "--browser"', qualify)
        self.assertLess(qualify.index('$arguments += "--browser"'), qualify.index("python @arguments"))

    def test_chrome_version_probe_trims_only_outer_ascii_whitespace(self):
        browser = self.release.split(
            "- name: Locate and verify hosted Chrome on Linux and macOS", 1
        )[1].split("- name: Setup MSVC dev environment", 1)[0]
        start = browser.index("trim_ascii_whitespace() {")
        end = browser.index('case "$STASIS_TARGET"', start)
        functions = textwrap.dedent(browser[start:end])
        bash = None
        if os.name == "nt":
            for candidate in (
                pathlib.Path(r"C:\Program Files\Git\bin\bash.exe"),
                pathlib.Path(r"C:\Program Files\Git\usr\bin\bash.exe"),
            ):
                if candidate.is_file():
                    bash = str(candidate)
                    break
        if bash is None:
            bash = shutil.which("bash")
        self.assertIsNotNone(bash, "Bash is required to execute the workflow's exact version helpers")

        cases = (
            ("Google Chrome 152.0.7977.83 ", "Google Chrome 152.0.7977.83", True),
            ("Google Chrome 154.0.8037.57 ", "Google Chrome 154.0.8037.57", True),
            ("Google Chrome 123.4.5", "Google Chrome 123.4.5", True),
            ("Google Chrome 123.4.5\r", "Google Chrome 123.4.5", True),
            (" \tGoogle Chrome 123.4.5 \f", "Google Chrome 123.4.5", True),
            (
                "Google Chrome for Testing 123.4.5",
                "Google Chrome for Testing 123.4.5",
                True,
            ),
            ("Chromium 154.0.8037.57", "Chromium 154.0.8037.57", False),
            ("Google Chrome 123x4.5", "Google Chrome 123x4.5", False),
            (
                "Google Chrome 123.4.5\nextra",
                "Google Chrome 123.4.5\nextra",
                False,
            ),
        )
        script = "set -euo pipefail\n" + functions + """
normalized="$(trim_ascii_whitespace "$STASIS_TEST_VERSION")"
if is_branded_chrome_version "$normalized"; then
  actual=accepted
else
  actual=rejected
fi
[[ "$normalized" == "$STASIS_TEST_EXPECTED_NORMALIZED" ]]
[[ "$actual" == "$STASIS_TEST_EXPECTED_RESULT" ]]
printf '%s\\n' "$actual"
"""
        for raw, expected_normalized, accepted in cases:
            with self.subTest(raw=raw):
                env = os.environ.copy()
                env["STASIS_TEST_VERSION"] = raw
                env["STASIS_TEST_EXPECTED_NORMALIZED"] = expected_normalized
                env["STASIS_TEST_EXPECTED_RESULT"] = "accepted" if accepted else "rejected"
                result = subprocess.run(
                    [bash, "--noprofile", "--norc", "-c", script],
                    env=env,
                    capture_output=True,
                    text=True,
                    check=False,
                    timeout=30,
                )
                self.assertEqual(0, result.returncode, result.stderr or result.stdout)
                self.assertEqual(
                    "accepted\n" if accepted else "rejected\n", result.stdout
                )

    def test_staged_review_evidence_is_uploaded_separately_from_receipts(self):
        desktop = self.release.split(
            "- name: Upload desktop staged archive review evidence", 1
        )[1].split("- name: Upload desktop staged archive receipt", 1)[0]
        self.assertIn("staged-release-evidence-${{ matrix.target }}", desktop)
        self.assertIn("if: always()", desktop)
        for filename in ("staged-android-archive-acceptance.yml", "staged-ios-archive-acceptance.yml"):
            workflow = (ROOT / ".github/workflows" / filename).read_text(encoding="utf-8")
            self.assertIn("Collect staged Android review evidence" if "android" in filename else "Collect staged iOS review evidence", workflow)
            self.assertIn("staged-release-evidence-android" if "android" in filename else "staged-release-evidence-ios", workflow)
            self.assertIn("Upload staged Android review evidence" if "android" in filename else "Upload staged iOS review evidence", workflow)
            self.assertIn("Upload staged Android archive receipt" if "android" in filename else "Upload staged iOS archive receipt", workflow)

    def test_generics_release_docs_describe_supported_targets_signing_and_vendor_recovery(self):
        generic_docs = (ROOT / "docs/generics.md").read_text(encoding="utf-8")
        sample_readme = (ROOT / "samples/generics_collections/README.md").read_text(encoding="utf-8")
        for required in (
            "three sample tests",
            "Windows, Linux, and macOS",
            "staged-release-evidence-*",
            "not Play-distribution signatures",
            "not evidence of general public-PKI trust",
            "does not establish signing or notarization",
        ):
            with self.subTest(required=required):
                self.assertIn(required, generic_docs)
        for required in (
            "stasis version",
            "stasis env",
            "stasis --json editor-info",
            "vendor update",
            "git restore --source=HEAD",
            "--json vendor status",
        ):
            with self.subTest(required=required):
                self.assertIn(required, sample_readme)

    def test_mobile_archive_lanes_extract_exact_archive_roots(self):
        desktop = self.release.split(
            "- name: Extract and qualify the exact staged archive", 1
        )[1].split("- name: Upload desktop staged archive receipt", 1)[0]
        self.assertIn("$archiveParent = Join-Path $env:GITHUB_WORKSPACE \"target\"", desktop)
        self.assertIn("tar -xf $archiveFile -C $archiveParent", desktop)
        self.assertIn("$archiveRoot = Join-Path $archiveParent $env:STASIS_ARTIFACT_NAME", desktop)
        self.assertNotIn("--strip-components", desktop)
        android = (ROOT / ".github/workflows/staged-android-archive-acceptance.yml").read_text(encoding="utf-8")
        ios = (ROOT / ".github/workflows/staged-ios-archive-acceptance.yml").read_text(encoding="utf-8")
        self.assertIn(
            'tar -xf "$STASIS_ARTIFACT_FILE" -C target',
            android,
        )
        self.assertIn(
            "--archive-root target/stasis-nightly-linux-x64",
            android,
        )
        self.assertIn(
            'tar -xf "$STASIS_ARTIFACT_FILE" -C target',
            ios,
        )
        self.assertIn(
            "--archive-root target/stasis-nightly-osx-arm64",
            ios,
        )

    def test_android_archive_lane_receives_the_exact_calling_run_id(self):
        android = (ROOT / ".github/workflows/staged-android-archive-acceptance.yml").read_text(encoding="utf-8")

        def mapping_block(source, name, indent):
            lines = source.splitlines()
            header = f"{' ' * indent}{name}:"
            self.assertEqual(lines.count(header), 1)
            start = lines.index(header) + 1
            body = []
            for line in lines[start:]:
                if line.strip():
                    line_indent = len(line) - len(line.lstrip(" "))
                    if line_indent <= indent:
                        break
                body.append(line)
            return "\n".join(body)

        on = mapping_block(android, "on", 0)
        workflow_call = mapping_block(on, "workflow_call", 2)
        inputs = mapping_block(workflow_call, "inputs", 4)
        run_id = mapping_block(inputs, "run_id", 6)
        self.assertIn("        required: true", run_id)
        self.assertIn("        type: string", run_id)
        secrets = mapping_block(workflow_call, "secrets", 4)
        secret_names = set(re.findall(r"(?m)^      ([A-Za-z0-9_-]+):$", secrets))
        self.assertEqual(
            secret_names,
            {
                "android_test_keystore_base64",
                "android_test_store_password",
                "android_test_key_password",
            },
        )
        self.assertIn('--run-id "${{ inputs.run_id }}"', android)
        caller = self.release.split("  staged_android_archive_acceptance:\n", 1)[1].split(
            "  staged_ios_archive_acceptance:\n", 1
        )[0]
        self.assertIn("run_id: ${{ format('{0}', github.run_id) }}", caller)

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
