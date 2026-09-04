import contextlib
import io
import pathlib
import re
import subprocess
import tempfile
import unittest
from unittest import mock

from tools.ci import run_windows_platform_seams as seam_runner


ROOT = pathlib.Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github/workflows/pr-ci.yml"
RUNNER = ROOT / "tools/ci/run_windows_platform_seams.py"
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
        cls.vscode = job(cls.workflow, "vscode-extension-e2e")

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
        self.assertEqual(self.windows.count("--suite DesktopSdl"), 1)
        self.assertEqual(self.windows.count("--suite MobileRuntime"), 1)
        for target in DESKTOP_SDL_TARGETS + MOBILE_RUNTIME_TARGETS:
            with self.subTest(target=target):
                self.assertEqual(self.runner.count(f'"{target}"'), 1)
                if target != "desktop_display_metrics_seam":
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

    def test_macos_retina_seam_is_platform_bound_and_uploads_evidence(self):
        step_name = "Test macOS Retina display metrics seam"
        self.assertEqual(self.vscode.count(step_name), 1)
        retina = step(self.vscode, step_name)
        required = (
            "if: runner.os == 'macOS'",
            'export STASIS_RUNTIME_DLL_PATH="$STASIS_RUNTIME_LIBRARY_PATH"',
            "python3 tools/cargo_cache.py run -- cargo test -p stasis --test desktop_display_metrics_seam -- --test-threads=1 --nocapture",
            "target/vscode-e2e/desktop-display-metrics-seam.log",
            "kind=sprite.*logical=20x12 raster=40x24.*density_generation=7",
            "kind=font.*logical_size=18 raster_size=36 atlas=1024x1024.*density_generation=7",
            "build/codex-cargo-target/seam-tests/it-007-desktop-display-metrics.json",
        )
        for marker in required:
            with self.subTest(marker=marker):
                self.assertIn(marker, retina)

        command = "--test desktop_display_metrics_seam"
        self.assertEqual(self.vscode.count(command), 1)
        self.assertEqual(self.workflow.count(command), 1)
        upload = step(self.vscode, "Upload VS Code E2E evidence")
        for evidence in (
            "target/vscode-e2e/frame.png",
            "target/vscode-e2e/desktop-display-metrics-seam.log",
            "target/vscode-e2e/it-007-desktop-display-metrics.json",
        ):
            with self.subTest(evidence=evidence):
                self.assertEqual(upload.count(evidence), 1)

    def test_runner_uses_cached_cargo_and_names_grouped_failures(self):
        cargo_tokens = (
            '"tools/cargo_cache.py"',
            '"cargo"',
            '"test"',
            '"--test"',
            '"--test-threads=1"',
            '"--nocapture"',
        )
        for token in cargo_tokens:
            with self.subTest(token=token):
                self.assertIn(token, self.runner)
        self.assertIn('print(f"::group::{suite} - {target}")', self.runner)
        self.assertIn("seam suite failed", self.runner)
        self.assertIn("remove_lingering_case_processes(target)", self.runner)

    def test_windows_cases_stream_stable_logs_and_upload_evidence(self):
        self.assertIn('"target/windows-platform-seams" / suite', self.runner)
        self.assertIn('log_dir / f"{target}.log"', self.runner)
        self.assertIn("stderr=subprocess.STDOUT", self.runner)
        self.assertIn("sys.stdout.write(line)", self.runner)
        self.assertIn("log.write(line)", self.runner)
        self.assertEqual(seam_runner.CASE_TIMEOUT_SECONDS, 900)
        self.assertIn("deadline = time.monotonic() + timeout_seconds", self.runner)
        self.assertIn("_terminate_process_tree(process)", self.runner)
        self.assertIn("timed out after {CASE_TIMEOUT_SECONDS} seconds", self.runner)

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
            "build/codex-cargo-target/seam-tests/",
        )
        for marker in upload_markers:
            with self.subTest(marker=marker):
                self.assertEqual(upload.count(marker), 1)

    def test_timeout_failure_does_not_skip_remaining_suite_cases(self):
        with tempfile.TemporaryDirectory() as directory:
            results = [seam_runner.CaseResult(exit_code=-1, timed_out=True)]
            results.extend(
                seam_runner.CaseResult(exit_code=0, timed_out=False)
                for _ in range(5)
            )
            with mock.patch.object(
                seam_runner, "run_command", side_effect=results
            ) as run_mock, mock.patch.object(
                seam_runner, "remove_lingering_case_processes", return_value=[]
            ), contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(
                io.StringIO()
            ) as errors:
                exit_code = seam_runner.run_suite(
                    pathlib.Path(directory), "DesktopSdl"
                )

            self.assertEqual(exit_code, 1)
            self.assertEqual(run_mock.call_count, 6)
            self.assertIn(
                "desktop_input_frame_seam timed out after 900",
                errors.getvalue(),
            )

    def test_windows_timeout_kill_is_scoped_to_spawned_process_tree(self):
        process = mock.Mock()
        process.pid = 4321
        process.poll.return_value = None
        process.wait.return_value = -1
        completed = subprocess.CompletedProcess([], 0, "", "")
        with mock.patch.object(seam_runner.os, "name", "nt"), mock.patch.object(
            seam_runner.subprocess, "run", return_value=completed
        ) as run_mock:
            seam_runner._terminate_process_tree(process)

        self.assertEqual(
            run_mock.call_args.args[0],
            ["taskkill", "/PID", "4321", "/T", "/F"],
        )
        process.kill.assert_not_called()

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
