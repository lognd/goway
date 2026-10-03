# Prints a deterministic snapshot of everything goway-setup may touch for one profile:
# the raw user Path (value and registry type), the Uninstall key, and the install/state dirs.
param([string]$ProfileName = 'goway-test')
$ErrorActionPreference = 'Stop'
$env_key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment')
$kind = 'absent'; $raw = ''
if ($env_key.GetValueNames() -contains 'Path') {
    $kind = $env_key.GetValueKind('Path').ToString()
    $raw = $env_key.GetValue('Path', $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
}
"path.type=$kind"
"path.value=$raw"
$un = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("Software\Microsoft\Windows\CurrentVersion\Uninstall\$ProfileName")
if ($un -eq $null) { 'uninstall.key=absent' } else {
    'uninstall.key=present'
    foreach ($n in ($un.GetValueNames() | Sort-Object)) {
        '  ' + $n + ' [' + $un.GetValueKind($n) + '] = ' + $un.GetValue($n, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    }
    '  subkeys=' + ($un.GetSubKeyNames() -join ',')
}
$local = $env:LOCALAPPDATA
"programs.dir=" + (Test-Path -LiteralPath "$local\Programs")
if (Test-Path -LiteralPath "$local\Programs") {
    'programs.children=' + ((Get-ChildItem -LiteralPath "$local\Programs" -Force | Sort-Object Name | ForEach-Object { $_.Name }) -join ',')
}
foreach ($root in @("$local\Programs\$ProfileName", "$local\$ProfileName")) {
    "dir $root exists=" + (Test-Path -LiteralPath $root)
    if (Test-Path -LiteralPath $root) {
        Get-ChildItem -LiteralPath $root -Recurse -Force | Sort-Object FullName | ForEach-Object {
            '  ' + $_.FullName.Substring($local.Length) + ' len=' + $(if ($_.PSIsContainer) { 'dir' } else { $_.Length })
        }
    }
}
# Temp leftovers of the installer (staging dir, relaunched uninstaller copy) count as drift too.
'temp.goway=' + ((Get-ChildItem -LiteralPath $env:TEMP -Force -Filter 'goway-*' -ErrorAction SilentlyContinue | Sort-Object Name | ForEach-Object { $_.Name }) -join ',')
