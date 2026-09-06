$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
$targetDir = Join-Path $repoRoot "target/task353-desktop-network-link"
$previousRustFlags = $env:RUSTFLAGS
Push-Location $repoRoot
try {
    $env:RUSTFLAGS = (($previousRustFlags, "-C target-feature=+crt-static") -join " ").Trim()
    & python (Join-Path $repoRoot "tools/cargo_cache.py") run -- cargo build `
        -p stasis_network --release --target-dir $targetDir
    if ($LASTEXITCODE -ne 0) { throw "stasis_network release build failed" }

    $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio/Installer/vswhere.exe"
    if (-not (Test-Path -LiteralPath $vswhere)) {
        throw "vswhere.exe was not found; install the Visual Studio C++ desktop workload"
    }
    $installation = (& $vswhere -latest -products * `
        -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
        -property installationPath | Select-Object -First 1)
    if (-not $installation) { throw "Visual Studio C++ desktop workload was not found" }
    $vcvars = Join-Path $installation "VC/Auxiliary/Build/vcvars64.bat"
    $source = Join-Path $repoRoot "runtime/tests/stasis_network_link_test.c"
    $include = Join-Path $repoRoot "crates/stasis_network/include"
    $library = Join-Path $targetDir "release/stasis_network.lib"
    $executable = Join-Path $targetDir "stasis_network_link_test.exe"
    foreach ($required in @($vcvars, $source, $library)) {
        if (-not (Test-Path -LiteralPath $required)) { throw "required link input missing: $required" }
    }

    $object = Join-Path $targetDir "stasis_network_link_test.obj"
    $compile = 'call "{0}" >nul && cl /nologo /W4 /WX /MT /I"{1}" "{2}" /Fo:"{3}" /Fe:"{4}" "{5}" ws2_32.lib bcrypt.lib userenv.lib ntdll.lib' -f `
        $vcvars, $include, $source, $object, $executable, $library
    & cmd.exe /d /c $compile
    if ($LASTEXITCODE -ne 0) { throw "native stasis_network link probe failed to compile" }
    & $executable
    if ($LASTEXITCODE -ne 0) { throw "native stasis_network link probe failed" }
} finally {
    $env:RUSTFLAGS = $previousRustFlags
    Pop-Location
}
