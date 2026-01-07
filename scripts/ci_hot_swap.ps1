param(
    [int]$Iterations = 5,
    [int]$Fps = 60,
    [int]$SleepAfterEditMs = 6000,
    [int]$PostEditsMs = 25000
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot

function Stop-Running {
    Get-Process stasis_runner, stasis, stasis_selfhost, Stasis.Cli -ErrorAction SilentlyContinue | Stop-Process -Force
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

    $argLine = ($ProcessArgs | ForEach-Object {
        $s = [string]$_
        if ($s -match "\s") {
            $escaped = $s -replace '"', '""'
            "`"$escaped`""
        } else {
            $s
        }
    }) -join " "

    $outDir = Split-Path -Parent $OutLog
    if ($outDir -and !(Test-Path $outDir)) { New-Item -ItemType Directory -Force -Path $outDir | Out-Null }

    $exeEsc = $Exe -replace '"', '""'
    $outEsc = $OutLog -replace '"', '""'
    $errEsc = $ErrLog -replace '"', '""'

    $cmdLine = $exeEsc + '"'
    if (![string]::IsNullOrEmpty($argLine)) {
        $cmdLine += ' ' + $argLine
    }
    $cmdLine += ' 1> "' + $outEsc + '" 2> "' + $errEsc + '"'

    # cmd.exe quoting: when the command begins with a quoted path, wrap the whole command in an extra pair of quotes.
    $cmdArg = '/c ""' + $cmdLine + '"'
    $p = Start-Process -FilePath "cmd.exe" -ArgumentList $cmdArg -NoNewWindow -PassThru
    return [pscustomobject]@{ Process = $p; OutLog = $OutLog; ErrLog = $ErrLog }
}

function Run-HotSwapBench {
    param(
        [string]$Name,
        [string]$Exe,
        [string[]]$CompilerArgs,
        [hashtable]$Env
    )

    Stop-Running

    $baseDir = (Get-Location).Path
    $buildDir = Join-Path $baseDir "build"
    $outLog = Join-Path $buildDir ("ci_{0}.out.log" -f $Name)
    $errLog = Join-Path $buildDir ("ci_{0}.err.log" -f $Name)

    $cap = Start-LoggedProcess -Exe $Exe -ProcessArgs $CompilerArgs -OutLog $outLog -ErrLog $errLog -Env $Env
    Start-Sleep -Seconds 2

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

    function Read-LatencyValues {
        $text = ""
        foreach ($p in @($outLog, $errLog)) {
            if (!(Test-Path $p)) { continue }
            try { $text += (Read-TextFileShared -Path $p) + "`n" } catch {}
        }

        $vals = @()
        $matches = [regex]::Matches($text, "HOTSWAP latency\\(ms\\):\\s*(\\-?\\d+)")
        foreach ($m in $matches) { $vals += [int]$m.Groups[1].Value }
        return ,$vals
    }

    for ($i = 0; $i -lt $Iterations; $i++) {
        if ($cap.Process.HasExited) { throw "$Name compiler exited early (code=$($cap.Process.ExitCode))" }
        Add-Content -Path "samples/hotstate_tick_watch.stasis" -Value "// ci bench $Name $i"
        Start-Sleep -Milliseconds $SleepAfterEditMs
    }

    Start-Sleep -Milliseconds $PostEditsMs

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

    $repoRoot = (Get-Location).Path

    $selfExe = Join-Path $repoRoot "build/stasis_selfhost.exe"
    if (!(Test-Path $selfExe)) { throw "missing build/stasis_selfhost.exe (build step should produce it)" }

    $csharpExe = Join-Path $repoRoot "Stasis.Cli\\bin\\Release\\net9.0\\Stasis.Cli.exe"
    if (!(Test-Path $csharpExe)) {
        $csharpExe = Join-Path $repoRoot "Stasis.Cli\\bin\\x64\\Release\\net9.0\\Stasis.Cli.exe"
    }
    if (!(Test-Path $csharpExe)) {
        $candidates =
            (Get-ChildItem -Recurse -Filter "Stasis.Cli.exe" -Path (Join-Path $repoRoot "Stasis.Cli\\bin") -ErrorAction SilentlyContinue |
                Where-Object { $_.FullName -like "*\\Release\\net9.0\\Stasis.Cli.exe" } |
                Select-Object -First 1)
        if ($candidates) {
            $csharpExe = $candidates.FullName
        }
    }
    if (!(Test-Path $csharpExe)) { throw "missing Stasis.Cli.exe under Stasis.Cli/bin (dotnet build step should produce it)" }

    $aot = Join-Path $repoRoot "tools/cranelift-aot/target/release/stasis-cranelift-aot.exe"
    if (!(Test-Path $aot)) { throw "missing $aot (cargo build step should produce it)" }

    $self = Run-HotSwapBench -Name "selfhost" -Exe $selfExe -CompilerArgs @("watch","run","--backend","llvm","--module","hot","--fps",$Fps,"samples/hotstate_tick_watch.stasis") -Env @{}
    $cs = Run-HotSwapBench -Name "csharp" -Exe $csharpExe -CompilerArgs @("run","samples/hotstate_tick_watch.stasis","--watch","--backend","cranelift","--module","hot","--fps",$Fps) -Env @{ "STASIS_CRANELIFT_AOT" = $aot }

    Write-Host "logs: selfhostOut=$($self.outLog) selfhostErr=$($self.errLog)"
    Write-Host "logs: csharpOut=$($cs.outLog) csharpErr=$($cs.errLog)"
    exit 0
}
finally {
    Stop-Running
    git checkout -- "samples/hotstate_tick_watch.stasis" | Out-Null
}
