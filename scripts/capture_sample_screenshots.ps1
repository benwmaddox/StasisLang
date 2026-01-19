param(
    [string]$OutDir = "",
    [string]$Backend = "cranelift",
    [string]$Configuration = "Release",
    [int]$Fps = 60,
    [double]$WarmupSeconds = 2.0,
    [int]$WindowTimeoutSeconds = 120,
    [switch]$NoOpen,
    [switch]$KeepExisting,
    [string[]]$Files = @()
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Find-RepoRoot([string]$start) {
    $current = (Resolve-Path $start).Path
    while ($true) {
        if (Test-Path (Join-Path $current "Stasis.sln")) { return $current }
        $parent = Split-Path -Parent $current
        if ($parent -eq $current -or [string]::IsNullOrWhiteSpace($parent)) { break }
        $current = $parent
    }
    throw "Could not find repo root (Stasis.sln) from '$start'."
}

Add-Type -AssemblyName System.Drawing | Out-Null

Add-Type @"
using System;
using System.Runtime.InteropServices;

public static class Win32 {
  [DllImport("user32.dll", SetLastError=true, CharSet=CharSet.Unicode)]
  public static extern IntPtr FindWindowW(string lpClassName, string lpWindowName);

  public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);

  [DllImport("user32.dll", SetLastError=true)]
  public static extern bool EnumWindows(EnumWindowsProc lpEnumFunc, IntPtr lParam);

  [DllImport("user32.dll", SetLastError=true, CharSet=CharSet.Unicode)]
  public static extern int GetWindowTextW(IntPtr hWnd, System.Text.StringBuilder lpString, int nMaxCount);

  [DllImport("user32.dll", SetLastError=true)]
  public static extern int GetWindowTextLengthW(IntPtr hWnd);

  [DllImport("user32.dll", SetLastError=true)]
  public static extern bool IsWindowVisible(IntPtr hWnd);

  [DllImport("user32.dll", SetLastError=true)]
  public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint lpdwProcessId);

  [DllImport("user32.dll", SetLastError=true)]
  public static extern bool GetClientRect(IntPtr hWnd, out RECT lpRect);

  [DllImport("user32.dll", SetLastError=true)]
  public static extern bool ClientToScreen(IntPtr hWnd, ref POINT lpPoint);

  [DllImport("user32.dll", SetLastError=true)]
  public static extern bool SetForegroundWindow(IntPtr hWnd);

  [StructLayout(LayoutKind.Sequential)]
  public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }

  [StructLayout(LayoutKind.Sequential)]
  public struct POINT { public int X; public int Y; }
}
"@ | Out-Null

function Get-WindowHandleByTitle([string]$title, [int]$timeoutMs = 15000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $timeoutMs) {
        $h = [Win32]::FindWindowW($null, $title)
        if ($h -ne [IntPtr]::Zero) { return $h }

        # Fall back to substring match (some SDL backends append suffixes).
        $found = [IntPtr]::Zero
        $cb = [Win32+EnumWindowsProc]{
            param([IntPtr]$hWnd, [IntPtr]$lp)
            if (-not [Win32]::IsWindowVisible($hWnd)) { return $true }
            $len = [Win32]::GetWindowTextLengthW($hWnd)
            if ($len -le 0) { return $true }
            $sb = New-Object System.Text.StringBuilder ($len + 1)
            [void][Win32]::GetWindowTextW($hWnd, $sb, $sb.Capacity)
            $t = $sb.ToString()
            if ($t -like "*$title*") {
                $script:__winFound = $hWnd
                return $false
            }
            return $true
        }
        $script:__winFound = [IntPtr]::Zero
        [Win32]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
        $found = $script:__winFound
        if ($found -ne [IntPtr]::Zero) { return $found }

        Start-Sleep -Milliseconds 50
    }
    return [IntPtr]::Zero
}

function Get-WindowHandleByPid([int]$targetPid, [int]$timeoutMs = 15000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $timeoutMs) {
        $cb = [Win32+EnumWindowsProc]{
            param([IntPtr]$hWnd, [IntPtr]$lp)
            if (-not [Win32]::IsWindowVisible($hWnd)) { return $true }
            $winPid = 0
            [void][Win32]::GetWindowThreadProcessId($hWnd, [ref]$winPid)
            if ($winPid -eq $script:__targetPid) {
                $script:__winFound = $hWnd
                return $false
            }
            return $true
        }

        $script:__winFound = [IntPtr]::Zero
        $script:__targetPid = $targetPid
        [Win32]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
        if ($script:__winFound -ne [IntPtr]::Zero) { return $script:__winFound }

        Start-Sleep -Milliseconds 50
    }
    return [IntPtr]::Zero
}

function Find-NewProcessIdByName([string]$processName, [int[]]$existingPids, [int]$timeoutMs = 15000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $timeoutMs) {
        $procs = Get-Process -Name $processName -ErrorAction SilentlyContinue
        foreach ($p in $procs) {
            if ($existingPids -notcontains $p.Id) {
                return $p.Id
            }
        }
        Start-Sleep -Milliseconds 50
    }
    return -1
}

