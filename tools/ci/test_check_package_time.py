"""Contract checks for the packaging performance gate."""

from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("check_package_time.py")


class CheckPackageTimeTests(unittest.TestCase):
    def run_gate(self, directory: Path, seconds: float, limit: float):
        return subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--label", "sample web",
                "--max-seconds", str(limit),
                "--expect-output", "package/index.html",
                "--expect-log", "using prebuilt runtime",
                "--",
                sys.executable,
                "-c",
                (
                    "from pathlib import Path; import time; "
                    f"time.sleep({seconds}); "
                    "Path('package').mkdir(); "
                    "Path('package/index.html').write_text('ok'); "
                    "print('using prebuilt runtime')"
                ),
            ],
            cwd=directory,
            capture_output=True,
            text=True,
            check=False,
        )

    def test_success_requires_output_and_log(self):
        with tempfile.TemporaryDirectory() as temp:
            result = self.run_gate(Path(temp), 0, 1)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("PASS", result.stdout)

    def test_slow_package_fails(self):
        with tempfile.TemporaryDirectory() as temp:
            result = self.run_gate(Path(temp), 0.05, 0.01)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("exceeded", result.stderr)

    def test_successful_command_without_package_output_fails(self):
        with tempfile.TemporaryDirectory() as temp:
            result = subprocess.run(
                [
                    sys.executable, str(SCRIPT),
                    "--label", "missing output",
                    "--expect-output", "package/index.html",
                    "--", sys.executable, "-c", "print('done')",
                ],
                cwd=temp,
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("expected package output missing", result.stderr)


if __name__ == "__main__":
    unittest.main()
