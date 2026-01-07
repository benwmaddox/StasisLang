param(
    [int]$Iterations = 5,
    [int]$WarnMs = 250,
    [int]$FailMs = 500
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot

function Read-TextFileWithRetries {
    param(
        [string]$Path,
        [int]$Attempts = 50
    )

    for ($i = 0; $i -lt $Attempts; $i++) {
        try {
            if (!(Test-Path $Path)) { return "" }
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
        catch {
            Start-Sleep -Milliseconds 50
        }
    }

    return ""
}

function Get-Latencies {
    param(
        [string]$OutLog,
        [string]$ErrLog,
        [int]$MaxCount
    )

    $pattern = "HOTSWAP latency\(ms\):\s*(-?\d+)"
    $vals = @()
    for ($attempt = 0; $attempt -lt 200; $attempt++) {
        $vals = @()
        foreach ($p in @($OutLog, $ErrLog)) {
            if (!(Test-Path $p)) { continue }
            try {
                $hits =
                    (Select-String -Path $p -Pattern $pattern -AllMatches | ForEach-Object {
                        foreach ($m in $_.Matches) { [int]$m.Groups[1].Value }
                    })
                if ($hits) { $vals += $hits }
            }
            catch {
                # ignore and retry
            }
        }

        if ($vals.Count -ge $MaxCount) { break }
        Start-Sleep -Milliseconds 100
    }

    if ($vals.Count -gt $MaxCount) {
        $vals = $vals[($vals.Count - $MaxCount)..($vals.Count - 1)]
    }
    while ($vals.Count -lt $MaxCount) { $vals += -1 }
    return $vals
}

function Check-Thresholds {
    param(
        [string]$Name,
        [int[]]$Latencies
    )

    $bad = $Latencies | Where-Object { $_ -ge $FailMs -or $_ -lt 0 }
    $warn = $Latencies | Where-Object { $_ -ge $WarnMs -and $_ -lt $FailMs }

    $avg = [int]([Math]::Round(($Latencies | Measure-Object -Average).Average))
    $max = ($Latencies | Measure-Object -Maximum).Maximum
    $min = ($Latencies | Measure-Object -Minimum).Minimum

    Write-Host "$Name hot swap latencies(ms): $($Latencies -join ', ')"
    Write-Host "$Name summary: min=$min avg=$avg max=$max warn>=$WarnMs fail>=$FailMs"

    if ($warn.Count -gt 0) {
        Write-Host "::warning::hot swap $Name has $($warn.Count) swaps >= $WarnMs ms (min=$min avg=$avg max=$max)"
    }
    if ($bad.Count -gt 0) {
        Write-Host "::error::hot swap $Name has $($bad.Count) swaps >= $FailMs ms or timed out (min=$min avg=$avg max=$max)"
        return 1
    }

    return 0
}

$outLog = Join-Path $repoRoot "build/ci_cranelift.out.log"
$errLog = Join-Path $repoRoot "build/ci_cranelift.err.log"

$latencies = Get-Latencies -OutLog $outLog -ErrLog $errLog -MaxCount $Iterations
$rc = Check-Thresholds -Name "cranelift" -Latencies $latencies
exit $rc
