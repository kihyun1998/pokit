param([string]$Label)
$ErrorActionPreference = 'Continue'
$key = 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
"== ${Label}: runtime $((Get-ItemProperty $key -ErrorAction SilentlyContinue).pv)"
$ci = Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion'
"edition: $($ci.EditionID), $($ci.ProductName), $($ci.InstallationType), build $($ci.CurrentBuild).$($ci.UBR)"
$whoami = Join-Path $env:SystemRoot 'System32\whoami.exe'
"user: $(& $whoami), session $([Diagnostics.Process]::GetCurrentProcess().SessionId)"
& $whoami /groups | Select-String 'Mandatory Label'
"EnableLUA: $((Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System').EnableLUA)"
foreach ($p in 'HKLM:\SOFTWARE\Policies\Microsoft\Edge', 'HKCU:\SOFTWARE\Policies\Microsoft\Edge',
               'HKLM:\SOFTWARE\Policies\Microsoft\EdgeUpdate', 'HKLM:\SOFTWARE\WOW6432Node\Policies\Microsoft\Edge') {
  if (Test-Path $p) { "policy $p"; Get-ChildItem $p -Recurse | ForEach-Object { "  $($_.Name)"; $_ | Get-ItemProperty | Out-String } ; Get-ItemProperty $p | Out-String }
  else { "no policy $p" }
}
Get-ChildItem env: | Where-Object Name -like 'WEBVIEW2*' | ForEach-Object { "env $($_.Name)=$($_.Value)" }

$exe = Resolve-Path 'target/fixture/debug/pokit-fixture.exe'
$tmp = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { $env:TEMP }
$dir = Join-Path $tmp "probe-$Label"
Remove-Item -Recurse -Force $dir -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $dir | Out-Null
$env:WEBVIEW2_USER_DATA_FOLDER = $dir
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--lang=ko-KR --remote-debugging-port=0'
$app = Start-Process $exe -PassThru -RedirectStandardError "$dir.err" -RedirectStandardOutput "$dir.out"
$f = Join-Path $dir 'EBWebView\DevToolsActivePort'
for ($i = 0; $i -lt 60 -and -not (Test-Path $f); $i++) { Start-Sleep -Milliseconds 500 }
"port file: $(Test-Path $f) $(if (Test-Path $f) { Get-Content $f -TotalCount 1 })"
"app alive: $(-not $app.HasExited)"
Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" | Where-Object { $_.CommandLine -notlike "*--type=*" } | ForEach-Object {
  "webview2 pid $($_.ProcessId) parent $($_.ParentProcessId): $($_.CommandLine)"
}
"profile contents:"; Get-ChildItem $dir -Recurse -Depth 1 -ErrorAction SilentlyContinue | ForEach-Object { "  $($_.FullName.Substring($dir.Length))" }
"app stderr:"; Get-Content "$dir.err" -ErrorAction SilentlyContinue
Stop-Process -Id $app.Id -Force -ErrorAction SilentlyContinue
Start-Sleep 2
exit 0
