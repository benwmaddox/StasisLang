import hashlib
import json
import pathlib
import subprocess
import sys
import tempfile
import unittest

from tools.generate_release_provenance import RUNTIME_FILES


ROOT = pathlib.Path(__file__).resolve().parents[2]
VERIFY = ROOT / "tools" / "verify_package_provenance.py"


class ReleaseProvenanceTests(unittest.TestCase):
    def test_wav_mixer_is_part_of_release_provenance(self):
        self.assertIn("stasis_audio_assets.c", RUNTIME_FILES)
        self.assertIn("stasis_audio_assets.h", RUNTIME_FILES)

    def test_mobile_preference_host_is_part_of_release_provenance(self):
        self.assertIn("stasis_platform_storage.c", RUNTIME_FILES)
        self.assertIn("stasis_platform_storage.h", RUNTIME_FILES)

    def test_windows_dpi_manifest_is_part_of_release_provenance(self):
        self.assertIn("stasis_runner.manifest", RUNTIME_FILES)

    def test_macos_retina_plist_is_part_of_release_provenance(self):
        self.assertIn("stasis_runner_macos.plist.in", RUNTIME_FILES)

    def test_package_verifier_detects_runtime_substitution(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            release = root / "release"
            package = root / "package"
            (release / "runtime").mkdir(parents=True)
            (package / "runtime").mkdir(parents=True)
            (release / "mobile/shells/common").mkdir(parents=True)
            (release / "mobile/shells/android").mkdir(parents=True)
            (package / "common").mkdir(parents=True)
            (package / "android").mkdir(parents=True)
            runtime = b"official renderer\n"
            expected = hashlib.sha256(runtime).hexdigest()
            common_shell = b"common\n"
            android_shell = (
                b"@STASIS_APP_NAME@ @STASIS_PACKAGE_ID@ "
                b"@STASIS_JNI_PACKAGE@ @STASIS_ANDROID_ORIENTATION@ "
                b"@STASIS_ANDROID_VERSION_CODE@ @STASIS_ANDROID_VERSION_NAME@\n"
            )
            manifest = {
                "schema": "stasis.release_provenance.v1",
                "release_tag": "v1.0.0",
                "source_commit": "0123456789012345678901234567890123456789",
                "development_build": False,
                "runtime_sources": {"runtime/stasis_graphics.c": expected},
                "mobile_shell_sources": {
                    "mobile/shells/common/main.c": hashlib.sha256(common_shell).hexdigest(),
                    "mobile/shells/android/main.c": hashlib.sha256(android_shell).hexdigest(),
                },
            }
            (release / "stasis_release_provenance.json").write_text(
                json.dumps(manifest), encoding="utf-8"
            )
            (package / "stasis_provenance.json").write_text(
                json.dumps(manifest), encoding="utf-8"
            )
            (package / "runtime/stasis_graphics.c").write_bytes(runtime)
            (release / "mobile/shells/common/main.c").write_bytes(common_shell)
            (release / "mobile/shells/android/main.c").write_bytes(android_shell)
            (package / "common/main.c").write_bytes(common_shell)
            (package / "android/main.c").write_bytes(
                b"Demo App com.example.demo com_example_demo sensorPortrait 7 2.1.0\n"
            )
            (package / "stasis_mobile_package.json").write_text(
                json.dumps(
                    {
                        "target": "android-arm64",
                        "name": "demo",
                        "app_name": "Demo App",
                        "package_id": "com.example.demo",
                        "android_orientation": "sensorPortrait",
                        "android_version_code": "7",
                        "android_version_name": "2.1.0",
                    }
                ),
                encoding="utf-8",
            )
            (package / "common/stasis_package_provenance.h").write_bytes(
                ("#ifndef STASIS_PACKAGE_PROVENANCE_H\n"
                "#define STASIS_PACKAGE_PROVENANCE_H\n"
                '#define STASIS_PACKAGE_RELEASE_TAG "v1.0.0"\n'
                '#define STASIS_PACKAGE_SOURCE_COMMIT "0123456789012345678901234567890123456789"\n'
                '#define STASIS_PACKAGE_BUILD_LABEL "official release"\n'
                "#endif\n").encode("utf-8")
            )

            command = [
                sys.executable,
                str(VERIFY),
                "--release-root",
                str(release),
                "--package-root",
                str(package),
                "--expect-runtime-sources",
            ]
            self.assertEqual(subprocess.run(command, check=False).returncode, 0)
            (release / "mobile/shells/android/main.c").write_bytes(b"substituted shell\n")
            shell_failed = subprocess.run(command, check=False, capture_output=True, text=True)
            self.assertNotEqual(shell_failed.returncode, 0)
            self.assertIn("shell tree does not match", shell_failed.stderr)
            (release / "mobile/shells/android/main.c").write_bytes(android_shell)
            (package / "runtime/stasis_graphics.c").write_bytes(b"local worktree\n")
            failed = subprocess.run(command, check=False, capture_output=True, text=True)
            self.assertNotEqual(failed.returncode, 0)
            self.assertIn("packaged runtime hash mismatch", failed.stderr)
            (package / "runtime/stasis_graphics.c").write_bytes(runtime)
            (package / "android/untracked.java").write_text("class Untracked {}\n", encoding="utf-8")
            extra_failed = subprocess.run(command, check=False, capture_output=True, text=True)
            self.assertNotEqual(extra_failed.returncode, 0)
            self.assertIn("source tree differs", extra_failed.stderr)
            (package / "android/untracked.java").unlink()
            (package / "stasis_mobile_package.json").write_text(
                json.dumps({"target": "windows-arm64", "name": "demo"}), encoding="utf-8"
            )
            target_failed = subprocess.run(command, check=False, capture_output=True, text=True)
            self.assertNotEqual(target_failed.returncode, 0)
            self.assertIn("unsupported mobile package target", target_failed.stderr)


if __name__ == "__main__":
    unittest.main()
