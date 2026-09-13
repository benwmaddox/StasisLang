import json
import os
import pathlib
import shutil
import subprocess
import tempfile
import textwrap
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tools" / "windows" / "stasis-signing.ps1"
POWERSHELL = shutil.which("powershell.exe")


@unittest.skipUnless(os.name == "nt" and POWERSHELL, "Windows PowerShell trust tests")
class WindowsSigningTrustTests(unittest.TestCase):
    def run_wrapper(self, source: str, environment: dict[str, str] | None = None):
        with tempfile.TemporaryDirectory() as directory:
            wrapper = pathlib.Path(directory) / "wrapper.ps1"
            wrapper.write_text(source, encoding="utf-8")
            environment = (environment or os.environ).copy()
            for name in (
                "STASIS_AOT_SIGN_TOOL",
                "STASIS_SIGNING_CERTIFICATE",
                "STASIS_SIGNING_CERT_THUMBPRINT",
                "STASIS_SIGNING_PFX_PASSWORD",
                "STASIS_SIGNING_PROFILE",
                "STASIS_SIGNING_MODE",
            ):
                environment.pop(name, None)
            return subprocess.run(
                [POWERSHELL, "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", str(wrapper)],
                cwd=ROOT,
                env=environment,
                capture_output=True,
                text=True,
                timeout=30,
                check=False,
            )

    def test_cng_reuse_rejects_every_export_policy_including_archiving(self):
        source = SCRIPT.read_text(encoding="utf-8")
        self.assertIn(
            "$key.Key.ExportPolicy -eq [Security.Cryptography.CngExportPolicies]::None",
            source,
        )
        self.assertNotIn("CngExportPolicies]::AllowExport -bor", source)

    def test_empty_path_segments_do_not_break_discovery(self):
        environment = os.environ.copy()
        environment.pop("STASIS_AOT_SIGN_TOOL", None)
        environment["PATH"] = f";{environment.get('PATH', '')};;"
        result = subprocess.run(
            [POWERSHELL, "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", str(SCRIPT), "status"],
            cwd=ROOT,
            env=environment,
            capture_output=True,
            text=True,
            timeout=30,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("certificate_configured", json.loads(result.stdout))

    def test_status_rejects_a_stale_local_certificate_record(self):
        with tempfile.TemporaryDirectory() as directory:
            record = pathlib.Path(directory) / "development-thumbprint.txt"
            record.write_text("0000000000000000000000000000000000000000\n", encoding="ascii")
            environment = os.environ.copy()
            environment["STASIS_SIGNING_LOCAL_RECORD"] = str(record)
            environment.pop("STASIS_SIGNING_CERTIFICATE", None)
            environment.pop("STASIS_SIGNING_CERT_THUMBPRINT", None)
            environment.pop("STASIS_SIGNING_PROFILE", None)
            result = subprocess.run(
                [POWERSHELL, "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", str(SCRIPT), "status", "-Tool", "C:\\missing\\signtool.exe"],
                cwd=ROOT,
                env=environment,
                capture_output=True,
                text=True,
                timeout=30,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            status = json.loads(result.stdout)
            self.assertFalse(status["certificate_configured"])
            self.assertFalse(status["local_development_certificate_configured"])
            self.assertIn("requires STASIS_SIGNING_CERTIFICATE", status["certificate_diagnostic"])

    def test_status_rejects_an_explicit_unusable_thumbprint(self):
        environment = os.environ.copy()
        environment.pop("STASIS_AOT_SIGN_TOOL", None)
        environment.pop("STASIS_SIGNING_CERTIFICATE", None)
        environment.pop("STASIS_SIGNING_CERT_THUMBPRINT", None)
        result = subprocess.run(
            [POWERSHELL, "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", str(SCRIPT), "status", "-Tool", "C:\\missing\\signtool.exe", "-Thumbprint", "0000000000000000000000000000000000000000"],
            cwd=ROOT,
            env=environment,
            capture_output=True,
            text=True,
            timeout=30,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        status = json.loads(result.stdout)
        self.assertFalse(status["certificate_configured"])
        self.assertIn("was not found as a usable certificate", status["certificate_diagnostic"])

    def test_invalid_pfx_diagnostic_does_not_expose_password(self):
        with tempfile.TemporaryDirectory() as directory:
            certificate = pathlib.Path(directory) / "invalid.pfx"
            certificate.write_bytes(b"not a certificate")
            secret = "sentinel-pfx-password-must-not-appear"
            environment = os.environ.copy()
            environment["STASIS_SIGNING_CERTIFICATE"] = str(certificate)
            environment["STASIS_SIGNING_PFX_PASSWORD"] = secret
            environment.pop("STASIS_SIGNING_CERT_THUMBPRINT", None)
            environment.pop("STASIS_SIGNING_PROFILE", None)
            environment.pop("STASIS_SIGNING_MODE", None)
            result = subprocess.run(
                [POWERSHELL, "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", str(SCRIPT), "status", "-Tool", "C:\\missing\\signtool.exe"],
                cwd=ROOT,
                env=environment,
                capture_output=True,
                text=True,
                timeout=30,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            status = json.loads(result.stdout)
            self.assertFalse(status["certificate_configured"])
            self.assertTrue(status["certificate_diagnostic"])
            self.assertNotIn(secret, result.stdout + result.stderr)

    def test_sign_uses_artifact_page_hash_policy_and_checks_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            tool = root / "signtool.cmd"
            log = root / "arguments.txt"
            exe = root / "program with spaces.exe"
            dll = root / "library with spaces.dll"
            exe.write_bytes(b"fixture")
            dll.write_bytes(b"fixture")
            tool.write_text(f'@echo off\r\n>> "{log}" echo %*\r\nexit /b 0\r\n', encoding="ascii")
            source = textwrap.dedent(
                f"""
                . '{SCRIPT}' status -Tool '{tool}'
                function Resolve-SignTool {{ [pscustomobject]@{{ Path = '{tool}'; Source = 'mock' }} }}
                function Get-ConfiguredCertificate {{ [pscustomobject]@{{ Arguments = @('/sha1', 'AABBCC'); Thumbprint = 'AABBCC' }} }}
                function Get-AuthenticodeSignature {{ param([string] $LiteralPath); [pscustomobject]@{{ Status = [System.Management.Automation.SignatureStatus]::Valid; StatusMessage = 'valid'; SignerCertificate = [pscustomobject]@{{ Thumbprint = 'AA BB CC' }} }} }}
                Invoke-SignArtifact '{exe}'
                Invoke-SignArtifact '{dll}'
                "verified=$true"
                """
            )
            result = self.run_wrapper(source)
            self.assertEqual(result.returncode, 0, result.stderr)
            calls = log.read_text(encoding="ascii").splitlines()
            self.assertEqual(len(calls), 2)
            self.assertIn("sign /fd SHA256 /ph /sha1 AABBCC", calls[0])
            self.assertIn("sign /fd SHA256 /nph /sha1 AABBCC", calls[1])
            self.assertIn("verified=True", result.stdout)

    def test_legacy_hook_receives_page_hash_policy_and_environment_is_restored(self):
        with tempfile.TemporaryDirectory(prefix="legacy signing spaces ") as directory:
            root = pathlib.Path(directory)
            tool = root / "legacy.cmd"
            log = root / "page-hashes.txt"
            exe = root / "program with spaces.exe"
            dll = root / "library with spaces.dll"
            exe.write_bytes(b"fixture")
            dll.write_bytes(b"fixture")
            tool.write_text(f'@echo off\r\n>> "{log}" echo [%STASIS_SIGN_PAGE_HASHES%]\r\nexit /b 0\r\n', encoding="ascii")
            result = self.run_wrapper(textwrap.dedent(f"""
                . '{SCRIPT}' status -Tool '{tool}'
                function Get-VerificationThumbprint {{ 'AABBCC' }}
                function Assert-AuthenticodeIdentity {{ }}
                $env:STASIS_SIGN_PAGE_HASHES = 'original'
                Invoke-SignArtifact '{exe}'
                Invoke-SignArtifact '{dll}'
                "restored=$env:STASIS_SIGN_PAGE_HASHES"
            """))
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(log.read_text(encoding="ascii").splitlines(), ["[1]", "[]"])
            self.assertIn("restored=original", result.stdout)

    def test_sign_rejects_a_different_signing_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            tool = root / "signtool.cmd"
            artifact = root / "program.exe"
            artifact.write_bytes(b"fixture")
            tool.write_text("@echo off\r\nexit /b 0\r\n", encoding="ascii")
            source = textwrap.dedent(
                f"""
                . '{SCRIPT}' status -Tool '{tool}'
                function Resolve-SignTool {{ [pscustomobject]@{{ Path = '{tool}'; Source = 'mock' }} }}
                function Get-ConfiguredCertificate {{ [pscustomobject]@{{ Arguments = @('/sha1', 'AABBCC'); Thumbprint = 'AABBCC' }} }}
                function Get-AuthenticodeSignature {{ [pscustomobject]@{{ Status = [System.Management.Automation.SignatureStatus]::Valid; StatusMessage = 'valid'; SignerCertificate = [pscustomobject]@{{ Thumbprint = 'DDEEFF' }} }} }}
                try {{ Invoke-SignArtifact '{artifact}'; exit 0 }} catch {{ [Console]::Error.WriteLine($_.Exception.Message); exit 7 }}
                """
            )
            result = self.run_wrapper(source)
            self.assertEqual(result.returncode, 7)
            self.assertIn("expected thumbprint AABBCC but found DDEEFF", result.stderr)

    def test_legacy_hook_zero_exit_cannot_pass_an_unsigned_artifact(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            tool = root / "legacy-hook.cmd"
            artifact = root / "program.exe"
            artifact.write_bytes(b"fixture")
            tool.write_text("@echo off\r\nexit /b 0\r\n", encoding="ascii")
            source = textwrap.dedent(
                f"""
                . '{SCRIPT}' status -Tool '{tool}'
                function Resolve-SignTool {{ [pscustomobject]@{{ Path = '{tool}'; Source = 'mock' }} }}
                function Get-VerificationThumbprint {{ $null }}
                function Get-AuthenticodeSignature {{ [pscustomobject]@{{ Status = [System.Management.Automation.SignatureStatus]::NotSigned; StatusMessage = 'not signed'; SignerCertificate = $null }} }}
                try {{ Invoke-SignArtifact '{artifact}'; exit 0 }} catch {{ [Console]::Error.WriteLine($_.Exception.Message); exit 8 }}
                """
            )
            result = self.run_wrapper(source)
            self.assertEqual(result.returncode, 8)
            self.assertIn("status NotSigned", result.stderr)

    def test_legacy_hook_cannot_replace_a_stale_recorded_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            tool = root / "legacy-hook.cmd"
            artifact = root / "program.exe"
            record = root / "development-thumbprint.txt"
            artifact.write_bytes(b"fixture")
            record.write_text("AABBCC\n", encoding="ascii")
            tool.write_text("@echo off\r\nexit /b 0\r\n", encoding="ascii")
            environment = os.environ.copy()
            environment["STASIS_SIGNING_LOCAL_RECORD"] = str(record)
            source = textwrap.dedent(
                f"""
                . '{SCRIPT}' status -Tool '{tool}'
                function Resolve-SignTool {{ [pscustomobject]@{{ Path = '{tool}'; Source = 'mock' }} }}
                function Get-AuthenticodeSignature {{ [pscustomobject]@{{ Status = [System.Management.Automation.SignatureStatus]::Valid; StatusMessage = 'valid'; SignerCertificate = [pscustomobject]@{{ Thumbprint = 'DDEEFF' }} }} }}
                try {{ Invoke-SignArtifact '{artifact}'; exit 0 }} catch {{ [Console]::Error.WriteLine($_.Exception.Message); exit 11 }}
                """
            )
            result = self.run_wrapper(source, environment)
            self.assertEqual(result.returncode, 11)
            self.assertIn("expected thumbprint AABBCC but found DDEEFF", result.stderr)

    def test_provision_reuses_usable_certificate_and_adds_only_root_trust(self):
        with tempfile.TemporaryDirectory() as directory:
            record = pathlib.Path(directory) / "development-thumbprint.txt"
            environment = os.environ.copy()
            environment["STASIS_SIGNING_LOCAL_RECORD"] = str(record)
            source = textwrap.dedent(
                f"""
                . '{SCRIPT}' status -Tool 'C:\\missing\\signtool.exe'
                $global:fakeCert = [pscustomobject]@{{ Subject = 'CN=StasisLang Development Signing'; Thumbprint = 'AABBCC'; NotAfter = [DateTime]::UtcNow.AddYears(1) }}
                function Get-LocalDevelopmentCertificate {{ $null }}
                function Get-ChildItem {{ $global:fakeCert }}
                function Test-CodeSigningCertificate {{ return $true }}
                function Test-CurrentUserRootTrust {{ return $false }}
                function Add-CurrentUserRootTrust {{ $global:trustTarget = 'CurrentUser\\Root'; $global:trusted = $true }}
                function Get-CurrentUserRootCertificate {{ if ($global:trusted) {{ [pscustomobject]@{{ HasPrivateKey = $false }} }} }}
                function New-SelfSignedCertificate {{ throw 'must reuse existing certificate' }}
                Invoke-Provision
                "trust=$global:trustTarget"
                """
            )
            result = self.run_wrapper(source, environment)
            self.assertEqual(result.returncode, 0, result.stderr)
            provisioned = json.loads(next(line for line in result.stdout.splitlines() if '"root_trust"' in line))
            self.assertEqual(provisioned["certificate"], "reused")
            self.assertEqual(provisioned["root_trust"], "added")
            self.assertEqual(provisioned["trust_store"], "CurrentUser\\Root")
            self.assertNotIn("TrustedPublisher", result.stdout)
            self.assertEqual(record.read_text(encoding="ascii").strip(), "AABBCC")

    def test_enrollment_failure_does_not_rewrite_untouched_record(self):
        with tempfile.TemporaryDirectory() as directory:
            record = pathlib.Path(directory) / "development-thumbprint.txt"
            record.write_text("OLDVALUE\n", encoding="ascii")
            environment = os.environ.copy()
            environment["STASIS_SIGNING_LOCAL_RECORD"] = str(record)
            source = textwrap.dedent(
                f"""
                . '{SCRIPT}' status -Tool 'C:\\missing\\signtool.exe'
                function Get-LocalDevelopmentCertificate {{ $null }}
                function Get-ChildItem {{ @() }}
                function New-SelfSignedCertificate {{ throw 'enrollment unavailable' }}
                function Set-Content {{ throw 'unexpected record write' }}
                try {{ Invoke-Provision; exit 0 }} catch {{ [Console]::Error.WriteLine($_.Exception.Message); exit 9 }}
                """
            )
            result = self.run_wrapper(source, environment)
            self.assertEqual(result.returncode, 9)
            self.assertIn("enrollment unavailable", result.stderr)
            self.assertIn("no changes required rollback", result.stderr)
            self.assertNotIn("unexpected record write", result.stderr)
            self.assertEqual(record.read_text(encoding="ascii"), "OLDVALUE\n")

    def test_provision_rolls_back_created_certificate_and_trust_on_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            record = pathlib.Path(directory) / "development-thumbprint.txt"
            record.write_text("OLDVALUE\n", encoding="ascii")
            environment = os.environ.copy()
            environment["STASIS_SIGNING_LOCAL_RECORD"] = str(record)
            source = textwrap.dedent(
                f"""
                . '{SCRIPT}' status -Tool 'C:\\missing\\signtool.exe'
                $global:fakeCert = [pscustomobject]@{{ Subject = 'CN=StasisLang Development Signing'; Thumbprint = 'AABBCC'; NotAfter = [DateTime]::UtcNow.AddYears(1) }}
                $global:removed = @()
                function Get-LocalDevelopmentCertificate {{ $null }}
                function Get-ChildItem {{ @() }}
                function New-SelfSignedCertificate {{ $global:fakeCert }}
                function Test-CodeSigningCertificate {{ return $true }}
                function Test-CurrentUserRootTrust {{ return $false }}
                function Add-CurrentUserRootTrust {{ $global:trusted = $true }}
                function Get-CurrentUserRootCertificate {{ if ($global:trusted) {{ [pscustomobject]@{{ HasPrivateKey = $false }} }} }}
                function Write-LocalDevelopmentRecord {{ throw 'simulated record failure' }}
                function Remove-CurrentUserCertificate {{ param($StoreName, $CertificateThumbprint); $global:removed += $StoreName }}
                try {{ Invoke-Provision; exit 0 }} catch {{ [Console]::Error.WriteLine($_.Exception.Message); "removed=$($global:removed -join ',')"; exit 9 }}
                """
            )
            result = self.run_wrapper(source, environment)
            self.assertEqual(result.returncode, 9)
            self.assertIn("Rollback: removed CurrentUser\\Root trust; removed CurrentUser\\My certificate; restored selection record", result.stderr)
            self.assertIn("removed=Root,My", result.stdout)
            self.assertEqual(record.read_text(encoding="ascii"), "OLDVALUE\n")

    def test_provision_continues_rollback_after_root_cleanup_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            record = pathlib.Path(directory) / "development-thumbprint.txt"
            record.write_text("OLDVALUE\n", encoding="ascii")
            environment = os.environ.copy()
            environment["STASIS_SIGNING_LOCAL_RECORD"] = str(record)
            source = textwrap.dedent(
                f"""
                . '{SCRIPT}' status -Tool 'C:\\missing\\signtool.exe'
                $global:fakeCert = [pscustomobject]@{{ Subject = 'CN=StasisLang Development Signing'; Thumbprint = 'AABBCC'; NotAfter = [DateTime]::UtcNow.AddYears(1) }}
                $global:removedMy = $false
                function Get-LocalDevelopmentCertificate {{ $null }}
                function Get-ChildItem {{ @() }}
                function New-SelfSignedCertificate {{ $global:fakeCert }}
                function Test-CodeSigningCertificate {{ return $true }}
                function Test-CurrentUserRootTrust {{ return $false }}
                function Add-CurrentUserRootTrust {{ $global:trusted = $true }}
                function Get-CurrentUserRootCertificate {{ if ($global:trusted) {{ [pscustomobject]@{{ HasPrivateKey = $false }} }} }}
                function Write-LocalDevelopmentRecord {{ throw 'simulated record failure' }}
                function Remove-CurrentUserCertificate {{ param($StoreName, $CertificateThumbprint); if ($StoreName -eq 'Root') {{ throw 'root busy' }}; $global:removedMy = $true }}
                try {{ Invoke-Provision; exit 0 }} catch {{ [Console]::Error.WriteLine($_.Exception.Message); "removedMy=$global:removedMy"; exit 10 }}
                """
            )
            result = self.run_wrapper(source, environment)
            self.assertEqual(result.returncode, 10)
            self.assertIn("CurrentUser\\Root rollback failed: root busy", result.stderr)
            self.assertIn("removed CurrentUser\\My certificate", result.stderr)
            self.assertIn("restored selection record", result.stderr)
            self.assertIn("removedMy=True", result.stdout)
            self.assertEqual(record.read_text(encoding="ascii"), "OLDVALUE\n")

    def test_local_discovery_requires_usable_trusted_cert_and_ignores_it_in_production(self):
        with tempfile.TemporaryDirectory() as directory:
            record = pathlib.Path(directory) / "development-thumbprint.txt"
            record.write_text("AABBCC\n", encoding="ascii")
            environment = os.environ.copy()
            environment["STASIS_SIGNING_LOCAL_RECORD"] = str(record)
            source = textwrap.dedent(
                f"""
                . '{SCRIPT}' status -Tool 'C:\\missing\\signtool.exe'
                $global:fakeCert = [pscustomobject]@{{ Subject = 'CN=StasisLang Development Signing'; Thumbprint = 'AABBCC' }}
                function Get-PersonalCertificate {{ $global:fakeCert }}
                function Test-CodeSigningCertificate {{ return $true }}
                function Test-CurrentUserRootTrust {{ return $true }}
                "development=$([bool](Get-LocalDevelopmentCertificate))"
                $env:STASIS_SIGNING_PROFILE = 'production'
                "production=$([bool](Get-LocalDevelopmentCertificate))"
                """
            )
            result = self.run_wrapper(source, environment)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("development=True", result.stdout)
            self.assertIn("production=False", result.stdout)


if __name__ == "__main__":
    unittest.main()
