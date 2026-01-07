param(
    [int]$Iterations = 5,
    [int]$Fps = 60,
    [int]$SleepAfterEditMs = 6000,
    [int]$SwapTimeoutMs = 60000
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot

function Stop-Running {
    Get-Process stasis_runner, Stasis.Cli -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 200
}

function Start-LoggedProcess {
    param(
        [string]$Exe,
        [string[]]$ProcessArgs,
        [string]$OutLog,
        [string]$ErrLog,
        [hashtable]$Env
    )

    if (Test-Path $OutLog) { Remove-Item $OutLog -Force }
    if (Test-Path $ErrLog) { Remove-Item $ErrLog -Force }

    foreach ($k in $Env.Keys) {
        Set-Item -Path ("Env:" + $k) -Value $Env[$k]
    }

    $outDir = Split-Path -Parent $OutLog
    if ($outDir -and !(Test-Path $outDir)) { New-Item -ItemType Directory -Force -Path $outDir | Out-Null }

    $argLine = ($ProcessArgs | ForEach-Object {
        $s = [string]$_
        if ($s -match "\s") {
            $escaped = $s -replace '"', '""'
            "`"$escaped`""
        }
        else {
            $s
        }
    }) -join " "

    $p = Start-Process `
        -FilePath $Exe `
        -ArgumentList $argLine `
        -NoNewWindow `
        -PassThru `
        -RedirectStandardOutput $OutLog `
        -RedirectStandardError $ErrLog
    return [pscustomobject]@{ Process = $p; OutLog = $OutLog; ErrLog = $ErrLog }
}

function Run-HotSwapBench {
    param(
        [string]$Name,
        [string]$Exe,
        [string[]]$CompilerArgs,
        [hashtable]$Env,
        [string]$SwapOkLog
    )

    Stop-Running

    $baseDir = (Get-Location).Path
    $buildDir = Join-Path $baseDir "build"
    $outLog = Join-Path $buildDir ("ci_{0}.out.log" -f $Name)
    $errLog = Join-Path $buildDir ("ci_{0}.err.log" -f $Name)

    $cap = Start-LoggedProcess -Exe $Exe -ProcessArgs $CompilerArgs -OutLog $outLog -ErrLog $errLog -Env $Env

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
            [string]$Pattern,
            [int]$TimeoutMs
        )

        $deadline = [DateTime]::UtcNow.AddMilliseconds($TimeoutMs)
        while ([DateTime]::UtcNow -lt $deadline) {
            if ($cap.Process.HasExited) { throw "$Name compiler exited early (code=$($cap.Process.ExitCode))" }
            if (Test-Path $Path) {
                try {
                    $text = Read-TextFileShared -Path $Path
                    if ($text -match $Pattern) { return }
                } catch {
                    # ignore
                }
            }
            Start-Sleep -Milliseconds 50
        }

        throw "$Name timed out waiting for $Pattern in $Path"
    }

    # Wait for the initial build to finish (and the watch loop/file watcher to be set up) before editing.
    Wait-ForText -Path $errLog -Pattern "HOTRELOAD phases\(ms\):" -TimeoutMs 180000
    if (![string]::IsNullOrEmpty($SwapOkLog)) {
        try { Wait-ForText -Path $SwapOkLog -Pattern "HOTSWAP loading:|HOTSWAP ok:" -TimeoutMs 60000 } catch {}
    }
    Start-Sleep -Milliseconds 250

    function Get-SwapOkCount {
        if (![string]::IsNullOrEmpty($SwapOkLog) -and (Test-Path $SwapOkLog)) {
            try {
                $text = Read-TextFileShared -Path $SwapOkLog
                return ([regex]::Matches($text, "HOTSWAP ok:")).Count
            } catch {
                return 0
            }
        }
        return 0
    }

    function Wait-ForSwapOk {
        param([int]$PrevCount)
        $deadline = [DateTime]::UtcNow.AddMilliseconds($SwapTimeoutMs)
        while ([DateTime]::UtcNow -lt $deadline) {
            $count = Get-SwapOkCount
            if ($count -gt $PrevCount) { return }
            Start-Sleep -Milliseconds 50
        }

        Write-Host "$Name timeout: expected runner HOTSWAP ok: (prevCount=$PrevCount)"
        Write-Host "runner log: $SwapOkLog"
        if (Test-Path $SwapOkLog) {
            try { Get-Content -Tail 80 $SwapOkLog } catch {}
        } else {
            Write-Host "runner log missing"
        }

        Write-Host "compiler out log: $outLog"
        if (Test-Path $outLog) {
            try { Get-Content -Tail 80 $outLog } catch {}
        } else {
            Write-Host "compiler out log missing"
        }

        Write-Host "compiler err log: $errLog"
        if (Test-Path $errLog) {
            try { Get-Content -Tail 80 $errLog } catch {}
        } else {
            Write-Host "compiler err log missing"
        }

        throw "$Name timed out waiting for HOTSWAP ok: in runner log (prevCount=$PrevCount)"
    }

    for ($i = 0; $i -lt $Iterations; $i++) {
        if ($cap.Process.HasExited) { throw "$Name compiler exited early (code=$($cap.Process.ExitCode))" }
        $prevCount = Get-SwapOkCount
        Add-Content -Path "samples/hotstate_tick_watch.stasis" -Value "// ci bench $Name $i" -Encoding ascii
        Start-Sleep -Milliseconds $SleepAfterEditMs
        Wait-ForSwapOk -PrevCount $prevCount
    }

    Stop-Running
    try { $cap.Process.Kill() } catch {}
    try { $cap.Process.WaitForExit(5000) | Out-Null } catch {}
    try { $cap.Process.Dispose() } catch {}
    Start-Sleep -Milliseconds 300

    git checkout -- "samples/hotstate_tick_watch.stasis" | Out-Null

    return [pscustomobject]@{
        name = $Name
        outLog = $outLog
        errLog = $errLog
    }
}

try {
    New-Item -ItemType Directory -Force -Path build | Out-Null
    New-Item -ItemType Directory -Force -Path build/hotstate | Out-Null

    $csharpExe = $null
    foreach ($p in @(
        (Join-Path $repoRoot "Stasis.Cli/bin/Release/net9.0/Stasis.Cli.exe"),
        (Join-Path $repoRoot "Stasis.Cli/bin/Release/net9.0/Stasis.Cli"),
        (Join-Path $repoRoot "Stasis.Cli/bin/x64/Release/net9.0/Stasis.Cli.exe"),
        (Join-Path $repoRoot "Stasis.Cli/bin/x64/Release/net9.0/Stasis.Cli")
    )) {
        if (Test-Path $p) { $csharpExe = $p; break }
    }
    if (!$csharpExe) {
        $fallback =
            (Get-ChildItem -Recurse -Path (Join-Path $repoRoot "Stasis.Cli/bin") -File -ErrorAction SilentlyContinue |
                Where-Object {
                    ($_.Name -eq "Stasis.Cli.exe" -or $_.Name -eq "Stasis.Cli") -and ($_.FullName -match "[/\\\\]Release[/\\\\].*[/\\\\]net9\\.0[/\\\\]")
                } |
                Select-Object -First 1)
        if ($fallback) { $csharpExe = $fallback.FullName }
    }
    if (!$csharpExe -or !(Test-Path $csharpExe)) { throw "missing Stasis.Cli executable (dotnet build step should produce it)" }

    $aot = Join-Path $repoRoot "tools/cranelift-aot/target/release/stasis-cranelift-aot.exe"
    if (!(Test-Path $aot)) { $aot = Join-Path $repoRoot "tools/cranelift-aot/target/release/stasis-cranelift-aot" }
    if (!(Test-Path $aot)) { throw "missing stasis-cranelift-aot (cargo build step should produce it)" }

    $swapOkLog = Join-Path $repoRoot "build/hotstate/hotstate_tick_watch.hot.runner.err.log"
    $cs = Run-HotSwapBench -Name "cranelift" -Exe $csharpExe -CompilerArgs @("run","samples/hotstate_tick_watch.stasis","--watch","--backend","cranelift","--module","hot","--fps",$Fps) -Env @{ "STASIS_CRANELIFT_AOT" = $aot } -SwapOkLog $swapOkLog

    Write-Host "logs: craneliftOut=$($cs.outLog) craneliftErr=$($cs.errLog)"
    exit 0
}
finally {
    Stop-Running
    git checkout -- "samples/hotstate_tick_watch.stasis" | Out-Null
}
