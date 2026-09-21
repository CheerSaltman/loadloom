[Console]::OutputEncoding = [Text.Encoding]::UTF8
Set-Location 'C:\Goose\tc'

# Order matters: longest / most specific patterns first.
$rules = @(
  @('loadloom_desktop_lib', 'loadloom_desktop_lib'),
  @('loadloom-desktop',     'loadloom-desktop'),
  @('loadloom-core',                'loadloom-core'),
  @('loadloom_core',                'loadloom_core'),
  @('com.agentic.loadloom',  'com.agentic.loadloom'),
  @('LoadLoom',              'LoadLoom'),
  @('LoadLoom',             'LoadLoom'),
  @('LOADLOOM',             'LOADLOOM'),
  @('loadloom',             'loadloom'),
  @('loadloom://',                  'loadloom://'),
  @('_ll=',                        '_ll='),
  @('高并发打流控制台',             'LoadLoom · 高并发打流控制台'),
  @('loadloom/0.4 (+authorized-load-test)', 'loadloom/0.4 (+authorized-load-test)')
)

$exts = @('*.md','*.rs','*.ts','*.tsx','*.json','*.html','*.toml','*.ps1','*.js','*.css','*.yml','*.yaml')
$skip = 'node_modules|\\target\\|\\dist\\|\\\.git\\|_scan\.ps1|_dump\.ps1|_availcheck\.ps1'

$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
$files = Get-ChildItem -Recurse -File -Include $exts | Where-Object { $_.FullName -notmatch $skip }

$changed = @()
foreach ($f in $files) {
  $text = [System.IO.File]::ReadAllText($f.FullName, [System.Text.Encoding]::UTF8)
  $orig = $text
  foreach ($r in $rules) { $text = $text.Replace($r[0], $r[1]) }
  if ($text -ne $orig) {
    [System.IO.File]::WriteAllText($f.FullName, $text, $utf8NoBom)
    $changed += $f.FullName.Replace('C:\Goose\tc\', '')
  }
}
Write-Output "changed files: $($changed.Count)"
$changed | Sort-Object | ForEach-Object { Write-Output "  $_" }

# rename the core crate directory
if (Test-Path 'crates\loadloom-core') {
  git mv 'crates/loadloom-core' 'crates/loadloom-core'
  Write-Output "git mv crates/loadloom-core -> crates/loadloom-core : $LASTEXITCODE"
}
