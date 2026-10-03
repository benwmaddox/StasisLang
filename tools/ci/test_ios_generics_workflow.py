import re
import unittest
from pathlib import Path


class IosGenericsWorkflowPolicyTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.repo = Path(__file__).resolve().parents[2]
        cls.workflow = (
            cls.repo / ".github/workflows/ios-generics-simulator.yml"
        ).read_text(encoding="utf-8")

    def test_manual_lane_is_arm64_and_separate_from_required_ci(self) -> None:
        triggers = self.workflow.split("on:\n", 1)[1].split("\n\n", 1)[0]
        self.assertEqual(triggers.strip(), "workflow_dispatch:")
        self.assertIn("runs-on: macos-15", self.workflow)
        self.assertIn('test "$(uname -m)" = "arm64"', self.workflow)
        self.assertIn("permissions:\n  contents: read", self.workflow)
        self.assertIn("cancel-in-progress: false", self.workflow)
        self.assertNotIn("ios-generics-simulator:", (
            self.repo / ".github/workflows/pr-ci.yml"
        ).read_text(encoding="utf-8"))
        self.assertNotIn("ios-generics-simulator:", (
            self.repo / ".github/workflows/nightly-validation.yml"
        ).read_text(encoding="utf-8"))

    def test_action_revisions_and_fresh_build_chain_are_pinned_and_bounded(self) -> None:
        revisions = re.findall(
            r"(?m)^\s*uses:\s+[^@\s]+@([^\s#]+)",
            self.workflow,
        )
        self.assertEqual(len(revisions), 4)
        self.assertTrue(all(re.fullmatch(r"[0-9a-f]{40}", ref) for ref in revisions))
        self.assertIn("targets: aarch64-apple-ios,aarch64-apple-ios-sim", self.workflow)
        self.assertIn("STASIS_SOURCE_COMMIT: ${{ github.sha }}", self.workflow)
        self.assertIn("STASIS_RELEASE_ID: ci-ios-generics-${{ github.run_id }}", self.workflow)
        self.assertIn("STASIS_RUNTIME_LIBRARY_PATH:", self.workflow)
        self.assertIn("compute_toolchain_fingerprint.py", self.workflow)
        self.assertIn("cmake --build", self.workflow)
        self.assertIn("cargo build -p stasis", self.workflow)
        self.assertGreaterEqual(self.workflow.count("timeout-minutes: 15"), 4)
        self.assertIn("STASIS_IOS_SIMULATOR_ACCEPTANCE: generics", self.workflow)
        self.assertIn("bash tools/ci/build_ios_package.sh", self.workflow)
        self.assertIn(
            'samples/generics_collections "dist/ios-ci-${GITHUB_RUN_ID}"',
            self.workflow,
        )
        self.assertNotIn("actions/cache", self.workflow)
        self.assertNotIn("secrets.", self.workflow)
        self.assertNotIn("DEVELOPMENT_TEAM", self.workflow)
        self.assertNotIn("macos-15-xlarge", self.workflow)
        ordered_stages = (
            "Compute run-specific CLI and runtime identity",
            "Build fresh host graphics runtime",
            "Build fresh host package compiler",
            "Build packages and run iOS Generics simulator acceptance",
            "Verify package provenance and complete evidence",
            "Upload simulator evidence and package manifests",
        )
        stage_positions = [self.workflow.index(stage) for stage in ordered_stages]
        self.assertEqual(stage_positions, sorted(stage_positions))

    def test_success_requires_exact_helper_evidence_and_source_provenance(self) -> None:
        for filename in (
            "simulator-runtimes.json",
            "simulator-device-types.json",
            "toolchain-build-fingerprint.txt",
            "host-runtime-build.log",
            "host-cli-build.log",
            "helper.log",
            "xcode-version.txt",
            "xcodebuild.log",
            "device-platform.txt",
            "linked-libraries.txt",
            "device-symbols.txt",
            "device-hashes.txt",
            "simulator-xcodebuild.log",
            "simulator-platform.txt",
            "simulator-linked-libraries.txt",
            "simulator-symbols.txt",
            "simulator-hashes.txt",
            "simulator-result.json",
            "simulator-frame.png",
            "simulator.log",
            "bounds-low.json",
            "bounds-high.json",
            "simulator-evidence.json",
            "simulator-evidence.txt",
        ):
            self.assertIn(filename, self.workflow)
        self.assertIn("bounds-low-crash.*", self.workflow)
        self.assertIn("bounds-high-crash.*", self.workflow)
        self.assertIn('provenance.get("source_commit") != expected_commit', self.workflow)
        self.assertIn('"ios-simulator-arm64"', self.workflow)
        self.assertIn("if: always()", self.workflow)
        self.assertIn("actions/upload-artifact@", self.workflow)
        self.assertIn("archive: true", self.workflow)


if __name__ == "__main__":
    unittest.main()
