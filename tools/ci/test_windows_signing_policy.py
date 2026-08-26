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
            self.assertIn("verify /pa /all", log.read_text(encoding="ascii"))

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


if __name__ == "__main__":
    unittest.main()
