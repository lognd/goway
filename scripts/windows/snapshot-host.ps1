# Prints a deterministic snapshot of the Windows-side state the host component may touch:
# Defender Firewall rules (name, display name, enabled, direction, action, profile, protocol/port),
# Hyper-V firewall rules and VM settings, non-Microsoft scheduled tasks (action, trigger, principal,
# enabled), the bytes of %USERPROFILE%\.wslconfig, and the files under %LOCALAPPDATA%\goway-test and
# %ProgramData%\goway* (the administrator-only host state). WSL-side state is snapshotted by
# snapshot-wsl.sh over ssh.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

'== firewall rules'
$ports = @{}
Get-NetFirewallPortFilter -All | ForEach-Object { $ports[$_.InstanceID] = $_.Protocol + '/' + ($_.LocalPort -join ',') }
Get-NetFirewallRule -All | Sort-Object Name | ForEach-Object {
    '{0}|{1}|en={2}|{3}|{4}|{5}|{6}' -f $_.Name, $_.DisplayName, $_.Enabled, $_.Direction, $_.Action, $_.Profile, $ports[$_.InstanceID]
}

'== hyper-v firewall settings'
Get-NetFirewallHyperVVMSetting -PolicyStore ActiveStore | ForEach-Object {
    '{0}|en={1}|in={2}|out={3}|loopback={4}|merge={5}' -f $_.Name, $_.Enabled, $_.DefaultInboundAction, $_.DefaultOutboundAction, $_.LoopbackEnabled, $_.AllowHostPolicyMerge
}
'== hyper-v firewall rules'
Get-NetFirewallHyperVRule | Sort-Object Name | ForEach-Object {
    '{0}|{1}|en={2}|{3}|{4}|{5}|{6}|{7}' -f $_.Name, $_.DisplayName, $_.Enabled, $_.Direction, $_.Action, $_.Protocol, ($_.LocalPorts -join ','), $_.VMCreatorId
}

'== scheduled tasks (non-Microsoft)'
Get-ScheduledTask | Where-Object { $_.TaskPath -notlike '\Microsoft*' } | Sort-Object TaskPath, TaskName | ForEach-Object {
    '{0}{1}|enabled={2}|user={3}|logon={4}|run={5}' -f $_.TaskPath, $_.TaskName, $_.Settings.Enabled, $_.Principal.UserId, $_.Principal.LogonType, $_.Principal.RunLevel
    foreach ($a in $_.Actions) { '  action: {0} {1}' -f $a.Execute, $a.Arguments }
    foreach ($t in $_.Triggers) { '  trigger: {0} enabled={1}' -f $t.CimClass.CimClassName, $t.Enabled }
    '  settings: battery={0} stopbattery={1} limit={2} multiple={3}' -f $_.Settings.DisallowStartIfOnBatteries, $_.Settings.StopIfGoingOnBatteries, $_.Settings.ExecutionTimeLimit, $_.Settings.MultipleInstances
}

'== .wslconfig'
$wc = Join-Path $env:USERPROFILE '.wslconfig'
if (Test-Path -LiteralPath $wc) {
    $bytes = [System.IO.File]::ReadAllBytes($wc)
    'bytes=' + $bytes.Length + ' sha256=' + (Get-FileHash -LiteralPath $wc -Algorithm SHA256).Hash
    'hex=' + (($bytes | ForEach-Object { $_.ToString('x2') }) -join '')
} else { 'absent' }

'== goway state (profile dirs, journals)'
foreach ($root in @("$env:LOCALAPPDATA\goway-test")) {
    "dir $root exists=" + (Test-Path -LiteralPath $root)
    if (Test-Path -LiteralPath $root) { Get-ChildItem -LiteralPath $root -Recurse -Force | Sort-Object FullName | ForEach-Object { '  ' + $_.FullName } }
}

'== goway administrator-only state (ProgramData\goway*)'
foreach ($item in @(Get-ChildItem -LiteralPath $env:ProgramData -Filter 'goway*' -Force -ErrorAction SilentlyContinue | Sort-Object FullName)) {
    $item.FullName
    if ($item.PSIsContainer) { Get-ChildItem -LiteralPath $item.FullName -Recurse -Force | Sort-Object FullName | ForEach-Object { '  ' + $_.FullName } }
}
