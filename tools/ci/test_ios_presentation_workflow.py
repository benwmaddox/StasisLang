from __future__ import annotations

import unittest
from pathlib import Path


class IosPresentationWorkflowContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        root = Path(__file__).resolve().parents[2]
        cls.workflow = (root / ".github/workflows/ios-presentation-simulator.yml").read_text(
            encoding="utf-8"
        )
        cls.generics = (root / ".github/workflows/ios-generics-simulator.yml").read_text(
            encoding="utf-8"
        )
        cls.helper = (root / "tools/ci/build_ios_package.sh").read_text(encoding="utf-8")
        cls.mobile = (root / "mobile/shells/common/stasis_mobile_main.c").read_text(
            encoding="utf-8"
        )

    def test_workflow_uses_real_shared_fixture_and_fresh_toolchain(self) -> None:
        for marker in (
            "samples/presentation_baseline",
            "STASIS_IOS_SIMULATOR_ACCEPTANCE: presentation",
            "Build fresh host graphics runtime",
            "Build fresh host package compiler",
            "STASIS_SOURCE_COMMIT: ${{ github.sha }}",
            "stasis_mobile_package.json",
            "mobile_aot_bundle_manifest.json",
            "simulator-initial-receipt.json",
            "simulator-initial-frame.png",
            "simulator-relaunch-receipt.json",
            "simulator-relaunch-frame.png",
            "simulator-presentation-lifecycle.mp4",
            "simulator-presentation-record-video.log",
            "simulator-presentation-evidence.json",
            "if-no-files-found: warn",
        ):
            with self.subTest(marker=marker):
                self.assertIn(marker, self.workflow)
        self.assertNotIn("samples/generics_collections", self.workflow)
        self.assertNotIn("bounds-low", self.workflow)

    def test_presentation_mode_is_additive_and_test_gated(self) -> None:
        for marker in (
            '"${simulator_acceptance}" = "generics"',
            '"${simulator_acceptance}" = "presentation"',
            "STASIS_TEST_PRESENTATION_POISON=1",
            "SIMCTL_CHILD_STASIS_ENABLE_TEST_INPUT=1",
            "SIMCTL_CHILD_STASIS_SEAM_TEST_ID=IOS-PRESENTATION-BASELINE",
            "SIMCTL_CHILD_STASIS_PRESENTATION_PHASE",
            "verify_ios_presentation_baseline.py",
            "production iOS device binary unexpectedly exports presentation poison seams",
            "physical_device_qualified=false",
            "same_process_context_loss_qualified=false",
        ):
            with self.subTest(marker=marker):
                self.assertIn(marker, self.helper)
        self.assertIn("stasis.ios.generics.v2", self.mobile)
        self.assertIn("stasis.ios.presentation_baseline.v1", self.mobile)
        self.assertIn("#if defined(STASIS_TEST_PRESENTATION_POISON)", self.mobile)
        self.assertNotIn("STASIS_TEST_PRESENTATION_POISON", self.generics)

    def test_frame_one_capture_is_held_until_the_host_screenshot(self) -> None:
        phase_start = self.helper.index("    run_presentation_phase() {")
        phase_end = self.helper.index("\n    run_presentation_phase initial", phase_start)
        phase = self.helper[phase_start:phase_end]
        self.assertIn(
            'local capture_release="${data_container}/Documents/'
            'stasis-ios-presentation-${phase}.captured"',
            phase,
        )
        self.assertIn('rm -f -- "${receipt}" "${capture_release}"', phase)
        ordered = (
            phase.index('wait_for_receipt "${receipt}"'),
            phase.index('xcrun simctl io "${simulator_udid}" screenshot'),
            phase.index('touch "${capture_release}"'),
            phase.index('if [[ ! -e "${capture_release}" ]]'),
            phase.index('xcrun simctl terminate'),
        )
        self.assertEqual(ordered, tuple(sorted(ordered)))
        self.assertIn("for _ in $(seq 1 40); do", phase)
        self.assertIn("sleep 0.25", phase)
        self.assertIn("capture_release_consumed=1", phase)
        self.assertIn("presentation capture barrier did not consume", phase)

        barrier_start = self.mobile.index(
            "static int hold_ios_presentation_frame_for_capture"
        )
        barrier_end = self.mobile.index("\n}\n#endif", barrier_start) + 2
        barrier = self.mobile[barrier_start:barrier_end]
        self.assertIn("stasis_mobile_poll_events()", barrier)
        self.assertIn("60000000000ULL", barrier)
        self.assertNotIn("stasis_mobile_runtime_step", barrier)
        guard = self.mobile[max(0, barrier_start - 80) : barrier_start]
        self.assertIn(
            "#if defined(__APPLE__) && !defined(__ANDROID__)", guard
        )

    def test_presentation_log_poll_requires_all_markers(self) -> None:
        poll_start = self.helper.index(
            "    presentation_log_ready=0",
            self.helper.index("    run_presentation_phase relaunch"),
        )
        poll_end = self.helper.index(
            '    python3 "${repo_root}/tools/ci/verify_ios_presentation_baseline.py"',
            poll_start,
        )
        poll = self.helper[poll_start:poll_end]
        self.assertIn("for _ in $(seq 1 40); do", poll)
        self.assertIn("sleep 0.25", poll)
        for marker in (
            "Stasis provenance: .* renderer=gfx_cmd schema=7",
            "Stasis iOS presentation baseline phase=initial",
            "Stasis iOS presentation baseline phase=relaunch",
            "Stasis iOS presentation capture barrier phase=initial state=holding",
            "Stasis iOS presentation capture barrier phase=initial state=released",
            "Stasis iOS presentation capture barrier phase=relaunch state=holding",
            "Stasis iOS presentation capture barrier phase=relaunch state=released",
        ):
            with self.subTest(marker=marker):
                self.assertIn(marker, poll)
        self.assertIn("presentation_log_ready=1", poll)
        self.assertIn(
            "simulator unified log did not publish provenance, presentation phases, and capture barriers",
            poll,
        )

    def test_presentation_video_spans_both_held_phases_and_is_finalized(self) -> None:
        presentation_start = self.helper.index(
            '  if [[ "${simulator_acceptance}" = "presentation" ]]; then'
        )
        presentation_end = self.helper.index("\n  result_receipt=", presentation_start)
        presentation = self.helper[presentation_start:presentation_end]
        ordered = (
            presentation.index("\n    start_presentation_video\n"),
            presentation.index("\n    run_presentation_phase initial\n"),
            presentation.index("\n    run_presentation_phase relaunch\n"),
            presentation.index("\n    stop_presentation_video 1\n"),
        )
        self.assertEqual(ordered, tuple(sorted(ordered)))
        self.assertIn(
            'require_simulator_video_live "before ${phase} launch"', presentation
        )
        self.assertIn(
            'require_simulator_video_live "after ${phase} termination"', presentation
        )

        self.assertIn(
            'xcrun simctl io "${simulator_udid}" recordVideo --codec=h264',
            self.helper,
        )
        self.assertIn("simulator_video_pid=$!", self.helper)
        self.assertIn('kill -INT "${simulator_video_pid}"', self.helper)
        self.assertIn('wait "${simulator_video_pid}"', self.helper)
        self.assertIn('return "${exit_status}"', self.helper)
        self.assertIn('! -s "${presentation_video_path}"', self.helper)
        self.assertIn('! -s "${presentation_video_log}"', self.helper)

        cleanup_start = self.helper.index("cleanup() {")
        cleanup_end = self.helper.index("\n}\ntrap cleanup EXIT", cleanup_start)
        cleanup = self.helper[cleanup_start:cleanup_end]
        self.assertIn("local status=$?", cleanup)
        self.assertIn('exit "${status}"', cleanup)
        self.assertLess(
            cleanup.index("stop_presentation_video 0"),
            cleanup.index('xcrun simctl shutdown "${simulator_udid}"'),
        )
        self.assertNotIn("recordVideo", self.generics)


if __name__ == "__main__":
    unittest.main()
