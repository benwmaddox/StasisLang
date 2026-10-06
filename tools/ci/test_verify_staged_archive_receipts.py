from __future__ import annotations

import hashlib
import json
from pathlib import Path
import tempfile
import unittest

from tools.ci import verify_staged_archive_receipts as verifier


REPOSITORY = "stasislang/StasisLang"
RUN_ID = "123456789"
RELEASE_ID = "nightly-20261002-379"
SOURCE_COMMIT = "99a5b3943f1759b7abbdbf0e37634965749c1337"
HASHES = {
    "windows": "1" * 64,
    "linux": "2" * 64,
    "macos": "3" * 64,
}


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def consumer(label: str, target: str) -> dict:
    item = {
        "commands": {name: "passed" for name in verifier.CONSUMER_COMMANDS},
        "post_update_release": RELEASE_ID,
        "post_update_sha256": "4" * 64,
    }
    if target in {"windows", "linux", "macos"}:
        item["native"] = {
            "result": "passed",
            "project_name": "generics_collections" if label == "bundled" else "ArchiveFreshWindows",
            "package_provenance_sha256": "5" * 64,
            "executable_sha256": "6" * 64,
            "build_fingerprint": "a" * 64,
            "frame_sha256": "7" * 64,
            "frame": {"width": 640, "height": 360, "teal": 15000, "red": 0, "blue": 215000},
        }
    if target in {"windows", "linux", "macos"}:
        item["web"] = {
            "result": "passed",
            "project_name": "generics_collections" if label == "bundled" else f"ArchiveFresh{target.title()}",
            "package_provenance_sha256": "5" * 64,
            "build_fingerprint": "a" * 64,
            "frame_sha256": "7" * 64,
            "result_sha256": "8" * 64,
            "state_digest": 507,
            "math_raw_digest": -1430176193,
            "bounds": {"low": True, "high": True},
        }
    return item


def mobile_package(target: str) -> dict:
    shipping_target, test_target = (
        ("android-arm64", "android-x86_64")
        if target == "android"
        else ("ios-arm64", "ios-simulator-arm64")
    )
    compiler_sha = HASHES["linux" if target == "android" else "macos"]
    runtime_sources = {"runtime/stasis_graphics.c": "f" * 64}

    def package(package_target: str, *, development: bool) -> dict:
        return {
            "target": package_target,
            "development_build": development,
            "release_tag": None if development else RELEASE_ID,
            "source_commit": None if development else SOURCE_COMMIT,
            "dirty_state": development,
            "compiler_sha256": compiler_sha,
            "runtime_sources": runtime_sources,
            "provenance_sha256": "b" * 64,
            "manifest_sha256": "c" * 64,
        }

    shipping = package(shipping_target, development=False)
    shipping.update(
        {
            "result": "passed",
            "release_id": RELEASE_ID,
            "source_commit": SOURCE_COMMIT,
            "runtime_fingerprint": "a" * 64,
            "linked_artifact_sha256": "9" * 64,
            "link_evidence": [{"path": "shipping-link.log", "sha256": "8" * 64}],
        }
    )
    test_only = package(test_target, development=True)
    test_only.update({"result": "passed", "runtime_fingerprint": "a" * 64})
    return {
        "result": "passed",
        "release_id": RELEASE_ID,
        "source_commit": SOURCE_COMMIT,
        "package_targets": sorted((shipping_target, test_target)),
        "shipping_target": shipping_target,
        "test_target": test_target,
        "compiler_sha256": compiler_sha,
        "runtime_fingerprint": "a" * 64,
        "packages": [shipping, test_only],
        "shipping_package": shipping,
        "test_package": test_only,
    }


def mobile_execution(target: str) -> dict:
    return {
        "result": "passed",
        "platform": target,
        "target": "android-x86_64" if target == "android" else "ios-simulator-arm64",
        "development_build": True,
        "purpose": "test_only_emulator_or_simulator_execution",
        "frame_sha256": "d" * 64,
        "evidence": [{"path": "evidence.json", "sha256": "e" * 64}],
    }


class VerifyReceiptsTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        root = Path(self.temporary.name)
        self.receipts = root / "receipts"
        self.archives = root / "archives"
        self.receipts.mkdir()
        self.archives.mkdir()
        payloads = {
            "windows": b"windows archive",
            "linux": b"shared linux/android archive",
            "macos": b"shared macOS/iOS archive",
        }
        for target, (_, filename) in verifier.TARGET_FILES.items():
            platform = "linux" if target == "android" else "macos" if target == "ios" else target
            (self.archives / filename).write_bytes(payloads[platform])
            receipt = self.make_receipt(target)
            receipt["archive_sha256"] = sha256(self.archives / filename)
            target_dir = self.receipts / target
            target_dir.mkdir()
            (target_dir / "staged-archive-receipt.json").write_text(
                json.dumps(receipt), encoding="utf-8"
            )

    @staticmethod
    def make_receipt(target: str) -> dict:
        platform = "linux" if target == "android" else "macos" if target == "ios" else target
        target_triple, cli, runtime = verifier.TOOLCHAIN_LAYOUT[target]
        toolchain = {
            "release_id": RELEASE_ID,
            "source_commit": SOURCE_COMMIT,
            "target": target_triple,
            "executable_path": cli,
            "runtime_path": runtime,
            "build_fingerprint": "a" * 64,
            "runtime_build_fingerprint": "a" * 64,
            "cli_sha256": HASHES[platform],
            "runtime_sha256": HASHES[platform],
            "runtime_sources": {"runtime/stasis_graphics.c": "f" * 64},
        }
        consumers = {label: consumer(label, target) for label in ("bundled", "generated")}
        receipt = {
            "schema": "stasis.staged_archive_acceptance.v2",
            "target": target,
            "repository": REPOSITORY,
            "workflow_run_id": RUN_ID,
            "artifact_name": verifier.TARGET_FILES[target][0],
            "archive_file": verifier.TARGET_FILES[target][1],
            "release_id": RELEASE_ID,
            "source_commit": SOURCE_COMMIT,
            "toolchain": toolchain,
            "consumers": consumers,
            "vendor_failure_rollback": {
                "result": "rejected_tampered_archive_without_consumer_changes",
                "consumer_tree_sha256": "1" * 64,
                "consumer_manifest_sha256": "2" * 64,
            },
        }
        if target in {"android", "ios"}:
            for label in consumers:
                consumers[label]["mobile"] = mobile_package(target)
            executions = {label: mobile_execution(target) for label in consumers}
            for label in consumers:
                consumers[label]["mobile_runtime"] = executions[label]
            receipt["runtime_acceptance"] = {
                "result": "passed",
                "platform": target,
                "execution_target": "android-x86_64" if target == "android" else "ios-simulator-arm64",
                "development_build": True,
                "purpose": "test_only_emulator_or_simulator_execution",
                "consumers": executions,
            }
        return receipt

    def verify(self) -> dict[str, str]:
        return verifier.verify_receipts(
            self.receipts,
            self.archives,
            expected_repository=REPOSITORY,
            expected_run_id=RUN_ID,
            expected_release_id=RELEASE_ID,
            expected_source_commit=SOURCE_COMMIT,
        )

    def load(self, target: str) -> tuple[Path, dict]:
        path = self.receipts / target / "staged-archive-receipt.json"
        return path, json.loads(path.read_text(encoding="utf-8"))

    def save(self, path: Path, receipt: dict) -> None:
        path.write_text(json.dumps(receipt), encoding="utf-8")

    def test_accepts_complete_same_run_five_target_receipts(self) -> None:
        checked = self.verify()
        self.assertEqual(set(checked), set(verifier.TARGET_FILES))
        self.assertEqual(checked["linux"], checked["android"])
        self.assertEqual(checked["macos"], checked["ios"])

    def test_rejects_missing_generated_native_consumer(self) -> None:
        path, receipt = self.load("linux")
        del receipt["consumers"]["generated"]["native"]
        self.save(path, receipt)
        with self.assertRaisesRegex(ValueError, "generated consumer omitted native"):
            self.verify()

    def test_rejects_missing_generated_web_consumer_on_each_desktop_target(self) -> None:
        for target in ("windows", "linux", "macos"):
            with self.subTest(target=target):
                path, receipt = self.load(target)
                del receipt["consumers"]["generated"]["web"]
                self.save(path, receipt)
                with self.assertRaisesRegex(ValueError, "generated consumer omitted packaged Web"):
                    self.verify()

    def test_rejects_web_receipt_with_missing_math_raw_digest(self) -> None:
        path, receipt = self.load("windows")
        del receipt["consumers"]["bundled"]["web"]["math_raw_digest"]
        self.save(path, receipt)
        with self.assertRaisesRegex(ValueError, "Web raw math digest differs"):
            self.verify()

    def test_rejects_web_receipt_with_mismatched_math_raw_digest(self) -> None:
        path, receipt = self.load("windows")
        receipt["consumers"]["bundled"]["web"]["math_raw_digest"] = -1430176192
        self.save(path, receipt)
        with self.assertRaisesRegex(ValueError, "Web raw math digest differs"):
            self.verify()

    def test_rejects_web_receipt_with_floating_point_math_raw_digest(self) -> None:
        path, receipt = self.load("windows")
        receipt["consumers"]["bundled"]["web"]["math_raw_digest"] = -1430176193.0
        self.save(path, receipt)
        with self.assertRaisesRegex(ValueError, "Web raw math digest differs"):
            self.verify()

    def test_rejects_legacy_v1_archive_receipt(self) -> None:
        path, receipt = self.load("windows")
        receipt["schema"] = "stasis.staged_archive_acceptance.v1"
        self.save(path, receipt)
        with self.assertRaisesRegex(ValueError, "unsupported staged archive receipt schema"):
            self.verify()

    def test_rejects_missing_mobile_consumer(self) -> None:
        path, receipt = self.load("android")
        del receipt["consumers"]["generated"]["mobile_runtime"]
        del receipt["runtime_acceptance"]["consumers"]["generated"]
        self.save(path, receipt)
        with self.assertRaisesRegex(ValueError, "runtime evidence omitted a required consumer"):
            self.verify()

    def test_rejects_mismatched_source_commit(self) -> None:
        path, receipt = self.load("ios")
        receipt["source_commit"] = "0" * 40
        self.save(path, receipt)
        with self.assertRaisesRegex(ValueError, "different source commit"):
            self.verify()

    def test_rejects_mismatched_workflow_run(self) -> None:
        path, receipt = self.load("macos")
        receipt["workflow_run_id"] = "another-run"
        self.save(path, receipt)
        with self.assertRaisesRegex(ValueError, "another workflow run"):
            self.verify()

    def test_rejects_archive_bytes_that_differ_from_the_qualified_payload(self) -> None:
        (self.archives / verifier.TARGET_FILES["linux"][1]).write_bytes(b"mutated Linux archive")
        with self.assertRaisesRegex(ValueError, "did not test the bytes selected for publication"):
            self.verify()

    def test_rejects_non_hex_toolchain_identity(self) -> None:
        path, receipt = self.load("windows")
        receipt["toolchain"]["build_fingerprint"] = "development"
        self.save(path, receipt)
        with self.assertRaisesRegex(ValueError, "lowercase SHA-256"):
            self.verify()

    def test_accepts_unsigned_windows_candidate_before_publication(self) -> None:
        path, receipt = self.load("windows")
        receipt["artifact_name"] = "stasis-nightly-win-x64-unsigned"
        self.save(path, receipt)
        self.assertEqual(set(self.verify()), set(verifier.TARGET_FILES))

    def test_publisher_rejects_unsigned_windows_candidate(self) -> None:
        path, receipt = self.load("windows")
        receipt["artifact_name"] = "stasis-nightly-win-x64-unsigned"
        self.save(path, receipt)
        with self.assertRaisesRegex(ValueError, "requires the signed Windows archive"):
            verifier.verify_receipts(
                self.receipts,
                self.archives,
                expected_repository=REPOSITORY,
                expected_run_id=RUN_ID,
                expected_release_id=RELEASE_ID,
                expected_source_commit=SOURCE_COMMIT,
                require_signed_windows=True,
            )

    def test_rejects_different_android_archive_bytes_from_linux_lane(self) -> None:
        path, receipt = self.load("android")
        receipt["toolchain"]["cli_sha256"] = "0" * 64
        for label in ("bundled", "generated"):
            mobile = receipt["consumers"][label]["mobile"]
            mobile["compiler_sha256"] = "0" * 64
            for package in mobile["packages"]:
                package["compiler_sha256"] = "0" * 64
            mobile["shipping_package"]["compiler_sha256"] = "0" * 64
            mobile["test_package"]["compiler_sha256"] = "0" * 64
        self.save(path, receipt)
        with self.assertRaisesRegex(ValueError, "exact Linux archive"):
            self.verify()

    def test_rejects_test_package_that_claims_release_provenance(self) -> None:
        path, receipt = self.load("android")
        for label in ("bundled", "generated"):
            receipt["consumers"][label]["mobile"]["test_package"]["release_tag"] = RELEASE_ID
        self.save(path, receipt)
        with self.assertRaisesRegex(ValueError, "falsely claims official provenance"):
            self.verify()

    def test_rejects_shipping_package_marked_as_development(self) -> None:
        path, receipt = self.load("ios")
        for label in ("bundled", "generated"):
            receipt["consumers"][label]["mobile"]["shipping_package"]["development_build"] = True
        self.save(path, receipt)
        with self.assertRaisesRegex(ValueError, "shipping package is not the official"):
            self.verify()


if __name__ == "__main__":
    unittest.main()
