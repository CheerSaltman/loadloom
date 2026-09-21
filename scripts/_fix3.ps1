[Console]::OutputEncoding = [Text.Encoding]::UTF8
Set-Location 'C:\Goose\tc'
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
$p = 'scripts\audit_residue.ps1'
$t = [System.IO.File]::ReadAllText($p, [Text.Encoding]::UTF8)
$t = [regex]::Replace($t, '(?m)^\$root = ''.*''', { '$root = Split-Path -Parent $PSScriptRoot   # repo root (this script lives in <repo>/scripts/)' })
[System.IO.File]::WriteAllText($p, $t, $utf8NoBom)
$p2 = 'scripts\verify_launch.ps1'
$t2 = [System.IO.File]::ReadAllText($p2, [Text.Encoding]::UTF8)
$t2 = [regex]::Replace($t2, '(?m)^\$exe = ''.*''', { '$exe = Join-Path (Split-Path -Parent $PSScriptRoot) ''target\release\loadloom-desktop.exe''' })
[System.IO.File]::WriteAllText($p2, $t2, $utf8NoBom)
foreach ($f in @($p, $p2)) {
  $err = $null
  [void][System.Management.Automation.Language.Parser]::ParseFile($f, [ref]$null, [ref]$err)
  Write-Output "$(Split-Path -Leaf $f): $((Get-Content $f).Count) lines, errors=$($err.Count), stale='$((Select-String -Path $f -Pattern 'traffic' -AllMatches).Matches.Count)'"
  Get-Content $f -Encoding UTF8 -TotalCount 2 | ForEach-Object { Write-Output "    | $_" }
}
