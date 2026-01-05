$ErrorActionPreference = "Stop"

$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$sample = Join-Path $root "samples\hotstate_tick_watch.stasis"
$outLog = Join-Path $root "build\ci_hotswap_timing.out.log"
$errLog = Join-Path $root "build\ci_hotswap_timing.err.log"

if (-not (Test-Path $sample)) {
    Write-Error "Sample not found: $sample"
    exit 1
}

$runnerExe = $env:STASIS_CRANELIFT_RUNNER_EXE
if (-not $runnerExe) {
    $candidate = Join-Path $root "runtime\build\bin\Release\stasis_runner.exe"
    if (Test-Path $candidate) {
        $runnerExe = $candidate
        $env:STASIS_CRANELIFT_RUNNER_EXE = $runnerExe
    }
}

if (-not $runnerExe -or -not (Test-Path $runnerExe)) {
    Write-Error "stasis_runner.exe not found. Build the runtime or set STASIS_CRANELIFT_RUNNER_EXE."
    exit 1
}

$aotTool = Join-Path $root "tools\cranelift-aot\target\release\stasis-cranelift-aot.exe"
if (Test-Path $aotTool) {
    $env:STASIS_CRANELIFT_AOT = $aotTool
    $env:STASIS_CRANELIFT_AOT_SERVER = "1"
    $env:STASIS_CRANELIFT_RUNNER_SERVER = "1"
}
$env:STASIS_SUPPRESS_WARNINGS = "1"
$env:STASIS_HOTSWAP_KEEP_OLD = "1"

New-Item -ItemType Directory -Force (Join-Path $root "build") | Out-Null
Remove-Item $outLog, $errLog -ErrorAction SilentlyContinue

function Show-LogTail([string]$label, [string]$path) {
    if (Test-Path $path) {
        $info = Get-Item $path
        Write-Host ("{0} size: {1} bytes" -f $label, $info.Length)
        Get-Content $path | Select-Object -Last 50 | ForEach-Object { Write-Host $_ }
    } else {
        Write-Host ("{0} log missing." -f $label)
    }
}

function Fail([string]$message) {
    Write-Host $message
    Show-LogTail "ERR" $errLog
    Show-LogTail "OUT" $outLog
    if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force }
    exit 1
}

Write-Host ("Runner: {0}" -f $runnerExe)
if (Test-Path $aotTool) {
    Write-Host ("AOT: {0}" -f $aotTool)
}
Write-Host ("Sample: {0}" -f $sample)

$cliProject = Join-Path $root "Stasis.Cli\Stasis.Cli.csproj"
$cmd = "dotnet"
$args = @("run", "--no-build", "--configuration", "Release", "--project", $cliProject, "--", "run", $sample, "--backend", "cranelift", "--watch", "--fps", "30")
$proc = Start-Process -FilePath $cmd -ArgumentList $args -WorkingDirectory $root -RedirectStandardOutput $outLog -RedirectStandardError $errLog -PassThru
Write-Host ("Started stasis: pid={0}" -f $proc.Id)

function Wait-ForLine([string]$pattern, [int]$timeoutSeconds, [Diagnostics.Process]$proc) {
    $deadline = (Get-Date).AddSeconds($timeoutSeconds)
    $nextLog = (Get-Date).AddSeconds(15)
    while ((Get-Date) -lt $deadline) {
        if ($proc.HasExited) {
            return $false
        }
        if (Test-Path $errLog) {
            $lines = Get-Content $errLog -ErrorAction SilentlyContinue
            if ($lines | Where-Object { $_ -match $pattern }) {
                return $true
            }
        }
        if ((Get-Date) -ge $nextLog) {
            $elapsed = [int]((Get-Date) - ($deadline.AddSeconds(-$timeoutSeconds))).TotalSeconds
            Write-Host ("Waiting for {0}... {1}s" -f $pattern, $elapsed)
            $nextLog = (Get-Date).AddSeconds(15)
        }
        Start-Sleep -Seconds 2
    }
    return $false
}

