from __future__ import annotations

import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from tools.ci import verify_android_test_signer as verifier


EXPECTED = "f115a250a33dc3e49b3b7f939075f6db118ce95e139e8c2703c437b1b3f37cc0"


class VerifyAndroidTestSignerTests(unittest.TestCase):
    def test_writes_public_signer_and_apk_hash_evidence(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            apk = root / "app-debug.apk"
            apk.write_bytes(b"test APK bytes")
            output = root / "evidence" / "signer.json"
            with patch.object(verifier, "verify_apk_signer", return_value=EXPECTED) as verify_signer:
                result = verifier.verify(apk, "apksigner", EXPECTED.upper(), output)
            verify_signer.assert_called_once_with(apk, "apksigner")
            saved = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(saved, result)
            self.assertEqual(saved["signer_sha256"], EXPECTED)
            self.assertEqual(saved["result"], "passed")

    def test_rejects_another_certificate_and_malformed_expected_digest(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            apk = root / "app-debug.apk"
            apk.write_bytes(b"test APK bytes")
            with patch.object(verifier, "verify_apk_signer", return_value="0" * 64):
                with self.assertRaisesRegex(ValueError, "differs"):
                    verifier.verify(apk, "apksigner", EXPECTED, root / "bad.json")
            with self.assertRaisesRegex(ValueError, "64 hexadecimal"):
                verifier.verify(apk, "apksigner", "not-a-digest", root / "bad.json")


if __name__ == "__main__":
    unittest.main()
