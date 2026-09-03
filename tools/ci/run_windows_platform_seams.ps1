param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("DesktopSdl", "MobileRuntime")]
    [string] $Suite
)

$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $false

$suites = @{
    DesktopSdl = @(
        "desktop_input_frame_seam"
        "desktop_display_metrics_seam"
        "desktop_manifest_assets_seam"
        "desktop_asset_load_stress"
        "desktop_render_recovery_seam"
        "desktop_hot_swap_generation_seam"
    )
    MobileRuntime = @(
        "generated_mobile_aot_runtime_seam"
        "mobile_packaged_assets_seam"
    )
}

$failures = [System.Collections.Generic.List[string]]::new()
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
$suiteLogDir = Join-Path $repoRoot "target/windows-platform-seams/$Suite"

function Remove-LingeringSeamProcesses {
    param([string] $Target)

    $prefix = "$Target-"
    $lingering = @(Get-Process -ErrorAction SilentlyContinue | Where-Object {
        try {
            $processBaseName = [System.IO.Path]::GetFileNameWithoutExtension($_.Path)
            $processBaseName.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)
        } catch {
            $false
        }
    })
    foreach ($process in $lingering) {
        $failures.Add("$Target left process $($process.Id) running")
        Write-Host "::error::$Target left process $($process.Id) running; terminating it."
        try {
            Stop-Process -Id $process.Id -Force
        } catch {
            $failures.Add("could not terminate $Target process $($process.Id): $($_.Exception.Message)")
        }
    }
}

Push-Location $repoRoot
try {
    New-Item -ItemType Directory -Force $suiteLogDir | Out-Null
    foreach ($target in $suites[$Suite]) {
        $logPath = Join-Path $suiteLogDir "$target.log"
        Write-Host "::group::$Suite - $target"
        try {
            & python tools/cargo_cache.py run -- cargo test -p stasis --test $target -- --test-threads=1 --nocapture 2>&1 | Tee-Object -FilePath $logPath
            $exitCode = $LASTEXITCODE
            if ($exitCode -ne 0) {
                $failures.Add("$target exited with code $exitCode (log: $logPath)")
            }
        } catch {
            $failures.Add("$target could not run: $($_.Exception.Message) (log: $logPath)")
        } finally {
            Remove-LingeringSeamProcesses -Target $target
            Write-Host "::endgroup::"
        }
    }
} finally {
    Pop-Location
}

if ($failures.Count -ne 0) {
    throw "$Suite seam suite failed:`n - $($failures -join "`n - ")"
}

Write-Host "$Suite seam suite passed ($($suites[$Suite].Count) cases)."
