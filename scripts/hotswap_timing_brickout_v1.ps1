param(
    [int]$Iterations = 8,
    [int]$Fps = 60,
    [int]$SleepAfterEditMs = 2000,
    [int]$SwapTimeoutMs = 30000,
    [ValidateSet("aot", "jit")]
    [string]$Mode = "aot"
)

$ErrorActionPreference = "Stop"

$root = Resolve-Path (Join-Path $PSScriptRoot "..")
Set-Location $root

$sample = Join-Path $root "samples\\brickout_revenge\\brickout_revenge_v1.stasis"
if (-not (Test-Path $sample)) {
    Write-Error "Sample not found: $sample"
    exit 1
}

$originalSample = Get-Content -Raw -Path $sample
$sampleTouched = $false

$buildDir = Join-Path $root "build"
$hotDir = Join-Path $buildDir "hotstate"
New-Item -ItemType Directory -Force $buildDir | Out-Null
New-Item -ItemType Directory -Force $hotDir | Out-Null

$outLog = Join-Path $buildDir "hotswap_brickout_v1.out.log"
$errLog = Join-Path $buildDir "hotswap_brickout_v1.err.log"
Remove-Item $outLog, $errLog -ErrorAction SilentlyContinue

$sampleBase = [System.IO.Path]::GetFileNameWithoutExtension($sample)
$runnerErrLog = Join-Path $hotDir ("{0}.brick.runner.err.log" -f $sampleBase)
Remove-Item $runnerErrLog -ErrorAction SilentlyContinue

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

function Summarize([string]$label, [int[]]$values, [string]$unit) {
    if ($values.Count -eq 0) { return "${label}: n/a" }
    $min = ($values | Measure-Object -Minimum).Minimum
    $max = ($values | Measure-Object -Maximum).Maximum
    $avg = [math]::Round(($values | Measure-Object -Average).Average, 2)
    return "${label}: min=${min}${unit} avg=${avg}${unit} max=${max}${unit}"
}

function SummarizeFloat([string]$label, [double[]]$values, [string]$unit) {
    if ($values.Count -eq 0) { return "${label}: n/a" }
    $min = ($values | Measure-Object -Minimum).Minimum
    $max = ($values | Measure-Object -Maximum).Maximum
    $avg = [math]::Round(($values | Measure-Object -Average).Average, 3)
    $min = [math]::Round($min, 3)
    $max = [math]::Round($max, 3)
    return "${label}: min=${min}${unit} avg=${avg}${unit} max=${max}${unit}"
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
        Start-Sleep -Milliseconds 50
    }
    return $false
}

function Parse-HotSwapOk([string[]]$lines) {
    $swaps = @()
    foreach ($line in $lines) {
        if ($line -like "*HOTSWAP ok:*") {
            $fields = @{}
            foreach ($m in [regex]::Matches($line, "([A-Za-z_]+)=([0-9]+(?:\\.[0-9]+)?)(ms|us)?")) {
                $name = $m.Groups[1].Value
                $value = [double]$m.Groups[2].Value
                $unit = $m.Groups[3].Value
                if ($unit -eq "us") { $value = $value / 1000.0 }
                $fields[$name] = $value
            }
            if ($fields.ContainsKey("load")) { $swaps += $fields }
        }
    }
    return $swaps
}

function Parse-HotSwapSummary([string[]]$lines) {
    $swaps = @()
    foreach ($line in $lines) {
        if ($line -like "*HOTSWAP(ms):*") {
            $fields = @{}
            foreach ($m in [regex]::Matches($line, "([A-Za-z_]+)=(-?[0-9]+(?:\\.[0-9]+)?)")) {
                $fields[$m.Groups[1].Value] = [double]$m.Groups[2].Value
            }
            if ($fields.ContainsKey("latency")) { $swaps += $fields }
        }
    }
    return $swaps
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
        Start-Sleep -Milliseconds 50
    }
    return -1
}

Write-Host ("Mode: {0}" -f $Mode)
Write-Host ("Sample: {0}" -f $sample)
Write-Host ("Runner log: {0}" -f $runnerErrLog)

