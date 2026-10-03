from __future__ import annotations

import hashlib
import json
from pathlib import Path
import tempfile
import unittest

from tools.ci.collect_staged_archive_evidence import collect_evidence


SHARED_LOGS = (
    "bundle-audit.json",
    "bundle-audit.log",
    "editor-info.log",
    "vendor-rollback-probe.log",
    "bundled-vendor-before.log",
    "bundled-vendor-update.log",
    "bundled-vendor-status.json.log",
    "generated-project-new.log",
    "generated-vendor-status.json.log",
)


def write(root: Path, relative: str, content: bytes = b"review evidence\n") -> Path:
    path = root / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(content)
    return path


def base_tree(root: Path) -> tuple[Path, Path]:
    staged = root / "staged"
    archive = root / "archive"
    for name in SHARED_LOGS:
        write(staged, name)
    write(archive, "stasis_release_provenance.json", b'{"release_tag":"nightly-test"}\n')
    write(
        staged,
        "staged-archive-receipt.json",
        json.dumps(
            {
                "target": "linux",
                "workflow_run_id": "123",
                "artifact_name": "stasis-nightly-linux-x64",
                "archive_sha256": "a" * 64,
                "release_id": "nightly-test",
                "source_commit": "b" * 40,
                "toolchain": {"build_fingerprint": "c" * 64},
                "consumers": {"bundled": {}, "generated": {}},
            }
        ).encode(),
    )
    for consumer in ("bundled-generics", "generated-generics"):
        write(staged, f"consumers/{consumer}/stasis.json", b'{"project":"fixture"}\n')
        write(staged, f"consumers/{consumer}/vendor/stasis/stasis.json", b'{"vendor":"fixture"}\n')
    return staged, archive


def add_android(root: Path, staged: Path) -> Path:
    runtime = root / "android-runtime"
    for label, consumer in (("bundled", "bundled-generics"), ("generated", "generated-generics")):
        runtime_root = runtime / label
        for name in (
            "evidence.json",
            "stable-frame.png",
            "android-logcat.txt",
            "bounds-low-logcat.txt",
            "bounds-high-logcat.txt",
            "android-test-signer.json",
        ):
            write(runtime_root, f"test/e/{name}")
        for relative in (
            "stasis_mobile_package.json",
            "stasis_provenance.json",
            "aot/mobile_aot_bundle_manifest.json",
            "aot/engine_bundle_manifest.json",
        ):
            write(runtime_root, f"test/w/d/{relative}")
            write(runtime_root, f"shipping/package/{relative}")
        write(runtime_root, "test/w/d/android/app/build/outputs/apk/debug/app-debug.apk", b"not review evidence")
        write(runtime_root, "shipping/android/app/build/outputs/apk/debug/app-debug.apk", b"not review evidence")
        for name in ("android-apk-audit.log", "gradle-link.log", "package-provenance.log", "apksigner.log"):
            write(runtime_root, f"shipping/{name}")
        write(runtime_root, "shipping/apksigner-receipt.json", b'{"result":"passed"}\n')
    return runtime


