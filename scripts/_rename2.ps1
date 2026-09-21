[Console]::OutputEncoding = [Text.Encoding]::UTF8
$root = 'C:\Goose\tc'
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)

function Patch($rel, [scriptblock]$mutate) {
  $p = Join-Path $root $rel
  $t = [System.IO.File]::ReadAllText($p, [Text.Encoding]::UTF8)
  $before = $t
  $t = & $mutate $t
  if ($t -ne $before) {
    [System.IO.File]::WriteAllText($p, $t, $utf8NoBom)
    Write-Output "PATCHED $rel"
  } else {
    Write-Output "nochange $rel"
  }
}

# --- 1. version bump 0.3.0 -> 0.4.0 in files where it is unambiguous ---
foreach ($rel in @('package.json', 'Cargo.toml', 'src-tauri\tauri.conf.json', 'README.md', 'README.en.md', 'docs\development.md')) {
  Patch $rel { param($t) $t.Replace('0.3.0', '0.4.0') }
}

# --- 2. package-lock.json: only the root package entry, never dependency versions ---
Patch 'package-lock.json' { param($t) [regex]::Replace($t, '("name": "loadloom",\s*"version": ")0\.3\.0(")', '${1}0.4.0${2}') }

# --- 3. CSS class prefix tc- -> ll- ---
foreach ($rel in @('src\App.tsx', 'src\index.css')) {
  Patch $rel { param($t) $t.Replace('tc-live-dot', 'll-live-dot').Replace('tc-pulse', 'll-pulse') }
}

# --- 4. helper scripts: derive paths from script location (no stale absolute paths) ---
Patch 'scripts\verify_launch.ps1' { param($t) [regex]::Replace($t, "(?m)^\$exe = '.*'$", "`$exe = Join-Path (Split-Path -Parent `$PSScriptRoot) 'target\release\loadloom-desktop.exe'") }
Patch 'scripts\audit_residue.ps1' { param($t) [regex]::Replace($t, "(?m)^\$root = '.*'$", "`$root = Split-Path -Parent `$PSScriptRoot") }

Write-Output ''
Write-Output '--- leftovers (0.3.0 / tc- / stale abs paths) ---'
Get-ChildItem -Recurse -File -Include *.md,*.json,*.toml,*.rs,*.ts,*.tsx,*.html,*.ps1,*.yml,*.css |
  Where-Object { $_.FullName -notmatch 'node_modules|\\target\\|\\dist\\|\\.git\\|_scan|_rename|_dump|_availcheck' } |
  Select-String -Pattern '0\.3\.0|tc-live-dot|tc-pulse|Traffic Console|traffic-console|traffic_core|traffic-core' |
  ForEach-Object { "$($_.Path.Replace('C:\Goose\tc\','')):$($_.LineNumber): $($_.Line.Trim())" }
