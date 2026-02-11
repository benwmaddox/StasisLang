param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]] $ForwardArgs
)

$ErrorActionPreference = "Stop"

function Resolve-RealPath([string] $PathValue) {
    try {
        return [System.IO.Path]::GetFullPath($PathValue)
    } catch {
        return $null
    }
}

function Map-IfStasisPath(
    [string] $ArgValue,
    [string] $RepoRoot,
    [string] $MappedRoot,
    [string] $CurrentDir
) {
    if (-not $ArgValue.EndsWith(".stasis", [System.StringComparison]::OrdinalIgnoreCase)) {
        return $ArgValue
    }

    $candidate = $ArgValue
    if (-not [System.IO.Path]::IsPathRooted($candidate)) {
        $candidate = Join-Path $CurrentDir $candidate
    }

    $full = Resolve-RealPath $candidate
    if ($null -eq $full) {
        return $ArgValue
    }

    $repoPrefix = $RepoRoot + [System.IO.Path]::DirectorySeparatorChar
    if ($full.StartsWith($repoPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        $relative = $full.Substring($repoPrefix.Length)
        return (Join-Path $MappedRoot $relative)
    }

    if ($full.Equals($RepoRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
        return $MappedRoot
    }

    return $ArgValue
}

function Transform-StasisSource([string] $Text) {
    # Receiver-form mutating conversions -> legacy bootstrap calls.
    # from_i32: target = i32_to_f32(expr);
    $Text = [regex]::Replace(
        $Text,
        '(?m)^([ \t]*)([A-Za-z_][A-Za-z0-9_]*(?:\[[^\]]+\]|\.[A-Za-z_][A-Za-z0-9_]*)*)\.from_i32\((.+?)\);\s*$',
        '$1$2 = i32_to_f32($3);')

    # from_f32: target = f32_to_i32(expr);
    $Text = [regex]::Replace(
        $Text,
        '(?m)^([ \t]*)([A-Za-z_][A-Za-z0-9_]*(?:\[[^\]]+\]|\.[A-Za-z_][A-Za-z0-9_]*)*)\.from_f32\((.+?)\);\s*$',
        '$1$2 = f32_to_i32($3);')

    # from_u32: target = u32_to_i32(expr);
    $Text = [regex]::Replace(
        $Text,
        '(?m)^([ \t]*)([A-Za-z_][A-Za-z0-9_]*(?:\[[^\]]+\]|\.[A-Za-z_][A-Za-z0-9_]*)*)\.from_u32\((.+?)\);\s*$',
        '$1$2 = u32_to_i32($3);')

    return $Text
}

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$dllPath = Join-Path $scriptDir "stasis-cli\Stasis.Cli.dll"
if (-not (Test-Path $dllPath)) {
    Write-Error "error: bootstrap compiler not found at '$dllPath'"
    exit 1
}

$repoRoot = Resolve-RealPath (Join-Path $scriptDir "..\..")
if ($null -eq $repoRoot) {
    Write-Error "error: could not resolve repository root"
    exit 1
}

$argsList = @()
if ($ForwardArgs) {
    $argsList = $ForwardArgs
}

if ($env:STASIS_BOOTSTRAP_NO_PREPROCESS -eq "1") {
    & dotnet $dllPath @argsList
    exit $LASTEXITCODE
}

$currentDir = Resolve-RealPath (Get-Location).Path
$tempRoot = Join-Path $env:TEMP ("stasis_bootstrap_pre_" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $tempRoot -Force | Out-Null

try {
    # Copy repository snapshot for import-relative compile consistency.
    Get-ChildItem $repoRoot -Force |
        Where-Object { $_.Name -ne ".git" } |
        ForEach-Object {
            Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $tempRoot $_.Name) -Recurse -Force
        }

    # Rewrite receiver-style conversion statements for bootstrap compiler compatibility.
    Get-ChildItem -Path $tempRoot -Filter "*.stasis" -Recurse -File | ForEach-Object {
        $raw = Get-Content $_.FullName -Raw
        $rewritten = Transform-StasisSource $raw
        if (-not [string]::Equals($raw, $rewritten, [System.StringComparison]::Ordinal)) {
            Set-Content -Path $_.FullName -Value $rewritten -Encoding ascii
        }
    }

    $mappedArgs = @()
    foreach ($arg in $argsList) {
        $mappedArgs += (Map-IfStasisPath -ArgValue $arg -RepoRoot $repoRoot -MappedRoot $tempRoot -CurrentDir $currentDir)
    }

    $mappedCwd = $currentDir
    $repoPrefix = $repoRoot + [System.IO.Path]::DirectorySeparatorChar
    if ($mappedCwd.StartsWith($repoPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        $relCwd = $mappedCwd.Substring($repoPrefix.Length)
        $mappedCwd = Join-Path $tempRoot $relCwd
    } elseif ($mappedCwd.Equals($repoRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
        $mappedCwd = $tempRoot
    }

    if (-not (Test-Path $mappedCwd)) {
        $mappedCwd = $tempRoot
    }

    Push-Location $mappedCwd
    try {
        & dotnet $dllPath @mappedArgs
        exit $LASTEXITCODE
    } finally {
        Pop-Location
    }
}
finally {
    try {
        Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
    } catch {
        # best-effort cleanup
    }
}
