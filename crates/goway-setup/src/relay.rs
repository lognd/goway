//! The NAT-mode relay: a Windows `portproxy` rule that forwards the helper's port to the WSL
//! distro, and the script that keeps it pointed at the distro's current address.
//!
//! WSL in NAT mode (Windows 10, or Windows 11 without mirrored networking) is reachable from
//! other computers only through Windows, and its internal address changes whenever WSL
//! restarts. Everything here is pure text and parsing (no probing, no side effects) so it is
//! unit tested off Windows; [`crate::hostsys`] runs the commands and [`crate::ps`] registers
//! the task.

use std::net::Ipv4Addr;

use serde::{Deserialize, Serialize};

/// The address the relay listens on: every interface, scoped by the firewall rules.
pub const LISTEN_ADDRESS: &str = "0.0.0.0";
/// How often the refresh task re-reads the distro's address, in minutes.
pub const REFRESH_MINUTES: u32 = 5;
/// File name of the refresh script inside the administrator-only directory.
pub const SCRIPT_NAME: &str = "relay-refresh.ps1";
/// Environment variable name prefixes that load code into a .NET process at start (profilers,
/// runtime knobs, startup hooks). The elevated refresh runs without any of them.
pub const SCRUB_PREFIXES: [&str; 4] = ["COR_", "CORECLR_", "COMPlus_", "DOTNET_"];
/// Exact environment variable names the elevated refresh also runs without: where PowerShell
/// looks for modules, its execution-policy override and lockdown switch, and compatibility shims.
pub const SCRUB_NAMES: [&str; 4] = [
    "PSModulePath",
    "PSExecutionPolicyPreference",
    "__PSLockdownPolicy",
    "__COMPAT_LAYER",
];

/// Whether the launch stub removes the environment variable `name` (names are not case
/// sensitive on Windows).
pub fn is_scrubbed(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    SCRUB_PREFIXES
        .iter()
        .any(|p| lower.starts_with(&p.to_ascii_lowercase()))
        || SCRUB_NAMES.iter().any(|n| n.eq_ignore_ascii_case(name))
}

/// The conhost arguments that run the refresh script without anything a same-account process
/// could have injected through its environment.
///
/// The elevated task runs as the user, in the user's environment, and the .NET runtime inside
/// PowerShell reads `COR_PROFILER` and friends before any script line runs, so the script cannot
/// protect itself. A native `cmd.exe` (not .NET, so it ignores those variables, and started with
/// `/D` so the user's `AutoRun` registry command is skipped) first deletes every variable of
/// [`SCRUB_PREFIXES`] and [`SCRUB_NAMES`] from its own environment, then starts PowerShell, which
/// inherits the cleaned one. All three programs are given by absolute path.
pub fn scrubbed_arguments(cmd_exe: &str, powershell_exe: &str, script: &str) -> String {
    let words = SCRUB_PREFIXES
        .iter()
        .chain(SCRUB_NAMES.iter())
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "--headless \"{cmd_exe}\" /D /S /C \"(for %p in ({words}) do @for /f \"delims==\" %v in ('set %p 2^>nul') do @set \"%v=\") & \"{powershell_exe}\" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"{script}\"\""
    )
}

/// One row of `netsh interface portproxy show v4tov4`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortProxyRule {
    /// Address the rule listens on.
    pub listen_address: Ipv4Addr,
    /// Port the rule listens on.
    pub listen_port: u16,
    /// Address it forwards to.
    pub connect_address: Ipv4Addr,
    /// Port it forwards to.
    pub connect_port: u16,
}

/// Spec of the portproxy resource (JSON in `Change::EnsureResource::spec`); the connect address
/// is deliberately absent because it is read from the distro when the rule is created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortProxySpec {
    /// Address the rule listens on.
    pub listen_address: String,
    /// Port the rule listens on and forwards to.
    pub port: u16,
}

/// Spec of the relay refresh task resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayTaskSpec {
    /// Distro whose address the task reads.
    pub distro: String,
    /// When it starts (follows the keepalive task).
    pub keepalive: crate::host::Keepalive,
    /// Absolute path of the refresh script in the administrator-only directory.
    pub script: String,
    /// Minutes between refreshes.
    pub interval_minutes: u32,
    /// Free-text description stored on the task.
    pub description: String,
}

/// Refuse a refresh-script path a task must not run: it has to be absolute and name
/// [`SCRIPT_NAME`] (the only file goway puts in the administrator-only directory for this), with
/// nothing that could end the quoted `-File` argument.
pub fn check_script_path(script: &str) -> Result<(), String> {
    let path = std::path::Path::new(script);
    let named = path.file_name().is_some_and(|n| n == SCRIPT_NAME);
    let plain = !script.contains(['"', '\'', '\n', '\r', '%', '`', '$']);
    if path.is_absolute() && named && plain {
        Ok(())
    } else {
        Err(format!(
            "refusing to register a task for {script:?}: not an absolute path to {SCRIPT_NAME}"
        ))
    }
}

