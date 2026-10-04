//! `goway add HOST`: register a helper laptop in one command.
//!
//! It chains what a newcomer would otherwise run one by one: the local
//! check (ssh and git on this laptop), key login with a confirmed host key
//! and one password prompt (`goway ssh setup`), the user-level toolchain on
//! the helper, and, with `--rsudo`, the helper's root fixes in one sudo
//! session. Running it again changes nothing that is already in place.
//!
//! goway never sees a password: ssh and sudo ask for them on the terminal
//! themselves, over ssh's encrypted connection for the helper.

use std::path::Path;

use crate::cli::{AddArgs, DoctorArgs, SshSetupArgs};
use crate::error::{Error, Result};
use crate::paths::Paths;
use crate::render::Renderer;
use crate::resolve::{Lookup, Prober};
use crate::{doctor, ssh, sshsetup};

/// A tool this laptop needs, and the package that provides it.
const LOCAL_TOOLS: &[(&str, &str)] = &[("ssh", "openssh-client"), ("git", "git")];

/// Which of [`LOCAL_TOOLS`] are missing, given a lookup; pure for tests.
pub fn missing_tools(has: &dyn Fn(&str) -> bool) -> Vec<(&'static str, &'static str)> {
    LOCAL_TOOLS
        .iter()
        .copied()
        .filter(|(tool, _)| !has(tool))
        .collect()
}

fn on_path(tool: &str) -> bool {
    std::process::Command::new(tool)
        .arg("-V")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

/// Make sure this laptop has ssh and git; with `--lsudo`, install them
/// (after one confirmation) with the system package manager.
fn local_check(renderer: Renderer, lsudo: bool, yes: bool) -> Result<()> {
    let missing = missing_tools(&on_path);
    if missing.is_empty() {
        return Ok(());
    }
    let packages: Vec<&str> = missing.iter().map(|(_, p)| *p).collect();
    let command = format!(
        "apt-get update && apt-get install -y {}",
        packages.join(" ")
    );
    let tools: Vec<&str> = missing.iter().map(|(t, _)| *t).collect();
    renderer.warn(format_args!(
        "this laptop is missing {}; goway needs them to reach the helper",
        tools.join(" and ")
    ));
    if !lsudo || cfg!(windows) {
        renderer.next(format_args!(
            "install them with: sudo bash -c '{command}' (or rerun with --lsudo)"
        ));
        return Err(Error::Usage(format!("missing {}", tools.join(", "))));
    }
    let approved = yes
        || crate::render::ask(&format!(
            "Run `sudo bash -c '{command}'` on this laptop? sudo asks for your password [y/N]: "
        ))
        .is_some_and(|a| matches!(a.trim(), "y" | "Y" | "yes" | "Yes" | "YES"));
    if !approved {
        return Err(Error::Usage("local tools not installed".to_owned()));
    }
    let Some(sudo) = system_sudo() else {
        return Err(Error::Usage(
            "sudo was not found in /usr/bin, /bin or /usr/local/bin; install the tools yourself"
                .to_owned(),
        ));
    };
    let ok = std::process::Command::new(sudo)
        .args(["/bin/bash", "-c", &command])
        .status()
        .is_ok_and(|s| s.success());
    if ok {
        renderer.ok(format_args!("installed {}", packages.join(" ")));
        Ok(())
    } else {
        Err(Error::Usage(format!(
            "installing {} failed",
            packages.join(" ")
        )))
    }
}

/// Root-owned system directories searched for sudo, in order. A PATH lookup
/// would let a user-writable directory early in PATH (such as `~/.local/bin`)
/// stand in for sudo and capture the password.
const SUDO_PATHS: &[&str] = &["/usr/bin/sudo", "/bin/sudo", "/usr/local/bin/sudo"];

/// The first existing system sudo, never found through PATH.
fn system_sudo() -> Option<&'static str> {
    SUDO_PATHS.iter().copied().find(|p| Path::new(p).is_file())
}

/// `goway add`.
pub fn add(
    paths: &Paths,
    renderer: Renderer,
    args: &AddArgs,
    lookup: &(dyn Lookup + Sync),
    prober: &(dyn Prober + Sync),
    settings: &ssh::Settings,
) -> Result<u8> {
    if (args.rsudo || args.lsudo)
        && !args.yes
        && !std::io::IsTerminal::is_terminal(&std::io::stdin())
    {
        return Err(Error::Usage(
            "--rsudo and --lsudo need a terminal to confirm (or add --yes); sudo itself asks for the password".to_owned(),
        ));
    }
    local_check(renderer, args.lsudo, args.yes)?;

    // 1. Key login and pinned identity (skipped when already done).
    if sshsetup::record_path(paths, &args.host).exists() {
        renderer.note(format_args!(
            "key login to {} was set up before; checking it",
            args.host
        ));
    } else {
        let setup = SshSetupArgs {
            host: args.host.clone(),
            undo: false,
            address: args.address.clone(),
            port: args.port,
            user: args.user.clone(),
            key: args.key.clone(),
            fingerprint: args.fingerprint.clone(),
            no_password: args.no_password,
        };
        let code = sshsetup::setup_with(paths, renderer, &setup, lookup, args.yes)?;
        if code != 0 {
            return Ok(code);
        }
    }

    // 2. Toolchain on the helper (user-level), root fixes with --rsudo.
    let doctor_args = DoctorArgs {
        host: Some(args.host.clone()),
        fix: true,
        rsudo: args.rsudo,
        yes: args.yes,
    };
    let code = doctor::doctor(paths, renderer, &doctor_args, lookup, prober, settings)?;
    if code == 0 {
        renderer.ok(format_args!("{} is ready", args.host));
        renderer.next(format_args!(
            "in a project folder, run a command there: goway run --host {} -- cargo test",
            args.host
        ));
    } else {
        renderer.next(format_args!(
            "fix what is listed above (`goway add {} --rsudo` runs the administrator steps), then check with `goway doctor {}`",
            args.host, args.host
        ));
    }
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sudo_is_only_ever_a_root_owned_absolute_path() {
        assert!(SUDO_PATHS.iter().all(|p| Path::new(p).is_absolute()));
        if let Some(sudo) = system_sudo() {
            assert!(SUDO_PATHS.contains(&sudo));
        }
    }

    #[test]
    fn missing_local_tools_map_to_packages() {
        assert!(missing_tools(&|_| true).is_empty());
        assert_eq!(missing_tools(&|t| t != "ssh"), [("ssh", "openssh-client")]);
    }
}
