//! Windows-side administrator steps: `--rsudo` for a helper, `--lsudo` for this laptop.
//!
//! Some steps need Windows administrator rights: authorizing goway's key on a native Windows
//! helper (`goway-setup install --host --native --authorized-key`), adding the OpenSSH client
//! on this laptop. A helper is elevated, in this order and never more than once each:
//!
//! 1. through the helper's own Windows OpenSSH server, as an administrator account whose key is
//!    in `administrators_authorized_keys` (unattended, nobody has to be at the helper);
//! 2. through `goway-setup` started by the helper's WSL (interop), which shows the UAC prompt on
//!    the helper's desktop when someone is logged in there;
//! 3. otherwise by stopping with the exact command to run in an administrator PowerShell.
//!
//! goway never stores, asks for or types a password: a key does the ssh login and Windows shows
//! its own UAC prompt. The elevated session never starts WSL: a step that touches a distro first
//! checks `wsl --list --running` (with `WSL_UTF8=1`) and stops when the distro is not running,
//! because WSL started from an administrator session lends the administrator token to every WSL
//! user through interop (see SECURITY.md).

use std::process::Stdio;

use crate::ssh::{self, KeyPolicy, Target};
use crate::transport;

/// One Windows-side step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WinStep {
    /// Why it needs administrator rights, shown when asking.
    pub why: String,
    /// The PowerShell command that does it (what to type in an administrator PowerShell).
    pub command: String,
    /// The WSL distro the step touches, if any: it must already be running.
    pub wsl_distro: Option<String>,
}

impl WinStep {
    /// A `goway-setup` invocation with `args`, run by name (it is on the helper's PATH).
    pub fn setup(args: &[&str], why: &str) -> Self {
        Self {
            why: why.to_owned(),
            command: transport::ps_call(&[&["goway-setup"], args].concat()),
            wsl_distro: None,
        }
    }

    /// The PowerShell source an administrator ssh session runs: the WSL guard (when the step
    /// touches a distro), the command, and its exit code.
    pub fn admin_source(&self) -> String {
        let mut source = String::from("$ErrorActionPreference = 'Stop'; $env:WSL_UTF8 = '1'; ");
        if let Some(distro) = &self.wsl_distro {
            source.push_str(&wsl_guard(distro));
        }
        source.push_str(&self.command);
        source.push_str("; exit $LASTEXITCODE");
        source
    }
}

/// The exit code of an administrator session that stopped because the distro is not running.
pub const EXIT_WSL_NOT_RUNNING: i32 = 3;

/// PowerShell that exits [`EXIT_WSL_NOT_RUNNING`] unless `distro` is already running. It only
/// lists running distros (`wsl --list --running` never starts one).
pub fn wsl_guard(distro: &str) -> String {
    let quoted = transport::ps_quote(distro);
    format!(
        "$running = @(& wsl.exe --list --running --quiet | ForEach-Object {{ ($_ -replace \"`0\", '').Trim() }}); \
         if ($running -notcontains {quoted}) {{ [Console]::Error.WriteLine('WSL distro ' + {quoted} + ' is not running, and an administrator session never starts WSL (interop would lend the administrator token to every WSL user). Start it from a normal terminal, then run this again'); exit {EXIT_WSL_NOT_RUNNING} }}; "
    )
}

/// Why a runner could not do a step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The route itself is unavailable (no login, no prompt): try the next one.
    Unavailable(String),
    /// The route worked and the step ran and failed: do not retry elsewhere.
    StepFailed(String),
}

/// What each route does; real ones use ssh, tests use a script.
pub trait WinRunner {
    /// Whether someone is logged in at the helper's desktop (a UAC prompt could be answered).
    fn desktop_logged_in(&self) -> bool;
    /// Run `step` through the helper's Windows OpenSSH server as the administrator `user`.
    fn admin_ssh(&self, user: &str, step: &WinStep) -> Result<(), Failure>;
    /// Run `step` through `goway-setup`'s UAC prompt on the helper's desktop.
    fn uac(&self, step: &WinStep) -> Result<(), Failure>;
}

/// How a step got its administrator rights.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Through the Windows OpenSSH server as an administrator account.
    AdminSsh,
    /// Through a UAC prompt on the helper's desktop.
    DesktopUac,
}

