# Package a Windows release zip next to the release binary.
# For MSI (Start Menu / ARP uninstall), see scripts/build-msi.ps1.
param(
    [string]$OutDir = "dist/captureview-windows-x64",
    [string]$ZipPath = "dist/captureview-windows-x64.zip",
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..")
Set-Location $root

if (-not $SkipBuild) {
    Write-Host "Building release binary..."
    cargo build --release -p captureview
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

$exe = Join-Path $root "target/release/captureview.exe"
if (-not (Test-Path $exe)) {
    throw "Missing $exe"
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
Copy-Item $exe (Join-Path $OutDir "captureview.exe") -Force
Copy-Item (Join-Path $root "README.md") (Join-Path $OutDir "README.md") -Force
Copy-Item (Join-Path $root "LICENSE") (Join-Path $OutDir "LICENSE") -Force
$licOut = Join-Path $OutDir "licenses"
if (Test-Path $licOut) { Remove-Item $licOut -Recurse -Force }
New-Item -ItemType Directory -Force -Path $licOut | Out-Null
if (Test-Path (Join-Path $root "licenses")) {
    Copy-Item (Join-Path $root "licenses\*") $licOut -Force
}
# Spout2 (BSD) — bundled; never ship NDI DLLs.
$spoutSrc = Join-Path $root "third_party/spout"
foreach ($dll in @("SpoutLibrary.dll", "spout_bridge.dll")) {
    $src = Join-Path $spoutSrc $dll
    if (-not (Test-Path $src)) {
        $alt = Join-Path $root "native/spout/bin/$dll"
        if (Test-Path $alt) { $src = $alt }
    }
    if (Test-Path $src) {
        Copy-Item $src (Join-Path $OutDir $dll) -Force
    } else {
        Write-Warning "Missing $dll — Spout export will extract/fail at runtime"
    }
}
if (Test-Path (Join-Path $spoutSrc "licence.txt")) {
    Copy-Item (Join-Path $spoutSrc "licence.txt") (Join-Path $OutDir "licenses/SPOUT-BSD.txt") -Force
}
$assets = Join-Path $OutDir "assets"
New-Item -ItemType Directory -Force -Path $assets | Out-Null
Copy-Item (Join-Path $root "assets/kapturkard-logo.png") (Join-Path $assets "kapturkard-logo.png") -Force

@"
CaptureView / KapturKard Windows x64

Run: captureview.exe
Mock source works without a capture card.
Logs: %APPDATA%\CaptureView\logs\
Spout: SpoutLibrary.dll is bundled (see docs/EXPORT.md). NDI is BYO from https://ndi.video/
"@ | Set-Content -Encoding utf8 (Join-Path $OutDir "RELEASE.txt")

New-Item -ItemType Directory -Force -Path (Split-Path $ZipPath) | Out-Null
if (Test-Path $ZipPath) { Remove-Item $ZipPath -Force }
Compress-Archive -Path (Join-Path $OutDir "*") -DestinationPath $ZipPath -Force
Write-Host "Wrote $ZipPath"
Get-Item $ZipPath | Format-List FullName, Length, LastWriteTime
