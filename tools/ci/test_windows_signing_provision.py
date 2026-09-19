import os
import pathlib
import re
import shutil
import subprocess
import tempfile
import textwrap
import time
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]
HELPER = ROOT / "tools/windows/invoke-bounded-powershell.ps1"
PROVISIONER = ROOT / "tools/windows/provision-ci-signing-pfx.ps1"
WORKFLOW = ROOT / ".github/workflows/pr-ci.yml"
POWERSHELL = shutil.which("pwsh") or shutil.which("powershell.exe")


def workflow_step(source: str, name: str) -> str:
    match = re.search(
        rf"(?ms)^      - name: {re.escape(name)}\n(.*?)(?=^      - name: |\Z)",
        source,
    )
    if match is None:
        raise AssertionError(f"missing workflow step: {name}")
    return match.group(1)


@unittest.skipUnless(
    os.name == "nt" and POWERSHELL,
    "PowerShell bounded signing provision tests",
)
class WindowsSigningProvisionProcessTests(unittest.TestCase):
    def run_fixture(
        self,
        root: pathlib.Path,
        fixture: pathlib.Path,
        mode: str,
        timeout_seconds: int = 10,
    ) -> subprocess.CompletedProcess[str]:
        environment = os.environ.copy()
        environment["STASIS_SIGNING_HELPER_TEST_MODE"] = mode
        return subprocess.run(
            [
                POWERSHELL,
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                str(HELPER),
                "-ScriptPath",
                str(fixture),
                "-Command",
                "provision",
                "-TimeoutSeconds",
                str(timeout_seconds),
            ],
            cwd=ROOT,
            env=environment,
            capture_output=True,
            text=True,
            timeout=20,
            check=False,
        )

    @staticmethod
    def write_fixture(root: pathlib.Path, mode: str, marker: pathlib.Path | None = None) -> pathlib.Path:
        fixture = root / "fixture.ps1"
        if mode == "tree-timeout":
            if marker is None:
                raise AssertionError("tree timeout fixture requires a marker path")
            marker_literal = str(marker).replace("'", "''")
            body = f"""
                $childCommand = "Start-Sleep -Seconds 3; Set-Content -LiteralPath '{marker_literal}' -Value child-alive"
                Start-Process -FilePath "$env:SystemRoot\\System32\\WindowsPowerShell\\v1.0\\powershell.exe" -ArgumentList @(
                    '-NoProfile', '-NonInteractive', '-Command', $childCommand
                ) | Out-Null
                Start-Sleep -Seconds 30
                exit 0
            """
        elif mode == "success":
            body = "Write-Output 'fixture stdout'; [Console]::Error.WriteLine('fixture stderr'); exit 0"
        elif mode == "failure":
            body = "Write-Output 'fixture stdout'; [Console]::Error.WriteLine('fixture stderr'); exit 7"
        elif mode == "timeout":
            body = "Start-Sleep -Seconds 30; exit 0"
        else:
            raise AssertionError(f"unknown fixture mode: {mode}")
        fixture.write_text(textwrap.dedent(body), encoding="utf-8")
        return fixture

    def test_captures_child_stdout_and_stderr(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            fixture = self.write_fixture(root, "success")
            result = self.run_fixture(root, fixture, "success")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("fixture stdout", result.stdout)
        self.assertIn("fixture stderr", result.stderr)

    def test_stdout_remains_capturable_when_helper_is_called_from_powershell(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            fixture = self.write_fixture(root, "success")
            captured = root / "captured.txt"
            wrapper = root / "wrapper.ps1"
            helper_literal = str(HELPER).replace("'", "''")
            fixture_literal = str(fixture).replace("'", "''")
            captured_literal = str(captured).replace("'", "''")
            wrapper.write_text(
                textwrap.dedent(
                    f"""
                    $output = @(& '{helper_literal}' -ScriptPath '{fixture_literal}' -Command provision -TimeoutSeconds 10)
                    if ($LASTEXITCODE -ne 0) {{ exit $LASTEXITCODE }}
                    Set-Content -LiteralPath '{captured_literal}' -Value ($output -join "`n")
                    """
                ),
                encoding="utf-8",
            )
            environment = os.environ.copy()
            environment["STASIS_SIGNING_HELPER_TEST_MODE"] = "success"
            result = subprocess.run(
                [
                    POWERSHELL,
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-File",
                    str(wrapper),
                ],
                cwd=ROOT,
                env=environment,
                capture_output=True,
                text=True,
                timeout=20,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("fixture stdout", captured.read_text(encoding="utf-8"))

    def test_preserves_nonzero_child_result_and_diagnostics(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            fixture = self.write_fixture(root, "failure")
            result = self.run_fixture(root, fixture, "failure")
        self.assertEqual(result.returncode, 7, result.stderr)
        self.assertIn("fixture stdout", result.stdout)
        self.assertIn("fixture stderr", result.stderr)
        self.assertIn("failed with exit code 7", result.stderr)

    def test_timeout_terminates_the_child_process_tree(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            marker = root / "child-marker.txt"
            fixture = self.write_fixture(root, "tree-timeout", marker)
            started = time.monotonic()
            result = self.run_fixture(root, fixture, "tree-timeout", timeout_seconds=1)
            elapsed = time.monotonic() - started
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("timed out after 1 seconds", result.stderr)
            self.assertLess(elapsed, 10, result.stderr)
            time.sleep(4)
            self.assertFalse(marker.exists(), "a descendant survived timeout cleanup")


class WindowsSigningProvisionPolicyTests(unittest.TestCase):
    def test_helper_has_bounded_capture_and_tree_termination(self):
        source = HELPER.read_text(encoding="utf-8")
        self.assertIn("Get-Command pwsh.exe", source)
        self.assertNotIn("Get-Command powershell.exe", source)
        self.assertIn("ReadToEndAsync", source)
        self.assertIn("WaitForExit($TimeoutSeconds * 1000)", source)
        self.assertIn("$Process.Kill($true)", source)
        self.assertIn("/PID $Process.Id /T /F", source)
        self.assertIn("WaitForExit(5000)", source)
        self.assertNotIn("WaitForExit()", source)

    def test_ephemeral_pfx_is_signtool_compatible_and_store_entry_is_removed(self):
        source = PROVISIONER.read_text(encoding="utf-8")
        for marker in (
            "New-SelfSignedCertificate",
            "-Type CodeSigningCert",
            "-KeyExportPolicy Exportable",
            "-KeySpec Signature",
            "-Provider 'Microsoft Enhanced RSA and AES Cryptographic Provider'",
            "Export-PfxCertificate",
            'Remove-Item -LiteralPath "Cert:\\CurrentUser\\My\\$($certificate.Thumbprint)"',
            "[Guid]::NewGuid()",
        ):
            with self.subTest(marker=marker):
                self.assertIn(marker, source)
        self.assertNotIn("CurrentUser\\Root", source)
        self.assertNotIn("CertificateRequest]::new", source)

    def test_workflow_bounds_both_calls_and_exports_only_after_validation(self):
        source = WORKFLOW.read_text(encoding="utf-8")
        step = workflow_step(source, "Provision ephemeral CI signing certificate")
        self.assertRegex(
            re.search(
                r"(?ms)^  bootstrap-smoke-windows:\n(.*?)(?=^  [A-Za-z0-9_-]+:\n|\Z)",
                source,
            ).group(1),
            r"(?m)^    timeout-minutes: 60$",
        )
        self.assertIn("invoke-bounded-powershell.ps1", step)
        for command in ("provision", "status", "sign", "verify"):
            with self.subTest(command=command):
                self.assertRegex(step, rf"-Command {command} .*?-TimeoutSeconds 120")
        self.assertNotRegex(
            step,
            r"&\s+powershell\.exe.*stasis-signing\.ps1.*(?:provision|status)",
        )
        validation = step.index("$status =")
        exports = step.index("$env:GITHUB_ENV")
        self.assertGreater(exports, validation)
        self.assertIn("certificate_configured", step[validation:exports])
        self.assertIn("required", step[validation:exports])
        self.assertIn("cl.exe /nologo /W4 /WX", step)
        self.assertNotIn("SystemRoot\\System32\\cmd.exe", step)
        self.assertIn("::add-mask::$pfxPassword", step)

        cleanup = workflow_step(source, "Cleanup ephemeral CI signing certificate")
        self.assertIn("if: always()", cleanup)
        self.assertIn("Remove-Item -LiteralPath $certificate", cleanup)


if __name__ == "__main__":
    unittest.main()
