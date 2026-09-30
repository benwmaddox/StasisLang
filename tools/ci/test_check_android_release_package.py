import hashlib
import json
import struct
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest.mock import patch

from tools.ci.check_android_release_package import (
    NETWORK_LIBRARY,
    REQUIRED_NATIVE_LIBRARIES,
    validate,
)


ROOT = Path(__file__).resolve().parents[2]


class AndroidReleasePackageTest(unittest.TestCase):
    def write_package(
        self,
        path: Path,
        abi: str,
        asset: str,
        asset_bytes: bytes = b"asset",
        expected_asset_bytes: bytes | None = None,
        network: bool = False,
        align_network: bool = True,
    ) -> None:
        prefix = "base/" if path.suffix == ".aab" else ""
        manifest = f"{prefix}manifest/AndroidManifest.xml" if prefix else "AndroidManifest.xml"
        expected_asset_bytes = expected_asset_bytes or asset_bytes
        asset_manifest = {
            "schema": "stasis-assets",
            "version": 1,
            "assets": [
                {
                    "id": "ball",
                    "path": asset,
                    "content_sha256": hashlib.sha256(expected_asset_bytes).hexdigest(),
                    "format": {"kind": "sprite", "encoding": "svg", "width": 1, "height": 1},
                    "dependencies": [],
                }
            ],
        }
        with zipfile.ZipFile(path, "w") as archive:
            archive.writestr(manifest, b"fixture")
            archive.writestr(
                f"{prefix}assets/stasis_game/assets/manifest.json",
                json.dumps(asset_manifest).encode(),
            )
            archive.writestr(f"{prefix}assets/stasis_game/{asset}", asset_bytes)
            for library in REQUIRED_NATIVE_LIBRARIES:
                archive.writestr(f"{prefix}lib/{abi}/{library}", b"native")
            if network:
                entry = f"{prefix}lib/{abi}/{NETWORK_LIBRARY}"
                info = zipfile.ZipInfo(entry)
                if align_network and not prefix:
                    data_without_extra = archive.fp.tell() + 30 + len(entry.encode())
                    padding = (-(data_without_extra + 4)) % (16 * 1024)
                    info.extra = struct.pack("<HH", 0xCAFE, padding) + b"\0" * padding
                archive.writestr(info, b"network")

    @staticmethod
    def readelf_output(_readelf: str, _library: Path, option: str) -> str:
        return {
            "-h": "  Machine:                           AArch64\n",
            "-d": f" 0x000000000000000e (SONAME) Library soname: [{NETWORK_LIBRARY}]\n",
            "-lW": (
                "  LOAD 0x000000 0x0000000000000000 0x0000000000000000 "
                "0x001000 0x001000 R E 0x4000\n"
                "  LOAD 0x004000 0x0000000000004000 0x0000000000004000 "
                "0x001000 0x001000 RW  0x4000\n"
            ),
        }[option]

    def test_accepts_release_apk(self):
        with tempfile.TemporaryDirectory() as directory:
            apk = Path(directory) / "game.apk"
            self.write_package(apk, "arm64-v8a", "assets/ball.svg")
            self.assertEqual(validate(apk)["format"], "apk")

    def test_accepts_release_bundle(self):
        with tempfile.TemporaryDirectory() as directory:
            bundle = Path(directory) / "game.aab"
            self.write_package(bundle, "arm64-v8a", "assets/ball.svg")
            self.assertEqual(validate(bundle)["format"], "aab")

    def test_rejects_workshop_native_library(self):
        with tempfile.TemporaryDirectory() as directory:
            apk = Path(directory) / "game.apk"
            self.write_package(apk, "arm64-v8a", "assets/ball.svg")
            with zipfile.ZipFile(apk, "a") as archive:
                archive.writestr(
                    "lib/arm64-v8a/libstasis_android_bridge.so", b"compiler"
                )
            with self.assertRaisesRegex(ValueError, "development files"):
                validate(apk)

    def test_rejects_separate_sdl_runtime(self):
        with tempfile.TemporaryDirectory() as directory:
            apk = Path(directory) / "game.apk"
            self.write_package(apk, "arm64-v8a", "assets/ball.svg")
            with zipfile.ZipFile(apk, "a") as archive:
                archive.writestr("lib/arm64-v8a/libSDL3.so", b"runtime")
            with self.assertRaisesRegex(ValueError, "unexpected native libraries"):
                validate(apk)

    def test_network_library_is_required_only_when_enabled(self):
        with tempfile.TemporaryDirectory() as directory:
            missing = Path(directory) / "missing.apk"
            self.write_package(missing, "arm64-v8a", "assets/ball.svg")
            with self.assertRaisesRegex(ValueError, NETWORK_LIBRARY):
                validate(missing, network_enabled=True, readelf="llvm-readelf")

            offline = Path(directory) / "offline.apk"
            self.write_package(
                offline, "arm64-v8a", "assets/ball.svg", network=True
            )
            with self.assertRaisesRegex(ValueError, "unexpected native libraries"):
                validate(offline)

    @patch(
        "tools.ci.check_android_release_package._run_readelf",
        side_effect=readelf_output,
    )
    def test_accepts_versioned_aligned_network_library(self, _readelf):
        with tempfile.TemporaryDirectory() as directory:
            apk = Path(directory) / "network.apk"
            self.write_package(apk, "arm64-v8a", "assets/ball.svg", network=True)
            summary = validate(
                apk, network_enabled=True, readelf="llvm-readelf"
            )
            self.assertEqual(summary["network_library"]["soname"], NETWORK_LIBRARY)
            self.assertEqual(summary["network_library"]["pt_load_count"], 2)

    @patch(
        "tools.ci.check_android_release_package._run_readelf",
        side_effect=readelf_output,
    )
    def test_rejects_misaligned_network_library_in_apk(self, _readelf):
        with tempfile.TemporaryDirectory() as directory:
            apk = Path(directory) / "network.apk"
            self.write_package(
                apk,
                "arm64-v8a",
                "assets/ball.svg",
                network=True,
                align_network=False,
            )
            with self.assertRaisesRegex(ValueError, "not 16 KiB aligned in APK"):
                validate(apk, network_enabled=True, readelf="llvm-readelf")

    def test_rejects_wrong_soname_and_pt_load_alignment(self):
        with tempfile.TemporaryDirectory() as directory:
            apk = Path(directory) / "network.apk"
            self.write_package(apk, "arm64-v8a", "assets/ball.svg", network=True)

            def wrong_soname(_readelf, _library, option):
                output = self.readelf_output(_readelf, _library, option)
                return output.replace(NETWORK_LIBRARY, "libstasis_network.so")

            with patch(
                "tools.ci.check_android_release_package._run_readelf",
                side_effect=wrong_soname,
            ), self.assertRaisesRegex(ValueError, "SONAME must be"):
                validate(apk, network_enabled=True, readelf="llvm-readelf")

            def bad_load(_readelf, _library, option):
                output = self.readelf_output(_readelf, _library, option)
                return output.replace("0x4000", "0x1000") if option == "-lW" else output

            with patch(
                "tools.ci.check_android_release_package._run_readelf",
                side_effect=bad_load,
            ), self.assertRaisesRegex(ValueError, "PT_LOAD segments"):
                validate(apk, network_enabled=True, readelf="llvm-readelf")

    def test_rejects_asset_hash_mismatch(self):
        with tempfile.TemporaryDirectory() as directory:
            apk = Path(directory) / "game.apk"
            self.write_package(
                apk,
                "arm64-v8a",
                "assets/ball.svg",
                asset_bytes=b"tampered",
                expected_asset_bytes=b"asset",
            )
            with self.assertRaisesRegex(ValueError, "asset hash mismatch"):
                validate(apk)


class AndroidReleaseWrapperTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.script = (ROOT / "mobile/android/build_release.ps1").read_text(
            encoding="utf-8"
        )

    def test_network_audit_discovers_readelf_from_the_selected_ndk(self):
        self.assertIn('$appGradle = Join-Path $androidRoot "app/build.gradle"', self.script)
        self.assertIn("'ndkVersion\\s+", self.script)
        self.assertIn("$env:ANDROID_NDK_ROOT", self.script)
        self.assertIn("$env:ANDROID_NDK_HOME", self.script)
        self.assertIn("$env:ANDROID_SDK_ROOT", self.script)
        self.assertIn("$env:ANDROID_HOME", self.script)
        self.assertIn(
            '"toolchains/llvm/prebuilt/$hostTag/bin/$executableName"', self.script
        )
        self.assertIn('$auditArguments += @("--readelf", $resolvedReadelf)', self.script)

    def test_network_audit_fails_closed_without_readelf(self):
        network_gate = self.script.index(
            'Test-Path -LiteralPath $networkLibrary -PathType Leaf'
        )
        missing_error = self.script.index(
            "Network-enabled Android validation requires llvm-readelf", network_gate
        )
        audit = self.script.index(
            'python (Join-Path $repoRoot "tools/ci/check_android_release_package.py")',
            missing_error,
        )
        self.assertLess(network_gate, missing_error)
        self.assertLess(missing_error, audit)


if __name__ == "__main__":
    unittest.main()
