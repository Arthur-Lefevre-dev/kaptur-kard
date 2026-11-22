# Long soak helper for CaptureView on Windows (Arthur).
# Samples Working Set / Private Bytes while the app runs; optional hot-plug checklist.
#
# Usage:
#   .\scripts\soak-windows.ps1 -Minutes 15
#   .\scripts\soak-windows.ps1 -Minutes 5 -Exe .\target\release\captureview.exe
#   .\scripts\soak-windows.ps1 -AttachOnly   # sample an already-running captureview

param(
    [double]$Minutes = 10,
    [string]$Exe = "",
    [switch]$AttachOnly,
    [string]$OutCsv = ""
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
if (-not $OutCsv) {
    $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
    $OutCsv = Join-Path $root "target\soak-$stamp.csv"
}

function Find-CaptureView {
    Get-Process -Name "captureview" -ErrorAction SilentlyContinue | Select-Object -First 1
}

$proc = $null
$startedHere = $false

if ($AttachOnly) {
    $proc = Find-CaptureView
    if (-not $proc) { throw "No captureview process found (AttachOnly)." }
} else {
    if (-not $Exe) {
        $cand = Join-Path $root "target\release\captureview.exe"
        if (-not (Test-Path $cand)) {
            Write-Host "Building release captureview..."
            Push-Location $root
            cargo build -p captureview --release
            Pop-Location
        }
        $Exe = $cand
    }
    if (-not (Test-Path $Exe)) { throw "Executable not found: $Exe" }
    Write-Host "Starting $Exe"
    $pInfo = Start-Process -FilePath $Exe -PassThru -WorkingDirectory $root
    Start-Sleep -Seconds 3
    $proc = Get-Process -Id $pInfo.Id -ErrorAction SilentlyContinue
    if (-not $proc) { $proc = Find-CaptureView }
    if (-not $proc) { throw "captureview failed to start." }
    $startedHere = $true
}

Write-Host "Soak PID=$($proc.Id) for $Minutes min → $OutCsv"
"timestamp_utc,elapsed_s,working_set_mb,private_mb,cpu_sec" | Set-Content -Path $OutCsv -Encoding utf8

$end = (Get-Date).AddMinutes($Minutes)
$t0 = Get-Date
$wsMin = [double]::MaxValue
$wsMax = 0.0
$wsSum = 0.0
$n = 0
$onePctNotes = @()

Write-Host ""
Write-Host "Checklist (manual while soak runs):"
Write-Host "  [ ] Hot-plug: unplug capture card USB → mock/fallback without freeze"
Write-Host "  [ ] Hot-plug: replug → device reappears, capture restarts"
Write-Host "  [ ] No-signal HDMI → 'Pas de signal' without audio glitch storm"
Write-Host "  [ ] F2 metrics: watch 1% low over the run (note dips)"
Write-Host "  [ ] Discord: Playing KapturKard stays up (details/state update)"
Write-Host ""

while ((Get-Date) -lt $end) {
    try {
        $proc.Refresh()
    } catch {
        Write-Host "Process exited early."
        break
    }
    $ws = [math]::Round($proc.WorkingSet64 / 1MB, 1)
    $priv = [math]::Round($proc.PrivateMemorySize64 / 1MB, 1)
    $cpu = [math]::Round($proc.TotalProcessorTime.TotalSeconds, 2)
    $elapsed = [math]::Round(((Get-Date) - $t0).TotalSeconds, 1)
    $line = "{0},{1},{2},{3},{4}" -f (Get-Date).ToUniversalTime().ToString("o"), $elapsed, $ws, $priv, $cpu
    Add-Content -Path $OutCsv -Value $line
    if ($ws -lt $wsMin) { $wsMin = $ws }
    if ($ws -gt $wsMax) { $wsMax = $ws }
    $wsSum += $ws
    $n++
    Write-Host ("[{0,7:N0}s] WS={1,6:N1} Mo  Private={2,6:N1} Mo  CPU={3}s" -f $elapsed, $ws, $priv, $cpu)
    Start-Sleep -Seconds 5
}

$wsAvg = if ($n -gt 0) { [math]::Round($wsSum / $n, 1) } else { 0 }
Write-Host ""
Write-Host "=== Soak summary ==="
Write-Host ("samples={0}  WS min/avg/max = {1} / {2} / {3} Mo" -f $n, $wsMin, $wsAvg, $wsMax)
Write-Host "CSV: $OutCsv"
Write-Host "Also check %APPDATA%\CaptureView\logs\ for soak_sample lines (ram_mb, one_pct, dropped)."

if ($startedHere -and $proc -and -not $proc.HasExited) {
    Write-Host "Leaving captureview running (close the window when done)."
}
