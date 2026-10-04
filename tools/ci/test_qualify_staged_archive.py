from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from tools.ci import qualify_staged_archive as qualify


RELEASE_ID = "nightly-20261002-379"
SOURCE_COMMIT = "99a5b3943f1759b7abbdbf0e37634965749c1337"
FINGERPRINT = "a" * 64


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


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


class ObsoleteVendorFixtureTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.project = self.root / "consumer"
        (self.project / "vendor/stasis/stdlib").mkdir(parents=True)
        (self.project / "vendor/stasis/stdlib/used.stasis").write_text(
            "function used(): i32 {\n    return 1;\n}\n", encoding="utf-8"
        )
        (self.project / "stasis.json").write_text(
            json.dumps(
                {
                    "name": "fixture",
                    "vendor": {
                        "stasis": {
                            "release_id": RELEASE_ID,
                            "sha256": "1" * 64,
                            "hash_version": 2,
                            "discarded_field": "must be replaced",
                        }
                    },
                }
            ),
            encoding="utf-8",
        )
        self.source = self.vendor_status(
            current=True,
            update_available=False,
            recorded_release="development",
            recorded_sha="1" * 64,
            actual_sha="1" * 64,
        )
        self.fixture_sha = "2" * 64
        self.stale = self.vendor_status(
            current=False,
            update_available=True,
            recorded_release=qualify.OBSOLETE_FIXTURE_LABEL,
            recorded_sha=self.fixture_sha,
            actual_sha=self.fixture_sha,
        )

    @staticmethod
    def vendor_status(
        *, current: bool, update_available: bool, recorded_release: str,
        recorded_sha: str, actual_sha: str,
    ) -> dict:
        return {
            "current": current,
            "update_available": update_available,
            "pin_verified": True,
            "legacy_pin_unverified": False,
            "local_changes": False,
            "recorded": {"release_id": recorded_release},
            "installed": {
                "release_id": RELEASE_ID,
                "sha256": actual_sha if current else "f" * 64,
                "hash_version": 2,
            },
            "recorded_hash_version": 2,
            "actual_hash_version": 2,
            "recorded_sha256": recorded_sha,
            "actual_sha256": actual_sha,
        }

    def run_fixture_staging(self, statuses: list[dict]) -> tuple[dict, dict, Path]:
        with patch.object(
            qualify,
            "run_json",
            side_effect=[{"ok": True, "result": status} for status in statuses],
        ):
            return qualify.stage_obsolete_vendor_fixture(
                Path("archived-stasis"),
                self.project,
                self.root / "evidence",
                expected_release=RELEASE_ID,
            )

    def test_uses_archived_cli_hash_and_replaces_complete_stale_pin(self) -> None:
        intermediate = {
            "actual_hash_version": 2,
            "actual_sha256": self.fixture_sha,
        }
        source, stale, fixture = self.run_fixture_staging(
            [self.source, intermediate, self.stale]
        )
        manifest = json.loads((self.project / "stasis.json").read_text(encoding="utf-8"))
        self.assertEqual(source["actual_sha256"], "1" * 64)
        self.assertEqual(stale["recorded_release_id"], qualify.OBSOLETE_FIXTURE_LABEL)
        self.assertEqual(stale["recorded_sha256"], self.fixture_sha)
        self.assertEqual(
            manifest["vendor"]["stasis"],
            {
                "release_id": "staged-archive-obsolete-fixture",
                "sha256": self.fixture_sha,
                "hash_version": 2,
            },
        )
        self.assertEqual(fixture.read_text(encoding="utf-8"), qualify.OBSOLETE_FIXTURE_SOURCE)
        self.assertNotIn("discarded_field", manifest["vendor"]["stasis"])

    def test_rejects_unverified_source_pin_before_creating_fixture(self) -> None:
        source = dict(self.source, pin_verified=False)
        with self.assertRaisesRegex(RuntimeError, "not verified and clean"):
            self.run_fixture_staging([source])
        self.assertFalse((self.project / qualify.OBSOLETE_FIXTURE_MODULE).exists())

    def test_rejects_source_pin_that_does_not_match_actual_tree(self) -> None:
        source = dict(self.source, actual_sha256="3" * 64)
        with self.assertRaisesRegex(RuntimeError, "not verified and clean"):
            self.run_fixture_staging([source])
        self.assertFalse((self.project / qualify.OBSOLETE_FIXTURE_MODULE).exists())

    def test_accepts_clean_source_with_update_available(self) -> None:
        source = self.vendor_status(
            current=False,
            update_available=True,
            recorded_release="development",
            recorded_sha="1" * 64,
            actual_sha="1" * 64,
        )
        intermediate = {"actual_hash_version": 2, "actual_sha256": self.fixture_sha}
        _, stale, _ = self.run_fixture_staging([source, intermediate, self.stale])
        self.assertEqual(stale["recorded_release_id"], qualify.OBSOLETE_FIXTURE_LABEL)

    def test_rejects_noncanonical_cli_fixture_hash(self) -> None:
        with self.assertRaisesRegex(RuntimeError, "canonical v2 fixture tree hash"):
            self.run_fixture_staging(
                [self.source, {"actual_hash_version": 1, "actual_sha256": self.fixture_sha}]
            )

    def test_rejects_dirty_or_current_fixture_status(self) -> None:
        intermediate = {"actual_hash_version": 2, "actual_sha256": self.fixture_sha}
        for status in (
            dict(self.stale, local_changes=True),
            dict(self.stale, current=True, update_available=False),
        ):
            with self.subTest(status=status):
                self.setUp()
                intermediate = {"actual_hash_version": 2, "actual_sha256": self.fixture_sha}
                with self.assertRaisesRegex(RuntimeError, "verified clean stale fixture"):
                    self.run_fixture_staging([self.source, intermediate, status])

    def test_removal_check_fails_closed_for_a_surviving_module(self) -> None:
        fixture = self.project / qualify.OBSOLETE_FIXTURE_MODULE
        fixture.parent.mkdir(parents=True, exist_ok=True)
        fixture.write_text("obsolete", encoding="utf-8")
        with self.assertRaisesRegex(RuntimeError, "retained the obsolete fixture"):
            qualify.require_obsolete_fixture_removed(fixture)
        fixture.unlink()
        qualify.require_obsolete_fixture_removed(fixture)

    def test_post_update_status_requires_verified_official_v2_pin(self) -> None:
        status = dict(self.source)
        status["recorded"] = {"release_id": RELEASE_ID}
        status["installed"] = {
            "release_id": RELEASE_ID,
            "sha256": status["actual_sha256"],
            "hash_version": 2,
        }
        with patch.object(
            qualify,
            "run_json",
            return_value={"ok": True, "result": status},
        ):
            current = qualify.require_vendor_current(
                Path("archived-stasis"),
                self.project,
                expected_release=RELEASE_ID,
                evidence=self.root / "evidence",
                label="after",
            )
        self.assertEqual(current["recorded_hash_version"], 2)

        invalid_statuses = (
            ("unverified", dict(status, pin_verified=False)),
            ("legacy", dict(status, legacy_pin_unverified=True)),
            ("hash mismatch", dict(status, recorded_sha256="b" * 64)),
            ("installed hash mismatch", dict(status, installed={
                "release_id": RELEASE_ID,
                "sha256": "b" * 64,
                "hash_version": 2,
            })),
        )
        for field, invalid in invalid_statuses:
            with self.subTest(field=field):
                with patch.object(
                    qualify,
                    "run_json",
                    return_value={"ok": True, "result": invalid},
                ):
                    with self.assertRaisesRegex(RuntimeError, "not current and clean"):
                        qualify.require_vendor_current(
                            Path("archived-stasis"),
                            self.project,
                            expected_release=RELEASE_ID,
                            evidence=self.root / "evidence",
                            label="after",
                        )


if __name__ == "__main__":
    unittest.main()