/// The resource name of the relay for `port` (`0.0.0.0:2222`).
pub fn relay_name(port: u16) -> String {
    format!("{LISTEN_ADDRESS}:{port}")
}

/// Split a relay resource name back into its listen address and port.
pub fn parse_relay_name(name: &str) -> Option<(Ipv4Addr, u16)> {
    let (addr, port) = name.rsplit_once(':')?;
    Some((addr.parse().ok()?, port.parse().ok()?))
}

/// The PowerShell lines that print `<address>/<prefix length>` for every IPv4 address of the WSL
/// virtual adapter (`vEthernet (WSL)`, or `vEthernet (WSL (Hyper-V firewall))` on newer builds).
///
/// Plain .NET, no module: an elevated task must not autoload modules a user-level `PSModulePath`
/// could redirect.
pub const ADAPTER_QUERY: &str = "foreach ($nic in [System.Net.NetworkInformation.NetworkInterface]::GetAllNetworkInterfaces()) {\n\
\x20   if ($nic.Name -like 'vEthernet (WSL*') {\n\
\x20       foreach ($ua in $nic.GetIPProperties().UnicastAddresses) {\n\
\x20           if ($ua.Address.AddressFamily -eq [System.Net.Sockets.AddressFamily]::InterNetwork) { '{0}/{1}' -f $ua.Address, $ua.PrefixLength }\n\
\x20       }\n\
\x20   }\n\
}\n";

/// One IPv4 address of the WSL virtual adapter: the Windows side of the NAT subnet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdapterAddr {
    /// The adapter's own address (the NAT gateway the distro sees).
    pub ip: Ipv4Addr,
    /// Prefix length of its subnet.
    pub prefix: u8,
}

/// The `<address>/<prefix>` lines [`ADAPTER_QUERY`] prints (anything else is skipped).
pub fn parse_adapter_addrs(text: &str) -> Vec<AdapterAddr> {
    text.lines()
        .filter_map(|line| {
            let (ip, prefix) = line.trim().split_once('/')?;
            let prefix: u8 = prefix.parse().ok().filter(|p| (1..=32).contains(p))?;
            Some(AdapterAddr {
                ip: ip.parse().ok()?,
                prefix,
            })
        })
        .collect()
}

/// Whether `ip` lies in the subnet of `adapter`.
pub fn in_adapter_subnet(ip: Ipv4Addr, adapter: AdapterAddr) -> bool {
    let mask = u32::MAX
        .checked_shl(u32::from(32 - adapter.prefix))
        .unwrap_or(0);
    u32::from(ip) & mask == u32::from(adapter.ip) & mask
}

/// The addresses in `hostname -I` output the relay could ever forward to: dotted quads only
/// (no IPv6, no stray words, no leading zeros) in the private RFC 1918 ranges. WSL's NAT
/// subnet always is; this keeps a public, loopback, link-local or unspecified address a
/// distro prints from ever reaching netsh.
fn private_candidates(text: &str) -> impl Iterator<Item = Ipv4Addr> + '_ {
    text.split_whitespace()
        .filter(|t| t.chars().all(|c| c.is_ascii_digit() || c == '.'))
        .filter_map(|t| t.parse::<Ipv4Addr>().ok())
        .filter(Ipv4Addr::is_private)
}

/// The first usable private IPv4 address in `hostname -I` output, or `None`.
pub fn parse_wsl_ip(text: &str) -> Option<Ipv4Addr> {
    private_candidates(text).next()
}

/// The address the relay may forward to: the first private address in `hostname -I` output that
/// lies in the subnet of the WSL virtual adapter and is not the adapter's own address. A
/// distro (anyone with root in it) cannot steer the relay to a machine the adapter cannot
/// reach, to a public address or to the Windows host itself.
pub fn choose_wsl_ip(text: &str, adapters: &[AdapterAddr]) -> Option<Ipv4Addr> {
    private_candidates(text).find(|ip| {
        adapters
            .iter()
            .any(|a| in_adapter_subnet(*ip, *a) && *ip != a.ip)
    })
}

/// Whether a portproxy rule is plausibly the relay goway made for `port`: it forwards to the
/// same port on a private address. Uninstall and status treat anything else as someone else's.
pub fn is_goway_relay(rule: &PortProxyRule, port: u16) -> bool {
    rule.listen_port == port && rule.connect_port == port && rule.connect_address.is_private()
}

/// The rules in `netsh interface portproxy show v4tov4` output (header lines are skipped).
pub fn parse_portproxy_table(text: &str) -> Vec<PortProxyRule> {
    text.lines()
        .filter_map(|line| {
            let cols: Vec<&str> = line.split_whitespace().collect();
            let [la, lp, ca, cp] = cols.as_slice() else {
                return None;
            };
            Some(PortProxyRule {
                listen_address: la.parse().ok()?,
                listen_port: lp.parse().ok()?,
                connect_address: ca.parse().ok()?,
                connect_port: cp.parse().ok()?,
            })
        })
        .collect()
}

