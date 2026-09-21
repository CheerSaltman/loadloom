[Console]::OutputEncoding = [Text.Encoding]::UTF8
$root = 'C:\Goose\tc'
$git = 'C:\Program Files\Git\cmd\git.exe'
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
Set-Location $root

& $git checkout -- scripts/audit_residue.ps1 scripts/verify_launch.ps1

# --- audit_residue.ps1 ---
$p = Join-Path $root 'scripts\audit_residue.ps1'
$t = [System.IO.File]::ReadAllText($p, [Text.Encoding]::UTF8)
$t = $t.Replace('traffic-core', 'loadloom-core')
$t = [regex]::Replace($t, '(?m)^\$root = ''.*''$', { '$root = Split-Path -Parent $PSScriptRoot   # repo root (this script lives in <repo>/scripts/)' })
[System.IO.File]::WriteAllText($p, $t, $utf8NoBom)

# --- verify_launch.ps1 ---
$p2 = Join-Path $root 'scripts\verify_launch.ps1'
$t2 = [System.IO.File]::ReadAllText($p2, [Text.Encoding]::UTF8)
$t2 = $t2.Replace('traffic-console-desktop', 'loadloom-desktop')
$t2 = [regex]::Replace($t2, '(?m)^\$exe = ''.*''$', { '$exe = Join-Path (Split-Path -Parent $PSScriptRoot) ''target\release\loadloom-desktop.exe''' })
[System.IO.File]::WriteAllText($p2, $t2, $utf8NoBom)

foreach ($f in @($p, $p2)) {
  $err = $null
  [void][System.Management.Automation.Language.Parser]::ParseFile($f, [ref]$null, [ref]$err)
  $lc = (Get-Content $f).Count
  Write-Output "$(Split-Path -Leaf $f): $((Get-Item $f).Length) bytes, $lc lines, parse errors = $($err.Count)"
  Get-Content $f -Encoding UTF8 -TotalCount 3 | ForEach-Object { Write-Output "    | $_" }
  Write-Output "    ... occurrence counts: loadloom=$((Select-String -Path $f -Pattern 'loadloom' -AllMatches).Matches.Count) old-name=$((Select-String -Path $f -Pattern 'traffic' -AllMatches).Matches.Count)"
}
