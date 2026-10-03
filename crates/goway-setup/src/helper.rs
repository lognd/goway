//! What a person setting up a helper laptop is told: the WSL precheck steps, the block with
//! the name, fingerprint, user and the exact `goway add` command, and the plain-words fixes.
//!
//! Everything here is pure text from probe results, so it is tested off Windows; the probes
//! themselves live in [`crate::hostsys`].

use crate::host::{DEFAULT_PORT, NetworkMode};

/// The ssh host key type goway pins; the one the next-steps block fingerprints.
pub const HOST_KEY_FILE: &str = "/etc/ssh/ssh_host_ed25519_key.pub";

/// What probing WSL on this laptop found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WslProbe {
    /// Whether `wsl.exe` exists in the system directory.
    pub wsl_exe: bool,
    /// Installed distro names, or `None` when WSL could not list them (not set up).
    pub distros: Option<Vec<String>>,
}

/// The verdict of the WSL precheck.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WslCheck {
    /// WSL and the wanted distro are there.
    Ready,
    /// WSL itself is not set up.
    NoWsl,
    /// WSL works but the wanted distro is not installed (`installed` lists what is).
    NoDistro {
        /// The distros that are installed.
        installed: Vec<String>,
    },
}

/// Decide whether this laptop is ready for the host install, before anything is changed.
pub fn check_wsl(probe: &WslProbe, distro: &str) -> WslCheck {
    if !probe.wsl_exe {
        return WslCheck::NoWsl;
    }
    match &probe.distros {
        None => WslCheck::NoWsl,
        Some(list) if list.iter().any(|d| d.eq_ignore_ascii_case(distro)) => WslCheck::Ready,
        Some(list) if list.is_empty() => WslCheck::NoWsl,
        Some(list) => WslCheck::NoDistro {
            installed: list.clone(),
        },
    }
}

/// The exact steps for a laptop that fails the precheck, in plain words; `None` when ready.
pub fn wsl_steps(check: &WslCheck, distro: &str) -> Option<String> {
    let why = match check {
        WslCheck::Ready => return None,
        WslCheck::NoWsl => {
            format!("This laptop does not have WSL (Linux inside Windows) with {distro} yet.")
        }
        WslCheck::NoDistro { installed } => format!(
            "WSL is here but the {distro} Linux is not installed (found: {}).",
            installed.join(", ")
        ),
    };
    Some(format!(
        "{why} Nothing was changed. Set it up once, then run this command again:\n\
         \n  1. Open the Start menu, type PowerShell, right-click \"Windows PowerShell\" and choose \"Run as administrator\".\
         \n  2. Type this and press Enter:    wsl --install -d {distro}\
         \n  3. Restart the laptop when it asks you to.\
         \n  4. When the {distro} window opens after the restart, wait. It asks you to create a Linux user name and a password: choose ones you will remember, you need both on your main laptop.\
         \n  5. Close the {distro} window and run this again:    goway-setup.exe install --host"
    ))
}

/// A name goway accepts for a host (letters, digits, `-` and `_`, 1 to 63 characters, not
/// starting with `-`) made from the Windows device name; `None` when nothing usable is left.
pub fn helper_name(device_name: &str) -> Option<String> {
    let mut name: String = device_name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    name = name.trim_matches('-').to_owned();
    name.truncate(63);
    let name = name.trim_end_matches('-').to_owned();
    (!name.is_empty()).then_some(name)
}

/// The `SHA256:...` token of `ssh-keygen -lf` output (`256 SHA256:abc root@host (ED25519)`).
pub fn parse_fingerprint(text: &str) -> Option<String> {
    text.split_whitespace()
        .find(|t| {
            t.strip_prefix("SHA256:").is_some_and(|b| {
                !b.is_empty()
                    && b.chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '='))
            })
        })
        .map(str::to_owned)
}