/// The result of [`elevate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The step ran through `route`.
    Ran(Route),
    /// The step ran through `route` and failed with `why`.
    Failed(Route, String),
    /// No route is available: run [`WinStep::command`] in an administrator PowerShell on the
    /// helper. `tried` says what was attempted and why it did not work.
    Manual {
        /// What each unavailable route reported.
        tried: Vec<String>,
    },
}

/// Run `step` with administrator rights on a helper, trying the administrator ssh account (when
/// one is given), then the desktop UAC prompt, then giving up with [`Outcome::Manual`]. Each
/// route is tried at most once (an extra failed login could trip fail2ban).
pub fn elevate(step: &WinStep, admin_user: Option<&str>, runner: &dyn WinRunner) -> Outcome {
    let mut tried = Vec::new();
    if let Some(user) = admin_user {
        tracing::info!(
            user,
            "elevating through the helper's Windows OpenSSH server"
        );
        match runner.admin_ssh(user, step) {
            Ok(()) => return Outcome::Ran(Route::AdminSsh),
            Err(Failure::StepFailed(why)) => return Outcome::Failed(Route::AdminSsh, why),
            Err(Failure::Unavailable(why)) => {
                tracing::info!(%why, "administrator ssh unavailable");
                tried.push(format!("administrator ssh as {user}: {why}"));
            }
        }
    } else {
        tried.push(
            "no administrator ssh account given (--windows-admin USER, a key in administrators_authorized_keys)"
                .to_owned(),
        );
    }
    if runner.desktop_logged_in() {
        tracing::info!("elevating through a UAC prompt on the helper's desktop");
        match runner.uac(step) {
            Ok(()) => return Outcome::Ran(Route::DesktopUac),
            Err(Failure::StepFailed(why)) => return Outcome::Failed(Route::DesktopUac, why),
            Err(Failure::Unavailable(why)) => {
                tracing::info!(%why, "UAC route unavailable");
                tried.push(format!("UAC prompt on the helper's desktop: {why}"));
            }
        }
    } else {
        tried.push("nobody is logged in at the helper's desktop to answer a UAC prompt".to_owned());
    }
    Outcome::Manual { tried }
}

/// The real runner for a helper: its Windows sshd for the administrator route, and (when the
/// helper has WSL) its WSL ssh target for the desktop route, where interop starts the Windows
/// program in the console session.
pub struct SshWinRunner<'a> {
    /// Name the Windows sshd's host key is pinned under.
    pub key_name: &'a str,
    /// The helper's address.
    pub address: &'a str,
    /// The Windows OpenSSH server's port.
    pub port: u16,
    /// ssh settings shared by every call.
    pub settings: &'a ssh::Settings,
    /// The helper's WSL ssh target, when it has one (the UAC route runs through it).
    pub wsl: Option<&'a Target>,
}

impl SshWinRunner<'_> {
    /// The ssh call running `remote` on the helper's WSL.
    fn wsl_command(&self, wsl: &Target, remote: &str) -> std::process::Command {
        let mut cmd = ssh::command(wsl, self.settings, KeyPolicy::Strict, remote);
        cmd.stdin(Stdio::null());
        cmd
    }
}

/// The exit code of ssh when it could not connect or log in.
const SSH_FAILED: i32 = 255;

/// Judge a finished ssh call: 255 is a route that did not work, other failures are the step's.
fn judge(out: &std::process::Output) -> Result<(), Failure> {
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
    match out.status.code() {
        Some(0) => Ok(()),
        Some(SSH_FAILED) => Err(Failure::Unavailable(stderr)),
        // Including EXIT_WSL_NOT_RUNNING: the session ran, the step did not.
        _ => Err(Failure::StepFailed(stderr)),
    }
}

impl WinRunner for SshWinRunner<'_> {
    fn desktop_logged_in(&self) -> bool {
        let Some(wsl) = self.wsl else { return false };
        self.wsl_command(
            wsl,
            "powershell.exe -NoProfile -Command \"if (Get-Process explorer -ErrorAction SilentlyContinue) { exit 0 } else { exit 1 }\"",
        )
        .status()
        .is_ok_and(|s| s.success())
    }

    fn admin_ssh(&self, user: &str, step: &WinStep) -> Result<(), Failure> {
        // The administrator's own key (default identities), the pinned host key, one attempt.
        let target = Target {
            name: self.key_name.to_owned(),
            address: self.address.to_owned(),
            port: self.port,
            user: Some(user.to_owned()),
            identity: None,
        };
        let line = transport::windows_ssh_line(&step.admin_source());
        let out = ssh::command(&target, self.settings, KeyPolicy::Strict, &line)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| Failure::Unavailable(format!("cannot run ssh: {e}")))?;
        judge(&out)
    }

    fn uac(&self, step: &WinStep) -> Result<(), Failure> {
        let Some(wsl) = self.wsl else {
            return Err(Failure::Unavailable(
                "the helper has no WSL to start goway-setup from".to_owned(),
            ));
        };
        let remote = format!(
            "powershell.exe -NoProfile -Command {}",
            ssh::shell_quote(&format!("{}; exit $LASTEXITCODE", step.command))
        );
        let out = self
            .wsl_command(wsl, &remote)
            .output()
            .map_err(|e| Failure::Unavailable(format!("cannot run ssh: {e}")))?;
        judge(&out)
    }
}

