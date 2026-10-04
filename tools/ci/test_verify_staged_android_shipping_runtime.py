from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest

from tools.ci import verify_staged_android_shipping_runtime as verifier


RELEASE_ID = "nightly-20261003-383"
SOURCE_COMMIT = "99a5b3943f1759b7abbdbf0e37634965749c1337"
FINGERPRINT = "a" * 64
VARIANTS = {
    "offline": "lib/libstasis_mobile_runtime_offline.a",
    "host": "lib/libstasis_mobile_runtime_host.a",
    "client": "lib/libstasis_mobile_runtime_client.a",
}
SOURCE_HASHES = {
    "stasis_runtime": "1" * 64,
    "sdl3": "2" * 64,
    "sdl3_image": "3" * 64,
}
COMPILE_CONTRACT = {
    "build_type": "Release",
    "position_independent_code": True,
    "disabled_sdl_subsystems": [
        "camera", "dialog", "gpu", "haptic", "hidapi", "joystick",
        "power", "sensor", "tray", "vulkan",
    ],
    "sdl_image_decoders": ["png-stb"],
    "runtime_variants": ["offline", "host", "client"],
    "system_libraries": [
        "m", "dl", "OpenSLES", "log", "android", "GLESv1_CM", "GLESv2",
    ],
}


def write_json(path: Path, value: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, sort_keys=True) + "\n", encoding="utf-8")


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


class StagedAndroidShippingRuntimeTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.archive = self.root / "archive"
        self.package = self.root / "package"
        self.archive_kit = self.archive / "mobile/android-runtime/arm64-v8a"
        self.package_kit = self.package / "android/runtime"
        self.make_fixture()

    def make_fixture(self) -> None:
        payloads = {
            "include/SDL3/SDL.h": b"SDL header",
            "include/SDL3_image/SDL_image.h": b"SDL image header",
            "java/org/libsdl/app/SDLActivity.java": b"SDL activity",
            "licenses/SDL3-LICENSE.txt": b"SDL license",
            "licenses/SDL3_image-LICENSE.txt": b"SDL image license",
            "lib/libSDL3.a": b"SDL archive",
            "lib/libSDL3_image.a": b"SDL image archive",
            "lib/libstasis_thorvg.a": b"ThorVG archive",
            VARIANTS["offline"]: b"offline runtime",
            VARIANTS["host"]: b"host runtime",
            VARIANTS["client"]: b"client runtime",
        }
        for relative, payload in payloads.items():
            path = self.archive_kit / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(payload)

        self.runtime_manifest = {
            "schema": "stasis.android_runtime.v1",
            "release_id": RELEASE_ID,
            "source_commit": SOURCE_COMMIT,
            "build_fingerprint": FINGERPRINT,
            "target": "android-arm64",
            "abi": "arm64-v8a",
            "android_api": 26,
            "ndk_version": "27.0.12077973",
            "cmake_version": "3.22.1",
            "generator": "Ninja",
            "cxx_runtime": "c++_static",
            "runtime_abi_version": 2,
            "graphics_abi_version": 4,
            "dependencies": {"sdl3": "3.4.10", "sdl3_image": "3.4.4"},
            "source_hashes": dict(SOURCE_HASHES),
            "compile_contract": dict(COMPILE_CONTRACT),
            "libraries": {
                "sdl3": "lib/libSDL3.a",
                "sdl3_image": "lib/libSDL3_image.a",
                "thorvg": "lib/libstasis_thorvg.a",
            },
            "runtime_variants": dict(VARIANTS),
            "files": {
                relative: digest(self.archive_kit / relative)
                for relative in sorted(payloads)
            },
        }
        archive_manifest_path = self.archive_kit / "manifest.json"
        write_json(archive_manifest_path, self.runtime_manifest)
        shutil.copytree(self.archive_kit, self.package_kit)

        prefix = "mobile/android-runtime/arm64-v8a/"
        artifact_hashes = {
            prefix + path.relative_to(self.archive_kit).as_posix(): digest(path)
            for path in sorted(self.archive_kit.rglob("*"))
            if path.is_file()
        }
        self.release_provenance = {
            "schema": "stasis.release_provenance.v1",
            "release_tag": RELEASE_ID,
            "source_commit": SOURCE_COMMIT,
            "dirty_state": False,
            "development_build": False,
            "android_runtime_artifacts": artifact_hashes,
        }
        write_json(self.archive / "stasis_release_provenance.json", self.release_provenance)
        write_json(self.package / "stasis_provenance.json", self.release_provenance)
        identity = {
            field: self.runtime_manifest[field]
            for field in verifier.EXPECTED_IDENTITY_FIELDS
        }
        identity.update(
            {
                "variant": "offline",
                "runtime_library": VARIANTS["offline"],
                "source_hashes": dict(SOURCE_HASHES),
                "compile_contract": dict(COMPILE_CONTRACT),
            }
        )
        write_json(
            self.package / "stasis_mobile_package.json",
            {
                "target": "android-arm64",
                "development_build": False,
                "android_runtime": {
                    "mode": "prebuilt",
                    "variant": "offline",
                    "manifest": "android/runtime/manifest.json",
                    "identity": identity,
                },
            },
        )

    def verify(self, *, release: str = RELEASE_ID, source: str = SOURCE_COMMIT,
               fingerprint: str = FINGERPRINT) -> dict:
        return verifier.verify_shipping_runtime(
            self.archive,
            self.package,
            expected_release=release,
            expected_source=source,
            expected_fingerprint=fingerprint,
        )

    def load_mobile_manifest(self) -> dict:
        path = self.package / "stasis_mobile_package.json"
        return json.loads(path.read_text(encoding="utf-8"))

    def save_mobile_manifest(self, value: dict) -> None:
        write_json(self.package / "stasis_mobile_package.json", value)

    def test_accepts_official_prebuilt_runtime_from_exact_archive(self) -> None:
        result = self.verify()
        self.assertEqual("passed", result["result"])
        self.assertEqual("prebuilt", result["mode"])
        self.assertEqual("offline", result["variant"])
        self.assertEqual(RELEASE_ID, result["release_id"])
        self.assertEqual(SOURCE_COMMIT, result["source_commit"])
        self.assertEqual(FINGERPRINT, result["build_fingerprint"])
        self.assertEqual(12, result["runtime_file_count"])

    def test_workflow_runs_archive_identity_gate_after_provenance_verification(self) -> None:
        workflow = (Path(__file__).resolve().parents[2]
                    / ".github/workflows/staged-android-archive-acceptance.yml").read_text(
                        encoding="utf-8"
                    )
        step = re.search(
            r"(?ms)^      - name: Package and link the shipping Android arm64 consumers\n"
            r"(?P<body>.*?)(?=^      - (?:name|uses):|\Z)",
            workflow,
        )
        self.assertIsNotNone(step, "shipping package step")
        body = step.group("body")
        provenance = body.index("tools/verify_package_provenance.py")
        archive_gate = body.index("tools/ci/verify_staged_android_shipping_runtime.py")
        gradle_link = body.index("gradle --init-script")
        self.assertLess(provenance, archive_gate)
        self.assertLess(archive_gate, gradle_link)
        for argument in (
            "--archive-root target/stasis-nightly-linux-x64",
            '--package-root "$package"',
            '--release-id "$STASIS_RELEASE_ID"',
            '--source-commit "$STASIS_SOURCE_COMMIT"',
            '--build-fingerprint "$STASIS_BUILD_FINGERPRINT"',
        ):
            with self.subTest(argument=argument):
                self.assertIn(argument, body)
        self.assertIn('tee "$shipping/runtime-kit-identity.log"', body)

        bash = None
        if os.name == "nt":
            for candidate in (
                Path(r"C:\Program Files\Git\bin\bash.exe"),
                Path(r"C:\Program Files\Git\usr\bin\bash.exe"),
            ):
                if candidate.is_file():
                    bash = str(candidate)
                    break
        if bash is None:
            bash = shutil.which("bash")
        if bash is None:
            self.skipTest("bash is unavailable; hosted workflow runner performs shell parsing")
        step_lines = body.splitlines()
        run_index = next(
            (index for index, line in enumerate(step_lines) if line == "        run: |"),
            None,
        )
        self.assertIsNotNone(run_index, "shipping step Bash script")
        raw_script_lines = step_lines[run_index + 1 :]
        self.assertTrue(raw_script_lines, "shipping step Bash script is empty")
        self.assertTrue(
            all(not line or line.startswith("          ") for line in raw_script_lines),
            "shipping step script contains a line outside its YAML block",
        )
        script = "\n".join(
            line[10:] if line.startswith("          ") else line
            for line in raw_script_lines
        )
        result = subprocess.run(
            [bash, "-n"],
            input=script,
            text=True,
            capture_output=True,
            check=False,
            timeout=30,
        )
        self.assertEqual(0, result.returncode, result.stderr)

    def test_rejects_missing_packaged_manifest(self) -> None:
        (self.package_kit / "manifest.json").unlink()
        with self.assertRaisesRegex(verifier.VerificationError, "manifest is missing"):
            self.verify()

    def test_rejects_stale_release_identity(self) -> None:
        with self.assertRaisesRegex(verifier.VerificationError, "release_tag differs"):
            self.verify(release="nightly-20261002-379")

    def test_rejects_stale_source_identity(self) -> None:
        with self.assertRaisesRegex(verifier.VerificationError, "source_commit differs"):
            self.verify(source="88a5b3943f1759b7abbdbf0e37634965749c1337")

    def test_rejects_source_runtime_mode_for_official_arm64(self) -> None:
        mobile = self.load_mobile_manifest()
        mobile["android_runtime"]["mode"] = "source"
        self.save_mobile_manifest(mobile)
        with self.assertRaisesRegex(verifier.VerificationError, "did not select.*prebuilt"):
            self.verify()

    def test_rejects_wrong_archive_abi(self) -> None:
        self.runtime_manifest["abi"] = "x86_64"
        write_json(self.archive_kit / "manifest.json", self.runtime_manifest)
        with self.assertRaisesRegex(verifier.VerificationError, "abi differs"):
            self.verify()

    def test_rejects_wrong_archive_target(self) -> None:
        self.runtime_manifest["target"] = "android-x86_64"
        write_json(self.archive_kit / "manifest.json", self.runtime_manifest)
        with self.assertRaisesRegex(verifier.VerificationError, "target differs"):
            self.verify()

    def test_rejects_wrong_ndk_version(self) -> None:
        self.runtime_manifest["ndk_version"] = "27.1.12297006"
        write_json(self.archive_kit / "manifest.json", self.runtime_manifest)
        with self.assertRaisesRegex(verifier.VerificationError, "ndk_version differs"):
            self.verify()

    def test_rejects_fingerprint_not_from_accepted_archive(self) -> None:
        with self.assertRaisesRegex(verifier.VerificationError, "build_fingerprint differs"):
            self.verify(fingerprint="b" * 64)

    def test_rejects_tampered_runtime_bytes(self) -> None:
        (self.package_kit / VARIANTS["offline"]).write_bytes(b"unrelated runtime")
        with self.assertRaisesRegex(verifier.VerificationError, "missing, stale, tampered"):
            self.verify()

    def test_rejects_unrelated_runtime_file_in_package(self) -> None:
        (self.package_kit / "lib/unrelated-runtime.a").write_bytes(b"other runtime")
        with self.assertRaisesRegex(verifier.VerificationError, "missing, stale, tampered"):
            self.verify()

    def test_rejects_unrelated_runtime_file_in_archive(self) -> None:
        (self.archive_kit / "lib/unrelated-runtime.a").write_bytes(b"other runtime")
        with self.assertRaisesRegex(verifier.VerificationError, "differ from its declared file hashes"):
            self.verify()

    def test_rejects_runtime_artifact_hash_not_recorded_in_release_provenance(self) -> None:
        path = "mobile/android-runtime/arm64-v8a/manifest.json"
        self.release_provenance["android_runtime_artifacts"][path] = "b" * 64
        write_json(self.archive / "stasis_release_provenance.json", self.release_provenance)
        write_json(self.package / "stasis_provenance.json", self.release_provenance)
        with self.assertRaisesRegex(verifier.VerificationError, "differ from release provenance hashes"):
            self.verify()

    def test_rejects_unsupported_runtime_variant(self) -> None:
        mobile = self.load_mobile_manifest()
        mobile["android_runtime"]["variant"] = "dual"
        self.save_mobile_manifest(mobile)
        with self.assertRaisesRegex(verifier.VerificationError, "unsupported runtime variant"):
            self.verify()

    def test_rejects_variant_identity_mismatch(self) -> None:
        mobile = self.load_mobile_manifest()
        mobile["android_runtime"]["variant"] = "host"
        self.save_mobile_manifest(mobile)
        with self.assertRaisesRegex(verifier.VerificationError, "identity differs"):
            self.verify()

    def test_rejects_stale_package_runtime_identity(self) -> None:
        mobile = self.load_mobile_manifest()
        mobile["android_runtime"]["identity"]["source_hashes"]["sdl3"] = "4" * 64
        self.save_mobile_manifest(mobile)
        with self.assertRaisesRegex(verifier.VerificationError, "identity differs"):
            self.verify()


if __name__ == "__main__":
    unittest.main()
