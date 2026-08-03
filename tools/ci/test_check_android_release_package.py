import hashlib
import json
import tempfile
import unittest
import zipfile
from pathlib import Path

from tools.ci.check_android_release_package import REQUIRED_NATIVE_LIBRARIES, validate


class AndroidReleasePackageTest(unittest.TestCase):
    def write_package(
        self,
        path: Path,
        abi: str,
        asset: str,
        asset_bytes: bytes = b"asset",
        expected_asset_bytes: bytes | None = None,
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


if __name__ == "__main__":
    unittest.main()
