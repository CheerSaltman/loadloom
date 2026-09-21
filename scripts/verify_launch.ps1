$ErrorActionPreference = 'Stop'
$exe = Join-Path (Split-Path -Parent $PSScriptRoot) 'target\release\loadloom-desktop.exe'

function Get-BrowserCount {
  @(Get-Process -ErrorAction SilentlyContinue |
    Where-Object { $_.ProcessName -match '^(msedge|chrome|firefox|iexplore)$' }).Count
}

$browserBefore = Get-BrowserCount
Write-Output "browser_processes_before=$browserBefore"

$p = Start-Process -FilePath $exe -PassThru
Start-Sleep -Seconds 10
$p.Refresh()

Write-Output "pid=$($p.Id)"
Write-Output "has_exited=$($p.HasExited)"
if ($p.HasExited) { Write-Output "exit_code=$($p.ExitCode)" }

if (-not $p.HasExited) {
  Write-Output "main_window_title=$($p.MainWindowTitle)"
  Write-Output "main_window_handle=$($p.MainWindowHandle)"
  Write-Output "threads=$($p.Threads.Count)"
  Write-Output "working_set_mb=$([math]::Round($p.WorkingSet64/1MB,1))"

  # A headless-core desktop app must NOT open any listening socket
  # (the old browser-mode build served HTTP on 127.0.0.1:18080).
  $listeners = @(Get-NetTCPConnection -OwningProcess $p.Id -State Listen -ErrorAction SilentlyContinue)
  Write-Output "listening_sockets=$($listeners.Count)"
  foreach ($l in $listeners) { Write-Output "  LISTEN $($l.LocalAddress):$($l.LocalPort)" }

  Write-Output '--- child processes ---'
  Get-CimInstance Win32_Process -Filter "ParentProcessId=$($p.Id)" |
    Select-Object -ExpandProperty Name | Sort-Object -Unique | ForEach-Object { Write-Output "  $_" }

  Write-Output '--- webview2 renderer count (embedded, expected) ---'
  Write-Output "msedgewebview2=$(@(Get-Process msedgewebview2 -ErrorAction SilentlyContinue).Count)"

  $browserAfter = Get-BrowserCount
  Write-Output "browser_processes_after=$browserAfter"

  Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
  Start-Sleep -Seconds 2
  Write-Output "terminated=$((Get-Process -Id $p.Id -ErrorAction SilentlyContinue) -eq $null)"
}
