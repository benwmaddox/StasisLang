import tempfile
import unittest
import zipfile
from pathlib import Path

from tools.ci.check_android_published_apk import BASE_REQUIRED_ENTRIES, validate


class AndroidPublishedApkTest(unittest.TestCase):
    def write_apk(self, path: Path, abi: str, asset: str) -> None:
        with zipfile.ZipFile(path, "w") as archive:
            for entry in BASE_REQUIRED_ENTRIES:
                archive.writestr(entry, b"fixture")
            archive.writestr(f"lib/{abi}/libstasis_mobile_smoke.so", b"native")
            archive.writestr(f"assets/stasis_game/{asset}", b"asset")

    def test_accepts_emulator_aot_abi_and_fixture_asset(self):
        with tempfile.TemporaryDirectory() as directory:
            apk = Path(directory) / "render-parity.apk"
            self.write_apk(apk, "x86_64", "assets/opaque.svg")
            summary = validate(apk, "x86_64", "assets/opaque.svg")
            self.assertEqual(summary["abi"], "x86_64")

    def test_rejects_native_library_from_another_abi(self):
        with tempfile.TemporaryDirectory() as directory:
            apk = Path(directory) / "wrong-abi.apk"
            self.write_apk(apk, "arm64-v8a", "assets/ball.svg")
            with self.assertRaisesRegex(ValueError, "missing required entries"):
                validate(apk, "x86_64", "assets/ball.svg")


if __name__ == "__main__":
    unittest.main()
