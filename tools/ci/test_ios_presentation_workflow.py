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


if __name__ == "__main__":
    unittest.main()
