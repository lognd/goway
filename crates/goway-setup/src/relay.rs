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

/// The resource name of the relay for `port` (`0.0.0.0:2222`).
pub fn relay_name(port: u16) -> String {
    format!("{LISTEN_ADDRESS}:{port}")
}

/// Split a relay resource name back into its listen address and port.
pub fn parse_relay_name(name: &str) -> Option<(Ipv4Addr, u16)> {
    let (addr, port) = name.rsplit_once(':')?;
    Some((addr.parse().ok()?, port.parse().ok()?))
}

/// The first usable IPv4 address in `hostname -I` output, or `None`.
///
/// Only a dotted quad passes (no IPv6, no stray words, no leading zeros), and the unspecified,
/// loopback and link-local ranges are refused, so nothing but a real address ever reaches netsh.
pub fn parse_wsl_ip(text: &str) -> Option<Ipv4Addr> {
    text.split_whitespace()
        .filter(|t| t.chars().all(|c| c.is_ascii_digit() || c == '.'))
        .filter_map(|t| t.parse::<Ipv4Addr>().ok())
        .find(|ip| !ip.is_unspecified() && !ip.is_loopback() && !ip.is_link_local())
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
pub fn refresh_script(profile: &str, distro: &str, port: u16) -> String {
    let set = netsh_args("set", LISTEN_ADDRESS, port, "$ip").join(" ");
    format!(
        "# Managed by goway-setup (profile {profile}); removed by `goway-setup uninstall`.\n\
         # Re-points the WSL port relay at the distro's current IPv4 address.\n\
         $ErrorActionPreference = 'Stop'\n\
         $distro = '{distro}'\n\
         $port = {port}\n\
         $system = Join-Path $env:SystemRoot 'System32'\n\
         $wsl = Join-Path $system 'wsl.exe'\n\
         $netsh = Join-Path $system 'netsh.exe'\n\
         $octet = '(25[0-5]|2[0-4][0-9]|1[0-9][0-9]|[1-9]?[0-9])'\n\
         $quad = '^' + $octet + '(\\.' + $octet + '){{3}}\\z'\n\
         $ip = $null\n\
         foreach ($token in ((& $wsl -d $distro --exec hostname -I | Out-String) -split '\\s+')) {{\n\
         \x20   if ($token -match $quad -and $token -notmatch '^(0|127|169\\.254)\\.') {{ $ip = $token; break }}\n\
         }}\n\
         if (-not $ip) {{ [Console]::Error.WriteLine('goway relay: no IPv4 address from the distro'); exit 1 }}\n\
         $current = $null\n\
         foreach ($line in (& $netsh interface portproxy show v4tov4)) {{\n\
         \x20   $cols = ($line.Trim() -split '\\s+')\n\
         \x20   if ($cols.Count -eq 4 -and $cols[0] -eq '{LISTEN_ADDRESS}' -and $cols[1] -eq [string]$port) {{ $current = $cols[2] }}\n\
         }}\n\
         if ($current -eq $ip) {{ exit 0 }}\n\
         & $netsh {set}\n\
         if ($LASTEXITCODE -ne 0) {{ exit $LASTEXITCODE }}\n"
    )
}
