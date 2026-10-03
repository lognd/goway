//! PowerShell script construction for the Windows-side resources (pure, so quoting is tested).
//!
//! Scripts are passed with `-EncodedCommand` (base64 of UTF-16LE), which sidesteps every
//! command-line quoting layer between goway-setup and PowerShell. Existence probes print `1` or
//! `0`; everything else prints nothing and signals failure through the exit code.

use crate::host::{
    FIREWALL_PROFILES, FirewallSpec, HyperVSpec, Keepalive, LOCAL_SUBNET, Scope, TaskSpec,
};
use crate::relay::RelayTaskSpec;

/// Single-quote a value as a PowerShell string literal (`'` doubles).
pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
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

/// The conhost command line that runs `sleep infinity` in the distro with no window.
pub fn keepalive_arguments(distro: &str) -> String {
    format!("--headless wsl.exe -d {distro} --exec /bin/sh -c \"exec sleep infinity\"")
}

/// Script registering the keepalive task for the invoking user.
pub fn task_create(name: &str, spec: &TaskSpec) -> String {
    let (trigger, logon) = match spec.keepalive {
        Keepalive::Logon => (
            "New-ScheduledTaskTrigger -AtLogOn -User $user",
            "Interactive",
        ),
        Keepalive::Boot => ("New-ScheduledTaskTrigger -AtStartup", "S4U"),
    };
    strict(&format!(
        "$user = [System.Security.Principal.WindowsIdentity]::GetCurrent().Name\n\
         $action = New-ScheduledTaskAction -Execute 'conhost.exe' -Argument {args}\n\
         $trigger = {trigger}\n\
         $principal = New-ScheduledTaskPrincipal -UserId $user -LogonType {logon} -RunLevel Limited\n\
         $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero) -MultipleInstances IgnoreNew\n\
         Register-ScheduledTask -TaskName {name} -Description {desc} -Action $action -Trigger $trigger -Principal $principal -Settings $settings | Out-Null",
        args = quote(&keepalive_arguments(&spec.distro)),
        name = quote(name),
        desc = quote(&spec.description),
    ))
}

/// The conhost command line that runs the refresh script with no window, through the absolute
/// `powershell.exe` and with the user's profile and the execution policy out of the picture.
pub fn relay_arguments(powershell_exe: &str, script: &str) -> String {
    format!(
        "--headless \"{powershell_exe}\" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"{script}\""
    )
}

/// Script registering the relay refresh task: it runs as the invoking user (only that user can
/// see the distro) with the highest privileges (netsh needs them), at logon or startup like the
/// keepalive and then every few minutes. No password is stored (`Interactive` or `S4U`).
pub fn relay_task_create(
    name: &str,
    spec: &RelayTaskSpec,
    conhost_exe: &str,
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
        args = quote(&relay_arguments(powershell_exe, &spec.script)),
        minutes = spec.interval_minutes,
        name = quote(name),
        desc = quote(&spec.description),
    ))
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
