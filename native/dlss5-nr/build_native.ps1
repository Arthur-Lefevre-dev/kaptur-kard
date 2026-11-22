# Build CaptureView DLSS 5 NR native bridge (MIT — adapted from ComfyUI-DLSS5-NR).
$ErrorActionPreference = 'Stop'

function Fail([string]$Message) {
    Write-Host "[CaptureView DLSS5-NR] ERROR: $Message" -ForegroundColor Red
    exit 1
}

$Native = $PSScriptRoot
$Bin = Join-Path $Native 'bin'
$CallerOut = Join-Path $Native 'runtime\caller'
$RuntimeOut = Join-Path $Native 'runtime'

$pf86 = [Environment]::GetFolderPath('ProgramFilesX86')
$pf = [Environment]::GetFolderPath('ProgramFiles')
$programRoots = @($pf86, $pf) | Where-Object { -not [string]::IsNullOrWhiteSpace($_) -and (Test-Path $_) } | Select-Object -Unique

$clCandidates = @()
foreach ($programRoot in $programRoots) {
    $vsRoot = Join-Path $programRoot 'Microsoft Visual Studio'
    if (-not (Test-Path $vsRoot)) { continue }
    $clCandidates += Get-ChildItem -Path $vsRoot -Filter cl.exe -File -Recurse -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -match '\\VC\\Tools\\MSVC\\[^\\]+\\bin\\Hostx64\\x64\\cl\.exe$' } |
        Select-Object -ExpandProperty FullName
}
$clCandidates = $clCandidates | Select-Object -Unique
if (-not $clCandidates -or $clCandidates.Count -eq 0) {
    Fail 'MSVC x64 cl.exe not found. Install Desktop development with C++.'
}

$cl = $clCandidates | Sort-Object {
    if ($_ -match '\\MSVC\\([^\\]+)\\bin\\Hostx64') {
        try { [version]$Matches[1] } catch { [version]'0.0' }
    } else { [version]'0.0' }
} -Descending | Select-Object -First 1

$msvcVersionDir = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $cl)))
if ($cl -match '^(.*\\MSVC\\[^\\]+)\\bin\\Hostx64\\x64\\cl\.exe$') {
    $msvcVersionDir = $Matches[1]
}
$msvcInclude = Join-Path $msvcVersionDir 'include'
$msvcLib = Join-Path $msvcVersionDir 'lib\x64'

$sdkRoot = $null
foreach ($programRoot in $programRoots) {
    $candidateSdk = Join-Path $programRoot 'Windows Kits\10'
    if (Test-Path (Join-Path $candidateSdk 'Include')) { $sdkRoot = $candidateSdk; break }
}
if (-not $sdkRoot) { Fail 'Windows Kits\10 not found.' }

$sdkIncludeRoot = Join-Path $sdkRoot 'Include'
$sdkLibRoot = Join-Path $sdkRoot 'Lib'
$sdkVersions = Get-ChildItem -Path $sdkIncludeRoot -Directory -ErrorAction SilentlyContinue |
    Where-Object {
        (Test-Path (Join-Path $_.FullName 'um\windows.h')) -and
        (Test-Path (Join-Path $sdkLibRoot ($_.Name + '\um\x64\d3d12.lib')))
    } |
    Sort-Object { try { [version]$_.Name } catch { [version]'0.0' } } -Descending
if (-not $sdkVersions) { Fail 'No usable Windows SDK with d3d12.lib.' }
$sdkVersion = $sdkVersions[0].Name
$sdkIncBase = Join-Path $sdkIncludeRoot $sdkVersion
$sdkLibBase = Join-Path $sdkLibRoot $sdkVersion

$includeDirs = @(
    $msvcInclude,
    (Join-Path $sdkIncBase 'ucrt'),
    (Join-Path $sdkIncBase 'shared'),
    (Join-Path $sdkIncBase 'um'),
    (Join-Path $sdkIncBase 'winrt'),
    (Join-Path $sdkIncBase 'cppwinrt')
) | Where-Object { Test-Path $_ }