function Save-ClientScreenshot([IntPtr]$hWnd, [string]$path) {
    $rect = New-Object Win32+RECT
    if (-not [Win32]::GetClientRect($hWnd, [ref]$rect)) {
        throw "GetClientRect failed for window handle $hWnd"
    }

    $w = $rect.Right - $rect.Left
    $h = $rect.Bottom - $rect.Top
    if ($w -le 0 -or $h -le 0) {
        throw "Invalid client rect size ($w x $h) for window handle $hWnd"
    }

    $pt = New-Object Win32+POINT
    $pt.X = $rect.Left
    $pt.Y = $rect.Top
    if (-not [Win32]::ClientToScreen($hWnd, [ref]$pt)) {
        throw "ClientToScreen failed for window handle $hWnd"
    }

    $bmp = New-Object System.Drawing.Bitmap $w, $h, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    try {
        $gfx = [System.Drawing.Graphics]::FromImage($bmp)
        try {
            $gfx.CopyFromScreen($pt.X, $pt.Y, 0, 0, $bmp.Size)
        } finally {
            $gfx.Dispose()
        }
        $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    } finally {
        $bmp.Dispose()
    }
}

function Sanitize-FileToken([string]$s) {
    $t = $s -replace "[\\\\/]+", "_"
    $t = $t -replace "[:\\s]+", "_"
    $t = $t -replace "[^A-Za-z0-9_\\-\\.]", "_"
    return $t.Trim("_")
}

function Parse-InitWindowTitle([string]$filePath) {
    $text = Get-Content -Raw -Path $filePath
    $m = [Regex]::Match($text, 'init_window\s*\([^\)]*?,\s*"([^"]+)"\s*\)')
    if ($m.Success) { return $m.Groups[1].Value }
    return ""
}

function Run-And-Screenshot([string]$root, [string]$relativeStasisPath) {
    $full = Join-Path $root $relativeStasisPath
    if (-not (Test-Path $full)) {
        Write-Warning "Skipping missing: $relativeStasisPath"
        return $null
    }

    $title = Parse-InitWindowTitle $full
    if ([string]::IsNullOrWhiteSpace($title)) {
        $title = [IO.Path]::GetFileNameWithoutExtension($full)
    }

    $token = Sanitize-FileToken $relativeStasisPath
    $pngPath = Join-Path $script:OutDirResolved ($token + ".png")

    $startedAt = Get-Date
    $existingRunnerPids = @()
    $existingLliPids = @()
    try { $existingRunnerPids = (Get-Process -Name stasis_runner -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Id) } catch {}
    try { $existingLliPids = (Get-Process -Name lli -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Id) } catch {}

    $cliProj = Join-Path $root "Stasis.Cli\\Stasis.Cli.csproj"
    $args = @(
        "run",
        "--no-build",
        "--configuration", $Configuration,
        "--project", $cliProj,
        "--",
        "run",
        $relativeStasisPath,
        "--backend", $Backend,
        "--graphics",
        "--fps", "$Fps"
    )

    Write-Host "Running: $relativeStasisPath (title='$title')"
    $proc = Start-Process -FilePath "dotnet" -ArgumentList $args -WorkingDirectory $root -PassThru -WindowStyle Hidden

    try {
        function Wait-ForWindow([string]$expectedTitle, [int]$maybePid, [int]$timeoutMs) {
            $sw = [Diagnostics.Stopwatch]::StartNew()
            while ($sw.ElapsedMilliseconds -lt $timeoutMs) {
                if ($proc.HasExited) {
                    throw "Process exited before window appeared (exit=$($proc.ExitCode)). Expected title='$expectedTitle'."
                }

                $h = [IntPtr]::Zero
                if ($maybePid -gt 0) {
                    $h = Get-WindowHandleByPid -targetPid $maybePid -timeoutMs 250
                }
                if ($h -eq [IntPtr]::Zero) {
                    $h = Get-WindowHandleByTitle -title $expectedTitle -timeoutMs 250
                }
                if ($h -ne [IntPtr]::Zero) { return $h }

                Start-Sleep -Milliseconds 50
            }
            return [IntPtr]::Zero
        }

        for ($attempt = 0; $attempt -lt 3; $attempt++) {
            $hWnd = [IntPtr]::Zero

            # Prefer PID-based discovery (more reliable for fullscreen/borderless windows).
            $runnerPid = -1
            if ($Backend -ieq "cranelift") {
                $runnerPid = Find-NewProcessIdByName -processName "stasis_runner" -existingPids $existingRunnerPids -timeoutMs 120000
            }

            $hWnd = Wait-ForWindow -expectedTitle $title -maybePid $runnerPid -timeoutMs ($WindowTimeoutSeconds * 1000)
            if ($hWnd -eq [IntPtr]::Zero) { throw "Timed out waiting for window: '$title'" }

            [Win32]::SetForegroundWindow($hWnd) | Out-Null
            Start-Sleep -Seconds $WarmupSeconds

            try {
                Save-ClientScreenshot -hWnd $hWnd -path $pngPath
                Write-Host "Saved: $pngPath"
                break
            }
            catch {
                if ($attempt -ge 2) { throw }
                Start-Sleep -Milliseconds 200
            }
        }

        return $pngPath
    }
    finally {
        if (-not $proc.HasExited) {
            try { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue } catch {}
        }

        # Best-effort cleanup of runner/interpreter processes (if any)
        if (-not $KeepExisting) {
            try { Get-Process -Name stasis_runner -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue } catch {}
            try { Get-Process -Name lli -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue } catch {}
        } else {
            try {
                $runnerNow = Get-Process -Name stasis_runner -ErrorAction SilentlyContinue
                foreach ($p in $runnerNow) {
                    if ($existingRunnerPids -notcontains $p.Id) {
                        try { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue } catch {}
                    }
                }
            } catch {}

            try {
                $lliNow = Get-Process -Name lli -ErrorAction SilentlyContinue
                foreach ($p in $lliNow) {
                    if ($existingLliPids -notcontains $p.Id) {
                        try { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue } catch {}
                    }
                }
            } catch {}
        }
    }
}

