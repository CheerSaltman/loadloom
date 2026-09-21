[Console]::OutputEncoding = [Text.Encoding]::UTF8
Set-Location 'C:\Goose\tc'
foreach ($f in @('crates\traffic-core\Cargo.toml','scripts\audit_residue.ps1','scripts\verify_launch.ps1','rust-toolchain.toml','src-tauri\capabilities\default.json','vite.config.ts')) {
  Write-Output "########## $f ##########"
  Get-Content $f -Encoding UTF8
  Write-Output ""
}
Write-Output "########## docs/development.md (55-98) ##########"
Get-Content 'docs\development.md' -Encoding UTF8 | Select-Object -Skip 54 -First 44
