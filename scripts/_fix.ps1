[Console]::OutputEncoding = [Text.Encoding]::UTF8
$root = 'C:\Goose\tc'
$git = 'C:\Program Files\Git\cmd\git.exe'
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
Set-Location $root

# restore the truncated file
& $git checkout -- scripts/audit_residue.ps1
Write-Output "restored: $((Get-Item scripts\audit_residue.ps1).Length) bytes"

# --- audit_residue.ps1: rename in body, then replace line 0 (hardcoded $root) ---
$p = Join-Path $root 'scripts\audit_residue.ps1'
$t = [System.IO.File]::ReadAllText($p, [Text.Encoding]::UTF8)
$t = $t.Replace('traffic-core', 'loadloom-core')
$lines = $t.Split("`r`n")
$lines[0] = '$root = Split-Path -Parent $PSScriptRoot   # repo root; the script lives in <repo>/scripts/'
[System.IO.File]::WriteAllText($p, ($lines -join "`r`n"), $utf8NoBom)
Write-Output "audit_residue.ps1 -> $((Get-Item $p).Length) bytes"

# --- verify_launch.ps1: replace the hardcoded exe path (line index 1) ---
$p2 = Join-Path $root 'scripts\verify_launch.ps1'
$t2 = [System.IO.File]::ReadAllText($p2, [Text.Encoding]::UTF8)
$lines2 = $t2.Split("`r`n")
$lines2[1] = '$exe = Join-Path (Split-Path -Parent $PSScriptRoot) ''target\release\loadloom-desktop.exe'''
[System.IO.File]::WriteAllText($p2, ($lines2 -join "`r`n"), $utf8NoBom)
Write-Output "verify_launch.ps1 -> $((Get-Item $p2).Length) bytes"

Write-Output ''
Write-Output '--- audit_residue.ps1 first 3 lines ---'
Get-Content $p -Encoding UTF8 -TotalCount 3
Write-Output '--- verify_launch.ps1 first 3 lines ---'
Get-Content $p2 -Encoding UTF8 -TotalCount 3

Write-Output ''
Write-Output '--- syntax check ---'
foreach ($f in @($p, $p2)) {
  $err = $null
  [void][System.Management.Automation.Language.Parser]::ParseFile($f, [ref]$null, [ref]$err)
  Write-Output "$(Split-Path -Leaf $f): parse errors = $($err.Count)"
}
