param(
    [string]$Toolchain,
    [string]$Supervisor,
    [switch]$InstalledToolchain
)

$ErrorActionPreference = "Stop"
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
$cargoTarget = Join-Path $repoRoot "build/codex-cargo-target"
$env:CARGO_TARGET_DIR = $cargoTarget
$env:CARGO_INCREMENTAL = "0"
$env:SDL_VIDEODRIVER = "dummy"
$env:STASIS_NETWORK_SUPERVISION_HANDLE = "stale-parent-control"
$env:STASIS_NETWORK_ADVERTISE_IPV4 = "127.0.0.1"

function Quote-ProcessArgument([string]$Value) {
    if ($Value.Contains('"')) {
        throw "test path contains an unsupported quote"
    }
    return '"' + $Value + '"'
}

function Invoke-BoundedProcess(
    [string]$FilePath,
    [string[]]$ArgumentList,
    [string]$WorkingDirectory,
    [int]$TimeoutMs
) {
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $FilePath
    $start.Arguments = (@($ArgumentList | ForEach-Object { Quote-ProcessArgument $_ }) -join " ")
    $start.WorkingDirectory = $WorkingDirectory
    $start.UseShellExecute = $false
    $process = [Diagnostics.Process]::Start($start)
    try {
        if (!$process.WaitForExit($TimeoutMs)) {
            & taskkill.exe /PID $process.Id /T /F 2>&1 | Out-Null
            $process.WaitForExit(5000) | Out-Null
            throw "bounded process timed out: $([IO.Path]::GetFileName($FilePath))"
        }
        return $process.ExitCode
    } finally {
        $process.Dispose()
    }
}

function Invoke-Cargo([string[]]$CargoArgs) {
    $arguments = @((Join-Path $repoRoot "tools/cargo_cache.py"), "run", "--", "cargo") + $CargoArgs
    if ((Invoke-BoundedProcess "python" $arguments $repoRoot 900000) -ne 0) {
        throw "cargo command failed"
    }
}

function Assert-Executable([string]$Path, [string]$Label) {
    if ([string]::IsNullOrWhiteSpace($Path) -or !(Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "$Label executable is unavailable"
    }
    return (Resolve-Path -LiteralPath $Path).Path
}

if ($InstalledToolchain) {
    if ([string]::IsNullOrWhiteSpace($Toolchain) -or [string]::IsNullOrWhiteSpace($Supervisor)) {
        throw "-InstalledToolchain requires both -Toolchain and -Supervisor"
    }
} else {
    if ([string]::IsNullOrWhiteSpace($Toolchain)) {
        Invoke-Cargo @("build", "-p", "stasis")
        $Toolchain = Join-Path $cargoTarget "debug/stasis.exe"
    }
    if ([string]::IsNullOrWhiteSpace($Supervisor)) {
        Invoke-Cargo @(
            "build", "-p", "stasis_network",
            "--features", "supervision-cli",
            "--bin", "stasis-network-supervise"
        )
        $Supervisor = Join-Path $cargoTarget "debug/stasis-network-supervise.exe"
    }
}
$Toolchain = Assert-Executable $Toolchain "Stasis toolchain"
$Supervisor = Assert-Executable $Supervisor "network supervisor"

$runId = [Guid]::NewGuid().ToString("N").Substring(0, 12)
$scratch = Join-Path $repoRoot "build/ns/$runId"
$processTemp = Join-Path $repoRoot ("build/nt/" + $runId.Substring(0, 8))
$authoritySource = Join-Path $repoRoot "tests/fixtures/network_supervision/authority"
$authorityProject = Join-Path $scratch "p"
New-Item -ItemType Directory -Path $scratch | Out-Null
New-Item -ItemType Directory -Path $processTemp -Force | Out-Null
$env:TEMP = $processTemp
$env:TMP = $processTemp
if (!$InstalledToolchain -and
    $env:STASIS_REQUIRE_SIGNED_EXECUTION -ne "1" -and
    $env:STASIS_SIGNING_MODE -ne "required") {
    Remove-Item Env:STASIS_AOT_SIGN_TOOL -ErrorAction SilentlyContinue
    $env:STASIS_SIGNING_LOCAL_RECORD = Join-Path $scratch "no-signing-record.json"
}
if (!$InstalledToolchain) {
    # The Cargo build may sit beside an unrelated previously built runtime DLL.
    # Isolate the source CLI so installed-toolchain identity checks cannot bind
    # it to that stale sibling while it builds a fresh development package.
    $isolatedToolchain = Join-Path $scratch "t/stasis.exe"
    New-Item -ItemType Directory -Path (Split-Path $isolatedToolchain -Parent) | Out-Null
    Copy-Item -LiteralPath $Toolchain -Destination $isolatedToolchain
    $Toolchain = $isolatedToolchain
}

Invoke-Cargo @("build", "-p", "stasis_network", "--example", "supervision_peer")
$builtPeer = Assert-Executable (Join-Path $cargoTarget "debug/examples/supervision_peer.exe") "network peer"
$peer = Join-Path $scratch "supervision_peer.exe"
Copy-Item -LiteralPath $builtPeer -Destination $peer
$peer = Assert-Executable $peer "isolated network peer"

Copy-Item -LiteralPath $authoritySource -Destination $authorityProject -Recurse
$stdlib = Join-Path $authorityProject ".stasis_cache/toolchain/src/stdlib"
New-Item -ItemType Directory -Path (Split-Path $stdlib -Parent) -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $repoRoot "src/stdlib") -Destination $stdlib -Recurse

