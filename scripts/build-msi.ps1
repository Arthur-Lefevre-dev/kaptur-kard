# Build captureview-windows-x64.msi with WiX v5 from a staged Windows package folder.
# Prerequisites: .NET SDK 6+, `dotnet tool install --global wix`, UI extension.
param(
    [string]$StageDir = "dist/captureview-windows-x64",
    [string]$MsiPath = "dist/captureview-windows-x64.msi",
    [string]$ProductVersion = "",
    [switch]$SkipStage
)

$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..")
Set-Location $root

function Get-WorkspaceVersion {
    $cargo = Get-Content (Join-Path $root "Cargo.toml") -Raw
    if ($cargo -match '(?m)^\[workspace\.package\][\s\S]*?^version\s*=\s*"([^"]+)"') {
        return $Matches[1]
    }
    throw "Could not parse workspace.package.version from Cargo.toml"
}

if (-not $ProductVersion) {
    $ProductVersion = Get-WorkspaceVersion
}

# MSI ProductVersion allows at most 3 numeric parts (major.minor.build).
$msiVersion = ($ProductVersion -split '[^0-9]+' | Where-Object { $_ -ne '' } | Select-Object -First 3) -join '.'
if (-not ($msiVersion -match '^\d+\.\d+\.\d+$')) {
    throw "ProductVersion '$ProductVersion' must resolve to major.minor.build (got '$msiVersion')"
}

if (-not $SkipStage) {
    Write-Host "Staging Windows package (zip payload)..."
    & (Join-Path $PSScriptRoot "package-windows.ps1") -OutDir $StageDir -ZipPath "dist/captureview-windows-x64.zip"
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

$stage = Resolve-Path $StageDir
$required = @(
    "captureview.exe",
    "SpoutLibrary.dll",
    "spout_bridge.dll",
    "README.md",
    "LICENSE",
    "RELEASE.txt",
    "assets\kapturkard-logo.png",
    "licenses\AMD-FSR1-MIT.txt",
    "licenses\NVIDIA-NIS-MIT.txt",
    "licenses\NVIDIA-STREAMLINE-NOTE.txt",
    "licenses\SPOUT-BSD.txt"
)
foreach ($rel in $required) {
    $p = Join-Path $stage $rel
    if (-not (Test-Path $p)) {
        throw "Missing staged file required for MSI: $p"
    }
}

$wix = Get-Command wix -ErrorAction SilentlyContinue
if (-not $wix) {
    Write-Host "Installing WiX .NET tool (global)..."
    dotnet tool install --global wix --version 5.0.2
    $env:Path = [System.Environment]::GetEnvironmentVariable("Path", "Machine") + ";" +
                [System.Environment]::GetEnvironmentVariable("Path", "User")
    $wix = Get-Command wix -ErrorAction Stop
}

# Ensure UI extension for WixUI_FeatureTree (idempotent).
& wix extension add -g WixToolset.UI.wixext/5.0.2 2>$null | Out-Null

$msiDir = Split-Path $MsiPath
if ($msiDir) {
    New-Item -ItemType Directory -Force -Path $msiDir | Out-Null
}
if (Test-Path $MsiPath) { Remove-Item $MsiPath -Force }

$wxs = Join-Path $root "wix\captureview.wxs"
Write-Host "Building MSI $MsiPath (version $msiVersion)..."
& wix build `
    -arch x64 `
    -ext WixToolset.UI.wixext `
    -d "ProductVersion=$msiVersion" `
    -d "StageDir=$stage" `
    -d "RepoRoot=$root" `
    -o $MsiPath `
    -pdbtype none `
    $wxs

if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
if (-not (Test-Path $MsiPath)) {
    throw "WiX completed but MSI missing: $MsiPath"
}

Write-Host "Wrote $MsiPath"
Get-Item $MsiPath | Format-List FullName, Length, LastWriteTime
