import pathlib
import os
import subprocess
import tempfile
import textwrap
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]


class WindowsSigningPolicyTests(unittest.TestCase):
    def test_powerShell_entrypoint_is_explicit_and_uses_page_hashes(self):
        source = (ROOT / "tools/windows/stasis-signing.ps1").read_text(encoding="utf-8")
        self.assertIn("ValidateSet('status', 'provision', 'sign', 'verify')", source)
        self.assertIn("Cert:\\CurrentUser\\My", source)
        self.assertIn("KeyExportPolicy NonExportable", source)
        self.assertIn("production signing never provisions", source)

    def test_rust_policy_keeps_legacy_hook_and_actionable_configuration(self):
        source = (ROOT / "apps/stasis/src/windows_signing.rs").read_text(encoding="utf-8")
        self.assertIn('STASIS_AOT_SIGN_TOOL', source)
        self.assertIn('STASIS_REQUIRE_SIGNED_EXECUTION', source)

    def test_release_workflows_ship_signing_entrypoint(self):
        for workflow in (".github/workflows/bootstrap-artifacts.yml", ".github/workflows/nightly-release.yml"):
            source = (ROOT / workflow).read_text(encoding="utf-8")
            self.assertIn("tools/windows/stasis-signing.ps1", source)

    def test_nightly_publication_requires_pinned_windows_release_signing(self):
        source = (ROOT / ".github/workflows/nightly-release.yml").read_text(
            encoding="utf-8"
        )
        self.assertNotIn("release_preconditions:", source)
        self.assertIn("needs: [detect, mobile_network_support]", source)
        build_job = source.split("  build:", 1)[1].split("  windows_signing:", 1)[0]
        signing_job = source.split("  windows_signing:", 1)[1].split("  vscode_extension:", 1)[0]
        self.assertNotIn("secrets.STASIS_SIGNING", build_job)
        self.assertIn("STASIS_SIGNING_PFX_BASE64", source)
        self.assertIn("STASIS_SIGNING_PFX_PASSWORD", source)
        self.assertIn('$env:STASIS_SIGNING_PROFILE = "production"', signing_job)
        self.assertIn("STASIS_SIGNING_TIMESTAMP_URLS", source)
        self.assertIn(
            'http://timestamp.digicert.com;http://timestamp.acs.microsoft.com',
            signing_job,
        )
        self.assertIn("Sign trusted Windows release files", signing_job)
        self.assertIn("if: needs.detect.outputs.should_release == 'true' && github.ref == 'refs/heads/main'", signing_job)
        self.assertIn("Require the trusted current main commit", signing_job)
        self.assertIn('$env:GITHUB_REF -ne "refs/heads/main"', signing_job)
        self.assertIn("git ls-remote origin refs/heads/main", signing_job)
        self.assertIn("stasis-signing-identity.ps1", source)
        self.assertIn("67132CE8553062F2145A1EBD7A88166910CDA7A6", source)
        self.assertIn("stasis_windows_signing.json", source)
        self.assertIn("windows_signing_manifest.py finalize", source)
        self.assertIn("windows_signing_manifest.py verify-files", source)
        self.assertIn("Remove-Item -LiteralPath $signingRoot", signing_job)
        self.assertLess(
            signing_job.index("Require the trusted current main commit"),
            signing_job.index("secrets.STASIS_SIGNING_PFX_BASE64"),
        )
        self.assertIn("tools/windows/stasis-signing.ps1", source)
        extracted_verification_step = signing_job.split(
            "- name: Verify extracted signed Windows toolchain", 1
        )[1].split("- name: Upload signed Windows toolchain", 1)[0]
        self.assertIn("stasis-signing.ps1 verify", extracted_verification_step)
        release_job = source.split("  release:", 1)[1].split("  no_changes:", 1)[0]
        self.assertIn("name: stasis-nightly-win-x64", release_job)
        self.assertNotIn("stasis-nightly-win-x64-unsigned", release_job)

    def test_cargo_runner_routes_signtool_through_policy_entrypoint(self):
        source = (ROOT / ".cargo/stasis-sign-and-run.cmd").read_text(encoding="utf-8")
        self.assertIn("stasis-signing.ps1", source)
        self.assertIn("SHA256", (ROOT / "tools/windows/stasis-signing.ps1").read_text(encoding="utf-8"))

    def test_policy_pins_security_module_to_the_active_host(self):
        source = (ROOT / "tools/windows/stasis-signing.ps1").read_text(encoding="utf-8")
        self.assertIn("Join-Path $PSHOME", source)
        self.assertIn("Microsoft.PowerShell.Security.psd1", source)
        self.assertIn("Import-Module $securityModule -ErrorAction Stop", source)

    def test_release_identity_validation_is_ephemeral_and_has_no_openssl_dependency(self):
        source = (ROOT / "tools/windows/stasis-signing-identity.ps1").read_text(
            encoding="utf-8"
        )
        self.assertIn("X509KeyStorageFlags]::EphemeralKeySet", source)
        self.assertIn("HasPrivateKey", source)
        self.assertIn("1.3.6.1.5.5.7.3.3", source)
        self.assertNotIn("openssl", source.casefold())

    @unittest.skipUnless(os.name == "nt", "PowerShell signing entrypoint test")
    def test_powershell_mock_certificate_receives_policy_arguments(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            tool = root / "signtool.cmd"
            log = root / "args.txt"
            artifact = root / "artifact.exe"
            artifact.write_bytes(b"fixture")
            tool.write_text(
                '@echo off\r\n'
                f'> "{log}" echo %*\r\n'
                'exit /b 0\r\n',
                encoding="ascii",
            )
            source = textwrap.dedent(
                f"""
                . '{ROOT / "tools/windows/stasis-signing.ps1"}' status -Tool '{tool}'
                function Resolve-SignTool {{ [pscustomobject]@{{ Path = '{tool}'; Source = 'mock' }} }}
                function Get-ConfiguredCertificate {{ [pscustomobject]@{{ Arguments = @('/sha1', 'AABBCC'); Thumbprint = 'AABBCC' }} }}
                function Assert-AuthenticodeIdentity {{ }}
                Invoke-SignArtifact '{artifact}'
                "signed=$true"
                """
            )
            wrapper = root / "wrapper.ps1"
            wrapper.write_text(source, encoding="utf-8")
            result = subprocess.run(
                ["powershell.exe", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", str(wrapper)],
                capture_output=True, text=True, timeout=30,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            args = log.read_text(encoding="ascii")
            self.assertIn("sign", args)
            self.assertIn("/fd SHA256 /ph /sha1 AABBCC", args)
            self.assertIn("signed=True", result.stdout)

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

    @unittest.skipUnless(os.name == "nt", "PowerShell 5 command-line escaping test")
    def test_powershell5_fallback_quotes_passwords_and_trailing_backslashes(self):
        source = textwrap.dedent(
            f"""
            . '{ROOT / "tools/windows/stasis-signing.ps1"}' status -Tool "$env:ComSpec"
            $encoded = ConvertTo-WindowsCommandLineArgument 'pa ss"tail\\'
            Write-Output "ENCODED=$encoded"
            """
        )
        with tempfile.TemporaryDirectory() as directory:
            wrapper = pathlib.Path(directory) / "wrapper.ps1"
            wrapper.write_text(source, encoding="utf-8")
            result = subprocess.run(
                ["powershell.exe", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", str(wrapper)],
                capture_output=True, text=True, timeout=30,
            )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(r'ENCODED="pa ss\"tail\\"', result.stdout)

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
                wrapper = root / "verify-wrapper.ps1"
                wrapper.write_text(
                    textwrap.dedent(
                        f"""
                        function Get-AuthenticodeSignature {{
                            [pscustomobject]@{{
                                Status = [System.Management.Automation.SignatureStatus]::UnknownError
                                StatusMessage = 'A certificate chain processed, but terminated in a root certificate which is not trusted by the trust provider.'
                                SignerCertificate = $null
                            }}
                        }}
                        & '{ROOT / "tools/windows/stasis-signing.ps1"}' verify -Tool '{tool}' -Artifact '{artifact}'
                        """
                    ),
                    encoding="utf-8",
                )
                return subprocess.run(
                    [
                        "powershell.exe", "-NoProfile", "-NonInteractive",
                        "-ExecutionPolicy", "Bypass", "-File",
                        str(wrapper),
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
            artifact.write_bytes(b"fixture")
            tool.write_text(
                "@echo off\r\n"
                f'>> "{log}" echo %*\r\n'
                'echo %* | findstr /C:"timestamp.acs.microsoft.com" >nul '
                "&& exit /b 1\r\n"
                "exit /b 0\r\n",
                encoding="ascii",
            )
            wrapper = root / "wrapper.ps1"
            wrapper.write_text(
                textwrap.dedent(
                    f"""
                    . '{ROOT / "tools/windows/stasis-signing.ps1"}' status -Tool '{tool}'
                    function Resolve-SignTool {{ [pscustomobject]@{{ Path = '{tool}'; Source = 'mock' }} }}
                    function Get-ConfiguredCertificate {{ [pscustomobject]@{{ Arguments = @('/sha1', 'AABBCC'); Thumbprint = 'AABBCC' }} }}
                    function Assert-AuthenticodeIdentity {{ }}
                    Invoke-SignArtifact '{artifact}'
                    "retried=$true"
                    """
                ),
                encoding="utf-8",
            )
            environment = os.environ.copy()
            environment["STASIS_SIGNING_TIMESTAMP_URLS"] = (
                "http://timestamp.acs.microsoft.com/;http://timestamp.digicert.com"
            )
            result = subprocess.run(
                [
                    "powershell.exe", "-NoProfile", "-NonInteractive",
                    "-ExecutionPolicy", "Bypass", "-File", str(wrapper),
                ],
                capture_output=True, text=True, env=environment, timeout=30,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            attempts = log.read_text(encoding="ascii")
            self.assertIn("http://timestamp.acs.microsoft.com/", attempts)
            self.assertIn("http://timestamp.digicert.com", attempts)


if __name__ == "__main__":
    unittest.main()
