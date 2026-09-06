"""Exercise the read-only Windows diagnostic report with deterministic providers."""

import json
from pathlib import Path
import shutil
import subprocess
import unittest


ROOT = Path(__file__).resolve().parents[2]
SHELL = shutil.which("pwsh") or shutil.which("powershell")


@unittest.skipUnless(SHELL, "PowerShell is required for Windows diagnostics")
class DesktopNetworkDiagnosticsTests(unittest.TestCase):
    def report(self, providers):
        script = ROOT / "tools/diagnose_desktop_network.ps1"
        result = subprocess.run(
            [SHELL, "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command",
             providers + "\n& '" + str(script).replace("'", "''") + "' -Port 45678"],
            capture_output=True, text=True, timeout=30, check=True,
        )
        return json.loads(result.stdout)

    def test_reports_listening_and_policy_without_sensitive_process_details(self):
        report = self.report("""
function Get-NetTCPConnection { [pscustomobject]@{
    LocalAddress='0.0.0.0'; LocalPort=45678; OwningProcess=42;
    CommandLine='secret-process-arguments'
} }
function Get-NetConnectionProfile { [pscustomobject]@{
    InterfaceIndex=3; NetworkCategory='Private'; IPv4Connectivity='LocalNetwork';
    Name='private-network-name'
} }
function Get-NetFirewallProfile { [pscustomobject]@{
    Name='Private'; Enabled=$true; DefaultInboundAction='Block';
    AllowInboundRules=$true; AllowLocalFirewallRules=$true
} }
""")
        self.assertEqual(report["listeners"][0]["OwningProcess"], 42)
        self.assertEqual(report["firewall_profiles"][0]["DefaultInboundAction"], "Block")
        self.assertEqual(report["unavailable"], [])
        self.assertNotIn("secret-process", json.dumps(report))
        self.assertNotIn("private-network-name", json.dumps(report))

    def test_provider_failures_are_redacted_and_do_not_claim_reachability(self):
        report = self.report("""
function Get-NetTCPConnection { throw 'sensitive query detail' }
function Get-NetConnectionProfile { throw 'sensitive query detail' }
function Get-NetFirewallProfile { throw 'sensitive query detail' }
""")
        self.assertEqual(len(report["unavailable"]), 3)
        self.assertEqual(report["listeners"], [])
        self.assertNotIn("sensitive query detail", json.dumps(report))
        self.assertIn("does not prove firewall reachability", report["guidance"])


if __name__ == "__main__":
    unittest.main()
