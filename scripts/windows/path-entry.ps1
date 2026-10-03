# Adds or removes one directory in the user Path, keeping the registry value type.
# Used only to stage the "Path already contains the directory" case with the test profile's dir.
param([ValidateSet('add', 'remove')][string]$Action, [string]$ProfileName = 'goway-test')
$ErrorActionPreference = 'Stop'
$bin = "$env:LOCALAPPDATA\Programs\$ProfileName\bin"
$key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
$kind = $key.GetValueKind('Path')
$raw = $key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
$parts = @($raw -split ';')
if ($Action -eq 'add') { if ($parts -notcontains $bin) { $parts += $bin } }
else { $idx = [array]::LastIndexOf($parts, $bin); if ($idx -ge 0) { $parts = $parts[0..($idx - 1)] + $(if ($idx + 1 -lt $parts.Count) { $parts[($idx + 1)..($parts.Count - 1)] } else { @() }) } }
$key.SetValue('Path', ($parts -join ';'), $kind)
