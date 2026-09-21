$root = Split-Path -Parent $PSScriptRoot   # repo root (this script lives in <repo>/scripts/)
$pattern = 'egui|eframe|webbrowser|axum|include_str!|Tkinter|PyQt|PySide|WinForms|wxWidgets|18080|ws://|/api/|Chart\.js|cdn\.|cdnjs|unpkg|jsdelivr'
$ext = @('.rs', '.ts', '.tsx', '.json', '.toml', '.html', '.css', '.mjs', '.js')

Write-Output '=== RESIDUE SCAN (tracked source only; target/ node_modules/ dist/ excluded) ==='
$hits = Get-ChildItem -Path $root -Recurse -File -ErrorAction SilentlyContinue |
  Where-Object { $ext -contains $_.Extension } |
  Where-Object { $_.FullName -notmatch '\\target\\|\\node_modules\\|\\dist\\' } |
  Select-String -Pattern $pattern

if ($hits.Count -eq 0) {
  Write-Output 'CLEAN: 0 matches'
} else {
  foreach ($h in $hits) {
    Write-Output ("{0}:{1}: {2}" -f $h.Path.Replace("$root\", ''), $h.LineNumber, $h.Line.Trim())
  }
}

Write-Output ''
Write-Output '=== DEPENDENCY GUARD: no GUI/transport crates anywhere ==='
Get-ChildItem -Path $root -Recurse -Filter 'Cargo.toml' -File -ErrorAction SilentlyContinue |
  Where-Object { $_.FullName -notmatch '\\target\\' } |
  ForEach-Object {
    $rel = $_.FullName.Replace("$root\", '')
    $bad = Select-String -Path $_.FullName -Pattern '^\s*(egui|eframe|webbrowser|axum|iced|gtk|qt|winit)\s*='
    if ($bad) { $bad | ForEach-Object { Write-Output ("LEAK {0}:{1}: {2}" -f $rel, $_.LineNumber, $_.Line.Trim()) } }
    else { Write-Output "OK   $rel" }
  }

Write-Output ''
Write-Output '=== loadloom-core dependency closure (must be GUI-free) ==='
$coreToml = Join-Path $root 'crates\loadloom-core\Cargo.toml'
(Get-Content $coreToml) | Where-Object { $_ -match '^\[dependencies\]' } | Out-Null
$inDeps = $false
Get-Content $coreToml | ForEach-Object {
  if ($_ -match '^\[') { $inDeps = ($_ -match '^\[dependencies\]') }
  elseif ($inDeps -and $_.Trim()) { Write-Output "  $_" }
}
