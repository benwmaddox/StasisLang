import pathlib
import re
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github/workflows/pr-ci.yml"
RUNNER = ROOT / "tools/ci/run_windows_platform_seams.ps1"
STRATEGY = ROOT / "docs/integration_seam_testing_strategy.md"

DESKTOP_SDL_TARGETS = (
    "desktop_input_frame_seam",
    "desktop_display_metrics_seam",
    "desktop_manifest_assets_seam",
    "desktop_asset_load_stress",
    "desktop_render_recovery_seam",
    "desktop_hot_swap_generation_seam",
)
MOBILE_RUNTIME_TARGETS = (
    "generated_mobile_aot_runtime_seam",
    "mobile_packaged_assets_seam",
)


def job(text: str, name: str) -> str:
    match = re.search(
        rf"(?ms)^  {re.escape(name)}:\n(.*?)(?=^  [A-Za-z0-9_-]+:\n|\Z)",
        text,
    )
    if match is None:
        raise AssertionError(f"missing workflow job: {name}")
    return match.group(1)


def step(text: str, name: str) -> str:
    match = re.search(
        rf"(?ms)^      - name: {re.escape(name)}\n(.*?)(?=^      - name: |\Z)",
        text,
    )
    if match is None:
        raise AssertionError(f"missing workflow step: {name}")
    return match.group(1)


class PrCiSeamPlacementTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.workflow = WORKFLOW.read_text(encoding="utf-8")
        cls.runner = RUNNER.read_text(encoding="utf-8")
        cls.strategy = STRATEGY.read_text(encoding="utf-8")
        cls.linux = job(cls.workflow, "test")
        cls.windows = job(cls.workflow, "bootstrap-smoke-windows")

    def test_linux_ordinary_rust_seams_run_once_in_workspace_lane(self):
        broad = "cargo test --workspace --all-targets -- --test-threads=1"
        self.assertEqual(self.linux.count(broad), 1)
        redundant_commands = (
            "--test host_frame_jit_seam",
            "gfx_cmd_capacity_overflow_matches_jit_and_linked_aot_trace",
            "startup_asset_externs_match_jit_and_linked_aot_recording_host",
            "stasis_window_requests_apply_once_after_pre_main_baseline",
        )
        for command in redundant_commands:
            with self.subTest(command=command):
                self.assertNotIn(command, self.linux)

    def test_windows_platform_suites_have_exact_ownership(self):
        self.assertEqual(self.windows.count("-Suite DesktopSdl"), 1)
        self.assertEqual(self.windows.count("-Suite MobileRuntime"), 1)
        for target in DESKTOP_SDL_TARGETS + MOBILE_RUNTIME_TARGETS:
            with self.subTest(target=target):
                self.assertEqual(self.runner.count(f'"{target}"'), 1)
                self.assertNotIn(target, self.workflow)

    def test_windows_duplicate_focused_seams_are_absent(self):
        duplicates = (
            "startup_asset_externs_match_jit_and_linked_aot_recording_host",
            "--test jit_aot_host_replay_seam",
            "stasis_window_requests_apply_once_after_pre_main_baseline",
        )
        for command in duplicates:
            with self.subTest(command=command):
                self.assertNotIn(command, self.windows)
        compiler_suite = (
            "cargo test -p stasis_compiler -- --test-threads=1 --nocapture"
        )
        self.assertEqual(self.windows.count(compiler_suite), 1)

    def test_capture_and_boundary_jobs_remain_separate(self):
        self.assertEqual(self.windows.count("--test windows_game_launch"), 1)
        self.assertIn("Capture the real Windows SDL parity fixture", self.windows)
        for boundary_job in (
            "vscode-extension-e2e",
            "android-package-link",
            "ios-package-link",
        ):
            with self.subTest(job=boundary_job):
                self.assertRegex(self.workflow, rf"(?m)^  {boundary_job}:$")

    def test_runner_uses_cached_cargo_and_names_grouped_failures(self):
        command = (
            "python tools/cargo_cache.py run -- cargo test -p stasis "
            "--test $target -- --test-threads=1 --nocapture"
        )
        self.assertEqual(self.runner.count(command), 1)
        self.assertIn('Write-Host "::group::$Suite - $target"', self.runner)
        self.assertIn("seam suite failed", self.runner)
        self.assertIn("Remove-LingeringSeamProcesses -Target $target", self.runner)

    def test_windows_cases_stream_stable_logs_and_upload_evidence(self):
        self.assertIn(
            '"target/windows-platform-seams/$Suite"', self.runner
        )
        self.assertIn('$logPath = Join-Path $suiteLogDir "$target.log"', self.runner)
        self.assertIn("2>&1 | Tee-Object -FilePath $logPath", self.runner)
        self.assertIn("$exitCode = $LASTEXITCODE", self.runner)

        upload_name = "Upload Windows platform seam evidence"
        self.assertEqual(self.windows.count(upload_name), 1)
        upload = step(self.windows, upload_name)
        upload_markers = (
            "if: always()",
            "uses: actions/upload-artifact@v4",
            "name: windows-platform-seam-evidence",
            "if-no-files-found: warn",
            "target/render-parity-ci/frame.png",
            "target/render-parity-ci/runtime.log",
            "target/render-parity-ci/evidence.json",
            "target/windows-platform-seams/**/*.log",
        )
        for marker in upload_markers:
            with self.subTest(marker=marker):
                self.assertEqual(upload.count(marker), 1)

    def test_documentation_states_the_placement_rule(self):
        normalized_strategy = " ".join(self.strategy.split())
        required = (
            "Ordinary Rust test targets belong only in the broad Cargo workspace lane",
            "genuine platform prerequisite",
            "Compiler seams remain in the compiler-package suite",
            "Package-link, device-acceptance, and editor boundaries remain separate jobs",
            "must not create a second CI invocation",
        )
        for phrase in required:
            with self.subTest(phrase=phrase):
                self.assertIn(phrase, normalized_strategy)


if __name__ == "__main__":
    unittest.main()
