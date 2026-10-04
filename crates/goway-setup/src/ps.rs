//! PowerShell script construction for the Windows-side resources (pure, so quoting is tested).
//!
//! Scripts are passed with `-EncodedCommand` (base64 of UTF-16LE), which sidesteps every
//! command-line quoting layer between goway-setup and PowerShell. Existence probes print `1` or
//! `0`; everything else prints nothing and signals failure through the exit code.

use crate::host::{
    FIREWALL_PROFILES, FirewallSpec, HyperVSpec, Keepalive, LOCAL_SUBNET, Scope, TaskSpec,
};
use crate::relay::RelayTaskSpec;

/// Single-quote a value as a PowerShell string literal: every quote character PowerShell
/// recognizes (the ASCII one and U+2018 to U+201B) doubles. goway's own quoter, shared.
pub fn quote(value: &str) -> String {
    goway_journal::ps_quote(value)
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding.
pub fn base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The `-EncodedCommand` argument for `script`.
pub fn encode_command(script: &str) -> String {
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64(&utf16)
}

/// Wrap `body` so any error aborts with a nonzero exit code and no progress output.
fn strict(body: &str) -> String {
    format!("$ErrorActionPreference = 'Stop'\n$ProgressPreference = 'SilentlyContinue'\n{body}\n")
}

/// A lookup of exactly the object called `name`, never a wildcard match.
///
/// `Get-NetFirewallRule -DisplayName`, `Get-ScheduledTask -TaskName` and friends treat `*` and
/// `?` as wildcards, so a name of `*` would select (and a following `Remove-` delete) every
/// rule or task. The name is escaped for the cmdlet and the result is filtered again on exact,
/// case-sensitive equality of `property`.
fn exact_lookup(cmdlet: &str, parameter: &str, property: &str, name: &str) -> String {
    format!(
        "{cmdlet} {parameter} ([WildcardPattern]::Escape({q})) -ErrorAction SilentlyContinue | Where-Object {{ $_.{property} -ceq {q} }}",
        q = quote(name)
    )
}

/// Script printing `1` when a Defender Firewall rule with this display name exists, else `0`.
pub fn firewall_exists(name: &str) -> String {
    strict(&format!(
        "if ({}) {{ '1' }} else {{ '0' }}",
        exact_lookup("Get-NetFirewallRule", "-DisplayName", "DisplayName", name)
    ))
}

/// The profile list of a rule as a PowerShell array literal.
///
/// Only the three real profile names are passed through; anything else (including `Any` and a
/// missing value) falls back to the default Private and Domain, so a rule is never created
/// open to Public networks by accident.
fn profiles_arg(scope: &Scope) -> String {
    let wanted = scope.profiles.as_deref().unwrap_or(FIREWALL_PROFILES);
    let mut names: Vec<&str> = wanted
        .split(',')
        .map(str::trim)
        .filter(|p| matches!(*p, "Private" | "Domain" | "Public"))
        .collect();
    if names.is_empty() {
        names = FIREWALL_PROFILES.split(',').collect();
    }
    names.iter().map(|n| quote(n)).collect::<Vec<_>>().join(",")
}

/// The remote address list of a rule as a PowerShell array literal (never empty: the local
/// subnet when nothing is given).
fn remote_arg(scope: &Scope) -> String {
    if scope.remote_addresses.is_empty() {
        return quote(LOCAL_SUBNET);
    }
    scope
        .remote_addresses
        .iter()
        .map(|a| quote(a))
        .collect::<Vec<_>>()
        .join(",")
}

/// Script creating an inbound TCP allow rule limited to the Private and Domain profiles and to
/// the local subnet plus any `--allow-from` addresses (never `-Profile Any`, never every address).
pub fn firewall_create(name: &str, spec: &FirewallSpec) -> String {
    strict(&format!(
        "New-NetFirewallRule -Name {n} -DisplayName {n} -Description {d} -Direction Inbound -Action Allow -Protocol TCP -LocalPort {p} -Profile {profiles} -RemoteAddress {remote} -Enabled True | Out-Null",
        n = quote(name),
        d = quote(&spec.description),
        p = spec.port,
        profiles = profiles_arg(&spec.scope),
        remote = remote_arg(&spec.scope),
    ))
}

/// Script removing the rule(s) with this display name; absent is success.
pub fn firewall_delete(name: &str) -> String {
    strict(&format!(
        "{} | Remove-NetFirewallRule",
        exact_lookup("Get-NetFirewallRule", "-DisplayName", "DisplayName", name)
    ))
}

/// Script printing `1` when a Hyper-V firewall rule with this name exists, else `0`.
pub fn hyperv_exists(name: &str) -> String {
    strict(&format!(
        "if ({}) {{ '1' }} else {{ '0' }}",
        exact_lookup("Get-NetFirewallHyperVRule", "-Name", "Name", name)
    ))
}

/// Script creating an inbound TCP allow rule for the WSL VM in the Hyper-V firewall, limited to
/// the same profiles and remote addresses as the Windows rule.
pub fn hyperv_create(name: &str, spec: &HyperVSpec) -> String {
    strict(&format!(
        "New-NetFirewallHyperVRule -Name {n} -DisplayName {n} -Direction Inbound -VMCreatorId {v} -Protocol TCP -LocalPorts {p} -Profiles {profiles} -RemoteAddresses {remote} -Action Allow | Out-Null",
        n = quote(name),
        v = quote(&spec.vm_creator_id),
        p = spec.port,
        profiles = profiles_arg(&spec.scope),
        remote = remote_arg(&spec.scope),
    ))
}

/// Script removing the Hyper-V firewall rule; absent is success.
pub fn hyperv_delete(name: &str) -> String {
    strict(&format!(
        "{} | Remove-NetFirewallHyperVRule",
        exact_lookup("Get-NetFirewallHyperVRule", "-Name", "Name", name)
    ))
}

/// Script printing `1` when the Hyper-V firewall cmdlets are available, else `0`.
pub fn hyperv_available() -> String {
    "if (Get-Command New-NetFirewallHyperVRule -ErrorAction SilentlyContinue) { '1' } else { '0' }\n"
        .to_owned()
}

/// Script printing `1` when a scheduled task with this name exists (any folder), else `0`.
pub fn task_exists(name: &str) -> String {
    strict(&format!(
        "if ({}) {{ '1' }} else {{ '0' }}",
        exact_lookup("Get-ScheduledTask", "-TaskName", "TaskName", name)
    ))
}

/// Script printing `<address>/<prefix>` for each IPv4 address of the WSL virtual adapter.
pub fn wsl_adapter_addresses() -> String {
    strict(crate::relay::ADAPTER_QUERY)
}

/// The conhost command line that runs `sleep infinity` in the distro with no window, through
/// the absolute `wsl.exe` (a task must never look a program up through a search path).
pub fn keepalive_arguments(distro: &str, wsl_exe: &str) -> String {
    format!("--headless \"{wsl_exe}\" -d {distro} --exec /bin/sh -c \"exec sleep infinity\"")
}

/// Script registering the keepalive task for the invoking user; `conhost_exe` and `wsl_exe` are
/// absolute System32 paths.
pub fn task_create(name: &str, spec: &TaskSpec, conhost_exe: &str, wsl_exe: &str) -> String {
    let (trigger, logon) = match spec.keepalive {
        Keepalive::Logon => (
            "New-ScheduledTaskTrigger -AtLogOn -User $user",
            "Interactive",
        ),
        Keepalive::Boot => ("New-ScheduledTaskTrigger -AtStartup", "S4U"),
    };
    strict(&format!(
        "$user = [System.Security.Principal.WindowsIdentity]::GetCurrent().Name\n\
         $action = New-ScheduledTaskAction -Execute {conhost} -Argument {args}\n\
         $trigger = {trigger}\n\
         $principal = New-ScheduledTaskPrincipal -UserId $user -LogonType {logon} -RunLevel Limited\n\
         $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero) -MultipleInstances IgnoreNew\n\
         Register-ScheduledTask -TaskName {name} -Description {desc} -Action $action -Trigger $trigger -Principal $principal -Settings $settings | Out-Null",
        conhost = quote(conhost_exe),
        args = quote(&keepalive_arguments(&spec.distro, wsl_exe)),
        name = quote(name),
        desc = quote(&spec.description),
    ))
}

/// The conhost command line that runs the refresh script with no window and without injectable
/// environment variables (see [`crate::relay::scrubbed_arguments`]); the programs are absolute
/// paths and the user's profile and the execution policy are out of the picture.
pub fn relay_arguments(cmd_exe: &str, powershell_exe: &str, script: &str) -> String {
    crate::relay::scrubbed_arguments(cmd_exe, powershell_exe, script)
}

/// Script registering the relay refresh task: it runs as the invoking user (only that user can
/// see the distro) with the highest privileges (netsh needs them), at logon or startup like the
/// keepalive and then every few minutes. No password is stored (`Interactive` or `S4U`). The
/// script starts through an environment-scrubbing `cmd.exe` stub ([`relay_arguments`]).
pub fn relay_task_create(
    name: &str,
    spec: &RelayTaskSpec,
    conhost_exe: &str,
    cmd_exe: &str,
    powershell_exe: &str,
) -> String {
    let (trigger, logon) = match spec.keepalive {
        Keepalive::Logon => (
            "New-ScheduledTaskTrigger -AtLogOn -User $user",
            "Interactive",
        ),
        Keepalive::Boot => ("New-ScheduledTaskTrigger -AtStartup", "S4U"),
    };
    strict(&format!(
        "$user = [System.Security.Principal.WindowsIdentity]::GetCurrent().Name\n\
         $action = New-ScheduledTaskAction -Execute {conhost} -Argument {args}\n\
         $trigger = {trigger}\n\
         $repeat = New-ScheduledTaskTrigger -Once -At (Get-Date) -RepetitionInterval (New-TimeSpan -Minutes {minutes}) -RepetitionDuration (New-TimeSpan -Days 3650)\n\
         $trigger.Repetition = $repeat.Repetition\n\
         $principal = New-ScheduledTaskPrincipal -UserId $user -LogonType {logon} -RunLevel Highest\n\
         $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit (New-TimeSpan -Minutes 5) -MultipleInstances IgnoreNew\n\
         Register-ScheduledTask -TaskName {name} -Description {desc} -Action $action -Trigger $trigger -Principal $principal -Settings $settings | Out-Null",
        conhost = quote(conhost_exe),
        args = quote(&relay_arguments(cmd_exe, powershell_exe, &spec.script)),
        minutes = spec.interval_minutes,
        name = quote(name),
        desc = quote(&spec.description),
    ))
}

/// Script printing `True` when the invoking account is in Administrators. The token's group list
/// is read (not `IsInRole`), so a UAC-filtered administrator, whose group is deny-only, counts.
pub fn admin_account() -> String {
    strict(
        "[bool]([System.Security.Principal.WindowsIdentity]::GetCurrent().Groups | Where-Object { $_.Value -eq 'S-1-5-32-544' })",
    )
}

/// Script printing the Windows build number.
pub fn windows_build() -> String {
    strict("(Get-CimInstance Win32_OperatingSystem).BuildNumber")
}

/// Script stopping (when running) and unregistering a task; absent is success.
pub fn task_delete(name: &str) -> String {
    strict(&format!(
        "$t = {lookup}\n\
         if ($t) {{ Stop-ScheduledTask -InputObject $t -ErrorAction SilentlyContinue; Unregister-ScheduledTask -InputObject $t -Confirm:$false }}",
        lookup = exact_lookup("Get-ScheduledTask", "-TaskName", "TaskName", name)
    ))
}

/// Script starting a registered task now.
pub fn task_start(name: &str) -> String {
    strict(&format!(
        "{} | Start-ScheduledTask",
        exact_lookup("Get-ScheduledTask", "-TaskName", "TaskName", name)
    ))
}

/// Script printing the name of every connected network the firewall classifies as Public.
pub fn public_networks() -> String {
    strict(
        "Get-NetConnectionProfile | Where-Object { $_.NetworkCategory -eq 'Public' } | ForEach-Object { $_.Name }",
    )
}

/// Script printing `key=value` facts about the native OpenSSH setup and the invoking account:
/// the account (name, SID, Administrators membership, profile directory), whether the OpenSSH
/// Server capability is installed, sshd's startup type and state, the current `DefaultShell` and
/// how the built-in firewall rule admits connections. Read-only.
pub fn native_probe() -> String {
    strict(&format!(
        "$id = [Security.Principal.WindowsIdentity]::GetCurrent()\n\
         'account=' + $id.Name\n\
         'sid=' + $id.User.Value\n\
         'admin=' + $(if ($id.Groups | Where-Object {{ $_.Value -eq 'S-1-5-32-544' }}) {{ '1' }} else {{ '0' }})\n\
         'profile=' + [Environment]::GetFolderPath('UserProfile')\n\
         $cap = Get-WindowsCapability -Online -Name {cap} | Where-Object {{ $_.Name -ceq {cap} }}\n\
         'capability=' + $(if ($cap -and $cap.State -eq 'Installed') {{ '1' }} else {{ '0' }})\n\
         $svc = Get-CimInstance Win32_Service -Filter \"Name='{svc}'\" -ErrorAction SilentlyContinue\n\
         if ($svc) {{ 'service_start=' + $svc.StartMode; 'service_running=' + $(if ($svc.State -eq 'Running') {{ '1' }} else {{ '0' }}) }}\n\
         $shell = (Get-ItemProperty -LiteralPath 'HKLM:\\SOFTWARE\\OpenSSH' -Name DefaultShell -ErrorAction SilentlyContinue).DefaultShell\n\
         if ($shell) {{ 'default_shell=' + $shell }}\n\
         $rule = Get-NetFirewallRule -Name {rule} -ErrorAction SilentlyContinue\n\
         if (-not $rule) {{ 'builtin_rule=absent' }} else {{ 'builtin_rule=' + $(if ({open}) {{ 'open' }} else {{ 'scoped' }}) }}",
        cap = quote(crate::native::CAPABILITY),
        svc = crate::native::SSHD_SERVICE,
        rule = quote(crate::native::BUILTIN_RULE),
        open = rule_is_open("$rule"),
    ))
}

/// The PowerShell condition that is true when the firewall rule held in `var` admits Public
/// networks or every address.
fn rule_is_open(var: &str) -> String {
    format!(
        "({var}.Profile.ToString() -match 'Public|Any') -or ((({var} | Get-NetFirewallAddressFilter).RemoteAddress | ForEach-Object {{ $_.ToString() }}) -contains 'Any')"
    )
}

/// Script printing `1` when the optional capability is installed, else `0`.
pub fn capability_exists(name: &str) -> String {
    strict(&format!(
        "$c = Get-WindowsCapability -Online -Name {q} | Where-Object {{ $_.Name -ceq {q} }}\n\
         if ($c -and $c.State -eq 'Installed') {{ '1' }} else {{ '0' }}",
        q = quote(name)
    ))
}

/// Script installing the optional capability; prints `restart` when Windows asks for one.
pub fn capability_add(name: &str) -> String {
    strict(&format!(
        "$r = Add-WindowsCapability -Online -Name {}\nif ($r.RestartNeeded) {{ 'restart' }}",
        quote(name)
    ))
}

/// Script removing the optional capability.
pub fn capability_remove(name: &str) -> String {
    strict(&format!(
        "Remove-WindowsCapability -Online -Name {} | Out-Null",
        quote(name)
    ))
}

/// Script printing `1` when sshd runs and starts automatically, else `0`.
pub fn sshd_exists() -> String {
    strict(&format!(
        "$s = Get-CimInstance Win32_Service -Filter \"Name='{}'\" -ErrorAction SilentlyContinue\n\
         if ($s -and $s.StartMode -eq 'Auto' -and $s.State -eq 'Running') {{ '1' }} else {{ '0' }}",
        crate::native::SSHD_SERVICE
    ))
}

/// Script making sshd start automatically and starting it now.
pub fn sshd_enable() -> String {
    strict(&format!(
        "Set-Service -Name {n} -StartupType Automatic\nStart-Service -Name {n}",
        n = quote(crate::native::SSHD_SERVICE)
    ))
}

/// Script putting sshd back as it was: stopped when it was not running, with its old startup
/// type. A service that no longer exists (its capability is gone) is success.
pub fn sshd_restore(spec: &crate::native::ServiceSpec) -> String {
    let stop = if spec.was_running {
        String::new()
    } else {
        format!(
            "Stop-Service -Name {} -Force -ErrorAction SilentlyContinue\n",
            quote(crate::native::SSHD_SERVICE)
        )
    };
    strict(&format!(
        "if (Get-Service -Name {n} -ErrorAction SilentlyContinue) {{\n{stop}Set-Service -Name {n} -StartupType {t}\n}}",
        n = quote(crate::native::SSHD_SERVICE),
        t = spec.restore.as_str()
    ))
}

/// Script printing `1` when the firewall rule with this exact `Name` is narrowed (it does not
/// admit Public networks or every address), or does not exist; else `0`.
pub fn firewall_scope_exists(name: &str) -> String {
    strict(&format!(
        "$rule = Get-NetFirewallRule -Name {n} -ErrorAction SilentlyContinue | Where-Object {{ $_.Name -ceq {n} }}\n\
         if (-not $rule) {{ '1' }} elseif ({open}) {{ '0' }} else {{ '1' }}",
        n = quote(name),
        open = rule_is_open("$rule"),
    ))
}

/// Script limiting the rule to the scope's profiles and remote addresses.
pub fn firewall_scope_set(name: &str, scope: &Scope) -> String {
    strict(&format!(
        "Set-NetFirewallRule -Name {} -Profile {} -RemoteAddress {}",
        quote(name),
        profiles_arg(scope),
        remote_arg(scope),
    ))
}

/// Script putting the rule back to every profile and address (what the system creates); a rule
/// that is gone is success.
pub fn firewall_scope_restore(name: &str) -> String {
    strict(&format!(
        "if (Get-NetFirewallRule -Name {n} -ErrorAction SilentlyContinue) {{ Set-NetFirewallRule -Name {n} -Profile Any -RemoteAddress Any }}",
        n = quote(name)
    ))
}

/// Script printing a file's DACL as SDDL (`D:...`).
pub fn acl_get(path: &str) -> String {
    strict(&format!(
        "[System.IO.File]::GetAccessControl({}).GetSecurityDescriptorSddlForm('Access')",
        quote(path)
    ))
}

/// Script replacing a file's DACL with `sddl` (the owner is left as it is).
pub fn acl_set(path: &str, sddl: &str) -> String {
    strict(&format!(
        "$sec = New-Object System.Security.AccessControl.FileSecurity\n\
         $sec.SetSecurityDescriptorSddlForm({}, 'Access')\n\
         [System.IO.File]::SetAccessControl({}, $sec)",
        quote(sddl),
        quote(path)
    ))
}
