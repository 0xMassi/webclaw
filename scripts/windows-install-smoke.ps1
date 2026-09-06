param([Parameter(Mandatory = $true)][string]$Binary)
$ErrorActionPreference = 'Stop'
# Verify the requested installation, regardless of other webclaw copies on PATH.
Remove-Item Env:WEBCLAW_API_KEY -ErrorAction SilentlyContinue
Remove-Item Env:WEBCLAW_PROXY -ErrorAction SilentlyContinue
Remove-Item Env:WEBCLAW_PROXY_FILE -ErrorAction SilentlyContinue
& $Binary --version
if ($LASTEXITCODE -ne 0) { throw 'Version check failed' }
$fixture = Join-Path $env:RUNNER_TEMP 'webclaw-install-fixture.html'
'<html><body><main><h1>Windows installation check</h1><p>This local document verifies that extraction runs successfully.</p></main></body></html>' | Set-Content -Path $fixture -Encoding utf8
$result = & $Binary --file $fixture --format json | ConvertFrom-Json
if ($LASTEXITCODE -ne 0 -or $result.content.markdown -notmatch 'Windows installation check') { throw 'Local extraction failed' }
$remote = & $Binary https://example.com --format json | ConvertFrom-Json
if ($LASTEXITCODE -ne 0 -or $remote.content.markdown -notmatch 'Example Domain') { throw 'HTTPS extraction failed' }
Write-Output 'PASS: version, local HTML extraction, and HTTPS extraction'