/// What the next-steps block needs to know about this helper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperInfo {
    /// The Windows device name as Windows reports it.
    pub device_name: String,
    /// The ssh host key fingerprint, when the key exists yet.
    pub fingerprint: Option<String>,
    /// The Linux user the password belongs to (the distro's default user).
    pub user: String,
    /// The sshd port of the install.
    pub port: u16,
    /// How other computers reach the distro (mirrored networking or the NAT relay).
    pub network: NetworkMode,
}

/// The block printed when the helper install finishes (and by `status --host`).
///
/// It is the whole hand-over: the name, the fingerprint to compare, the Linux user and the one
/// command to run on the main laptop. The port is added to the command only when it is not the
/// default 2222.
pub fn next_steps(info: &HelperInfo) -> String {
    let rule = "=".repeat(70);
    let name = helper_name(&info.device_name);
    let shown_name = name.as_deref().unwrap_or("NAME-FOR-THIS-LAPTOP");
    let user = &info.user;
    let mut lines = vec![
        rule.clone(),
        " THIS LAPTOP IS READY TO BE A HELPER. Next, on your MAIN laptop.".to_owned(),
        rule.clone(),
        String::new(),
        format!("Name to use for this helper:  {shown_name}  (this laptop's Windows name)"),
    ];
    if name.is_none() {
        lines.push(
            "  (the Windows name has no letters or digits goway can use: pick any short name)"
                .to_owned(),
        );
    }
    match &info.fingerprint {
        Some(fp) => lines.extend([
            format!("Its ssh host key fingerprint: {fp}"),
            format!(
                "Its Linux user:               {user}  (goway asks for this user's password once: the one you chose when Ubuntu was set up)"
            ),
            String::new(),
            "On your main laptop run exactly this:".to_owned(),
            String::new(),
            format!("    {}", add_command(shown_name, fp, user, info.port)),
            String::new(),
            "goway shows the same fingerprint there; it must match the one above.".to_owned(),
        ]),
        None => lines.extend([
            "Its ssh host key fingerprint: not available yet (sshd has not made its host key)."
                .to_owned(),
            format!("Its Linux user:               {user}"),
            String::new(),
            "Wait a minute, then run `goway-setup.exe status --host` on this laptop to see this again."
                .to_owned(),
        ]),
    }
    lines.push(String::new());
    lines.push(network_line(info.network, info.port));
    if user == "root" {
        lines.push(
            "Note: the Linux user is root, so Ubuntu has no ordinary user yet. Open Ubuntu from the Start menu and create one first."
                .to_owned(),
        );
    }
    lines.push(rule);
    lines.join("\n")
}

/// The one-line statement of how this helper is reached, for the next-steps block and `status`.
pub fn network_line(network: NetworkMode, port: u16) -> String {
    match network {
        NetworkMode::Mirrored => {
            "Network mode: mirrored (WSL shares this laptop's network; the firewall rules are the only gate).".to_owned()
        }
        NetworkMode::Nat => format!(
            "Network mode: nat (the helper is reached through the Windows relay on port {port}, which a scheduled task keeps pointed at WSL; the command above is unchanged)."
        ),
    }
}

/// The command to run on the main laptop to add this helper.
pub fn add_command(name: &str, fingerprint: &str, user: &str, port: u16) -> String {
    let base = format!("goway add {name} --fingerprint {fingerprint} --user {user}");
    if port == DEFAULT_PORT {
        base
    } else {
        format!("{base} --port {port}")
    }
}

/// The warning for a connected network Windows classifies as Public, with the one-line fix.
pub fn public_network_warning(name: &str) -> String {
    format!(
        "this laptop is on the network {name:?}, which Windows treats as Public (Windows does this for cafe and hotel Wi-Fi). \
         goway only opens its door on home or work (Private) networks, so your main laptop cannot reach this helper over this network. \
         If this is a network you trust, open PowerShell as administrator and run: Set-NetConnectionProfile -Name '{}' -NetworkCategory Private",
        name.replace('\'', "''")
    )
}
