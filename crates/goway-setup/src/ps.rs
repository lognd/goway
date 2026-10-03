//! PowerShell script construction for the Windows-side resources (pure, so quoting is tested).
//!
//! Scripts are passed with `-EncodedCommand` (base64 of UTF-16LE), which sidesteps every
//! command-line quoting layer between goway-setup and PowerShell. Existence probes print `1` or
//! `0`; everything else prints nothing and signals failure through the exit code.

use crate::host::{FirewallSpec, HyperVSpec, Keepalive, TaskSpec};

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

/// Script printing `1` when a Defender Firewall rule with this display name exists, else `0`.
pub fn firewall_exists(name: &str) -> String {
    strict(&format!(
        "if (Get-NetFirewallRule -DisplayName {} -ErrorAction SilentlyContinue) {{ '1' }} else {{ '0' }}",
        quote(name)
    ))
}

/// Script creating an inbound TCP allow rule for every profile.
pub fn firewall_create(name: &str, spec: &FirewallSpec) -> String {
    strict(&format!(
        "New-NetFirewallRule -Name {n} -DisplayName {n} -Description {d} -Direction Inbound -Action Allow -Protocol TCP -LocalPort {p} -Profile Any -Enabled True | Out-Null",
        n = quote(name),
        d = quote(&spec.description),
        p = spec.port
    ))
}

/// Script removing the rule(s) with this display name; absent is success.
pub fn firewall_delete(name: &str) -> String {
    strict(&format!(
        "Get-NetFirewallRule -DisplayName {} -ErrorAction SilentlyContinue | Remove-NetFirewallRule",
        quote(name)
    ))
}

/// Script printing `1` when a Hyper-V firewall rule with this name exists, else `0`.
pub fn hyperv_exists(name: &str) -> String {
    strict(&format!(
        "if (Get-NetFirewallHyperVRule -Name {} -ErrorAction SilentlyContinue) {{ '1' }} else {{ '0' }}",
        quote(name)
    ))
}

/// Script creating an inbound TCP allow rule for the WSL VM in the Hyper-V firewall.
pub fn hyperv_create(name: &str, spec: &HyperVSpec) -> String {
    strict(&format!(
        "New-NetFirewallHyperVRule -Name {n} -DisplayName {n} -Direction Inbound -VMCreatorId {v} -Protocol TCP -LocalPorts {p} -Action Allow | Out-Null",
        n = quote(name),
        v = quote(&spec.vm_creator_id),
        p = spec.port
    ))
}

/// Script removing the Hyper-V firewall rule; absent is success.
pub fn hyperv_delete(name: &str) -> String {
    strict(&format!(
        "Get-NetFirewallHyperVRule -Name {} -ErrorAction SilentlyContinue | Remove-NetFirewallHyperVRule",
        quote(name)
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
        "if (Get-ScheduledTask -TaskName {} -ErrorAction SilentlyContinue) {{ '1' }} else {{ '0' }}",
        quote(name)
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

/// Script stopping (when running) and unregistering a task; absent is success.
pub fn task_delete(name: &str) -> String {
    strict(&format!(
        "$t = Get-ScheduledTask -TaskName {n} -ErrorAction SilentlyContinue\n\
         if ($t) {{ Stop-ScheduledTask -InputObject $t -ErrorAction SilentlyContinue; Unregister-ScheduledTask -InputObject $t -Confirm:$false }}",
        n = quote(name)
    ))
}

/// Script starting a registered task now.
pub fn task_start(name: &str) -> String {
    strict(&format!("Start-ScheduledTask -TaskName {}", quote(name)))
}
