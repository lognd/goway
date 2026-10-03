# Waits (up to 60 s) until the profile's install and state directories are gone, as after the
# relaunched uninstaller finishes in the background.
param([string]$ProfileName = 'goway-test')
$local = $env:LOCALAPPDATA
for ($i = 0; $i -lt 120; $i++) {
    if (-not (Test-Path -LiteralPath "$local\Programs\$ProfileName") -and -not (Test-Path -LiteralPath "$local\$ProfileName")) {
        Start-Sleep -Seconds 4   # let the temp copy's scheduled self-delete run
        Write-Output "uninstalled after $($i / 2) s"; exit 0
    }
    Start-Sleep -Milliseconds 500
}
Write-Output 'FAIL: directories still present after 60 s'; exit 1
