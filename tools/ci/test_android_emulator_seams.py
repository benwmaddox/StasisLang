from pathlib import Path
import json
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
        cls.emulator_script = read("mobile/android/test_release_shell_emulator.ps1")
        cls.strategy = read("docs/integration_seam_testing_strategy.md")
        cls.touch_expectations = json.loads(
            read("samples/android_touch_seam/android_seam_expectations.json")
        )

    def test_workflow_uses_hosted_x86_emulator(self):
        self.assertIn("runs-on: ubuntu-latest", self.workflow)
        self.assertNotIn("runs-on: macos-15", self.workflow)
        self.assertIn("reactivecircus/android-emulator-runner@v2", self.workflow)
        self.assertIn("api-level: 35", self.workflow)
        self.assertIn("arch: x86_64", self.workflow)
        self.assertIn("Enable KVM", self.workflow)
        self.assertIn("workflow_call:", self.workflow)
        self.assertNotIn("pull_request:", self.workflow)
        self.assertNotIn("workflow_dispatch:", self.workflow)
        self.assertIn('uses: actions/setup-python@v5', self.workflow)
        self.assertIn('python-version: "3.12"', self.workflow)
        self.assertIn("group: android-emulator-seams-nightly", self.workflow)
        self.assertIn("cancel-in-progress: false", self.workflow)
        self.assertIn("uses: ./.github/workflows/android-device-seams.yml", self.nightly_workflow)
        self.assertIn(
            "needs: [detect, build, vscode_extension, integration_seams, android_device_seams]",
            self.nightly_workflow,
        )
        self.assertIn("uses: ./.github/workflows/pr-ci.yml", self.nightly_workflow)
        self.assertIn("run_slow_seams: true", self.nightly_workflow)
        self.assertEqual(
            4,
            self.pr_workflow.count(
                "if: ${{ github.event_name == 'workflow_call' && inputs.run_slow_seams }}"
            ),
        )
        self.assertNotIn("self-hosted", self.workflow)
        self.assertNotIn("runs-on: [self-hosted, Windows, android-device]", self.workflow)
        self.assertNotIn("ANDROID_SERIAL", self.workflow)

    def test_nightly_grants_reusable_ci_read_permissions(self):
        self.assertIn("  contents: write", self.nightly_workflow)
        self.assertIn("  pull-requests: read", self.nightly_workflow)
        self.assertIn("  actions: read", self.nightly_workflow)

    def test_workflow_supplies_build_inputs_and_uploads_each_seam(self):
        self.assertIn("gradle-version: \"8.9\"", self.workflow)
        self.assertIn("ndk: 27.0.12077973", self.workflow)
        self.assertIn("cmake: 3.22.1", self.workflow)
        self.assertIn("8e37db5e797b6167f3a00d697d816a684bd259c7", self.workflow)
        self.assertIn("bec9134a26c7d0f31b36d6083c25296e04cabff5", self.workflow)
        self.assertLess(
            self.workflow.index("- name: Setup Gradle"),
            self.workflow.index("- name: Checkout SDL3"),
        )
        for artifact in (
            "android-release-shell-seam",
            "android-touch-roundtrip-seam",
            "android-orientation-metrics-seam",
        ):
            self.assertIn(artifact, self.workflow)
        self.assertEqual(3, self.workflow.count("        if: always()"))
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
            "samples/android_aot_seam",
            "samples/android_touch_seam",
            "samples/android_orientation_seam",
        ):
            self.assertEqual(1, self.emulator_script.count(project))

    def test_strategy_makes_emulator_the_readiness_gate(self):
        self.assertIn("hosted x86_64 emulator is the CI and readiness", self.strategy)
        self.assertIn("Production Android packaging remains ARM64", self.strategy)
        for test_id in ("IT-017", "IT-018", "IT-019"):
            row = next(line for line in self.strategy.splitlines() if f"| {test_id} |" in line)
            self.assertTrue(row.endswith("| Emulator |"), row)

    def test_touch_drag_is_long_enough_for_hosted_emulator_sampling(self):
        inside_drag = next(
            gesture
            for gesture in self.touch_expectations["touch"]["gestures"]
            if gesture["name"] == "inside_drag"
        )
        self.assertGreaterEqual(inside_drag["duration_ms"], 2000)


if __name__ == "__main__":
    unittest.main()
