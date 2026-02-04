param(
    [int]$Iterations = 25,
    [int]$Fps = 60,
    [int]$SleepAfterEditMs = 250,
    [int]$SwapTimeoutMs = 5000
)

$ErrorActionPreference = "Stop"

$root = Resolve-Path (Join-Path $PSScriptRoot "..")
Set-Location $root

$sample = Join-Path $root "examples\\bytecode_counter.stasis"
if (-not (Test-Path $sample)) {
    Write-Error "Sample not found: $sample"
    exit 1
}

$originalSample = Get-Content -Raw -Path $sample
$sampleTouched = $false

$buildDir = Join-Path $root "build"
New-Item -ItemType Directory -Force $buildDir | Out-Null

$outLog = Join-Path $buildDir "hotswap_bytecode_counter.out.log"
$errLog = Join-Path $buildDir "hotswap_bytecode_counter.err.log"
Remove-Item $outLog, $errLog -ErrorAction SilentlyContinue

function Read-TextFileShared {
    param([string]$Path)
    $fs = $null
    $sr = $null
    try {
        $fs = [System.IO.File]::Open($Path, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
        $sr = New-Object System.IO.StreamReader($fs, [System.Text.Encoding]::UTF8, $true)
        return $sr.ReadToEnd()
    }
    finally {
        if ($sr) { try { $sr.Dispose() } catch {} }
        if ($fs) { try { $fs.Dispose() } catch {} }
    }
}

function Wait-ForText {
    param(
        [string]$Path,
        [string]$Needle,
        [int]$TimeoutMs
    )

    $deadline = [DateTime]::UtcNow.AddMilliseconds($TimeoutMs)
    while ([DateTime]::UtcNow -lt $deadline) {
        if (Test-Path $Path) {
            try {
                $text = Read-TextFileShared -Path $Path
                if ($text.IndexOf($Needle, [System.StringComparison]::Ordinal) -ge 0) { return $true }
            } catch {
                # ignore
            }
        }
        Start-Sleep -Milliseconds 25
    }
    return $false
}

function Wait-ForSwapCountIncrease {
    param(
        [string]$Path,
        [string]$Pattern,
        [int]$PrevCount,
        [int]$TimeoutMs
    )

    $deadline = [DateTime]::UtcNow.AddMilliseconds($TimeoutMs)
    while ([DateTime]::UtcNow -lt $deadline) {
        if (Test-Path $Path) {
            try {
                $text = Read-TextFileShared -Path $Path
                $count = ([regex]::Matches($text, $Pattern)).Count
                if ($count -gt $PrevCount) { return $count }
            } catch {
                # ignore
            }
        }
        Start-Sleep -Milliseconds 25
    }
    return -1
}

function Parse-HotSwapSummary([string[]]$lines) {
    $swaps = @()
    foreach ($line in $lines) {
        if ($line -like "*HOTSWAP(ms):*") {
            $fields = @{}
            foreach ($m in [regex]::Matches($line, "([A-Za-z_]+)=(-?[0-9]+(?:\.[0-9]+)?)")) {
                $fields[$m.Groups[1].Value] = [double]$m.Groups[2].Value
            }
            if ($fields.ContainsKey("load")) { $swaps += $fields }
        }
    }
    return $swaps
}

function Touch-Sample([string]$Path, [string]$Tag) {
    $text = "`n// hotswap timing bytecode_counter $Tag $([DateTime]::UtcNow.Ticks)`n"
    Add-Content -Path $Path -Value $text -Encoding ascii
}

Write-Host ("Sample: {0}" -f $sample)

$cliArgs = @("run", $sample, "--watch", "--backend", "bytecode", "--module", "bc", "--fps", [string]$Fps)

$cliExe = Join-Path $root "Stasis.Cli\\bin\\Release\\net9.0\\Stasis.Cli.exe"
Write-Host "Building CLI (Release)..."
dotnet build -c Release | Out-Null
if (-not (Test-Path $cliExe)) {
    throw "CLI not found: $cliExe"
}

$proc = Start-Process -FilePath $cliExe -ArgumentList $cliArgs -WorkingDirectory $root -RedirectStandardOutput $outLog -RedirectStandardError $errLog -PassThru
try {
    if (-not (Wait-ForText -Path $outLog -Needle "HOTSWAP(ms):" -TimeoutMs 60000)) {
        throw "Timed out waiting for initial HOTSWAP(ms) marker."
    }

    function Wait-ForFileLengthIncrease {
        param(
            [string]$Path,
            [long]$PrevLength,
            [int]$TimeoutMs
        )

        $deadline = [DateTime]::UtcNow.AddMilliseconds($TimeoutMs)
        while ([DateTime]::UtcNow -lt $deadline) {
            if (Test-Path $Path) {
                try {
                    $len = (Get-Item $Path).Length
                    if ($len -gt $PrevLength) { return $len }
                } catch {
                    # ignore
                }
            }
            Start-Sleep -Milliseconds 25
        }
        return -1
    }

    $prevLen = 0
    if (Test-Path $outLog) { try { $prevLen = (Get-Item $outLog).Length } catch {} }

    # Arm the watcher with a known edit so we don't get tripped up by any initial spurious change events.
    $armed = $false
    for ($w = 0; $w -lt 5; $w++) {
        $prevLen = 0
        if (Test-Path $outLog) { try { $prevLen = (Get-Item $outLog).Length } catch {} }

        Touch-Sample -Path $sample -Tag ("warmup{0}" -f $w)
        $sampleTouched = $true
        Start-Sleep -Milliseconds $SleepAfterEditMs

        $len = Wait-ForFileLengthIncrease -Path $outLog -PrevLength $prevLen -TimeoutMs $SwapTimeoutMs
        if ($len -gt 0) {
            $prevLen = $len
            $armed = $true
            break
        }
    }

    if (-not $armed) {
        $errText = ""
        if (Test-Path $errLog) { try { $errText = Read-TextFileShared $errLog } catch {} }
        throw "Timed out waiting for HOTSWAP(ms) after warmup edit(s). stderr=`n$errText"
    }

    for ($i = 0; $i -lt $Iterations; $i++) {
        $prevLen = 0
        if (Test-Path $outLog) { try { $prevLen = (Get-Item $outLog).Length } catch {} }

        Touch-Sample -Path $sample -Tag $i
        Start-Sleep -Milliseconds $SleepAfterEditMs

        $len = Wait-ForFileLengthIncrease -Path $outLog -PrevLength $prevLen -TimeoutMs $SwapTimeoutMs
        if ($len -lt 0) {
            $errText = ""
            if (Test-Path $errLog) { try { $errText = Read-TextFileShared $errLog } catch {} }
            throw "Timed out waiting for HOTSWAP(ms) after edit $i. stderr=`n$errText"
        }
        $prevLen = $len
    }

    $lines = @()
    if (Test-Path $outLog) {
        $lines = (Read-TextFileShared $outLog) -split "`n"
    }

    $initialTotal = $null
    $totals = @()
    $latencies = @()
    $loads = @()

    foreach ($line in $lines) {
        if ($line -notlike "*HOTSWAP(ms):*") { continue }

        $mTotal = [regex]::Match($line, "total=(-?[0-9]+(?:\.[0-9]+)?)")
        $mLatency = [regex]::Match($line, "latency=(-?[0-9]+(?:\.[0-9]+)?)")
        $mLoad = [regex]::Match($line, "load=(-?[0-9]+(?:\.[0-9]+)?)")
        if (-not ($mTotal.Success -and $mLatency.Success -and $mLoad.Success)) { continue }

        if ($env:STASIS_BC_TIMING_DEBUG -eq "1") {
            Write-Host ("DBG: totalRaw='{0}' latencyRaw='{1}' loadRaw='{2}'" -f $mTotal.Groups[1].Value, $mLatency.Groups[1].Value, $mLoad.Groups[1].Value)
        }

        $t = [double]$mTotal.Groups[1].Value
        $lat = [double]$mLatency.Groups[1].Value
        $load = [double]$mLoad.Groups[1].Value

        if ($lat -lt 0 -or $load -lt 0) {
            if ($initialTotal -eq $null) { $initialTotal = $t }
            continue
        }

        $totals += $t
        $latencies += $lat
        $loads += $load
    }

    if ($initialTotal -ne $null) {
        $ci = [System.Globalization.CultureInfo]::InvariantCulture
        Write-Host ("Initial compile: {0}ms" -f ([double]$initialTotal).ToString("0.###", $ci))
    }

    $ci = [System.Globalization.CultureInfo]::InvariantCulture
    $tStats = $totals | Measure-Object -Minimum -Maximum -Average
    $latStats = $latencies | Measure-Object -Minimum -Maximum -Average
    $loadStats = $loads | Measure-Object -Minimum -Maximum -Average

    $tMin = ([math]::Round([double]$tStats.Minimum, 3)).ToString("0.###", $ci)
    $tAvg = ([math]::Round([double]$tStats.Average, 3)).ToString("0.###", $ci)
    $tMax = ([math]::Round([double]$tStats.Maximum, 3)).ToString("0.###", $ci)
    Write-Host ("HOTSWAP total: min={0}ms avg={1}ms max={2}ms" -f $tMin, $tAvg, $tMax)

    $latMin = ([math]::Round([double]$latStats.Minimum, 3)).ToString("0.###", $ci)
    $latAvg = ([math]::Round([double]$latStats.Average, 3)).ToString("0.###", $ci)
    $latMax = ([math]::Round([double]$latStats.Maximum, 3)).ToString("0.###", $ci)
    Write-Host ("HOTSWAP latency: min={0}ms avg={1}ms max={2}ms" -f $latMin, $latAvg, $latMax)

    $loadMin = ([math]::Round([double]$loadStats.Minimum, 3)).ToString("0.###", $ci)
    $loadAvg = ([math]::Round([double]$loadStats.Average, 3)).ToString("0.###", $ci)
    $loadMax = ([math]::Round([double]$loadStats.Maximum, 3)).ToString("0.###", $ci)
    Write-Host ("HOTSWAP load: min={0}ms avg={1}ms max={2}ms" -f $loadMin, $loadAvg, $loadMax)
    Write-Host ("Log: {0}" -f $outLog)
}
finally {
    try { if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force } } catch {}

    if ($sampleTouched) {
        Set-Content -Path $sample -Value $originalSample -Encoding ascii
    }
}