/// The result of [`run_local`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Local {
    /// The step ran (its UAC prompt was answered) and succeeded.
    Done,
    /// PowerShell is not reachable from here (no Windows, no WSL interop).
    NoWindows,
    /// The step ran and failed, or the prompt was declined.
    Failed(String),
}

/// The PowerShell that starts `step` in an elevated PowerShell (the UAC prompt), waits for it and
/// exits with its exit code.
pub fn local_source(step: &WinStep) -> String {
    let inner = format!("{}; exit $LASTEXITCODE", step.command);
    format!(
        "$p = Start-Process powershell -Verb RunAs -Wait -PassThru -ArgumentList '-NoProfile','-Command',{}; exit $p.ExitCode",
        transport::ps_quote(&inner)
    )
}

/// Run `step` on this machine's Windows side with its own UAC prompt (`--lsudo`): PowerShell
/// starts an elevated PowerShell with `-Verb RunAs` and waits. Works from native Windows and
/// from WSL through interop; nothing is typed or stored.
pub fn run_local(step: &WinStep) -> Local {
    let program = if cfg!(windows) {
        "powershell"
    } else {
        "powershell.exe"
    };
    let outer = local_source(step);
    tracing::info!(
        program,
        "running a Windows-side step through this laptop's UAC prompt"
    );
    match std::process::Command::new(program)
        .args(["-NoProfile", "-Command", &outer])
        .stdin(Stdio::null())
        .output()
    {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Local::NoWindows,
        Err(e) => Local::Failed(format!("cannot run {program}: {e}")),
        Ok(out) if out.status.success() => Local::Done,
        Ok(out) => {
            let why: String = String::from_utf8_lossy(&out.stderr)
                .trim()
                .chars()
                .take(400)
                .collect();
            Local::Failed(if why.is_empty() {
                "the elevated step failed or the UAC prompt was declined".to_owned()
            } else {
                why
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A runner that answers from a script and records which routes were tried.
    struct Script {
        desktop: bool,
        admin: Result<(), Failure>,
        uac: Result<(), Failure>,
        calls: RefCell<Vec<&'static str>>,
    }

    impl Script {
        fn new(desktop: bool, admin: Result<(), Failure>, uac: Result<(), Failure>) -> Self {
            Self {
                desktop,
                admin,
                uac,
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl WinRunner for Script {
        fn desktop_logged_in(&self) -> bool {
            self.calls.borrow_mut().push("desktop");
            self.desktop
        }
        fn admin_ssh(&self, _user: &str, _step: &WinStep) -> Result<(), Failure> {
            self.calls.borrow_mut().push("admin");
            self.admin.clone()
        }
        fn uac(&self, _step: &WinStep) -> Result<(), Failure> {
            self.calls.borrow_mut().push("uac");
            self.uac.clone()
        }
    }

    fn step() -> WinStep {
        WinStep::setup(
            &[
                "install",
                "--host",
                "--native",
                "--authorized-key",
                "ssh-ed25519 AAAA k",
            ],
            "authorizes goway's key",
        )
    }

    fn down(why: &str) -> Result<(), Failure> {
        Err(Failure::Unavailable(why.to_owned()))
    }

    // frob:tests crates/goway/src/winadmin.rs::elevate
    #[test]
    fn an_administrator_ssh_account_is_used_first_and_unattended() {
        let r = Script::new(true, Ok(()), Ok(()));
        assert_eq!(
            elevate(&step(), Some("admin"), &r),
            Outcome::Ran(Route::AdminSsh)
        );
        assert_eq!(*r.calls.borrow(), ["admin"], "no UAC prompt when ssh works");
    }

    // frob:tests crates/goway/src/winadmin.rs::elevate
    #[test]
    fn without_an_admin_account_a_logged_in_desktop_gets_the_uac_prompt() {
        let r = Script::new(true, Ok(()), Ok(()));
        assert_eq!(elevate(&step(), None, &r), Outcome::Ran(Route::DesktopUac));
        assert_eq!(*r.calls.borrow(), ["desktop", "uac"]);
        // An admin account that cannot log in falls through, once.
        let r = Script::new(true, down("Permission denied"), Ok(()));
        assert_eq!(
            elevate(&step(), Some("admin"), &r),
            Outcome::Ran(Route::DesktopUac)
        );
        assert_eq!(*r.calls.borrow(), ["admin", "desktop", "uac"]);
    }

    // frob:tests crates/goway/src/winadmin.rs::elevate
    #[test]
    fn with_no_route_it_stops_with_what_was_tried_and_never_prompts_an_empty_desktop() {
        let r = Script::new(false, down("Permission denied"), Ok(()));
        let Outcome::Manual { tried } = elevate(&step(), Some("admin"), &r) else {
            panic!("expected the manual outcome");
        };
        assert_eq!(tried.len(), 2, "{tried:?}");
        assert!(tried[0].contains("admin") && tried[0].contains("Permission denied"));
        assert!(tried[1].contains("nobody is logged in"));
        assert_eq!(
            *r.calls.borrow(),
            ["admin", "desktop"],
            "no UAC with nobody there"
        );
    }

    // frob:tests crates/goway/src/winadmin.rs::elevate
    #[test]
    fn a_step_that_ran_and_failed_is_not_retried_on_another_route() {
        let r = Script::new(true, Err(Failure::StepFailed("exit 1".into())), Ok(()));
        assert_eq!(
            elevate(&step(), Some("admin"), &r),
            Outcome::Failed(Route::AdminSsh, "exit 1".into())
        );
        assert_eq!(*r.calls.borrow(), ["admin"]);
    }

    // frob:tests crates/goway/src/winadmin.rs::wsl_guard
    // frob:tests crates/goway/src/winadmin.rs::WinStep
    #[test]
    fn an_administrator_session_checks_wsl_is_running_and_never_starts_it() {
        let mut s = step();
        s.wsl_distro = Some("Ubuntu".to_owned());
        let src = s.admin_source();
        let guard = src.find("--list --running").expect("guard present");
        let command = src.find("goway-setup").expect("command present");
        assert!(guard < command, "the check comes first: {src}");
        assert!(src.contains("WSL_UTF8"), "{src}");
        assert!(
            src.contains(&format!("exit {EXIT_WSL_NOT_RUNNING}")),
            "{src}"
        );
        // Nothing in the session can start a distro.
        for starter in ["wsl -d", "wsl.exe -d", "--exec", "wsl --exec", "bash.exe"] {
            assert!(!src.contains(starter), "{starter} in {src}");
        }
        // A step that touches no distro carries no guard.
        assert!(!step().admin_source().contains("wsl.exe"));
    }

    // frob:tests crates/goway/src/winadmin.rs::local_source
    #[test]
    fn a_local_step_goes_through_this_laptops_uac_prompt_and_keeps_its_exit_code() {
        let step = WinStep {
            why: "adds a feature".to_owned(),
            command: "Add-WindowsCapability -Online -Name 'X'".to_owned(),
            wsl_distro: None,
        };
        let src = local_source(&step);
        assert!(
            src.contains("-Verb RunAs") && src.contains("-Wait"),
            "{src}"
        );
        // The inner command is one quoted word: its own quotes are doubled.
        assert!(src.contains("-Name ''X''"), "{src}");
        assert!(src.ends_with("exit $p.ExitCode"), "{src}");
        assert!(
            !src.contains("Password") && !src.contains("-Credential"),
            "{src}"
        );
    }

    // frob:tests crates/goway/src/winadmin.rs::WinStep
    #[test]
    fn a_setup_step_quotes_its_arguments_for_powershell_and_keeps_the_exit_code() {
        let s = step();
        assert!(
            s.command.starts_with("& 'goway-setup' 'install'"),
            "{}",
            s.command
        );
        assert!(s.command.contains("'ssh-ed25519 AAAA k'"), "{}", s.command);
        assert!(s.admin_source().ends_with("; exit $LASTEXITCODE"));
    }
}
