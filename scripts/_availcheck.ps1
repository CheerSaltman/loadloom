[Console]::OutputEncoding = [Text.Encoding]::UTF8
$names = @('loadloom','flowloom','pulseloom','surgeloom','tideforge','fluxgate','loomgate','loomdeck')
foreach ($n in $names) {
  $crate = '?'
  try {
    $r = Invoke-RestMethod -Uri "https://crates.io/api/v1/crates?q=$n" -Headers @{'User-Agent'='avail-check'} -TimeoutSec 20
    $exact = @($r.crates | Where-Object { $_.name -eq $n })
    $crate = if ($exact.Count -gt 0) { 'TAKEN' } else { 'free' }
  } catch { $crate = 'ERR' }

  $npm = '?'
  try { Invoke-RestMethod -Uri "https://registry.npmjs.org/-/package/$n" -TimeoutSec 20 | Out-Null; $npm = 'TAKEN' }
  catch { if ($_.Exception.Response.StatusCode.value__ -eq 404) { $npm = 'free' } else { $npm = 'ERR' } }

  $py = '?'
  try { Invoke-RestMethod -Uri "https://pypi.org/pypi/$n/json" -TimeoutSec 20 | Out-Null; $py = 'TAKEN' }
  catch { if ($_.Exception.Response.StatusCode.value__ -eq 404) { $py = 'free' } else { $py = 'ERR' } }

  $gh = '?'
  try {
    $u = Invoke-RestMethod -Uri "https://api.github.com/search/repositories?q=$n+in:name" -Headers @{'User-Agent'='avail-check'} -TimeoutSec 20
    $exact = @($u.items | Where-Object { $_.name -eq $n })
    $gh = if ($exact.Count -gt 0) { "TAKEN($($exact.Count))" } else { 'free' }
  } catch { $gh = 'ERR' }

  $handle = '?'
  try { Invoke-RestMethod -Uri "https://api.github.com/users/$n" -Headers @{'User-Agent'='avail-check'} -TimeoutSec 20 | Out-Null; $handle = 'TAKEN' }
  catch { if ($_.Exception.Response.StatusCode.value__ -eq 404) { $handle = 'free' } else { $handle = 'ERR' } }

  Write-Output ("{0,-12} crates={1,-6} npm={2,-6} pypi={3,-6} ghrepo={4,-10} ghhandle={5}" -f $n, $crate, $npm, $py, $gh, $handle)
  Start-Sleep -Seconds 2
}