Start-Sleep -Seconds 30
if ($proc.HasExited) {
    Fail ("stasis run exited before swap triggers (exit={0})." -f $proc.ExitCode)
}
for ($i = 0; $i -lt 5; $i++) {
    (Get-Item $sample).LastWriteTime = Get-Date
    Start-Sleep -Seconds 3
}

if (-not (Wait-ForLine "HOTSWAP ok:" 120 $proc)) {
    Fail "Timed out waiting for HOTSWAP output."
}

Start-Sleep -Seconds 5
if (-not $proc.HasExited) {
    Stop-Process -Id $proc.Id -Force
}
Start-Sleep -Milliseconds 500

if (Test-Path $errLog) {
    $errLines = Get-Content $errLog
} else {
    $errLines = @()
}
$parseAttempts = 0
do {
    $parseAttempts++
    $layoutWarnings = $errLines | Where-Object { $_ -match "^HOTSWAP warning: state layout changed" }
    $reloads = @()
    $swaps = @()
    foreach ($line in $errLines) {
        if ($line -match "HOTRELOAD phases\\(ms\\):") {
            $fields = @{}
            $parts = $line -replace ".*HOTRELOAD phases\\(ms\\):\\s*", "" -split "\\s+"
            foreach ($p in $parts) {
                if ($p -match "^(\\w+)=([0-9]+)$") { $fields[$matches[1]] = [int]$matches[2] }
            }
            if ($fields.ContainsKey("total")) { $reloads += $fields }
        } elseif ($line -match "HOTSWAP ok:") {
            $fields = @{}
            $parts = $line -replace ".*HOTSWAP ok:\\s*", "" -split "\\s+"
            foreach ($p in $parts) {
                if ($p -match "^(\\w+)=([0-9]+)(us)?$") { $fields[$matches[1]] = [int]$matches[2] }
            }
            if ($fields.ContainsKey("load")) { $swaps += $fields }
        }
    }
    if ($swaps.Count -eq 0 -and $parseAttempts -lt 5) {
        Start-Sleep -Seconds 2
        $errLines = if (Test-Path $errLog) { Get-Content $errLog } else { @() }
    }
} while ($swaps.Count -eq 0 -and $parseAttempts -lt 5)

if ($swaps.Count -eq 0) {
    Fail "No HOTSWAP timings captured."
}
if ($layoutWarnings.Count -gt 0) {
    Write-Host "State layout warning detected during hot-swap (continuing to report timings)."
}

function Summarize([string]$label, [int[]]$values, [string]$unit) {
    if ($values.Count -eq 0) { return "${label}: n/a" }
    $min = ($values | Measure-Object -Minimum).Minimum
    $max = ($values | Measure-Object -Maximum).Maximum
    $avg = [math]::Round(($values | Measure-Object -Average).Average, 2)
    return "${label}: min=${min}${unit} avg=${avg}${unit} max=${max}${unit}"
}

$reloadTotals = $reloads | ForEach-Object { $_["total"] }
$reloadLinks = $reloads | ForEach-Object { $_["link"] }
$swapLoads = $swaps | ForEach-Object { $_["load"] }
$swapSaves = $swaps | ForEach-Object { $_["save"] }
$swapRestores = $swaps | ForEach-Object { $_["restore"] }

Write-Host ("Reloads: {0} Swaps: {1}" -f $reloads.Count, $swaps.Count)
Write-Host (Summarize "HOTRELOAD total" $reloadTotals "ms")
Write-Host (Summarize "HOTRELOAD link" $reloadLinks "ms")
Write-Host (Summarize "HOTSWAP load" $swapLoads "us")
Write-Host (Summarize "HOTSWAP save" $swapSaves "us")
Write-Host (Summarize "HOTSWAP restore" $swapRestores "us")
