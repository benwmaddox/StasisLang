param(
    [string]$Toolchain,
    [switch]$InstalledToolchain
)

$ErrorActionPreference = "Stop"
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
$runId = [Guid]::NewGuid().ToString("N").Substring(0, 12)
$scratch = Join-Path $repoRoot "build/nc/$runId"
$project = Join-Path $scratch "p"
$hostProcess = $null
$clientProcess = $null
$previousRustFlags = $env:RUSTFLAGS
$previousCmakeGenerator = $env:CMAKE_GENERATOR

function Invoke-Cargo([string[]]$CargoArgs) {
    & python (Join-Path $repoRoot "tools/cargo_cache.py") run -- cargo @CargoArgs
    if ($LASTEXITCODE -ne 0) { throw "Cargo command failed" }
}

function Wait-BoundedProcess([Diagnostics.Process]$Process, [int]$TimeoutMs, [string]$Label) {
    if (!$Process.WaitForExit($TimeoutMs)) {
        $Process.Kill($true)
        $Process.WaitForExit(5000) | Out-Null
        throw "$Label timed out"
    }
}

Push-Location $repoRoot
try {
    if ([string]::IsNullOrWhiteSpace($Toolchain)) {
        if ($InstalledToolchain) { throw "-InstalledToolchain requires -Toolchain" }
        Invoke-Cargo @("build", "-p", "stasis")
        $metadataJson = & python (Join-Path $repoRoot "tools/cargo_cache.py") run -- cargo metadata --no-deps --format-version 1
        if ($LASTEXITCODE -ne 0) { throw "Cargo target metadata query failed" }
        $cargoTarget = ($metadataJson | ConvertFrom-Json).target_directory
        $Toolchain = Join-Path $cargoTarget "debug/stasis.exe"
    } else {
        $Toolchain = (Resolve-Path -LiteralPath $Toolchain).Path
    }
    if (!(Test-Path -LiteralPath $Toolchain -PathType Leaf)) {
        throw "Stasis toolchain executable is unavailable"
    }

    if (!$InstalledToolchain) {
        $env:RUSTFLAGS = (($previousRustFlags, "-C target-feature=+crt-static") -join " ").Trim()
        Invoke-Cargo @("build", "-p", "stasis_network", "--release")
        $env:RUSTFLAGS = $previousRustFlags
    }
    New-Item -ItemType Directory -Path $scratch | Out-Null
    if (!$InstalledToolchain) {
        $isolatedToolchain = Join-Path $scratch "toolchain/stasis.exe"
        New-Item -ItemType Directory -Path (Split-Path $isolatedToolchain -Parent) | Out-Null
        Copy-Item -LiteralPath $Toolchain -Destination $isolatedToolchain
        $Toolchain = $isolatedToolchain
    }

    $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio/Installer/vswhere.exe"
    if (!(Test-Path -LiteralPath $vswhere)) {
        throw "vswhere.exe was not found; install the Visual Studio C++ desktop workload"
    }
    $installation = (& $vswhere -latest -products * `
        -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
        -property installationPath | Select-Object -First 1)
    if (!$installation) { throw "Visual Studio C++ desktop workload was not found" }
    $vcvars = Join-Path $installation "VC/Auxiliary/Build/vcvars64.bat"
    $hostSource = Join-Path $repoRoot "runtime/tests/stasis_packaged_network_client_host.c"
    if ($InstalledToolchain) {
        $toolchainRoot = Split-Path $Toolchain -Parent
        $include = Join-Path $toolchainRoot "desktop/network/include"
        $library = Join-Path $toolchainRoot "desktop/network/windows-x86_64/stasis_network.lib"
    } else {
        $include = Join-Path $repoRoot "crates/stasis_network/include"
        $library = Join-Path $cargoTarget "release/stasis_network.lib"
    }
    $hostObject = Join-Path $scratch "packaged_network_client_host.obj"
    $hostExecutable = Join-Path $scratch "packaged_network_client_host.exe"
    foreach ($required in @($vcvars, $hostSource, $library)) {
        if (!(Test-Path -LiteralPath $required)) { throw "required package test input is missing" }
    }
    $compile = 'call "{0}" >nul && cl /nologo /W4 /WX /MT /I"{1}" "{2}" /Fo:"{3}" /Fe:"{4}" "{5}" ws2_32.lib iphlpapi.lib bcrypt.lib userenv.lib ntdll.lib' -f `
        $vcvars, $include, $hostSource, $hostObject, $hostExecutable, $library
    & cmd.exe /d /c $compile
    if ($LASTEXITCODE -ne 0) { throw "packaged network client host failed to compile" }

    Copy-Item -LiteralPath (Join-Path $repoRoot "tests/fixtures/windows_network_client_package") `
        -Destination $project -Recurse
    $vendorDir = Join-Path $project "vendor/stasis/stdlib"
    New-Item -ItemType Directory -Path $vendorDir -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $repoRoot "src/stdlib/network_client.stasis") `
        -Destination (Join-Path $vendorDir "network_client.stasis")
    Push-Location $project
    try {
        if ([string]::IsNullOrWhiteSpace($env:CMAKE_GENERATOR)) {
            $env:CMAKE_GENERATOR = "Ninja"
        }
        $packageArgs = @("package", "--target", "desktop")
        if (!$InstalledToolchain) { $packageArgs += "--development-build" }
        & $Toolchain @packageArgs
        if ($LASTEXITCODE -ne 0) { throw "Windows network client package generation failed" }
    } finally {
        Pop-Location
    }

    $joinFile = Join-Path $scratch "join-url.txt"
    $hostInfo = [Diagnostics.ProcessStartInfo]::new()
    $hostInfo.FileName = $hostExecutable
    $hostInfo.Arguments = '"' + $joinFile + '"'
    $hostInfo.WorkingDirectory = $scratch
    $hostInfo.UseShellExecute = $false
    $hostInfo.CreateNoWindow = $true
    $hostInfo.RedirectStandardOutput = $true
    $hostInfo.RedirectStandardError = $true
    $hostProcess = [Diagnostics.Process]::Start($hostInfo)
    $hostStdout = $hostProcess.StandardOutput.ReadToEndAsync()
    $hostStderr = $hostProcess.StandardError.ReadToEndAsync()

    $joinDeadline = [DateTime]::UtcNow.AddSeconds(30)
    while (!(Test-Path -LiteralPath $joinFile -PathType Leaf)) {
        if ($hostProcess.HasExited) { throw "network host exited before publishing readiness" }
        if ([DateTime]::UtcNow -ge $joinDeadline) { throw "network host readiness timed out" }
        Start-Sleep -Milliseconds 25
    }
    $privateJoinUrl = [IO.File]::ReadAllText($joinFile)
    Remove-Item -LiteralPath $joinFile -Force
    if ([string]::IsNullOrWhiteSpace($privateJoinUrl) -or !$privateJoinUrl.Contains("#secret=")) {
        throw "network host published an invalid private join URL"
    }

    $packageExecutable = Join-Path $project "dist/windows_network_client_package-desktop/windows_network_client_package.exe"
    if (!(Test-Path -LiteralPath $packageExecutable -PathType Leaf)) {
        throw "generated Windows client package executable is unavailable"
    }
    $clientInfo = [Diagnostics.ProcessStartInfo]::new()
    $clientInfo.FileName = $packageExecutable
    $clientInfo.WorkingDirectory = Split-Path $packageExecutable -Parent
    $clientInfo.UseShellExecute = $false
    $clientInfo.CreateNoWindow = $true
    $clientInfo.RedirectStandardOutput = $true
    $clientInfo.RedirectStandardError = $true
    $clientInfo.Environment["STASIS_NETWORK_JOIN_URL"] = $privateJoinUrl
    $clientInfo.Environment["SDL_VIDEODRIVER"] = "dummy"
    $clientInfo.Environment["SDL_RENDER_DRIVER"] = "software"
    $clientInfo.Environment["SDL_AUDIODRIVER"] = "dummy"
    $clientProcess = [Diagnostics.Process]::Start($clientInfo)
    $clientStdout = $clientProcess.StandardOutput.ReadToEndAsync()
    $clientStderr = $clientProcess.StandardError.ReadToEndAsync()
    Wait-BoundedProcess $clientProcess 60000 "generated Windows network client package"
    if ($clientProcess.ExitCode -ne 83) {
        throw "generated Windows network client returned an unexpected contract code"
    }
    Wait-BoundedProcess $hostProcess 10000 "packaged network client host shutdown"
    if ($hostProcess.ExitCode -ne 0) { throw "packaged network client host contract failed" }

    $clientLog = ($clientStdout.Result + $clientStderr.Result)
    $hostLog = ($hostStdout.Result + $hostStderr.Result)
    if ($clientLog.Contains($privateJoinUrl) -or $clientLog.Contains("#secret=") -or
        $hostLog.Contains($privateJoinUrl) -or $hostLog.Contains("#secret=")) {
        throw "native client package logs exposed private join material"
    }
    if (!$hostLog.Contains("join, send/poll, checkpoint/resume, shutdown passed")) {
        throw "packaged network client host did not report the complete contract"
    }
    Write-Output "Windows generated network client package: join, send/poll, checkpoint/resume, shutdown, and credential-redacted logs passed"
} finally {
    $env:RUSTFLAGS = $previousRustFlags
    $env:CMAKE_GENERATOR = $previousCmakeGenerator
    if ($clientProcess -ne $null -and !$clientProcess.HasExited) { $clientProcess.Kill($true) }
    if ($hostProcess -ne $null -and !$hostProcess.HasExited) { $hostProcess.Kill($true) }
    if ($clientProcess -ne $null) { $clientProcess.Dispose() }
    if ($hostProcess -ne $null) { $hostProcess.Dispose() }
    if (Test-Path -LiteralPath $scratch) {
        Remove-Item -LiteralPath $scratch -Recurse -Force -ErrorAction SilentlyContinue
    }
    Pop-Location
}
