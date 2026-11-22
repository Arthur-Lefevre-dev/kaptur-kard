# Package a Windows release zip next to the release binary.
param(
    [string]$OutDir = "dist/captureview-windows-x64",
    [string]$ZipPath = "dist/captureview-windows-x64.zip"
)

$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..")
Set-Location $root

Write-Host "Building release binary..."
cargo build --release -p captureview
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$exe = Join-Path $root "target/release/captureview.exe"
if (-not (Test-Path $exe)) {
    throw "Missing $exe"
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
Copy-Item $exe (Join-Path $OutDir "captureview.exe") -Force
Copy-Item (Join-Path $root "README.md") (Join-Path $OutDir "README.md") -Force
Copy-Item (Join-Path $root "LICENSE") (Join-Path $OutDir "LICENSE") -Force
if (Test-Path (Join-Path $root "licenses")) {
    Copy-Item (Join-Path $root "licenses") (Join-Path $OutDir "licenses") -Recurse -Force
}
$assets = Join-Path $OutDir "assets"
New-Item -ItemType Directory -Force -Path $assets | Out-Null
Copy-Item (Join-Path $root "assets/kapturkard-logo.png") (Join-Path $assets "kapturkard-logo.png") -Force

@"
CaptureView / KapturKard Windows x64

Run: captureview.exe
Mock source works without a capture card.
Logs: %APPDATA%\CaptureView\logs\
"@ | Set-Content -Encoding utf8 (Join-Path $OutDir "RELEASE.txt")

New-Item -ItemType Directory -Force -Path (Split-Path $ZipPath) | Out-Null
if (Test-Path $ZipPath) { Remove-Item $ZipPath -Force }
Compress-Archive -Path (Join-Path $OutDir "*") -DestinationPath $ZipPath -Force
Write-Host "Wrote $ZipPath"
Get-Item $ZipPath | Format-List FullName, Length, LastWriteTime
