import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
RUNNER = ROOT / ".cargo" / "stasis-sign-and-run.cmd"


@unittest.skipUnless(sys.platform == "win32", "Windows command runner test")
class WindowsSignRunnerTests(unittest.TestCase):
    def test_batch_signer_returns_to_runner_and_target_executes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            temp = Path(directory)
            signer = temp / "sign.cmd"
            target = temp / "target.cmd"
            signed_marker = temp / "signed.txt"
            ran_marker = temp / "ran.txt"

            signer.write_text(
                '@echo off\r\n> "%~dp0signed.txt" echo signed:%~1\r\nexit /b 0\r\n',
                encoding="ascii",
            )
            target.write_text(
                '@echo off\r\n> "%~dp0ran.txt" echo %*\r\nexit /b 0\r\n',
                encoding="ascii",
            )

            environment = os.environ.copy()
            environment["STASIS_AOT_SIGN_TOOL"] = str(signer)
            environment["STASIS_REQUIRE_SIGNED_EXECUTION"] = "1"
            command = f'call "{RUNNER}" "{target}" alpha "two words"'
            result = subprocess.run(
                command,
                cwd=ROOT,
                env=environment,
                capture_output=True,
                text=True,
                timeout=30,
                check=False,
                shell=True,
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertTrue(signed_marker.exists(), result.stdout)
            self.assertTrue(ran_marker.exists(), result.stdout)
            forwarded = ran_marker.read_text(encoding="ascii")
            self.assertIn("alpha", forwarded)
            self.assertIn("two words", forwarded)


if __name__ == "__main__":
    unittest.main()