Push-Location $authorityProject
try {
    $packageArgs = @("package", "--target", "desktop")
    if (!$InstalledToolchain) {
        $packageArgs += "--development-build"
    }
    $normalizedLauncher = Join-Path $repoRoot "tests/fixtures/network_supervision/run_process.py"
    $normalizedArgs = @($normalizedLauncher, $authorityProject, $Toolchain) + $packageArgs
    if ((Invoke-BoundedProcess "python" $normalizedArgs $authorityProject 900000) -ne 0) {
        throw "production authority package failed"
    }
} finally {
    Pop-Location
}

$package = Join-Path $authorityProject "dist/network_supervision_authority-desktop"
$authorityExeName = "network_supervision_authority.exe"
$packagedAuthority = Assert-Executable (Join-Path $package $authorityExeName) "packaged authority"
function Copy-Instance([string]$Name) {
    $instance = Join-Path $scratch $Name
    Copy-Item -LiteralPath $package -Destination $instance -Recurse
    return $instance
}

$activeSupervisors = [Collections.Generic.List[Diagnostics.Process]]::new()
function Start-Supervisor([string]$Instance, [string[]]$PeerArgs) {
    $authority = Join-Path $Instance $authorityExeName
    $arguments = @(
        "--authority", (Quote-ProcessArgument $authority),
        "--peer", (Quote-ProcessArgument $peer),
        "--startup-ms", "30000",
        "--action-ms", "30000",
        "--shutdown-ms", "10000"
    )
    if ($PeerArgs.Count -gt 0) {
        $arguments += "--"
        $arguments += $PeerArgs
    }
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $Supervisor
    $start.Arguments = $arguments -join " "
    $start.WorkingDirectory = $Instance
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $start
    if (!$process.Start()) {
        throw "network supervisor failed to start"
    }
    $activeSupervisors.Add($process)
    return $process
}

function Wait-Supervisors([Diagnostics.Process[]]$Processes, [int[]]$ExpectedCodes, [int]$TimeoutMs) {
    $deadline = [DateTime]::UtcNow.AddMilliseconds($TimeoutMs)
    while ($true) {
        $running = @($Processes | Where-Object { !$_.HasExited })
        if ($running.Count -eq 0) {
            break
        }
        if ([DateTime]::UtcNow -ge $deadline) {
            foreach ($process in $running) {
                $process.Kill()
            }
            throw "network supervision exceeded its outer bound"
        }
        Start-Sleep -Milliseconds 50
    }
    for ($index = 0; $index -lt $Processes.Count; $index += 1) {
        $process = $Processes[$index]
        $stdout = $process.StandardOutput.ReadToEnd()
        $stderr = $process.StandardError.ReadToEnd()
        $captured = $stdout + $stderr
        if ($captured -match '#secret=' -or $captured -match 'stasis-resume-v1') {
            throw "network supervision output exposed credential material"
        }
        if ($process.ExitCode -ne $ExpectedCodes[$index]) {
            throw "network supervisor returned unexpected exit code $($process.ExitCode)"
        }
        $activeSupervisors.Remove($process) | Out-Null
        $process.Dispose()
    }
}