class CollectStagedArchiveEvidenceTests(unittest.TestCase):
    def test_android_evidence_retains_frames_signers_manifests_and_skips_apks(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            staged, archive = base_tree(root)
            runtime = add_android(root, staged)
            output = root / "evidence"
            manifest = collect_evidence(
                target="android",
                staged_root=staged,
                archive_root=archive,
                runtime_root=runtime,
                output=output,
                lane_passed=True,
            )
            self.assertEqual(manifest["status"], "complete", manifest["missing_required"])
            paths = {item["path"] for item in manifest["files"]}
            self.assertIn("android/generated/runtime/android-test-signer.json", paths)
            self.assertTrue(any(path.startswith("android/generated/test-package-manifests/") for path in paths))
            self.assertTrue(any(path.endswith("shipping-package-manifests/stasis_provenance.json") for path in paths))
            self.assertFalse(any(path.endswith(".apk") or "/build/" in path for path in paths))
            self.assertNotIn("staged/hosted-chrome-version.txt", paths)
            for item in manifest["files"]:
                retained = output / item["path"]
                self.assertEqual(item["sha256"], hashlib.sha256(retained.read_bytes()).hexdigest())

    def test_ios_evidence_retains_simulator_frame_and_both_bounds_crash_reports(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            staged, archive = base_tree(root)
            for consumer in ("bundled-generics", "generated-generics"):
                build_root = Path("consumers") / consumer / "target/ios-archive-acceptance"
                for name in (
                    "evidence.txt",
                    "xcode-version.txt",
                    "xcodebuild.log",
                    "device-platform.txt",
                    "linked-libraries.txt",
                    "device-symbols.txt",
                    "device-hashes.txt",
                    "simulator-platform.txt",
                    "simulator-linked-libraries.txt",
                    "simulator-symbols.txt",
                    "simulator-hashes.txt",
                    "simulator-xcodebuild.log",
                    "simulator-launch.txt",
                    "simulator-result.json",
                    "simulator-frame.png",
                    "simulator.log",
                    "simulator-evidence.json",
                    "simulator-evidence.txt",
                    "bounds-low.json",
                    "bounds-high.json",
                    "bounds-low-launch.txt",
                    "bounds-high-launch.txt",
                    "bounds-low-crash.ips",
                    "bounds-high-crash.ips",
                ):
                    write(staged, f"{build_root}/{name}")
                for package_name in (f"staged-ios-{consumer}", f"staged-ios-{consumer}-simulator"):
                    for relative in (
                        "stasis_mobile_package.json",
                        "stasis_provenance.json",
                        "aot/mobile_aot_bundle_manifest.json",
                        "aot/engine_bundle_manifest.json",
                    ):
                        write(staged, f"consumers/{consumer}/dist/{package_name}/{relative}")
            output = root / "evidence"
            manifest = collect_evidence(
                target="ios",
                staged_root=staged,
                archive_root=archive,
                output=output,
                lane_passed=True,
            )
            self.assertEqual(manifest["status"], "complete", manifest["missing_required"])
            paths = {item["path"] for item in manifest["files"]}
            self.assertIn("ios/generated/staged-ios-generated-generics-simulator/manifests/aot/engine_bundle_manifest.json", paths)
            self.assertIn("ios/generated/acceptance/bounds-low-crash.ips", paths)
            self.assertIn("ios/generated/acceptance/bounds-high-crash.ips", paths)
            self.assertNotIn("staged/hosted-chrome-version.txt", paths)

    def test_passed_desktop_lane_fails_closed_if_required_frame_was_not_retained(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            staged, archive = base_tree(root)
            write(staged, "hosted-chrome-version.txt", b"Google Chrome 140.0.7339.80\n")
            for label, consumer in (("bundled", "bundled-generics"), ("generated", "generated-generics")):
                for name in (
                    "desktop-package.log",
                    "desktop-provenance-audit.log",
                    "desktop-runtime.log",
                    "desktop-frame.png",
                ):
                    content = b"x" * (20 * 1024 * 1024 + 1) if label == "generated" and name == "desktop-frame.png" else b"frame"
                    write(staged, f"desktop/{label}/{name}", content)
                write(staged, f"consumers/{consumer}/dist/staged-desktop-linux/stasis_provenance.json")
                write(staged, f"consumers/{consumer}/build/staged-web/stasis_provenance.json")
                for name in ("fmt-check", "check", "test", "jit-headless"):
                    write(staged, f"{label}-{name}.log")
                write(staged, f"web/{label}/web-package.log")
                write(staged, f"web/{label}/web-browser.log")
                write(staged, f"web/{label}/browser/receipt.json")
                write(staged, f"web/{label}/browser/browser.png")
            manifest = collect_evidence(
                target="linux",
                staged_root=staged,
                archive_root=archive,
                output=root / "evidence",
                lane_passed=True,
            )
            self.assertEqual(manifest["status"], "incomplete")
            self.assertTrue(any("generated/desktop-frame.png" in item and "not retained" in item for item in manifest["missing_required"]))

    def test_failed_lane_still_writes_a_diagnostic_manifest(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = root / "evidence"
            manifest = collect_evidence(
                target="ios",
                staged_root=root / "missing-staged",
                archive_root=root / "missing-archive",
                output=output,
                lane_passed=False,
            )
            self.assertEqual(manifest["status"], "diagnostic_partial")
            self.assertTrue((output / "evidence-manifest.json").is_file())


if __name__ == "__main__":
    unittest.main()
