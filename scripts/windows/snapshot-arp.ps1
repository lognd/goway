# Prints the machine-wide Add/Remove Programs entries named goway* (HKLM Uninstall) with every
# value, sorted. Appended to the host snapshot (a separate script: cmd.exe caps a command line at
# 8191 characters, and snapshot-host.ps1 is already near that).
$ErrorActionPreference = 'Stop'
'== HKLM Add/Remove Programs entries (goway*)'
$root = 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall'
foreach ($k in @(Get-ChildItem -LiteralPath $root | Where-Object { $_.PSChildName -like 'goway*' } | Sort-Object PSChildName)) {
    $k.PSChildName
    foreach ($n in ($k.GetValueNames() | Sort-Object)) { '  {0}={1}' -f $n, $k.GetValue($n) }
}
'== end'
