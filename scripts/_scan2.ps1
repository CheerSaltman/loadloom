[Console]::OutputEncoding = [Text.Encoding]::UTF8
Set-Location 'C:\Goose\tc'
$exclude = 'node_modules|\\target\\|\\dist\\|\\.git\\|package-lock'

Write-Output '--- tc- prefixed classes ---'
Get-ChildItem -Recurse -File -Include *.css,*.tsx,*.ts -Path src |
  Select-String -Pattern 'tc-' |
  ForEach-Object { "$($_.Filename):$($_.LineNumber): $($_.Line.Trim())" }

Write-Output ''
Write-Output '--- version 0.3.0 occurrences ---'
Get-ChildItem -Recurse -File -Include *.md,*.json,*.toml,*.rs,*.ts,*.tsx,*.html,*.ps1,*.yml,*.lock |
  Where-Object { $_.FullName -notmatch $exclude } |
  Select-String -Pattern '0\.3\.0' |
  ForEach-Object { "$($_.Path.Replace('C:\Goose\tc\','')):$($_.LineNumber): $($_.Line.Trim())" }

Write-Output ''
Write-Output '--- App.tsx lines 88-112 ---'
Get-Content 'src\App.tsx' -Encoding UTF8 | Select-Object -Skip 87 -First 25
