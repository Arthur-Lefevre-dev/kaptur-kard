# Build CaptureView Spout send bridge (loads SpoutLibrary.dll at runtime).
$ErrorActionPreference = 'Stop'

function Fail([string]$Message) {
    Write-Host "[CaptureView Spout] ERROR: $Message" -ForegroundColor Red
    exit 1
}

$Native = $PSScriptRoot
$Bin = Join-Path $Native 'bin'
$ThirdParty = Join-Path (Split-Path $Native -Parent) (Join-Path '..' 'third_party\spout')
$ThirdParty = (Resolve-Path (Join-Path $Native '..\..\third_party\spout')).Path
$SpoutDll = Join-Path $ThirdParty 'SpoutLibrary.dll'
if (-not (Test-Path $SpoutDll)) {
    Fail "Missing $SpoutDll — place Spout2 SpoutLibrary.dll under third_party/spout/"
}

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

$msvcVersionDir = $null
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
        (Test-Path (Join-Path $sdkLibRoot ($_.Name + '\um\x64')))
    } |
    Sort-Object { try { [version]$_.Name } catch { [version]'0.0' } } -Descending
if (-not $sdkVersions) { Fail 'No usable Windows SDK.' }
$sdkVersion = $sdkVersions[0].Name
$sdkIncBase = Join-Path $sdkIncludeRoot $sdkVersion
$sdkLibBase = Join-Path $sdkLibRoot $sdkVersion

$includeDirs = @(
    $Native,
    $ThirdParty,
    $msvcInclude,
    (Join-Path $sdkIncBase 'ucrt'),
    (Join-Path $sdkIncBase 'shared'),
    (Join-Path $sdkIncBase 'um')
) | Where-Object { Test-Path $_ }

$libDirs = @(
    $msvcLib,
    (Join-Path $sdkLibBase 'ucrt\x64'),
    (Join-Path $sdkLibBase 'um\x64')
) | Where-Object { Test-Path $_ }

$clDir = Split-Path -Parent $cl
$env:PATH = "$clDir;$env:PATH"

New-Item -ItemType Directory -Force -Path $Bin | Out-Null
$obj = Join-Path $Bin 'spout_bridge.obj'
$out = Join-Path $Bin 'spout_bridge.dll'

$includeArgs = @()
foreach ($d in $includeDirs) { $includeArgs += "/I$d" }
$libArgs = @()
foreach ($d in $libDirs) { $libArgs += "/LIBPATH:$d" }

Write-Host "[CaptureView Spout] Building spout_bridge.dll with $cl"
& $cl /nologo /EHsc /MD /O2 /DNDEBUG /DSPOUT_BRIDGE_EXPORTS /c `
    (Join-Path $Native 'spout_bridge.cpp') `
    /Fo$obj `
    @includeArgs
if ($LASTEXITCODE -ne 0) { Fail 'cl compile failed' }

$link = Join-Path $clDir 'link.exe'
& $link /nologo /DLL /OUT:$out $obj `
    @libArgs `
    user32.lib gdi32.lib opengl32.lib advapi32.lib ole32.lib
if ($LASTEXITCODE -ne 0) { Fail 'link failed' }

Copy-Item $SpoutDll (Join-Path $Bin 'SpoutLibrary.dll') -Force
Write-Host "[CaptureView Spout] OK → $out (+ SpoutLibrary.dll)"
Get-Item $out, (Join-Path $Bin 'SpoutLibrary.dll') | Format-Table Name, Length -AutoSize