$root = Find-RepoRoot $PSScriptRoot

function Ensure-LlvmToolsOnPath([string]$repoRoot) {
    if (Get-Command clang -ErrorAction SilentlyContinue) {
        return
    }

    $llvmDirs = Get-ChildItem -Path (Join-Path $repoRoot ".tools") -Directory -Filter "llvm-*" -ErrorAction SilentlyContinue |
        Sort-Object Name -Descending

    foreach ($d in $llvmDirs) {
        $bin = Join-Path $d.FullName "bin"
        $clang = Join-Path $bin "clang.exe"
        if (Test-Path $clang) {
            $env:PATH = "$bin;$env:PATH"
            return
        }
    }

    throw "clang not found in PATH and no repo-pinned LLVM under '.tools/llvm-*/bin'. Run `env.bat` or install LLVM (clang)."
}

Ensure-LlvmToolsOnPath $root

if ($Backend -ieq "cranelift") {
    $aot = $env:STASIS_CRANELIFT_AOT
    if ([string]::IsNullOrWhiteSpace($aot)) {
        $candidate = Join-Path $root "tools\\cranelift-aot\\target\\release\\stasis-cranelift-aot.exe"
        if (-not (Test-Path $candidate)) {
            throw "Backend=cranelift requested but stasis-cranelift-aot not found at '$candidate' and STASIS_CRANELIFT_AOT is not set."
        }
    }
}

if (-not $KeepExisting) {
    try { Get-Process -Name stasis_runner -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue } catch {}
    try { Get-Process -Name lli -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue } catch {}
}

if ([string]::IsNullOrWhiteSpace($OutDir)) {
    $stamp = Get-Date -Format "yyyyMMdd_HHmmss"
    $OutDir = Join-Path $root (Join-Path "artifacts\\screenshots" $stamp)
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$script:OutDirResolved = (Resolve-Path $OutDir).Path

Write-Host "Output dir: $script:OutDirResolved"

$cliProj = Join-Path $root "Stasis.Cli\\Stasis.Cli.csproj"
Write-Host "Building CLI: $cliProj ($Configuration)"
& dotnet build --nologo --configuration $Configuration $cliProj | Out-Null

$files = @()
if ($Files.Count -gt 0) {
    $files += $Files
} else {
    if (Get-Command rg -ErrorAction SilentlyContinue) {
        $files += (rg -l 'init_window\(' samples examples | Where-Object { $_ -like "*.stasis" } | Sort-Object)
    } else {
        $candidates = Get-ChildItem -Path (Join-Path $root "samples"), (Join-Path $root "examples") -Recurse -Filter "*.stasis" -ErrorAction SilentlyContinue
        foreach ($c in $candidates) {
            if (Select-String -Path $c.FullName -Pattern "init_window(" -SimpleMatch -Quiet) {
                $files += $c.FullName.Substring($root.Length).TrimStart("\\" , "/")
            }
        }
        $files = $files | Sort-Object
    }
}

if ($files.Count -eq 0) {
    throw "No windowed .stasis samples found (init_window)."
}

$saved = @()
$failures = @()
foreach ($f in $files) {
    try {
        $p = Run-And-Screenshot -root $root -relativeStasisPath $f
        if ($p) { $saved += $p }
    }
    catch {
        $failures += "${f}: $($_.Exception.Message)"
        Write-Warning "Failed: $f ($($_.Exception.Message))"
    }
}

if ($saved.Count -gt 0) {
    if (-not $NoOpen) {
        foreach ($p in $saved) {
            Start-Process $p | Out-Null
        }
    }
    Start-Process $script:OutDirResolved | Out-Null
}

Write-Host "Done. Captured $($saved.Count) screenshots."
if ($failures.Count -gt 0) {
    Write-Warning "Some captures failed:"
    foreach ($x in $failures) { Write-Warning "  $x" }
}
