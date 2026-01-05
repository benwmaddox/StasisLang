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

$cmd = Join-Path $root "stasis.bat"
$args = @("run", $sample, "--backend", "cranelift", "--watch", "--fps", "30")
$proc = Start-Process -FilePath $cmd -ArgumentList $args -RedirectStandardOutput $outLog -RedirectStandardError $errLog -PassThru

function Wait-ForHotreload([int]$timeoutSeconds) {
    $deadline = (Get-Date).AddSeconds($timeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        if (Test-Path $errLog) {
            $lines = Get-Content $errLog -ErrorAction SilentlyContinue
            if ($lines | Where-Object { $_ -match "^HOTRELOAD phases\\(ms\\):" }) {
                return $true
            }
        }
        Start-Sleep -Seconds 2
    }
    return $false
}

if (-not (Wait-ForHotreload 90)) {
    Write-Error "Timed out waiting for HOTRELOAD output. See $errLog."
    if (Test-Path $errLog) {
        Get-Content $errLog | Select-Object -Last 50 | ForEach-Object { Write-Host $_ }
    }
    if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force }
    exit 1
}
for ($i = 0; $i -lt 5; $i++) {
    (Get-Item $sample).LastWriteTime = Get-Date
    Start-Sleep -Seconds 3
}

if (-not $proc.HasExited) {
    Stop-Process -Id $proc.Id -Force
}
Start-Sleep -Milliseconds 500

$errLines = if (Test-Path $errLog) { Get-Content $errLog } else { @() }
$layoutWarnings = $errLines | Where-Object { $_ -match "^HOTSWAP warning: state layout changed" }
$reloads = @()
$swaps = @()
foreach ($line in $errLines) {
    if ($line -match "^HOTRELOAD phases\\(ms\\):") {
        $fields = @{}
        $parts = $line -replace "^HOTRELOAD phases\\(ms\\):\\s*", "" -split "\\s+"
        foreach ($p in $parts) {
            if ($p -match "^(\\w+)=([0-9]+)$") { $fields[$matches[1]] = [int]$matches[2] }
        }
        if ($fields.ContainsKey("total")) { $reloads += $fields }
    } elseif ($line -match "^HOTSWAP ok:") {
        $fields = @{}
        $parts = $line -replace "^HOTSWAP ok:\\s*", "" -split "\\s+"
        foreach ($p in $parts) {
            if ($p -match "^(\\w+)=([0-9]+)(us)?$") { $fields[$matches[1]] = [int]$matches[2] }
        }
        if ($fields.ContainsKey("load")) { $swaps += $fields }
    }
}

if ($swaps.Count -eq 0) {
    Write-Error "No HOTSWAP timings captured. See $errLog."
    if (Test-Path $errLog) {
        Get-Content $errLog | Select-Object -Last 50 | ForEach-Object { Write-Host $_ }
    }
    exit 1
}
if ($layoutWarnings.Count -gt 0) {
    Write-Error "State layout warning detected during hot-swap (this is treated as a failure). See $errLog."
    if (Test-Path $errLog) {
        Get-Content $errLog | Select-Object -Last 50 | ForEach-Object { Write-Host $_ }
    }
    exit 1
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
