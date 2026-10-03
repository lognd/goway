# Checks that an install of the profile took effect; exits 1 with a reason otherwise.
param([string]$ProfileName = 'goway-test')
$ErrorActionPreference = 'Stop'
$bin = "$env:LOCALAPPDATA\Programs\$ProfileName\bin"
$user_path = [Environment]::GetEnvironmentVariable('Path', 'User')
if (-not (($user_path -split ';') -contains $bin)) { Write-Output "FAIL: user Path lacks $bin"; exit 1 }
$un = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("Software\Microsoft\Windows\CurrentVersion\Uninstall\$ProfileName")
if ($un -eq $null) { Write-Output 'FAIL: uninstall key missing'; exit 1 }
foreach ($n in 'DisplayName', 'DisplayVersion', 'UninstallString', 'InstallLocation') {
    if (-not $un.GetValue($n)) { Write-Output "FAIL: uninstall value $n missing"; exit 1 }
}
if ($un.GetValue('NoModify') -ne 1 -or $un.GetValue('NoRepair') -ne 1) { Write-Output 'FAIL: NoModify/NoRepair'; exit 1 }
$uninst = $un.GetValue('UninstallString')
if (-not $uninst.Contains("goway-setup.exe") -or -not $uninst.Contains("uninstall --profile $ProfileName")) { Write-Output "FAIL: UninstallString $uninst"; exit 1 }
if (-not (Test-Path -LiteralPath "$env:LOCALAPPDATA\Programs\$ProfileName\goway-setup.exe")) { Write-Output 'FAIL: installed goway-setup.exe missing'; exit 1 }
$ver = & "$bin\goway.exe" --version
if ($LASTEXITCODE -ne 0) { Write-Output "FAIL: goway.exe --version exit $LASTEXITCODE"; exit 1 }
Write-Output "OK: Path has $bin; uninstall key present; $ver"
