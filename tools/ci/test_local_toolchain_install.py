"""Deterministic source-contract checks for the Windows local installer."""

from pathlib import Path
import unittest
from tools.compute_toolchain_fingerprint import fingerprint


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = (ROOT / "scripts" / "install_local_toolchain.ps1").read_text(encoding="ascii")


class LocalToolchainInstallTests(unittest.TestCase):
    def test_fingerprint_helper_is_deterministic(self):
        value = fingerprint("abc123", "local-test")
        self.assertEqual(value, fingerprint("abc123", "local-test"))
        self.assertEqual(len(value), 64)
        self.assertNotEqual(value, fingerprint("abc124", "local-test"))

    def test_builds_both_halves_from_one_fingerprint(self):
        self.assertIn('STASIS_BUILD_FINGERPRINT', SCRIPT)
        self.assertIn('-DSTASIS_BUILD_FINGERPRINT=$fingerprint', SCRIPT)
        self.assertIn('cargo_cache.py", "run", "--", "cargo", "build"', SCRIPT)
        self.assertIn('"--path-format=absolute", "--git-common-dir"', SCRIPT)
        self.assertIn('Join-Path $cargoTarget "release/stasis.exe"', SCRIPT)
        self.assertIn('editor-info', SCRIPT)
        self.assertIn('$editorInfo.result.build_fingerprint', SCRIPT)
        self.assertIn('windows_launch_smoke', SCRIPT)

    def test_stages_complete_dynamic_toolchain(self):
        for required in (
            'stasis_dynload.dll',
            'stasis_dynload.dll.lib',
            'stasis_graphics.dll',
            'stasis_runner.exe',
            'Get-ChildItem -LiteralPath (Split-Path -Parent $runtime) -Filter "*.dll"',
            'Join-Path $staging "runtime"',
            'Join-Path $staging "mobile"',
            'Join-Path $staging "tools/windows"',
            '$signingArtifacts',
            'configured local signer failed',
        ):
            self.assertIn(required, SCRIPT)

    def test_promotion_is_staged_and_rolls_back(self):
        self.assertIn('Move-Item -LiteralPath $Destination -Destination $backup', SCRIPT)
        self.assertIn('Move-Item -LiteralPath $Staging -Destination $Destination', SCRIPT)
        self.assertIn('previous bin was restored', SCRIPT)
        self.assertIn('TestInjectPromotionFailure', SCRIPT)
        self.assertIn('STASIS_TEST_MODE -ne "1"', SCRIPT)
        self.assertIn('required build output is missing', SCRIPT)

    def test_no_development_fingerprint_fallback(self):
        self.assertNotIn('STASIS_BUILD_FINGERPRINT = "development"', SCRIPT)
        self.assertIn('clean source revision', SCRIPT)


if __name__ == "__main__":
    unittest.main()