$cliArgs = @("run", $sample, "--watch", "--backend", "cranelift", "--graphics", "--module", "brick", "--fps", [string]$Fps)

$env:STASIS_DEV = "1"
if (-not $env:STASIS_HOTSWAP_DELAY_MS) {
    $env:STASIS_HOTSWAP_DELAY_MS = "500"
}
if ($Mode -eq "jit") {
    $env:STASIS_CRANELIFT_JIT_RUNNER = "1"
} else {
    Remove-Item Env:STASIS_CRANELIFT_JIT_RUNNER -ErrorAction SilentlyContinue
}

if (-not $env:STASIS_CLANG) {
    $clang = Join-Path $root ".tools\\llvm-20.1.2\\bin\\clang.exe"
    if (Test-Path $clang) {
        $env:STASIS_CLANG = $clang
    }
}

$proc = Start-Process -FilePath (Join-Path $root "stasis.bat") -ArgumentList $cliArgs -WorkingDirectory $root -RedirectStandardOutput $outLog -RedirectStandardError $errLog -PassThru
try {
    $initialTimeoutMs = if ($Mode -eq "jit") { 600000 } else { 180000 }
    if (-not (Wait-ForText -Path $outLog -Needle "HOTSWAP(ms):" -TimeoutMs $initialTimeoutMs)) {
        throw "Timed out waiting for initial HOTSWAP(ms) marker."
    }

    $swapLog = $outLog
    $swapPattern = if ($Mode -eq "jit") { "HOTSWAP\\(ms\\):.*latency=[0-9]" } else { "HOTSWAP\\(ms\\):.*load=[0-9]" }

    $prevSwapCount = 0
    if (Test-Path $swapLog) {
        try { $prevSwapCount = ([regex]::Matches((Read-TextFileShared $swapLog), $swapPattern)).Count } catch {}
    }

    for ($i = 0; $i -lt $Iterations; $i++) {
        $text = "`n// hotswap timing brickout_v1 $Mode $i $([DateTime]::UtcNow.Ticks)`n"
        Add-Content -Path $sample -Value $text -Encoding ascii
        $sampleTouched = $true
        Start-Sleep -Milliseconds $SleepAfterEditMs

        $count = Wait-ForSwapCountIncrease -Path $swapLog -Pattern $swapPattern -PrevCount $prevSwapCount -TimeoutMs $SwapTimeoutMs
        if ($count -lt 0) {
            throw "Timed out waiting for HOTSWAP(ms) after edit $i."
        }
        $prevSwapCount = $count
    }
}
finally {
    try { if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force } } catch {}
    try { Get-Process stasis_runner -ErrorAction SilentlyContinue | Stop-Process -Force } catch {}
    try { Get-Process stasis-cranelift-jit-runner -ErrorAction SilentlyContinue | Stop-Process -Force } catch {}
    if ($sampleTouched) {
        try { Set-Content -Path $sample -Value $originalSample -Encoding ascii } catch {}
    }
}

$errLines = if (Test-Path $errLog) { Get-Content $errLog } else { @() }
$outLines = if (Test-Path $outLog) { Get-Content $outLog } else { @() }

$summaries = Parse-HotSwapSummary $outLines

$swaps = if ($Mode -eq "jit") {
    $summaries | Where-Object { $_.ContainsKey("latency") -and $_["latency"] -ge 0 }
} else {
    $summaries | Where-Object { $_.ContainsKey("load") -and $_["load"] -ge 0 }
}

$totals = $swaps | ForEach-Object { [int]([math]::Round($_["total"], 0)) }
$latencies = $swaps | ForEach-Object { [int]([math]::Round($_["latency"], 0)) }
$loadMs = if ($Mode -eq "jit") { @() } else { $swaps | ForEach-Object { [math]::Round($_["load"], 3) } }

Write-Host ("Swaps: {0}" -f $swaps.Count)
Write-Host (Summarize "HOTSWAP total" $totals "ms")
Write-Host (Summarize "HOTSWAP latency" $latencies "ms")
if ($Mode -eq "jit") {
    Write-Host "HOTSWAP load: n/a (jit mode does not emit load=...)"
} else {
    Write-Host (SummarizeFloat "HOTSWAP load" $loadMs "ms")
}
