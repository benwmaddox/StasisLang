import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
RUNNER = ROOT / ".cargo" / "stasis-sign-and-run.cmd"


def isolated_signing_environment() -> dict[str, str]:
    return {
        name: value for name, value in os.environ.items()
        if not name.startswith("STASIS_SIGNING_")
        and name not in {"STASIS_AOT_SIGN_TOOL", "STASIS_REQUIRE_SIGNED_EXECUTION"}
    }


@unittest.skipUnless(sys.platform == "win32", "Windows command runner test")
class WindowsSignRunnerTests(unittest.TestCase):
    def test_production_ignores_local_development_record(self) -> None:
        with tempfile.TemporaryDirectory(prefix="stasis production spaces ") as directory:
            temp = Path(directory)
            record = temp / "development-thumbprint.txt"
            record.write_text("ABCDEF123456\n", encoding="ascii")
            target = temp / "target.cmd"
            target.write_text("@echo off\nexit /b 0\n", encoding="ascii")
            for variable in ("STASIS_SIGNING_MODE", "STASIS_SIGNING_PROFILE"):
                with self.subTest(variable=variable):
                    environment = isolated_signing_environment()
                    environment[variable] = "production"
                    environment["STASIS_SIGNING_LOCAL_RECORD"] = str(record)
                    result = subprocess.run(
                        f'call "{RUNNER}" "{target}"', cwd=ROOT, env=environment,
                        capture_output=True, text=True, timeout=30, shell=True,
                    )
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertNotIn("repository signing", result.stderr.lower())

    def test_batch_signer_returns_to_runner_and_target_executes(self) -> None:
        with tempfile.TemporaryDirectory(prefix="stasis signing spaces ") as directory:
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

            environment = isolated_signing_environment()
            environment["STASIS_AOT_SIGN_TOOL"] = str(signer)
            environment["STASIS_REQUIRE_SIGNED_EXECUTION"] = "1"
            # Isolate the launch contract from Authenticode with a repository-policy double.
            runner = temp / ".cargo" / RUNNER.name
            runner.parent.mkdir()
            runner.write_bytes(RUNNER.read_bytes())
            policy = temp / "tools" / "windows" / "stasis-signing.ps1"
            policy.parent.mkdir(parents=True)
            policy.write_text(
                "param($Command, $Artifact)\n& $env:STASIS_AOT_SIGN_TOOL $Artifact\nexit $LASTEXITCODE\n",
                encoding="ascii",
            )
            command = f'call "{runner}" "{target}" alpha "two words"'
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

    def test_required_mode_never_executes_after_missing_or_failed_hook(self) -> None:
        with tempfile.TemporaryDirectory(prefix="stasis required spaces ") as directory:
            temp = Path(directory)
            target = temp / "target.exe"
            marker = temp / "ran.txt"
            shutil.copyfile(sys.executable, target)
            failing = temp / "failed signer.cmd"
            failing.write_text('@echo off\nexit /b 19\n', encoding="ascii")
            unsigned = temp / "unsigned signer.cmd"
            unsigned.write_text('@echo off\nexit /b 0\n', encoding="ascii")
            for signer in (temp / "missing signer.cmd", failing, unsigned):
                for variable in ("STASIS_SIGNING_MODE", "STASIS_REQUIRE_SIGNED_EXECUTION"):
                    with self.subTest(signer=signer.name, required=variable):
                        environment = isolated_signing_environment()
                        environment.pop("STASIS_REQUIRE_SIGNED_EXECUTION", None)
                        environment.pop("STASIS_SIGNING_MODE", None)
                        environment[variable] = "required" if variable == "STASIS_SIGNING_MODE" else "1"
                        environment["STASIS_AOT_SIGN_TOOL"] = str(signer)
                        # A successful hook must still prove the selected identity.
                        environment["STASIS_SIGNING_CERT_THUMBPRINT"] = "0" * 40
                        environment.pop("STASIS_SIGNING_CERTIFICATE", None)
                        environment["STASIS_TEST_MARKER"] = str(marker)
                        result = subprocess.run(
                            f'call "{RUNNER}" "{target}" -c "import os; open(os.environ[\'STASIS_TEST_MARKER\'], \'w\').close()"', cwd=ROOT, env=environment,
                            capture_output=True, text=True, timeout=30, shell=True,
                        )
                        self.assertNotEqual(result.returncode, 0)
                        self.assertFalse(marker.exists(), result.stdout + result.stderr)
                        self.assertIn("target was not executed", result.stderr)
                        self.assertNotIn("unsupported Authenticode artifact type", result.stderr)

    def test_optional_persisted_policy_configuration_is_attempted_but_nonblocking(self) -> None:
        with tempfile.TemporaryDirectory(prefix="stasis signing spaces ") as directory:
            temp = Path(directory)
            target = temp / "target.cmd"
            ran_marker = temp / "ran.txt"
            target.write_text(
                '@echo off\r\n> "%~dp0ran.txt" echo %*\r\nexit /b 0\r\n',
                encoding="ascii",
            )
            local_app_data = temp / "localappdata"
            record = local_app_data / "Stasis" / "signing" / "development-thumbprint.txt"
            record.parent.mkdir(parents=True)
            record.write_text("ABCDEF123456\n", encoding="ascii")
            environment = isolated_signing_environment()
            environment.pop("STASIS_AOT_SIGN_TOOL", None)
            environment.pop("STASIS_REQUIRE_SIGNED_EXECUTION", None)
            environment["STASIS_SIGNING_MODE"] = "optional"
            environment["LOCALAPPDATA"] = str(local_app_data)
            command = f'call "{RUNNER}" "{target}"'
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
            self.assertTrue(ran_marker.exists())
            self.assertIn("ignoring optional repository signing failure", result.stderr.lower())
            self.assertNotIn("required repository signing", result.stderr.lower())

    def test_unconfigured_runner_is_unsigned_noop(self) -> None:
        with tempfile.TemporaryDirectory(prefix="stasis signing spaces ") as directory:
            temp = Path(directory)
            target = temp / "target.cmd"
            ran_marker = temp / "ran.txt"
            target.write_text(
                '@echo off\r\n> "%~dp0ran.txt" echo ran\r\nexit /b 0\r\n',
                encoding="ascii",
            )
            environment = isolated_signing_environment()
            environment.pop("STASIS_AOT_SIGN_TOOL", None)
            environment.pop("STASIS_REQUIRE_SIGNED_EXECUTION", None)
            environment.pop("STASIS_SIGNING_MODE", None)
            environment["LOCALAPPDATA"] = str(temp / "no-local-record")
            result = subprocess.run(
                f'call "{RUNNER}" "{target}"',
                cwd=ROOT,
                env=environment,
                capture_output=True,
                text=True,
                timeout=30,
                check=False,
                shell=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertTrue(ran_marker.exists())
            self.assertNotIn("repository signing", result.stderr.lower())


if __name__ == "__main__":
    unittest.main()
