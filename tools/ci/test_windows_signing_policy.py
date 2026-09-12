import pathlib
import os
import subprocess
import tempfile
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]


class WindowsSigningPolicyTests(unittest.TestCase):
    def test_powerShell_entrypoint_is_explicit_and_uses_page_hashes(self):
        source = (ROOT / "tools/windows/stasis-signing.ps1").read_text(encoding="utf-8")
        self.assertIn("ValidateSet('status', 'provision', 'sign', 'verify')", source)
        self.assertIn("'/fd', 'SHA256', '/ph'", source)
        self.assertIn("Cert:\\CurrentUser\\My", source)
        self.assertIn("KeyExportPolicy NonExportable", source)
        self.assertIn("production signing never provisions", source)

    def test_rust_policy_keeps_legacy_hook_and_actionable_configuration(self):
        source = (ROOT / "apps/stasis/src/windows_signing.rs").read_text(encoding="utf-8")
        self.assertIn('STASIS_AOT_SIGN_TOOL', source)
        self.assertIn('STASIS_REQUIRE_SIGNED_EXECUTION', source)
        self.assertIn('"/fd", "SHA256", "/ph"', source)
        self.assertIn("CurrentUser development certificate", source)
        self.assertIn("Production credentials", source)

    def test_release_workflows_ship_signing_entrypoint(self):
        for workflow in (".github/workflows/bootstrap-artifacts.yml", ".github/workflows/nightly-release.yml"):
            source = (ROOT / workflow).read_text(encoding="utf-8")
            self.assertIn("tools/windows/stasis-signing.ps1", source)

    def test_nightly_publication_requires_and_verifies_production_signing(self):
        source = (ROOT / ".github/workflows/nightly-release.yml").read_text(
            encoding="utf-8"
        )
        self.assertIn("release_preconditions:", source)
        self.assertIn("Require production Windows signing configuration", source)
        self.assertIn("owned by Maddox task #525", source)
        self.assertIn("needs: [detect, mobile_network_support, release_preconditions]", source)
        self.assertIn("STASIS_SIGNING_PROFILE: production", source)
        self.assertIn("STASIS_SIGNING_ALLOW_PINNED_SELF_SIGNED_VERIFY: '1'", source)
        self.assertIn("'tools/windows/stasis-signing.ps1'", source)
        self.assertIn("'tools/windows/stasis-signing-identity.ps1'", source)
        self.assertIn('$expectedThumbprint = "67132CE8553062F2145A1EBD7A88166910CDA7A6"', source)
        self.assertNotIn('"TrustedPeople"', source)
        self.assertIn("STASIS_SIGNING_TIMESTAMP_URLS", source)
        self.assertIn("STASIS_SIGNING_TIMEOUT_SECONDS", source)
        self.assertIn("http://timestamp.acs.microsoft.com/;http://timestamp.digicert.com", source)
        self.assertIn("Invoke-BoundedSigningCommand sign $_", source)
        self.assertIn("Invoke-BoundedSigningCommand verify $_", source)
        self.assertIn('Invoke-BoundedPowerShellCommand "pinned signer identity validation"', source)
        self.assertNotIn("temporary signer trust", source)
        signing_step = source.split(
            "- name: Authenticode sign Stasis Windows binaries", 1
        )[1].split("- name: Assemble bundle (unix)", 1)[0]
        self.assertIn("timeout-minutes: 15", signing_step)
        self.assertIn("$process.WaitForExit(90000)", signing_step)
        self.assertIn("$process.Kill()", signing_step)
        self.assertIn("$process.WaitForExit(5000)", signing_step)
        self.assertIn('Write-Host "Starting $label"', signing_step)
        self.assertIn("$startInfo.UseShellExecute = $false", signing_step)
        self.assertIn("$startInfo.ArgumentList.Add($argument)", signing_step)
        self.assertIn("$process.ExitCode -ne 0", signing_step)
        self.assertNotIn("Remove nightly signing root trust", source)
        self.assertNotIn("runner.os == 'Windows' && env.STASIS_SIGNING_PFX_BASE64 != ''", source)
        extracted_verification_step = source.split(
            "- name: Verify extracted Windows editor toolchain", 1
        )[1].split("- name: Smoke test bundled graphics runtime (windows)", 1)[0]
        self.assertIn('$env:STASIS_SIGNING_TIMEOUT_SECONDS = "30"', extracted_verification_step)
        self.assertIn("& pwsh -NoProfile -File tools/windows/stasis-signing.ps1 verify", extracted_verification_step)
        self.assertNotIn("& powershell.exe", extracted_verification_step)

        identity_source = (ROOT / "tools/windows/stasis-signing-identity.ps1").read_text(
            encoding="utf-8"
        )
        self.assertIn("Get-Command openssl.exe", identity_source)
        self.assertIn("'env:STASIS_SIGNING_PFX_PASSWORD'", identity_source)
        self.assertNotIn("'-legacy'", identity_source)
        self.assertIn("$publicCertificate.Thumbprint -ne $ExpectedThumbprint", identity_source)
        self.assertIn("$publicCertificate.Subject -ne $publicCertificate.Issuer", identity_source)
        self.assertIn("$process.WaitForExit(30000)", identity_source)
        self.assertIn("$process.Kill()", identity_source)
        for forbidden in ("Get-AuthenticodeSignature", "certutil", "certmgr", "X509Store", "CertAddEncodedCertificateToStore"):
            self.assertNotIn(forbidden.lower(), identity_source.lower())
        self.assertNotIn("X509Certificate2]::new", signing_step)

        signing_source = (ROOT / "tools/windows/stasis-signing.ps1").read_text(encoding="utf-8")
        self.assertIn("STASIS_SIGNING_ALLOW_PINNED_SELF_SIGNED_VERIFY", signing_source)
        self.assertIn("0x800B0109", signing_source)
        self.assertIn("Number of errors:", signing_source)
        self.assertIn("Number of warnings:", signing_source)
        self.assertIn("TRUST_E_BAD_DIGEST", signing_source)
        self.assertIn("'/tw', '/v'", signing_source)

    def test_cargo_runner_routes_signtool_through_policy_entrypoint(self):
        source = (ROOT / ".cargo/stasis-sign-and-run.cmd").read_text(encoding="utf-8")
        self.assertIn("stasis-signing.ps1", source)
        self.assertIn("SHA256", (ROOT / "tools/windows/stasis-signing.ps1").read_text(encoding="utf-8"))

    @unittest.skipUnless(os.name == "nt", "PowerShell signing entrypoint test")
    def test_powershell_fake_signtool_receives_policy_arguments_without_printing_password(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            tool = root / "signtool.cmd"
            log = root / "args.txt"
            artifact = root / "artifact.exe"
            certificate = root / "signing.pfx"
            artifact.write_bytes(b"fixture")
            certificate.write_bytes(b"fixture")
            tool.write_text(
                '@echo off\r\n'
                f'> "{log}" echo %*\r\n'
                'if /I "%1"=="verify" exit /b 0\r\n'
                'exit /b 0\r\n',
                encoding="ascii",
            )
            secret = "fixture-secret-not-for-output"
            environment = os.environ.copy()
            environment["STASIS_SIGNING_PFX_PASSWORD"] = secret
            command = [
                "powershell.exe",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                str(ROOT / "tools/windows/stasis-signing.ps1"),
                "sign",
                "-Tool",
                str(tool),
                "-Certificate",
                str(certificate),
                "-Artifact",
                str(artifact),
            ]
            result = subprocess.run(command, capture_output=True, text=True, env=environment, timeout=30)
            self.assertEqual(result.returncode, 0, result.stderr)
            args = log.read_text(encoding="ascii")
            self.assertIn("sign", args)
            self.assertIn("/fd SHA256 /ph", args)
            self.assertIn(f"/p {secret}", args)
            self.assertNotIn(secret, result.stdout + result.stderr)

            record = root / "development-thumbprint.txt"
            record.write_text("ABCDEF123456\n", encoding="ascii")
            environment.pop("STASIS_SIGNING_CERTIFICATE", None)
            environment["STASIS_SIGNING_LOCAL_RECORD"] = str(record)
            local_sign = subprocess.run(
                [
                    "powershell.exe",
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-File",
                    str(ROOT / "tools/windows/stasis-signing.ps1"),
                    "sign",
                    "-Tool",
                    str(tool),
                    "-Artifact",
                    str(artifact),
                ],
                capture_output=True,
                text=True,
                env=environment,
                timeout=30,
            )
            self.assertEqual(local_sign.returncode, 0, local_sign.stderr)
            self.assertIn("/sha1 ABCDEF123456", log.read_text(encoding="ascii"))
            environment["STASIS_SIGNING_PROFILE"] = "production"
            production_local = subprocess.run(
                [
                    "powershell.exe",
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-File",
                    str(ROOT / "tools/windows/stasis-signing.ps1"),
                    "sign",
                    "-Tool",
                    str(tool),
                    "-Artifact",
                    str(artifact),
                ],
                capture_output=True,
                text=True,
                env=environment,
                timeout=30,
            )
            self.assertNotEqual(production_local.returncode, 0)
            self.assertIn("requires STASIS_SIGNING_CERTIFICATE", production_local.stderr)

            verify = subprocess.run(
                [
                    "powershell.exe",
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-File",
                    str(ROOT / "tools/windows/stasis-signing.ps1"),
                    "verify",
                    "-Tool",
                    str(tool),
                    "-Artifact",
                    str(artifact),
                ],
                capture_output=True,
                text=True,
                env=environment,
                timeout=30,
            )
            self.assertEqual(verify.returncode, 0, verify.stderr)
            self.assertIn("verify /pa /all /tw /v", log.read_text(encoding="ascii"))

            legacy = root / "legacy-hook.cmd"
            legacy.write_text("@echo off\r\nexit /b 0\r\n", encoding="ascii")
            rejected = subprocess.run(
                [
                    "powershell.exe",
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-File",
                    str(ROOT / "tools/windows/stasis-signing.ps1"),
                    "verify",
                    "-Tool",
                    str(legacy),
                    "-Artifact",
                    str(artifact),
                ],
                capture_output=True,
                text=True,
                env=environment,
                timeout=30,
            )
            self.assertNotEqual(rejected.returncode, 0)
            self.assertIn("real signtool.exe", rejected.stderr)

    def test_signtool_timestamp_attempts_are_bounded_and_retryable(self):
        source = (ROOT / "tools/windows/stasis-signing.ps1").read_text(encoding="utf-8")
        self.assertIn("Invoke-BoundedSignTool", source)
        self.assertIn("$process.WaitForExit($timeoutSeconds * 1000)", source)
        self.assertIn("$process.Kill()", source)
        self.assertNotIn("$process.Kill($true)", source)
        self.assertIn("$process.WaitForExit(5000)", source)
        self.assertNotIn("$process.WaitForExit()", source)
        self.assertIn("$env:STASIS_SIGNING_TIMESTAMP_URLS -split ';'", source)
        self.assertIn("foreach ($timestamp in $timestamps)", source)
        self.assertIn(
            "Invoke-BoundedSignTool $signer.Path @('verify', '/pa', '/all', '/tw', '/v', $path) -AllowPinnedSelfSigned",
            source,
        )

    @unittest.skipUnless(os.name == "nt", "PowerShell pinned verification bridge test")
    def test_pinned_self_signed_bridge_accepts_only_the_expected_trust_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            tool = root / "signtool.cmd"
            artifact = root / "artifact.exe"
            artifact.write_bytes(b"fixture")
            expected_output = (
                "SignTool Error: A certificate chain processed, but terminated in a root\r\n"
                "certificate "
                "which is not trusted by the trust provider. (0x800B0109)\r\n"
                "Number of warnings: 0\r\n"
                "Number of errors: 1\r\n"
            )
            environment = os.environ.copy()
            environment.pop("STASIS_SIGNING_TIMEOUT_SECONDS", None)
            environment["STASIS_SIGNING_PROFILE"] = "production"
            environment["STASIS_SIGNING_ALLOW_PINNED_SELF_SIGNED_VERIFY"] = "1"

            def verify(output):
                tool.write_text(
                    "@echo off\r\n"
                    + "".join(f"echo {line}\r\n" for line in output.splitlines())
                    + "exit /b 1\r\n",
                    encoding="ascii",
                )
                return subprocess.run(
                    [
                        "powershell.exe", "-NoProfile", "-NonInteractive",
                        "-ExecutionPolicy", "Bypass", "-File",
                        str(ROOT / "tools/windows/stasis-signing.ps1"), "verify",
                        "-Tool", str(tool), "-Artifact", str(artifact),
                    ],
                    capture_output=True, text=True, env=environment, timeout=30,
                )

            accepted = verify(expected_output)
            self.assertEqual(accepted.returncode, 0, accepted.stderr)

            rejected = verify(expected_output + "SignTool Error: TRUST_E_BAD_DIGEST (0x80096010)\r\n")
            self.assertNotEqual(rejected.returncode, 0)
            self.assertIn("signature verification failed", rejected.stderr)

    @unittest.skipUnless(os.name == "nt", "PowerShell timestamp retry test")
    def test_powershell_retries_the_next_timestamp_authority(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            tool = root / "signtool.cmd"
            log = root / "args.txt"
            artifact = root / "artifact.exe"
            certificate = root / "signing.pfx"
            artifact.write_bytes(b"fixture")
            certificate.write_bytes(b"fixture")
            tool.write_text(
                "@echo off\r\n"
                f'>> "{log}" echo %*\r\n'
                'echo %* | findstr /C:"timestamp.acs.microsoft.com" >nul '
                "&& exit /b 1\r\n"
                "exit /b 0\r\n",
                encoding="ascii",
            )
            environment = os.environ.copy()
            environment["STASIS_SIGNING_TIMESTAMP_URLS"] = (
                "http://timestamp.acs.microsoft.com/;http://timestamp.digicert.com"
            )
            result = subprocess.run(
                [
                    "powershell.exe", "-NoProfile", "-NonInteractive",
                    "-ExecutionPolicy", "Bypass", "-File",
                    str(ROOT / "tools/windows/stasis-signing.ps1"), "sign",
                    "-Tool", str(tool), "-Certificate", str(certificate),
                    "-Artifact", str(artifact),
                ],
                capture_output=True, text=True, env=environment, timeout=30,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            attempts = log.read_text(encoding="ascii")
            self.assertIn("http://timestamp.acs.microsoft.com/", attempts)
            self.assertIn("http://timestamp.digicert.com", attempts)


if __name__ == "__main__":
    unittest.main()
