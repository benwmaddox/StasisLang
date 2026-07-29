[CmdletBinding()]
param(
    [string]$Stasis = "target/debug/stasis.exe"
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$stasisPath = [System.IO.Path]::GetFullPath((Join-Path $root $Stasis))
$manifestPath = Join-Path $root "stasis.json"
$removedApiProbe = Join-Path $root "target/typed-drawable-removed-api.stasis"
$entries = @(
    "samples/typed_sprite/main.stasis",
    "samples/typed_drawable_visual/typed.stasis",
    "samples/immediate_axis_layout/main.stasis",
    "samples/render_parity/main.stasis",
    "samples/bucket_catcher.stasis",
    "samples/brickout_revenge/brickout_revenge.stasis",
    "samples/brickout_revenge/brickout_revenge_v1.stasis",
    "samples/brickout_revenge/brickout_revenge_v1_cmd.stasis",
    "mobile/android/app/src/main/assets/workshop_sample/src/main.stasis",
    "mobile/android/app/src/main/assets/exploration_sample/src/host_aot.stasis"
)
$forbiddenPattern = '\b(gfx_load_sprite|gfx_release_sprite|gfx_draw_sprite|gfx_cache_text|gfx_measure_text_cached|gfx_measure_text_cached_height|draw_text|draw_text_cached)\b'

if (Test-Path -LiteralPath $manifestPath) {
    throw "Refusing to replace existing repository-root manifest: $manifestPath"
}

Push-Location $root
try {
    $violations = & rg -n $forbiddenPattern src/stdlib samples mobile/android/app/src/main/assets -g '*.stasis'
    if ($LASTEXITCODE -eq 0) {
        throw "Removed drawable API name remains in Stasis source:`n$($violations -join "`n")"
    }
    if ($LASTEXITCODE -ne 1) {
        throw "Drawable API scan failed with exit code $LASTEXITCODE"
    }

    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    foreach ($entry in $entries) {
        $manifest = @"
{
  "manifest_version": 1,
  "name": "typed_drawable_migration",
  "entry": "$entry",
  "tests": "samples/typed_sprite/tests",
  "output": "target/typed-drawable-migration"
}
"@
        [System.IO.File]::WriteAllText($manifestPath, $manifest, $utf8NoBom)
        & $stasisPath check --workspace $root
        if ($LASTEXITCODE -ne 0) {
            throw "Typed drawable migration check failed for $entry with exit code $LASTEXITCODE"
        }
    }

    $probe = @"
import "../src/stdlib/graphics.stasis";
function main(): i32 {
    return gfx_load_sprite("removed.svg", 16, 16);
}
"@
    [System.IO.Directory]::CreateDirectory((Split-Path -Parent $removedApiProbe)) | Out-Null
    [System.IO.File]::WriteAllText($removedApiProbe, $probe, $utf8NoBom)
    $negativeManifest = @"
{
  "manifest_version": 1,
  "name": "typed_drawable_removed_api",
  "entry": "target/typed-drawable-removed-api.stasis",
  "tests": "samples/typed_sprite/tests",
  "output": "target/typed-drawable-migration"
}
"@
    [System.IO.File]::WriteAllText($manifestPath, $negativeManifest, $utf8NoBom)
    $ErrorActionPreference = "Continue"
    $negativeOutput = & $stasisPath check --workspace $root 2>&1
    $negativeExitCode = $LASTEXITCODE
    $ErrorActionPreference = "Stop"
    if ($negativeExitCode -eq 0) {
        throw "Removed gfx_load_sprite API unexpectedly compiled"
    }
    if (($negativeOutput -join "`n") -notmatch "(no matching overload|unknown call target).*gfx_load_sprite") {
        throw "Removed API probe failed for an unexpected reason:`n$($negativeOutput -join "`n")"
    }
} finally {
    Pop-Location
    if (Test-Path -LiteralPath $manifestPath) {
        Remove-Item -LiteralPath $manifestPath -Force
    }
    if (Test-Path -LiteralPath $removedApiProbe) {
        Remove-Item -LiteralPath $removedApiProbe -Force
    }
}

Write-Output "typed drawable migration passed for $($entries.Count) entries"