try {
    $first = Copy-Instance "concurrent-a"
    $second = Copy-Instance "concurrent-b"
    $concurrent = @(
        (Start-Supervisor $first @()),
        (Start-Supervisor $second @())
    )
    Wait-Supervisors $concurrent @(0, 0) 75000

    $crashInstance = Copy-Instance "peer-crash"
    $crash = Start-Supervisor $crashInstance @("--fail-after-invite")
    Wait-Supervisors @($crash) @(1) 75000

    $authorityCrashInstance = Copy-Instance "authority-crash"
    $authorityCrash = Start-Supervisor $authorityCrashInstance @("--crash-authority")
    Wait-Supervisors @($authorityCrash) @(1) 75000

    $actionTimeoutInstance = Copy-Instance "action-timeout"
    $actionTimeout = Start-Supervisor $actionTimeoutInstance @("--hang-after-invite")
    Wait-Supervisors @($actionTimeout) @(1) 75000

    $startup = [Diagnostics.ProcessStartInfo]::new()
    $startup.FileName = $Supervisor
    $startup.Arguments = @(
        "--authority", (Quote-ProcessArgument $peer),
        "--peer", (Quote-ProcessArgument $peer),
        "--startup-ms", "500",
        "--action-ms", "30000",
        "--shutdown-ms", "10000"
    ) -join " "
    $startup.WorkingDirectory = $scratch
    $startup.UseShellExecute = $false
    $startup.CreateNoWindow = $true
    $startup.RedirectStandardOutput = $true
    $startup.RedirectStandardError = $true
    $startupProcess = [Diagnostics.Process]::new()
    $startupProcess.StartInfo = $startup
    if (!$startupProcess.Start()) {
        throw "startup-bound supervisor failed to start"
    }
    $activeSupervisors.Add($startupProcess)
    Wait-Supervisors @($startupProcess) @(1) 16000

    $supervisorCrashInstance = Copy-Instance "supervisor-crash"
    $supervisorCrash = Start-Supervisor $supervisorCrashInstance @("--hang-after-invite")
    $childrenDeadline = [DateTime]::UtcNow.AddSeconds(10)
    do {
        $ownedAuthority = @(Get-Process -Name "network_supervision_authority" -ErrorAction SilentlyContinue |
            Where-Object { $_.Path -and $_.Path.StartsWith($supervisorCrashInstance, [StringComparison]::OrdinalIgnoreCase) })
        $ownedPeer = @(Get-Process -Name "supervision_peer" -ErrorAction SilentlyContinue |
            Where-Object { $_.Path -and $_.Path.Equals($peer, [StringComparison]::OrdinalIgnoreCase) })
        if ($ownedAuthority.Count -eq 1 -and $ownedPeer.Count -eq 1) {
            break
        }
        Start-Sleep -Milliseconds 50
    } while ([DateTime]::UtcNow -lt $childrenDeadline)
    if ($ownedAuthority.Count -ne 1 -or $ownedPeer.Count -ne 1) {
        throw "supervisor-crash case did not reach the active peer stage"
    }
    $supervisorCrash.Kill()
    if (!$supervisorCrash.WaitForExit(5000)) {
        throw "supervisor-crash process did not terminate"
    }
    $activeSupervisors.Remove($supervisorCrash) | Out-Null
    $supervisorCrash.Dispose()
    $jobCleanupDeadline = [DateTime]::UtcNow.AddSeconds(5)
    do {
        $ownedAuthority = @(Get-Process -Name "network_supervision_authority" -ErrorAction SilentlyContinue |
            Where-Object { $_.Path -and $_.Path.StartsWith($supervisorCrashInstance, [StringComparison]::OrdinalIgnoreCase) })
        $ownedPeer = @(Get-Process -Name "supervision_peer" -ErrorAction SilentlyContinue |
            Where-Object { $_.Path -and $_.Path.Equals($peer, [StringComparison]::OrdinalIgnoreCase) })
        if ($ownedAuthority.Count -eq 0 -and $ownedPeer.Count -eq 0) {
            break
        }
        Start-Sleep -Milliseconds 50
    } while ([DateTime]::UtcNow -lt $jobCleanupDeadline)
    if ($ownedAuthority.Count -ne 0 -or $ownedPeer.Count -ne 0) {
        throw "supervisor job left a child process after supervisor termination"
    }

    $teardownDeadline = [DateTime]::UtcNow.AddSeconds(5)
    do {
        $remaining = @(Get-Process -Name "network_supervision_authority" -ErrorAction SilentlyContinue |
            Where-Object { $_.Path -and $_.Path.StartsWith($scratch, [StringComparison]::OrdinalIgnoreCase) })
        $remainingPeer = @(Get-Process -Name "supervision_peer" -ErrorAction SilentlyContinue |
            Where-Object { $_.Path -and $_.Path.Equals($peer, [StringComparison]::OrdinalIgnoreCase) })
        if ($remaining.Count -eq 0 -and $remainingPeer.Count -eq 0) {
            break
        }
        Start-Sleep -Milliseconds 50
    } while ([DateTime]::UtcNow -lt $teardownDeadline)
    if ($remaining.Count -ne 0 -or $remainingPeer.Count -ne 0) {
        throw "supervised child process remained after teardown"
    }
} finally {
    foreach ($process in @($activeSupervisors)) {
        if (!$process.HasExited) {
            $process.Kill()
            $process.WaitForExit(5000) | Out-Null
        }
        $process.Dispose()
    }
}

Write-Output "NETWORK_SUPERVISION_ACCEPTANCE_OK"
