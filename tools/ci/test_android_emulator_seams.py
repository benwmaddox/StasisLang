from pathlib import Path
import json
import re
import unittest


ROOT = Path(__file__).resolve().parents[2]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


class AndroidEmulatorSeamContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.workflow = read(".github/workflows/android-device-seams.yml")
        cls.nightly_workflow = read(".github/workflows/nightly-release.yml")
        cls.pr_workflow = read(".github/workflows/pr-ci.yml")
        cls.release_script = read("mobile/android/test_release_shell.ps1")
        cls.release_runner = read("tools/ci/run_android_release_shell_seam.py")
        cls.emulator_script = read("mobile/android/test_release_shell_emulator.ps1")
        cls.strategy = read("docs/integration_seam_testing_strategy.md")
        cls.touch_expectations = json.loads(
            read("samples/android_touch_seam/android_seam_expectations.json")
        )
        cls.asset_rejection_expectations = json.loads(
            read("samples/android_asset_rejection_seam/android_seam_expectations.json")
        )
        cls.workshop_script = read("mobile/android/test_render_emulator.ps1")
        cls.rust_bridge_script = read("mobile/android/build_rust_bridge.ps1")
        cls.provenance_script = read("mobile/android/rust_bridge_provenance.ps1")

    def workflow_job_body(self, name: str) -> str:
        match = re.search(
            rf"(?ms)^  {re.escape(name)}:\n(?P<body>.*?)(?=^  [A-Za-z_][A-Za-z0-9_-]*:\n|\Z)",
            self.workflow,
        )
        self.assertIsNotNone(match, name)
        return match.group("body")

    def test_workflow_uses_hosted_x86_emulator(self):
        self.assertIn("runs-on: ubuntu-latest", self.workflow)
        self.assertNotIn("runs-on: macos-15", self.workflow)
        self.assertIn("reactivecircus/android-emulator-runner@v2", self.workflow)
        self.assertIn("api-level: 35", self.workflow)
        self.assertIn("arch: x86_64", self.workflow)
        self.assertIn("Enable KVM", self.workflow)
        self.assertIn("workflow_call:", self.workflow)
        self.assertIn("workflow_dispatch:", self.workflow)
        self.assertNotIn("pull_request:", self.workflow)
        self.assertIn('uses: actions/setup-python@v5', self.workflow)
        self.assertIn('python-version: "3.12"', self.workflow)
        self.assertIn("group: android-emulator-seams-nightly", self.workflow)
        self.assertIn("cancel-in-progress: false", self.workflow)
        self.assertIn("uses: ./.github/workflows/android-device-seams.yml", self.nightly_workflow)
        self.assertIn(
            "needs: [detect, build, vscode_extension, integration_seams, android_device_seams]",
            self.nightly_workflow,
        )
        self.assertIn(
            "uses: ./.github/workflows/pr-ci.yml\n"
            "    with:\n"
            "      run_slow_seams: true",
            self.nightly_workflow,
        )
        self.assertNotIn("self-hosted", self.workflow)
        self.assertNotIn("runs-on: [self-hosted, Windows, android-device]", self.workflow)
        self.assertNotIn("ANDROID_SERIAL", self.workflow)

    def test_pr_ci_slow_seams_are_boolean_input_gated(self):
        input_declaration = (
            "    inputs:\n"
            "      run_slow_seams:\n"
            "        description: Run platform integration and packaging seams.\n"
            "        required: false\n"
            "        default: false\n"
            "        type: boolean\n"
        )
        for event in ("workflow_dispatch", "workflow_call"):
            match = re.search(
                rf"(?ms)^  {event}:\n(?P<body>.*?)(?=^  [A-Za-z_][A-Za-z0-9_-]*:|\Z)",
                self.pr_workflow,
            )
            self.assertIsNotNone(match, event)
            self.assertIn(input_declaration, match.group("body"))

        self.assertNotIn(
            "github.event_name == 'workflow_call' && inputs.run_slow_seams",
            self.pr_workflow,
        )
        slow_jobs = (
            "bootstrap-smoke-windows",
            "vscode-extension-e2e",
            "android-package-link",
            "ios-package-link",
        )
        slow_gate = "if: ${{ inputs.run_slow_seams }}"
        self.assertEqual(4, self.pr_workflow.count(slow_gate))
        for job in slow_jobs:
            match = re.search(
                rf"(?ms)^  {re.escape(job)}:\n(?P<body>.*?)(?=^  [A-Za-z_][A-Za-z0-9_-]*:|\Z)",
                self.pr_workflow,
            )
            self.assertIsNotNone(match, job)
            self.assertEqual(1, match.group("body").count(slow_gate))
            if job == "bootstrap-smoke-windows":
                self.assertEqual(
                    1,
                    match.group("body").count(
                        "run: cargo test -p stasis_compiler -- --test-threads=1 --nocapture"
                    ),
                )

    def test_nightly_grants_reusable_ci_read_permissions(self):
        self.assertIn("  contents: write", self.nightly_workflow)
        self.assertIn("  pull-requests: read", self.nightly_workflow)
        self.assertIn("  actions: read", self.nightly_workflow)

    def test_workflow_supplies_build_inputs_and_uploads_each_seam(self):
        job_names = ("release-shell-seams", "workshop-seams")
        job_bodies = {name: self.workflow_job_body(name) for name in job_names}
        self.assertNotIn("generated-release-shell", self.workflow)
        self.assertNotIn("needs:", self.workflow)
        for body in job_bodies.values():
            self.assertEqual(1, body.count("runs-on: ubuntu-latest"))
            self.assertEqual(1, body.count("reactivecircus/android-emulator-runner@v2"))
            for setup in (
                "- name: Setup Gradle",
                "- name: Checkout SDL3",
                "- name: Checkout SDL3_image",
                "- name: Install Rust",
                "- name: Install Android Rust targets",
                "- name: Setup Python",
                "- name: Enable KVM",
            ):
                self.assertEqual(1, body.count(setup + "\n"), setup)
            self.assertIn('gradle-version: "8.9"', body)
            self.assertIn("ndk: 27.0.12077973", body)
            self.assertIn("cmake: 3.22.1", body)
            self.assertIn("api-level: 35", body)
            self.assertIn("target: google_apis", body)
            self.assertIn("arch: x86_64", body)
            self.assertIn('CARGO_BUILD_JOBS: "2"', body)
            self.assertIn("cores: 2", body)
            self.assertIn("8e37db5e797b6167f3a00d697d816a684bd259c7", body)
            self.assertIn("bec9134a26c7d0f31b36d6083c25296e04cabff5", body)
            self.assertIn(
                "rustup target add aarch64-linux-android x86_64-linux-android", body
            )
        self.assertEqual(2, self.workflow.count('CARGO_BUILD_JOBS: "2"'))
        self.assertEqual(2, self.workflow.count("cores: 2"))
        release_body = job_bodies["release-shell-seams"]
        workshop_body = job_bodies["workshop-seams"]
        release_script = "pwsh -NoProfile -File ./mobile/android/test_release_shell_emulator.ps1"
        workshop_script = (
            "pwsh -NoProfile -File ./mobile/android/test_render_emulator.ps1 "
            "-Headless -AvdName test -StepTimeoutSeconds 600 -RenderTimeoutSeconds 90"
        )
        self.assertEqual(1, release_body.count(release_script))
        self.assertNotIn("test_render_emulator.ps1", release_body)
        self.assertEqual(1, workshop_body.count(workshop_script))
        self.assertNotIn("test_release_shell_emulator.ps1", workshop_body)
        release_artifacts = re.findall(r"(?m)^\s+name: (android-[^\s]+)$", release_body)
        workshop_artifacts = re.findall(r"(?m)^\s+name: (android-[^\s]+)$", workshop_body)
        self.assertEqual(
            sorted(
                (
                    "android-release-shell-seam",
                    "android-resource-restore-seam",
                    "android-touch-roundtrip-seam",
                    "android-orientation-metrics-seam",
                    "android-packaged-assets-seam",
                    "android-asset-rejection-seam",
                )
            ),
            sorted(release_artifacts),
        )
        self.assertEqual(["android-workshop-it025-seam"], workshop_artifacts)
        self.assertEqual(7, self.workflow.count("          name: android-"))
        self.assertEqual(7, self.workflow.count("        if: always()"))
        self.assertNotIn("\n      if: always()", self.workflow)

    def test_release_wrapper_uses_platform_appropriate_tools_and_paths(self):
        self.assertIn('"adb$executableSuffix"', self.release_script)
        self.assertIn('"stasis$executableSuffix"', self.release_script)
        self.assertIn("$runningOnWindows", self.release_script)
        self.assertNotIn("$isWindows", self.release_script)
        self.assertIn("$runningOnWindows", self.emulator_script)
        self.assertNotIn("$isWindows", self.emulator_script)
        self.assertIn("[System.IO.Path]::Combine", self.release_script)
        self.assertNotIn('"platform-tools\\adb.exe"', self.release_script)
        self.assertNotIn('"android\\app\\build', self.release_script)

    def test_emulator_entrypoint_rejects_physical_targets_and_runs_all_seams(self):
        self.assertIn("Expected exactly one ready Android emulator", self.emulator_script)
        self.assertIn("^emulator-\\d+$", self.emulator_script)
        self.assertIn('"x86_64" -notin $abiList', self.emulator_script)
        self.assertIn("-Target android-x86_64", self.emulator_script)
        for project in (
            "samples/android_resource_restore_seam",
            "samples/android_aot_seam",
            "samples/android_touch_seam",
            "samples/android_orientation_seam",
        ):
            self.assertEqual(1, self.emulator_script.count(project))
        self.assertEqual(2, self.emulator_script.count("samples/android_packaged_assets_seam"))
        self.assertIn("samples/android_asset_rejection_seam/android_seam_expectations.json", self.emulator_script)
        self.assertIn("[int]$PerSeamTimeoutSeconds = 660", self.emulator_script)
        self.assertLess(5 * 660, 75 * 60)

    def test_release_wrapper_defaults_to_all_seams_in_stable_order(self):
        self.assertIn('[string]$TestId = ""', self.emulator_script)
        self.assertIn('$selectedSeams = if ($TestId)', self.emulator_script)
        ordered_ids = [
            self.emulator_script.index(f'TestId = "{test_id}"')
            for test_id in ("IT-020", "IT-017", "IT-018", "IT-019", "IT-021", "IT-022")
        ]
        self.assertEqual(sorted(ordered_ids), ordered_ids)
        self.assertIn('} else {\n    $seams\n}', self.emulator_script)

    def test_workflow_dispatch_can_scope_it021_without_changing_workflow_call(self):
        self.assertIn("workflow_call:\n  workflow_dispatch:\n    inputs:", self.workflow)
        self.assertIn("release_shell_test_id:", self.workflow)
        self.assertIn('type: string', self.workflow)
        self.assertIn(
            'STASIS_RELEASE_SHELL_TEST_ID: ${{ inputs.release_shell_test_id }}',
            self.workflow,
        )
        self.assertIn(
            'test_release_shell_emulator.ps1 -TestId "$STASIS_RELEASE_SHELL_TEST_ID"',
            self.workflow,
        )
        self.assertNotIn(
            'test_release_shell_emulator.ps1 -TestId "$env:STASIS_RELEASE_SHELL_TEST_ID"',
            self.workflow,
        )
        self.assertNotIn(
            'test_release_shell_emulator.ps1 -TestId "${{ inputs.release_shell_test_id }}"',
            self.workflow,
        )
        self.assertIn('Where-Object { $_.TestId -eq $TestId }', self.emulator_script)
        self.assertIn('TestId = "IT-021"', self.emulator_script)

    def test_release_wrapper_rejects_unknown_test_ids_before_execution(self):
        self.assertIn('$TestId -notin $validTestIds', self.emulator_script)
        self.assertIn('Unknown Android release-shell seam test ID', self.emulator_script)
        validation = self.emulator_script.index('$validTestIds =')
        first_execution = self.emulator_script.index('test_release_shell.ps1')
        self.assertLess(validation, first_execution)

    def test_packaged_assets_use_the_non_lifecycle_resource_pixel_oracle(self):
        self.assertIn('if expectations.get("resource_regions"):', self.release_runner)
        expectations = json.loads(
            read("samples/android_packaged_assets_seam/android_seam_expectations.json")
        )
        self.assertNotIn("lifecycle", expectations)
        self.assertGreater(len(expectations["resource_regions"]), 0)

    def test_it022_declares_all_controlled_rejection_variants(self):
        self.assertEqual(
            {
                "missing",
                "tampered",
                "traversal",
                "duplicate",
                "oversized",
                "malformed-manifest",
            },
            set(self.asset_rejection_expectations["variants"]),
        )
        self.assertIn("asset_rejection", self.release_runner)
        self.assertIn("native_rejection_before_game_runtime", self.release_runner)
        variant_copy = self.release_script.index("Copy-Item -LiteralPath $packageRoot")
        variant_build = self.release_script.index(":app:assembleDebug", variant_copy)
        self.assertLess(variant_copy, variant_build)
        self.assertIn("--asset-variant $variant", self.release_script)
        self.assertIn("one-byte bound override", read("mobile/shells/android/README.md"))

    def test_strategy_makes_emulator_the_readiness_gate(self):
        self.assertIn("hosted x86_64 emulator is the CI and readiness", self.strategy)
        self.assertRegex(
            self.strategy, r"Production Android\s+packaging remains ARM64"
        )
        for test_id in ("IT-017", "IT-018", "IT-019", "IT-021"):
            row = next(line for line in self.strategy.splitlines() if f"| {test_id} |" in line)
            self.assertTrue(row.endswith("| Emulator |"), row)

    def test_touch_drag_is_long_enough_for_hosted_emulator_sampling(self):
        inside_drag = next(
            gesture
            for gesture in self.touch_expectations["touch"]["gestures"]
            if gesture["name"] == "inside_drag"
        )
        self.assertGreaterEqual(inside_drag["duration_ms"], 2000)

    def test_workshop_it025_isolated_from_release_shells(self):
        self.assertIn("[int]$StepTimeoutSeconds = 300", self.workshop_script)

    def test_workshop_fatal_scan_delegates_only_valid_it031_case_records(self):
        self.assertIn("ConvertFrom-Json -ErrorAction Stop", self.workshop_script)
        self.assertIn('$case.test_id -eq "IT-031"', self.workshop_script)
        self.assertIn("$null -ne $case.native", self.workshop_script)
        self.assertIn("$null -ne $case.ui", self.workshop_script)
        self.assertIn("$markerIndex = $line.IndexOf($markerText)", self.workshop_script)
        self.assertIn("$line = $line.Remove($markerIndex, $markerText.Length)", self.workshop_script)
        self.assertIn("$fatalScanLog | Select-String -SimpleMatch $fatalPatterns", self.workshop_script)
        self.assertIn("Leave malformed case lines in the fatal scan", self.workshop_script)

    def test_workshop_fatal_scan_retains_ambient_prefix_before_case_record(self):
        self.assertIn('"Render resource error"', self.workshop_script)
        self.assertIn('"resource restore failed"', self.workshop_script)
        self.assertIn("Stasis Workshop IT-031 case:\\s+(\\{.*\\})\\s*$", self.workshop_script)
        self.assertIn("$line = $_", self.workshop_script)
        self.assertIn("$markerIndex = $line.IndexOf($markerText)", self.workshop_script)
        self.assertIn("$line = $line.Remove($markerIndex, $markerText.Length)", self.workshop_script)
        self.assertIn("$line\n    })", self.workshop_script)
        self.assertIn("[math]::Min($StepTimeoutSeconds, $remainingSeconds)", self.workshop_script)
        self.assertIn("verify_android_workshop_seam.py", self.workshop_script)
        self.assertIn('Join-Path (Join-Path (Join-Path $repoRoot "artifacts") "android_workshop_seam") "e"', self.workshop_script)
        self.assertIn("Reusing ready Android emulator", self.workshop_script)
        self.assertIn('GetMethod("Kill", [Type[]]@([bool]))', self.workshop_script)
        self.assertIn('$process.Kill()', self.workshop_script)
        self.assertIn('taskkill.exe /PID $process.Id /T /F', self.workshop_script)
        self.assertNotIn('"-SkipRustBridgeBuild"', self.workshop_script)
        for path_fragment in ("tools\\ci\\", "samples\\render_parity\\", "app\\build\\outputs\\apk\\", "artifacts\\android_workshop_seam\\"):
            self.assertNotIn(path_fragment, self.workshop_script)
        self.assertIn('"linux-x86_64"', self.rust_bridge_script)
        self.assertIn("cargo_cache.py", self.rust_bridge_script)
        self.assertIn("[System.IO.Path]::IsPathRooted", self.rust_bridge_script)
        self.assertNotIn('"app\\src\\workshop\\jniLibs\\$abi"', self.rust_bridge_script)
        self.assertNotIn('"$abi\\libstasis_android_bridge.so"', self.provenance_script)


if __name__ == "__main__":
    unittest.main()
