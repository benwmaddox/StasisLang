# Read-only diagnostics. Never accept or print a join URL or process command line.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateRange(1, 65535)]
    [int]$Port
)

$ErrorActionPreference = 'Stop'
$report = [ordered]@{
    schema = 'stasis.desktop_network_diagnostics.v1'
    port = $Port
    listeners = @()
    connection_profiles = @()
    firewall_profiles = @()
    unavailable = @()
    guidance = 'Use the same trusted LAN. Check the advertised IPv4 address and client isolation. A listening socket does not prove firewall reachability.'
}

try {
    $report.listeners = @(Get-NetTCPConnection -State Listen -LocalPort $Port -ErrorAction Stop |
        Select-Object LocalAddress, LocalPort, OwningProcess)
} catch {
    $report.unavailable += 'listener_query_failed_or_no_listener'
}
try {
    $report.connection_profiles = @(Get-NetConnectionProfile -ErrorAction Stop |
        Select-Object InterfaceIndex, NetworkCategory, IPv4Connectivity)
} catch {
    $report.unavailable += 'connection_profiles_unavailable'
}
try {
    $report.firewall_profiles = @(Get-NetFirewallProfile -PolicyStore ActiveStore -ErrorAction Stop |
        Select-Object Name, Enabled, DefaultInboundAction, AllowInboundRules, AllowLocalFirewallRules)
} catch {
    $report.unavailable += 'firewall_profiles_unavailable'
}

$report | ConvertTo-Json -Depth 4
