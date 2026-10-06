from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import shutil
import tempfile
import unittest
from unittest.mock import patch

from tools.ci import qualify_staged_archive as qualify


RELEASE_ID = "nightly-20261002-379"
SOURCE_COMMIT = "99a5b3943f1759b7abbdbf0e37634965749c1337"
FINGERPRINT = "a" * 64
RELEASE_VENDOR_SHA256 = "ecc6d516797ac0e84d9b4b9ca86925dee09d0ba06c559f4536248dd66f4779a1"
HISTORICAL_VENDOR_SHA256 = "1c91faa3e6baac7f72faecd11da75c781d688be7f0e817f9ea23e16a44b29623"


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def captured_current_vendor_status() -> dict:
    return {
        "actual_hash_version": 2,
        "actual_legacy_sha256": None,
        "actual_sha256": RELEASE_VENDOR_SHA256,
        "current": True,
        "expected_hash_version": 2,
        "expected_sha256": RELEASE_VENDOR_SHA256,
        "installed": {
            "hash_version": 2,
            "release_id": "nightly-20261005-391",
            "sha256": RELEASE_VENDOR_SHA256,
        },
        "legacy_pin_unverified": False,
        "local_changes": False,
        "pin_verified": True,
        "recorded": {
            "hash_version": 2,
            "release_id": "development",
            "sha256": RELEASE_VENDOR_SHA256,
        },
        "recorded_hash_version": 2,
        "recorded_sha256": RELEASE_VENDOR_SHA256,
        "update_available": False,
    }


def captured_historical_stale_vendor_status() -> dict:
    return {
        "actual_hash_version": 2,
        "actual_legacy_sha256": None,
        "actual_sha256": HISTORICAL_VENDOR_SHA256,
        "current": False,
        "expected_hash_version": 2,
        "expected_sha256": HISTORICAL_VENDOR_SHA256,
        "installed": {
            "hash_version": 2,
            "release_id": "nightly-20261005-391",
            "sha256": RELEASE_VENDOR_SHA256,
        },
        "legacy_pin_unverified": False,
        "local_changes": False,
        "pin_verified": True,
        "recorded": {
            "hash_version": 2,
            "release_id": "nightly-20260923-344",
            "sha256": HISTORICAL_VENDOR_SHA256,
        },
        "recorded_hash_version": 2,
        "recorded_sha256": HISTORICAL_VENDOR_SHA256,
        "update_available": True,
    }


def captured_updated_historical_vendor_status() -> dict:
    status = captured_current_vendor_status()
    status["recorded"]["release_id"] = "nightly-20261005-391"
    return status