$libDirs = @(
    $msvcLib,
    (Join-Path $sdkLibBase 'ucrt\x64'),
    (Join-Path $sdkLibBase 'um\x64')
) | Where-Object { Test-Path $_ }

$clDir = Split-Path -Parent $cl
$env:PATH = "$clDir;$env:PATH"
$env:INCLUDE = ($includeDirs -join ';')
$env:LIB = ($libDirs -join ';')

New-Item -ItemType Directory -Force -Path $Bin | Out-Null
New-Item -ItemType Directory -Force -Path $CallerOut | Out-Null
New-Item -ItemType Directory -Force -Path $RuntimeOut | Out-Null

function Run-Cl([string[]]$Arguments, [string]$StepName) {
    Write-Host $StepName
    & $cl @Arguments
    if ($LASTEXITCODE -ne 0) { Fail "$StepName failed ($LASTEXITCODE)" }
}

$common = @('/nologo', '/std:c++17', '/EHsc', '/MT', '/LD')
foreach ($inc in $includeDirs) { $common += ('/I' + $inc) }
$libPathArgs = $libDirs | ForEach-Object { '/LIBPATH:' + $_ }

$callerDll = Join-Path $CallerOut 'nvngx.dll_comfy.dll'
Run-Cl ($common + @('/Od', (Join-Path $Native 'caller_shim.cpp'), '/link', ('/OUT:' + $callerDll)) + $libPathArgs) '[1/3] Caller shim'

$bridgeDll = Join-Path $Bin 'dlss5nr_bridge.dll'
Run-Cl ($common + @(
    '/O2',
    (Join-Path $Native 'dlss5nr_bridge.cpp'),
    (Join-Path $Native 'nvof_flow.cpp'),
    '/link',
    ('/OUT:' + $bridgeDll),
    'd3d12.lib', 'd3d11.lib', 'dxgi.lib', 'ole32.lib'
) + $libPathArgs) '[2/3] DLSS5 NR bridge'

# FG bridge links its own copy of nvof_flow (separate DLL address space).
$fgDll = Join-Path $Bin 'dlss5g_bridge.dll'
Run-Cl ($common + @(
    '/O2',
    (Join-Path $Native 'dlss5g_bridge.cpp'),
    (Join-Path $Native 'nvof_flow.cpp'),
    '/link',
    ('/OUT:' + $fgDll),
    'd3d12.lib', 'd3d11.lib', 'dxgi.lib', 'ole32.lib', 'user32.lib'
) + $libPathArgs) '[3/3] DLSS-G EvaluateFeature + present-queue bridge'

Get-ChildItem -Path $Native -Filter '*.obj' -File -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue
Get-ChildItem -Path $Native -Filter '*.exp' -File -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue
Get-ChildItem -Path $Native -Filter '*.lib' -File -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue

# Placeholder so Arthur sees where to drop the proprietary runtime.
$readme = Join-Path $RuntimeOut 'PLACE_nvngx_dlssnr_HERE.txt'
@"
Place your legally obtained nvngx_dlssnr.dll (and optional nvngx_dlssg.dll) in this folder.

Then either:
  set CAPTUREVIEW_DLSSNR_DIR=$RuntimeOut
or copy this whole runtime\ tree to %APPDATA%\CaptureView\dlss5-nr\

Also need dlss5nr_bridge.dll / dlss5g_bridge.dll from this build script (copied beside runtime or from bin\).

Official GPU support: RTX 50-series NR + Ada+ FG typical; driver 616.56+. See docs/DLSS.md.
"@ | Set-Content -Path $readme -Encoding UTF8

Write-Host "NR bridge: $bridgeDll" -ForegroundColor Green
Write-Host "FG bridge: $fgDll" -ForegroundColor Green
Write-Host "Shim:      $callerDll" -ForegroundColor Green
exit 0
