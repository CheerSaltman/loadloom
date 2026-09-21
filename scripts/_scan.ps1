[Console]::OutputEncoding = [Text.Encoding]::UTF8
Set-Location 'C:\Goose\tc'

$exclude = 'node_modules|\\target\\|\\dist\\|\\\.git\\|package-lock'

function Show($title, $pattern, [string[]]$include) {
  Write-Output "=== $title ==="
  Get-ChildItem -Recurse -File -Include $include |
    Where-Object { $_.FullName -notmatch $exclude } |
    Select-String -Pattern $pattern -CaseSensitive:$false |
    ForEach-Object { "$($_.Path.Replace('C:\Goose\tc\','')):$($_.LineNumber): $($_.Line.Trim())" }
}

Show '_tc param' '_tc' @('*.rs','*.ts','*.tsx','*.md')
Show 'titles/identifier' '打流控制台|trafficconsole' @('*.md','*.json','*.html','*.tsx','*.rs','*.css')
Show 'docs/CONTRIB/SECURITY' 'traffic' @('CONTRIBUTING.md','SECURITY.md','*.md')
Show 'App.tsx/css' 'traffic|console' @('*.tsx','*.css')
