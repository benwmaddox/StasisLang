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


if __name__ == "__main__":
    unittest.main()
