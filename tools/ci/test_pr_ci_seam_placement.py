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
NIGHTLY_WORKFLOW = ROOT / ".github/workflows/nightly-validation.yml"
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
        cls.nightly_workflow = NIGHTLY_WORKFLOW.read_text(encoding="utf-8")
        cls.runner = RUNNER.read_text(encoding="utf-8")
        cls.strategy = STRATEGY.read_text(encoding="utf-8")
        cls.linux = job(cls.workflow, "test")
        cls.nightly_summary = job(cls.nightly_workflow, "test")
        cls.linux_ordinary = "\n".join(
            job(cls.nightly_workflow, name)
            for name in (
                "pr-ci-preflight",
                "pr-ci-cargo-workspace",
                "pr-ci-cargo-stasis-library",
                "pr-ci-cargo-stasis-test-harness",
                "pr-ci-cargo-stasis-main",
                "pr-ci-cargo-stasis-provenance",
                "pr-ci-cargo-stasis-integration",
            )
        )
        cls.windows = job(cls.nightly_workflow, "bootstrap-smoke-windows")
        cls.generics = job(cls.nightly_workflow, "pr-ci-generics-cross-platform")

    def test_linux_ordinary_rust_seams_run_once_in_bounded_shards(self):
        commands = (
            "cargo test --workspace --exclude stasis --all-targets -- --test-threads=1",
            "cargo build -p stasis --bin stasis",
            "cargo test -p stasis --lib -- --test-threads=1",
            "cargo test -p stasis --bin stasis --\n          --skip toolchain_cli::tests::release_provenance_rejects_substituted_renderer_sources\n          --test-threads=1",
            "cargo test -p stasis --bin stasis --no-run",
            "./target/pr-ci-stasis/provenance-test-harness\n          toolchain_cli::tests::release_provenance_rejects_substituted_renderer_sources",
            'cargo test -p stasis "${test_args[@]}" -- --test-threads=1',
            "cargo test -p stasis --test web_package -- --test-threads=1",
        )
        expected_counts = {
            "cargo build -p stasis --bin stasis": 2,
        }
        for command in commands:
            with self.subTest(command=command):
                self.assertEqual(
                    self.linux_ordinary.count(command),
                    expected_counts.get(command, 1),
                )
        self.assertEqual(self.linux_ordinary.count("timeout-minutes: 15"), 9)
        self.assertIn("find apps/stasis/tests", self.linux_ordinary)
        self.assertIn("needs:", self.nightly_summary)
        self.assertIn("always()", self.nightly_summary)
        for lane in (
            "pr-ci-preflight",
            "pr-ci-cargo-workspace",
            "pr-ci-cargo-stasis-library",
            "pr-ci-cargo-stasis-test-harness",
            "pr-ci-cargo-stasis-main",
            "pr-ci-cargo-stasis-provenance",
            "pr-ci-cargo-stasis-integration",
        ):
            with self.subTest(lane=lane):
                self.assertIn(f"needs.{lane}.result", self.nightly_summary)
        self.assertIn("actions/upload-artifact@", self.linux_ordinary)
        self.assertIn("actions/download-artifact@", self.linux_ordinary)
        redundant_commands = (
            "--test host_frame_jit_seam",
            "gfx_cmd_capacity_overflow_matches_jit_and_linked_aot_trace",
            "startup_asset_externs_match_jit_and_linked_aot_recording_host",
            "stasis_window_requests_apply_once_after_pre_main_baseline",
        )
        for command in redundant_commands:
            with self.subTest(command=command):
                self.assertNotIn(command, self.linux_ordinary)

    def test_pull_request_workflow_declares_only_three_required_jobs(self):
        jobs = re.findall(
            r"(?m)^  ([A-Za-z0-9_-]+):\s*$",
            self.workflow.split("jobs:", 1)[1],
        )
        self.assertEqual(
            jobs,
            ["test", "pr-ci-preflight", "pr-ci-core-cargo"],
        )
        self.assertIn("needs.pr-ci-preflight.result", self.linux)
        self.assertIn("needs.pr-ci-core-cargo.result", self.linux)
        self.assertIn('[[ "$result" != "success" ]]', self.linux)
        self.assertNotIn("run_slow_seams", self.workflow)

    def test_pr_core_cargo_keeps_fast_linux_coverage_and_formatting(self):
        core = job(self.workflow, "pr-ci-core-cargo")
        commands = (
            "cargo fmt --all --check",
            "cargo test --workspace --exclude stasis --all-targets -- --test-threads=1",
            "cargo build -p stasis --bin stasis",
            "cargo test -p stasis --lib -- --test-threads=1",
            "cargo test -p stasis --bin stasis --",
            "python tools/ci/run_architecture_characterization.py --run-fast",
        )
        for command in commands:
            with self.subTest(command=command):
                self.assertIn(command, core)
        self.assertEqual(core.count("cargo build -p stasis --bin stasis"), 1)
        self.assertLess(
            core.index("cargo build -p stasis --bin stasis"),
            core.index("run_architecture_characterization.py --run-fast"),
        )
        self.assertIn("--skip toolchain_cli::tests::release_provenance_rejects_substituted_renderer_sources", core)
        self.assertIn("npm ci --prefix vscode-stasis", core)
        preflight = job(self.workflow, "pr-ci-preflight")
        self.assertIn("run_architecture_characterization.py --check", preflight)
        self.assertNotIn("run_architecture_characterization.py --run-fast", preflight)
        self.assertNotIn("npm ci --prefix vscode-stasis", preflight)

    def test_nightly_validation_reusable_workflow_runs_and_requires_full_suite(self):
        self.assertNotIn("pull_request:", self.nightly_workflow)
        self.assertIn("workflow_dispatch:", self.nightly_workflow)
        self.assertIn("workflow_call:", self.nightly_workflow)
        self.assertNotIn("run_slow_seams", self.nightly_workflow)
        full_lanes = (
            "pr-ci-preflight",
            "pr-ci-cargo-workspace",
            "pr-ci-cargo-stasis-library",
            "pr-ci-cargo-stasis-test-harness",
            "pr-ci-cargo-stasis-main",
            "pr-ci-cargo-stasis-provenance",
            "pr-ci-cargo-stasis-integration",
            "pr-ci-generics-cross-platform",
            "pr-ci-browser-compiler",
            "bootstrap-smoke-windows",
            "vscode-extension-e2e",
            "android-package-link",
        )
        for lane in full_lanes:
            with self.subTest(lane=lane):
                self.assertRegex(self.nightly_summary, rf"(?m)^\s+- {lane}$")
                self.assertIn(f"needs.{lane}.result", self.nightly_summary)
        self.assertIn('[[ "$result" != "success" ]]', self.nightly_summary)

    def test_web_packages_have_a_separate_bounded_step_in_required_integration_job(self):
        integration = job(self.nightly_workflow, "pr-ci-cargo-stasis-integration")
        ordinary = step(integration, "Run Stasis integration Cargo tests")
        web = step(integration, "Run Stasis Web package Cargo tests")
        self.assertIn('if [[ "$test_name" == "web_package" ]]; then continue; fi', ordinary)
        self.assertIn("timeout-minutes: 15", web)
        self.assertIn("cargo test -p stasis --test web_package -- --test-threads=1", web)
        self.assertNotIn("continue-on-error", integration)

    def test_windows_platform_suites_have_exact_ownership(self):
        self.assertEqual(self.windows.count("--suite DesktopSdl"), 1)
        self.assertEqual(self.windows.count("--suite MobileRuntime"), 1)
        for target in DESKTOP_SDL_TARGETS + MOBILE_RUNTIME_TARGETS:
            with self.subTest(target=target):
                self.assertEqual(self.runner.count(f'"{target}"'), 1)
                self.assertNotIn(target, self.nightly_workflow)

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

    def test_windows_bootstrap_requires_ephemeral_signing_before_aot_seams(self):
        provision = self.windows.index(
            "- name: Provision ephemeral CI signing certificate"
        )
        capture = self.windows.index(
            "- name: Capture the real Windows SDL parity fixture"
        )
        compiler = self.windows.index(
            "- name: Bootstrap Compile Smoke (compiler .stasis)"
        )
        self.assertLess(provision, capture)
        self.assertLess(provision, compiler)
        for marker in (
            "STASIS_SIGNING_LOCAL_RECORD: ${{ runner.temp }}\\stasis-ci-signing-thumbprint.txt",
            "STASIS_SIGNING_CERTIFICATE: ${{ runner.temp }}\\stasis-ci-signing\\stasis-ci-signing.pfx",
            "STASIS_SIGNING_EPHEMERAL_PFX: \"1\"",
            "STASIS_SIGNING_TIMEOUT_SECONDS: \"120\"",
            'STASIS_REQUIRE_SIGNED_EXECUTION: "1"',
            "STASIS_SIGNING_MODE: required",
            "provision-ci-signing-pfx.ps1",
            "-Command status -TimeoutSeconds 120",
            "-Command sign -ScriptArguments",
            "-Command verify -ScriptArguments",
            "certificate_configured",
            "required",
        ):
            with self.subTest(marker=marker):
                self.assertIn(marker, self.windows)
        self.assertNotIn("STASIS_REQUIRE_SIGNED_EXECUTION", self.generics)

    def test_capture_and_boundary_jobs_remain_separate(self):
        self.assertEqual(self.windows.count("--test windows_game_launch"), 1)
        self.assertIn("Capture the real Windows SDL parity fixture", self.windows)
        for boundary_job in (
            "vscode-extension-e2e",
            "android-package-link",
        ):
            with self.subTest(job=boundary_job):
                self.assertRegex(self.nightly_workflow, rf"(?m)^  {boundary_job}:$")
        self.assertNotIn("ios-package-link:", self.nightly_workflow)
        self.assertNotIn("ios-generics-simulator:", self.nightly_workflow)

    def test_generics_desktop_parity_runs_and_uploads_on_linux_and_windows(self):
        for marker in (
            "os: ubuntu-latest",
            "os: windows-latest",
            "expected_arch: x86_64",
            "STASIS_EXPECTED_HOST_ARCH: ${{ matrix.expected_arch }}",
            "--test generics_collections_jit_aot_wasm",
            "--test generics_collections_aot_seam",
            "--test generics_collections_desktop",
            "Run packaged generics desktop acceptance on Unix",
            "Run packaged generics desktop acceptance on Windows",
            "Build matching packaged Web runtime",
            "Build matching packaged desktop runtime on Unix",
            "xvfb-run -a",
            "pkg-config",
            "libegl1",
            "libgl1-mesa-dri",
            "if: always()",
            "target/generics-desktop-${{ matrix.evidence }}/**",
        ):
            with self.subTest(marker=marker):
                self.assertIn(marker, self.generics)
        self.assertNotIn("macos-", self.generics)
        self.assertNotIn("STASIS_REQUIRE_SIGNED_EXECUTION", self.generics)
        self.assertEqual(self.generics.count("-DSTASIS_BUILD_RUNNER=ON"), 3)
        self.assertEqual(
            self.generics.count(
                "--target stasis_graphics stasis_runner"
            ),
            3,
        )
        self.assertEqual(self.generics.count("STASIS_RUNTIME_RUNNER_PATH="), 2)
        self.assertIn("if-no-files-found: error", self.generics)
        self.assertIn(
            "name: generics-collections-${{ matrix.evidence }}-evidence",
            self.generics,
        )
        windows_acceptance = self.generics.split(
            "- name: Run packaged generics desktop acceptance on Windows", 1
        )[1].split("\n      - name:", 1)[0]
        self.assertIn("if: runner.os == 'Windows'", windows_acceptance)
        self.assertIn("shell: pwsh", windows_acceptance)
        self.assertIn("timeout-minutes: 15", windows_acceptance)
        unix_acceptance = self.generics.split(
            "- name: Run packaged generics desktop acceptance on Unix", 1
        )[1].split("\n      - name:", 1)[0]
        self.assertIn("timeout-minutes: 15", unix_acceptance)

    def test_linux_presentation_baseline_uses_an_isolated_test_runtime(self):
        build = step(self.generics, "Build isolated presentation-poison runtime on Linux")
        acceptance = step(self.generics, "Run packaged presentation baseline on Linux")
        verification = step(self.generics, "Verify presentation baseline evidence on Linux")
        upload = step(self.generics, "Upload Linux presentation baseline evidence")
        normal_runtime = step(self.generics, "Build matching packaged desktop runtime on Unix")

        self.assertIn("if: runner.os == 'Linux'", build)
        self.assertIn("timeout-minutes: 15", build)
        self.assertIn("target/presentation-desktop-runtime", build)
        self.assertIn("-DSTASIS_TEST_PRESENTATION_POISON=ON", build)
        self.assertIn("ci-presentation-desktop-${GITHUB_RUN_ID}", build)
        self.assertIn("compute_toolchain_fingerprint.py", build)
        self.assertNotIn("STASIS_TEST_PRESENTATION_POISON", normal_runtime)
        self.assertIn("default-OFF production runtime unexpectedly exports", build)
        self.assertIn("default-OFF production runner unexpectedly contains", build)
        self.assertIn("default-off-runtime-symbols.txt", build)
        self.assertIn("instrumented-runtime-symbols.txt", build)
        self.assertIn("STASIS_TEST_PRESENTATION_POISON_ONCE", build)
        self.assertIn("STASIS_PRESENTATION_DESKTOP_EVIDENCE_DIR", acceptance)
        self.assertIn("target/presentation-desktop-runtime/bin/libstasis_graphics.so", acceptance)
        self.assertIn("target/presentation-desktop-runtime/bin/Release/stasis_runner", acceptance)
        self.assertIn("xvfb-run -a", acceptance)
        self.assertIn('STASIS_RELEASE_ID="$STASIS_PRESENTATION_RELEASE_ID"', acceptance)
        self.assertIn(
            'STASIS_BUILD_FINGERPRINT="$STASIS_PRESENTATION_BUILD_FINGERPRINT"',
            acceptance,
        )
        self.assertIn("--test presentation_baseline_desktop", acceptance)
        self.assertIn(
            "packaged_presentation_baseline_initializes_poisoned_target_without_guest_clear",
            acceptance,
        )
        for artifact in (
            "presentation-desktop-frame.png",
            "presentation-desktop-provenance.json",
            "presentation-desktop-runtime.log",
            "presentation-desktop-receipt.json",
        ):
            with self.subTest(artifact=artifact):
                self.assertIn(artifact, verification)
        self.assertIn("if-no-files-found: error", upload)
        self.assertIn("presentation-baseline-linux-x64-${{ github.run_id }}", upload)

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
            "uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a",
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