/// The first rule that listens on `port` on any address, if any: a conflict for a new relay.
pub fn rule_on_port(rules: &[PortProxyRule], port: u16) -> Option<&PortProxyRule> {
    rules.iter().find(|r| r.listen_port == port)
}

/// netsh arguments that create the relay (`add`) or re-point it (`set`) to `connect`.
pub fn netsh_args(verb: &str, listen: &str, port: u16, connect: &str) -> Vec<String> {
    [
        "interface".to_owned(),
        "portproxy".to_owned(),
        verb.to_owned(),
        "v4tov4".to_owned(),
        format!("listenaddress={listen}"),
        format!("listenport={port}"),
        format!("connectaddress={connect}"),
        format!("connectport={port}"),
    ]
    .into()
}

/// netsh arguments that delete the relay listening on `listen:port`.
pub fn netsh_delete_args(listen: &str, port: u16) -> Vec<String> {
    [
        "interface".to_owned(),
        "portproxy".to_owned(),
        "delete".to_owned(),
        "v4tov4".to_owned(),
        format!("listenaddress={listen}"),
        format!("listenport={port}"),
    ]
    .into()
}

/// The refresh script: read the distro's IPv4, validate it, and re-point the relay only when it
/// changed. Distro and port are literals (both validated before they get here).
///
/// The task runs it elevated, in the user's environment, so it asks Windows (not `%SystemRoot%`,
/// which a user-level variable can override) for the system directory and takes the address
/// only if it is private and inside the WSL adapter's subnet (see [`choose_wsl_ip`]).
pub fn refresh_script(profile: &str, distro: &str, port: u16) -> String {
    let set = netsh_args("set", LISTEN_ADDRESS, port, "$ip").join(" ");
    format!(
        "# Managed by goway-setup (profile {profile}); removed by `goway-setup uninstall`.\n\
         # Re-points the WSL port relay at the distro's current IPv4 address.\n\
         $ErrorActionPreference = 'Stop'\n\
         $distro = '{distro}'\n\
         $port = {port}\n\
         $system = [Environment]::SystemDirectory\n\
         $env:PSModulePath = Join-Path $system 'WindowsPowerShell\\v1.0\\Modules'\n\
         $wsl = Join-Path $system 'wsl.exe'\n\
         $netsh = Join-Path $system 'netsh.exe'\n\
         $octet = '(25[0-5]|2[0-4][0-9]|1[0-9][0-9]|[1-9]?[0-9])'\n\
         $quad = '^' + $octet + '(\\.' + $octet + '){{3}}\\z'\n\
         $private = '^(10\\.|172\\.(1[6-9]|2[0-9]|3[01])\\.|192\\.168\\.)'\n\
         function ConvertTo-Number([string]$text) {{\n\
         \x20   $n = [long]0\n\
         \x20   foreach ($byte in ([System.Net.IPAddress]::Parse($text)).GetAddressBytes()) {{ $n = ($n -shl 8) -bor $byte }}\n\
         \x20   return $n\n\
         }}\n\
         $adapters = @(\n\
         {ADAPTER_QUERY}\
         )\n\
         # Never start WSL from this elevated task: interop would inherit the administrator token.\n\
         $running = ((& $wsl --list --running --quiet | Out-String) -replace [char]0, '') -split '\\s+'\n\
         if ($running -notcontains $distro) {{ exit 0 }}\n\
         $ip = $null\n\
         foreach ($token in ((& $wsl -d $distro --exec hostname -I | Out-String) -split '\\s+')) {{\n\
         \x20   if ($token -match $quad -and $token -match $private) {{\n\
         \x20       foreach ($entry in $adapters) {{\n\
         \x20           $gateway, $length = $entry -split '/'\n\
         \x20           $mask = ([long]4294967295 -shl (32 - [int]$length)) -band 4294967295\n\
         \x20           if ($token -ne $gateway -and ((ConvertTo-Number $token) -band $mask) -eq ((ConvertTo-Number $gateway) -band $mask)) {{ $ip = $token; break }}\n\
         \x20       }}\n\
         \x20   }}\n\
         \x20   if ($ip) {{ break }}\n\
         }}\n\
         if (-not $ip) {{ [Console]::Error.WriteLine('goway relay: no address inside the WSL adapter subnet from the distro'); exit 1 }}\n\
         $current = $null\n\
         foreach ($line in (& $netsh interface portproxy show v4tov4)) {{\n\
         \x20   $cols = ($line.Trim() -split '\\s+')\n\
         \x20   if ($cols.Count -eq 4 -and $cols[0] -eq '{LISTEN_ADDRESS}' -and $cols[1] -eq [string]$port) {{ $current = $cols[2] }}\n\
         }}\n\
         if ($current -eq $ip) {{ exit 0 }}\n\
         & $netsh {set}\n\
         if ($LASTEXITCODE -ne 0) {{ exit $LASTEXITCODE }}\n",
    )
}