class HistoricalStaleFixtureTests(unittest.TestCase):
    def test_fixture_matches_its_provenance_and_historical_vendor_hash(self) -> None:
        fixture = qualify.HISTORICAL_STALE_FIXTURE
        metadata = json.loads(
            qualify.HISTORICAL_STALE_FIXTURE_METADATA.read_text(encoding="utf-8")
        )
        manifest = json.loads((fixture / "stasis.json").read_text(encoding="utf-8"))
        vendor_root = fixture / "vendor" / "stasis"
        digest = hashlib.sha256()
        files = sorted(
            (path.relative_to(vendor_root).as_posix(), path)
            for path in vendor_root.rglob("*")
            if path.is_file()
        )
        total_bytes = 0
        for relative, path in files:
            self.assertFalse(path.is_symlink(), relative)
            content = path.read_bytes()
            total_bytes += len(content)
            if (
                Path(relative).suffix[1:] in {"stasis", "md", "json", "svg"}
                and b"\0" not in content
                and b"\r\n" in content
            ):
                content = content.decode("utf-8").replace("\r\n", "\n").encode("utf-8")
            digest.update(relative.encode("utf-8"))
            digest.update(b"\0")
            digest.update(content)
            digest.update(b"\0")

        self.assertEqual(qualify.validate_historical_stale_fixture(), fixture)
        self.assertEqual(manifest["vendor"]["stasis"], qualify.HISTORICAL_STALE_PIN)
        self.assertEqual(metadata["source_commit"], qualify.HISTORICAL_STALE_SOURCE_COMMIT)
        self.assertEqual(metadata["pin"], qualify.HISTORICAL_STALE_PIN)
        self.assertEqual(
            metadata["sample_tree_sha256"],
            qualify.HISTORICAL_STALE_SAMPLE_TREE_SHA256,
        )
        self.assertEqual(
            metadata["sample_file_count"], qualify.HISTORICAL_STALE_SAMPLE_FILE_COUNT
        )
        self.assertEqual(
            metadata["sample_raw_bytes"], qualify.HISTORICAL_STALE_SAMPLE_RAW_BYTES
        )
        self.assertEqual(
            metadata["vendor_file_count"], qualify.HISTORICAL_STALE_VENDOR_FILE_COUNT
        )
        self.assertEqual(
            metadata["vendor_raw_bytes"], qualify.HISTORICAL_STALE_VENDOR_RAW_BYTES
        )
        self.assertEqual(
            qualify.tree_sha256(fixture), qualify.HISTORICAL_STALE_SAMPLE_TREE_SHA256
        )
        self.assertEqual(len(files), 64)
        self.assertEqual(total_bytes, 262051)
        self.assertEqual(digest.hexdigest(), HISTORICAL_VENDOR_SHA256)

    def test_validator_rejects_untracked_historical_fixture_content_drift(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            temporary_root = Path(temporary)
            fixture = temporary_root / "stale_generics_344"
            metadata = temporary_root / "stale_generics_344.provenance.json"
            shutil.copytree(qualify.HISTORICAL_STALE_FIXTURE, fixture)
            shutil.copyfile(qualify.HISTORICAL_STALE_FIXTURE_METADATA, metadata)
            with (fixture / "README.md").open("ab") as readme:
                readme.write(b"\nfixture drift\n")

            with (
                patch.object(qualify, "HISTORICAL_STALE_FIXTURE", fixture),
                patch.object(qualify, "HISTORICAL_STALE_FIXTURE_METADATA", metadata),
                self.assertRaisesRegex(RuntimeError, "provenance changed"),
            ):
                qualify.validate_historical_stale_fixture()


class VendorUpdateOrchestrationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.project = self.root / "consumer"
        self.project.mkdir()
        self.evidence = self.root / "evidence"
        self.evidence.mkdir()

    def transition(
        self,
        statuses: list[dict],
        *,
        events: list[str],
        initial_state: str = "current-or-stale",
        expected_pin: dict | None = None,
        before_update=None,
    ) -> dict:
        pending = iter(statuses)

        def fake_run_json(_cli, args, **_kwargs):
            command = args[-1]
            events.append(command)
            if command == "status":
                return {"ok": True, "result": next(pending)}
            if command == "update":
                return {"ok": True, "result": {"updated": True}}
            self.fail(f"unexpected vendor command: {command}")

        with patch.object(qualify, "run_json", side_effect=fake_run_json):
            return qualify.qualify_vendor_update(
                Path("stasis"),
                self.project,
                expected_release="nightly-20261005-391",
                evidence=self.evidence,
                label="fixture",
                initial_state=initial_state,
                expected_pin=expected_pin,
                before_update=before_update,
            )

    def test_latest_bundled_consumer_accepts_current_pin_and_runs_noop_update(self) -> None:
        events: list[str] = []
        transition = self.transition(
            [
                captured_current_vendor_status(),
                captured_updated_historical_vendor_status(),
            ],
            events=events,
        )
        self.assertEqual(transition["initial_state"], "current")
        self.assertEqual(events, ["status", "update", "status"])
        self.assertIs(transition["after"]["current"], True)

    def test_latest_bundled_consumer_accepts_verified_stale_content_and_updates_it(self) -> None:
        events: list[str] = []
        transition = self.transition(
            [
                captured_historical_stale_vendor_status(),
                captured_updated_historical_vendor_status(),
            ],
            events=events,
        )
        self.assertEqual(transition["initial_state"], "stale")
        self.assertEqual(events, ["status", "update", "status"])
        self.assertIs(transition["after"]["current"], True)

    def test_historical_fixture_requires_exact_pin_and_probes_rollback_before_update(self) -> None:
        events: list[str] = []

        def rollback_probe() -> dict:
            events.append("rollback")
            return {"result": "rejected_tampered_archive_without_consumer_changes"}

        transition = self.transition(
            [
                captured_historical_stale_vendor_status(),
                captured_updated_historical_vendor_status(),
            ],
            events=events,
            initial_state="stale",
            expected_pin=qualify.HISTORICAL_STALE_PIN,
            before_update=rollback_probe,
        )
        self.assertEqual(events, ["status", "rollback", "update", "status"])
        self.assertEqual(transition["initial_state"], "stale")
        self.assertEqual(
            transition["before_update_result"]["result"],
            "rejected_tampered_archive_without_consumer_changes",
        )
        self.assertIs(transition["after"]["current"], True)

    def test_malformed_latest_status_fails_before_update(self) -> None:
        invalid_statuses = []
        local_edit = captured_historical_stale_vendor_status()
        local_edit["local_changes"] = True
        invalid_statuses.append(local_edit)
        unverified_pin = captured_historical_stale_vendor_status()
        unverified_pin["pin_verified"] = False
        invalid_statuses.append(unverified_pin)
        mismatched_pin = captured_historical_stale_vendor_status()
        mismatched_pin["expected_sha256"] = "d" * 64
        invalid_statuses.append(mismatched_pin)
        contradictory_flags = captured_current_vendor_status()
        contradictory_flags["update_available"] = True
        invalid_statuses.append(contradictory_flags)

        for status in invalid_statuses:
            with self.subTest(status=status):
                events: list[str] = []
                with self.assertRaises(RuntimeError):
                    self.transition([status], events=events)
                self.assertEqual(events, ["status"])

    def test_historical_fixture_rejects_wrong_pin_before_rollback_or_update(self) -> None:
        mismatches = []
        wrong_release = captured_historical_stale_vendor_status()
        wrong_release["recorded"]["release_id"] = "nightly-20260924-345"
        mismatches.append(wrong_release)
        wrong_hash = captured_historical_stale_vendor_status()
        wrong_hash["recorded"]["sha256"] = "e" * 64
        wrong_hash["recorded_sha256"] = "e" * 64
        wrong_hash["expected_sha256"] = "e" * 64
        wrong_hash["actual_sha256"] = "e" * 64
        mismatches.append(wrong_hash)

        for status in mismatches:
            with self.subTest(status=status):
                events: list[str] = []
                with self.assertRaises(RuntimeError):
                    self.transition(
                        [status],
                        events=events,
                        initial_state="stale",
                        expected_pin=qualify.HISTORICAL_STALE_PIN,
                        before_update=lambda: events.append("rollback"),
                    )
                self.assertEqual(events, ["status"])

    def test_vendor_update_must_leave_consumer_current(self) -> None:
        events: list[str] = []
        with self.assertRaisesRegex(RuntimeError, "not current"):
            self.transition(
                [
                    captured_historical_stale_vendor_status(),
                    captured_historical_stale_vendor_status(),
                ],
                events=events,
            )
        self.assertEqual(events, ["status", "update", "status"])


class PackageVendorIdentityTests(unittest.TestCase):
    def package_vendor(self, status: dict) -> dict:
        return {
            "release_id": status["recorded"]["release_id"],
            "recorded_sha256": status["recorded_sha256"],
            "actual_sha256": status["actual_sha256"],
            "recorded_hash_version": status["recorded_hash_version"],
            "actual_hash_version": status["actual_hash_version"],
        }

    def test_accepts_current_development_pin_when_package_matches_verified_status(self) -> None:
        status = captured_current_vendor_status()
        package_vendor = self.package_vendor(status)

        qualify.require_package_vendor_matches_status(
            package_vendor,
            status,
            expected_release="nightly-20261005-391",
            label="desktop package",
        )
        self.assertEqual(package_vendor["release_id"], "development")

    def test_accepts_stale_consumer_after_verified_update_to_archive_release(self) -> None:
        status = captured_updated_historical_vendor_status()
        package_vendor = self.package_vendor(status)

        qualify.require_package_vendor_matches_status(
            package_vendor,
            status,
            expected_release="nightly-20261005-391",
            label="Web package",
        )
        self.assertEqual(package_vendor["release_id"], "nightly-20261005-391")

    def test_rejects_forged_package_vendor_pin_hash_and_versions(self) -> None:
        status = captured_current_vendor_status()
        valid_vendor = self.package_vendor(status)
        mutations = {
            "wrong recorded release": ("release_id", "nightly-20261005-391"),
            "wrong recorded hash": ("recorded_sha256", "d" * 64),
            "wrong actual hash": ("actual_sha256", "e" * 64),
            "wrong recorded hash version": ("recorded_hash_version", 1),
            "wrong actual hash version": ("actual_hash_version", 1),
        }

        for label, (field, value) in mutations.items():
            with self.subTest(label=label):
                package_vendor = dict(valid_vendor)
                package_vendor[field] = value
                with self.assertRaises(RuntimeError):
                    qualify.require_package_vendor_matches_status(
                        package_vendor,
                        status,
                        expected_release="nightly-20261005-391",
                        label="desktop package",
                    )

    def test_rejects_package_when_verified_installed_snapshot_contradicts_archive(self) -> None:
        valid_vendor = self.package_vendor(captured_current_vendor_status())
        invalid_statuses = {
            "wrong installed release": captured_current_vendor_status(),
            "wrong installed hash": captured_current_vendor_status(),
            "contradictory currentness": captured_current_vendor_status(),
        }
        invalid_statuses["wrong installed release"]["installed"]["release_id"] = (
            "nightly-20261004-390"
        )
        invalid_statuses["wrong installed hash"]["installed"]["sha256"] = "f" * 64
        invalid_statuses["contradictory currentness"]["current"] = False
        invalid_statuses["contradictory currentness"]["update_available"] = True

        for label, status in invalid_statuses.items():
            with self.subTest(label=label), self.assertRaises(RuntimeError):
                qualify.require_package_vendor_matches_status(
                    valid_vendor,
                    status,
                    expected_release="nightly-20261005-391",
                    label="desktop package",
                )


class VerifyIdentityTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        (self.root / "stasis.exe").write_bytes(b"release compiler")
        (self.root / "stasis_graphics.dll").write_bytes(b"release runtime")
        self.provenance = {
            "schema": "stasis.release_provenance.v1",
            "release_tag": RELEASE_ID,
            "source_commit": SOURCE_COMMIT,
            "dirty_state": False,
            "development_build": False,
            "compiler": {"path": "stasis.exe", "sha256": sha256(self.root / "stasis.exe")},
            "runtime_sources": {},
        }
        (self.root / "stasis_release_provenance.json").write_text(
            json.dumps(self.provenance), encoding="utf-8"
        )
        self.editor_info = {
            "target": "x86_64-pc-windows-msvc",
            "release_id": RELEASE_ID,
            "source_commit": SOURCE_COMMIT,
            "build_fingerprint": FINGERPRINT,
            "executable": {
                "path": str(self.root / "stasis.exe"),
                "sha256": sha256(self.root / "stasis.exe"),
            },
            "graphics_runtime": {
                "path": str(self.root / "stasis_graphics.dll"),
                "sha256": sha256(self.root / "stasis_graphics.dll"),
                "release_id": RELEASE_ID,
                "build_fingerprint": FINGERPRINT,
            },
        }

    def run_identity(self) -> dict:
        return qualify.verify_identity(
            self.root / "stasis.exe",
            self.root,
            target="windows",
            expected_release=RELEASE_ID,
            expected_source=SOURCE_COMMIT,
            evidence=self.root / "evidence",
        )

    def test_requires_runtime_path_inside_exact_archive(self) -> None:
        outside = self.root.parent / "ambient-graphics.dll"
        outside.write_bytes(b"ambient runtime")
        self.addCleanup(lambda: outside.unlink(missing_ok=True))
        self.editor_info["graphics_runtime"]["path"] = str(outside)
        with patch.object(
            qualify,
            "run_json",
            return_value={"result": self.editor_info},
        ):
            with self.assertRaisesRegex(RuntimeError, "escapes the extracted archive"):
                self.run_identity()

    def test_requires_editor_target_to_match_archive(self) -> None:
        self.editor_info["target"] = "x86_64-unknown-linux-gnu"
        with patch.object(
            qualify,
            "run_json",
            return_value={"result": self.editor_info},
        ):
            with self.assertRaisesRegex(RuntimeError, "archive target"):
                self.run_identity()

    def test_strips_ambient_runtime_override_from_editor_info_probe(self) -> None:
        with patch.dict(os.environ, {"STASIS_RUNTIME_LIBRARY_PATH": "ambient.dll"}):
            environment = qualify.toolchain_environment()
        self.assertNotIn("STASIS_RUNTIME_LIBRARY_PATH", environment)


class PackageIdentityTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.path = Path(self.temporary.name) / "stasis_provenance.json"
        self.identity = {
            "cli_sha256": "b" * 64,
            "build_fingerprint": FINGERPRINT,
            "runtime_build_fingerprint": FINGERPRINT,
            "runtime_sources": {"runtime/stasis_graphics.c": "c" * 64},
        }
        self.provenance = {
            "schema": "stasis.release_provenance.v1",
            "release_tag": RELEASE_ID,
            "source_commit": SOURCE_COMMIT,
            "dirty_state": False,
            "development_build": False,
            "compiler": {"sha256": "b" * 64},
            "runtime_sources": {"runtime/stasis_graphics.c": "c" * 64},
        }
        self.path.write_text(json.dumps(self.provenance), encoding="utf-8")

    def test_package_provenance_must_match_release_compiler_and_runtime(self) -> None:
        self.assertEqual(
            qualify.validate_package_identity(
                self.path,
                identity=self.identity,
                expected_release=RELEASE_ID,
                expected_source=SOURCE_COMMIT,
            ),
            self.provenance,
        )

    def test_package_with_wrong_release_is_rejected(self) -> None:
        self.provenance["release_tag"] = "nightly-20261001-378"
        self.path.write_text(json.dumps(self.provenance), encoding="utf-8")
        with self.assertRaisesRegex(RuntimeError, "release tag"):
            qualify.validate_package_identity(
                self.path,
                identity=self.identity,
                expected_release=RELEASE_ID,
                expected_source=SOURCE_COMMIT,
            )


class GenericsBrowserReceiptTests(unittest.TestCase):
    def setUp(self) -> None:
        self.receipt = {
            "schema": "stasis.generics_collections_browser_acceptance.v2",
            "mainResult": 0,
            "stateDigest": 507,
            "mathRawDigest": -1430176193,
            "bounds": {"low": True, "high": True},
            "failures": [],
        }

    def test_accepts_v2_receipt_with_expected_math_oracle(self) -> None:
        qualify.validate_generics_browser_receipt(self.receipt)

    def test_rejects_legacy_v1_receipt(self) -> None:
        value = dict(self.receipt)
        value["schema"] = "stasis.generics_collections_browser_acceptance.v1"
        with self.assertRaisesRegex(RuntimeError, "schema.*raw math digest"):
            qualify.validate_generics_browser_receipt(value)

    def test_rejects_missing_or_mismatched_math_oracle(self) -> None:
        for label, raw in (("missing", None), ("mismatched", -1430176192)):
            with self.subTest(label=label):
                value = dict(self.receipt)
                if raw is None:
                    del value["mathRawDigest"]
                else:
                    value["mathRawDigest"] = raw
                with self.assertRaisesRegex(RuntimeError, "raw math digest"):
                    qualify.validate_generics_browser_receipt(value)

    def test_rejects_floating_point_math_oracle(self) -> None:
        value = dict(self.receipt)
        value["mathRawDigest"] = -1430176193.0
        with self.assertRaisesRegex(RuntimeError, "raw math digest"):
            qualify.validate_generics_browser_receipt(value)


if __name__ == "__main__":
    unittest.main()
