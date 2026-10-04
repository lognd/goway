//! Command line and command dispatch.

use std::io::IsTerminal as _;
use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand};
use goway_journal::{LocalSystem, SystemError, sha256_hex, still_applied};

use crate::admin;
use crate::app::{self, Retry};
use crate::elevate;
use crate::error::SetupError;
use crate::helper::{HelperInfo, check_wsl, next_steps, public_network_warning, wsl_steps};
use crate::host::{
    self, DEFAULT_DISTRO, DEFAULT_PORT, HostFacts, HostParams, HostSettings, Keepalive,
    NetworkChoice, NetworkMode, host_plan,
};
use crate::hostsys::{HostSystem, Invocation, ProcessRunner, Runner, probe_wsl, wsl_exe_present};
use crate::layout::{DEFAULT_PROFILE, Layout};
use crate::native::{self, NATIVE_PORT};
use crate::plan::{Component, Sources, build};
use crate::render::{ColorWhen, Renderer};
use crate::stage;
use crate::sysapi::{Tool, tool_path};
use crate::tune;
use crate::windows::{
    broadcast_environment_change, schedule_dir_removal, schedule_self_delete, spawn_detached,
};

/// Retry budget when files are briefly locked (a parent process still exiting, an antivirus scan).
const RETRY: Retry = Retry {
    attempts: 20,
    delay: std::time::Duration::from_millis(250),
};

/// Install and provably uninstall goway.
#[derive(Debug, Parser)]
#[command(name = "goway-setup", version, about)]
pub struct Cli {
    /// When to color output.
    #[arg(long, value_enum, default_value_t, global = true)]
    pub color: ColorWhen,
    /// Raise diagnostic verbosity (repeat for more).
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,
    /// What to do.
    #[command(subcommand)]
    pub command: Command,
}

/// Which installation profile to act on.
#[derive(Debug, Args)]
pub struct ProfileArg {
    /// Profile name: names the install directory, journal and Add/Remove Programs entry.
    #[arg(long, default_value = DEFAULT_PROFILE)]
    pub profile: String,
}

/// Hidden arguments of the elevated re-run (the host component only; see [`Elevate`]).
#[derive(Debug, Clone, Default, Args)]
pub struct ChildArgs {
    /// Internal: this is the elevated re-run; never elevate again.
    #[arg(long, hide = true)]
    pub elevated_child: bool,
    /// Internal: SID of the user who started the install; the elevated run must be that user.
    #[arg(long, hide = true, value_name = "SID")]
    pub invoker_sid: Option<String>,
    /// Internal: SHA-256 of the exe the parent staged and locked; the elevated run hashes its own
    /// image and refuses to continue when it differs.
    #[arg(long, hide = true, value_name = "HEX")]
    pub exe_sha256: Option<String>,
    /// Internal: name of the log file (in the administrator-only directory) for the output.
    #[arg(long, hide = true, value_name = "NAME")]
    pub elevated_log: Option<String>,
}

/// The goway-setup subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Install goway for the current user and record every change in a journal.
    Install {
        /// Install the client component (goway.exe, user Path, Add/Remove Programs); the default.
        #[arg(long)]
        client: bool,
        /// Install the host component (firewall, keepalive task, .wslconfig, WSL sshd); needs administrator rights.
        #[arg(long)]
        host: bool,
        /// Show the changes without making them.
        #[arg(long)]
        dry_run: bool,
        /// Host: set the machine up through Windows' own OpenSSH Server instead of WSL (a Windows
        /// helper without WSL). Installs the OpenSSH Server capability when absent, starts sshd
        /// on boot, opens port 22 to local networks only and makes PowerShell the login shell.
        #[arg(long, requires = "host")]
        native: bool,
        /// Host (native): a public key to authorize for this account: one ssh public key line or
        /// the path of a `.pub` file (administrator accounts use `administrators_authorized_keys`).
        #[arg(long, value_name = "KEY", requires = "native")]
        authorized_key: Option<String>,
        /// Host: TCP port the WSL sshd listens on (default 2222; a native install uses 22).
        #[arg(long)]
        port: Option<u16>,
        /// Host: WSL distro to set up.
        #[arg(long, default_value = DEFAULT_DISTRO)]
        distro: String,
        /// Host: when the keepalive task starts the distro.
        #[arg(long, value_enum, default_value_t)]
        keepalive: Keepalive,
        /// Host: allow `--keepalive boot` on an administrator account although WSL interop stays
        /// on: a boot task gets the full administrator token, so every user of the distro could
        /// then act as a Windows administrator through interop.
        #[arg(long)]
        allow_elevated_wsl: bool,
        /// Host: how other computers reach the WSL sshd. `auto` uses mirrored networking when
        /// this Windows supports it (11 22H2+) and `.wslconfig` does not say nat, otherwise a
        /// Windows port relay (netsh portproxy) that a scheduled task keeps pointed at WSL.
        #[arg(long, value_enum, default_value_t)]
        network: NetworkChoice,
        /// Host: deprecated no-op; hardening is the default (see --no-harden).
        #[arg(long, hide = true, conflicts_with = "no_harden")]
        harden: bool,
        /// Host: do not disable sshd password logins (by default they are disabled once the
        /// distro's default user has an authorized key).
        #[arg(long)]
        no_harden: bool,
        /// Host: also admit this remote address or CIDR (repeatable; for example Tailscale's
        /// 100.64.0.0/10); by default only the local subnet may connect.
        #[arg(long, value_name = "CIDR")]
        allow_from: Vec<String>,
        /// Host: allow `--allow-from` ranges wider than /8 (IPv4) or /16 (IPv6). Such ranges
        /// admit large parts of the internet; the Private and Domain firewall profiles remain.
        #[arg(long)]
        allow_wide: bool,
        /// Host: do not reload or restart sshd or start the keepalive task afterwards.
        #[arg(long)]
        no_activate: bool,
        /// Host: fail instead of asking Windows (UAC) for administrator rights.
        #[arg(long)]
        no_elevate: bool,
        /// Host: ask no questions; when WSL needs a restart, print the command instead of offering it.
        #[arg(long)]
        yes: bool,
        /// Profile selection.
        #[command(flatten)]
        profile: ProfileArg,
        /// Internal: elevated-run plumbing.
        #[command(flatten)]
        child: ChildArgs,
    },
    /// Undo everything the install did by replaying its journal backwards.
    Uninstall {
        /// Uninstall only the client component (default: every component with a journal).
        #[arg(long)]
        client: bool,
        /// Uninstall only the host component.
        #[arg(long)]
        host: bool,
        /// Host: do not reload or restart sshd afterwards.
        #[arg(long)]
        no_activate: bool,
        /// Host: fail instead of asking Windows (UAC) for administrator rights.
        #[arg(long)]
        no_elevate: bool,
        /// Profile selection.
        #[command(flatten)]
        profile: ProfileArg,
        /// Internal: elevated-run plumbing.
        #[command(flatten)]
        child: ChildArgs,
        /// Internal: this is the relaunched temporary copy; delete its directory when done.
        #[arg(long, hide = true, value_name = "DIR")]
        relaunched: Option<PathBuf>,
    },
    /// Give the WSL helper more of this machine: journaled `.wslconfig` memory, swap,
    /// processors and nested virtualization (undone by `uninstall --host`).
    Tune {
        /// Memory for WSL, such as 12GB.
        #[arg(long, value_name = "SIZE")]
        memory: Option<String>,
        /// Swap for WSL, such as 4GB (0 disables it).
        #[arg(long, value_name = "SIZE")]
        swap: Option<String>,
        /// Processors for WSL.
        #[arg(long, value_name = "N")]
        processors: Option<u32>,
        /// Nested virtualization (KVM inside WSL).
        #[arg(long, value_name = "BOOL")]
        nested_virtualization: Option<bool>,
        /// WSL distro whose goway jobs are counted before a restart.
        #[arg(long, default_value = DEFAULT_DISTRO)]
        distro: String,
        /// Change `.wslconfig` even while goway jobs run (WSL is then never restarted for you).
        #[arg(long)]
        yes: bool,
        /// Print the changes and change nothing.
        #[arg(long)]
        dry_run: bool,
        /// Profile selection.
        #[command(flatten)]
        profile: ProfileArg,
    },
    /// Show the journal and whether each target still holds goway's value.
    Status {
        /// Profile selection.
        #[command(flatten)]
        profile: ProfileArg,
    },
}

/// Components selected by the flags; the client when none is named.
pub fn selected_components(client: bool, host: bool) -> Vec<Component> {
    match (client, host) {
        (false | true, false) => vec![Component::Client],
        (false, true) => vec![Component::Host],
        (true, true) => vec![Component::Client, Component::Host],
    }
}

/// The sshd port of the request: Windows' own sshd listens on 22 and the native install does not
/// move it; the WSL sshd defaults to 2222.
fn resolve_port(native: bool, port: Option<u16>) -> Result<u16, SetupError> {
    match (native, port) {
        (true, None) => Ok(NATIVE_PORT),
        (true, Some(p)) if p == NATIVE_PORT => Ok(p),
        (true, Some(p)) => Err(SetupError::NativeOption(format!(
            "--port {p}: Windows' OpenSSH Server stays on port {NATIVE_PORT} in this version"
        ))),
        (false, p) => Ok(p.unwrap_or(DEFAULT_PORT)),
    }
}

/// Everything `install` was asked for, gathered from the flags.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)] // one bool per command-line switch
struct InstallRequest {
    components: Vec<Component>,
    native: bool,
    authorized_key: Option<String>,
    dry_run: bool,
    port: u16,
    distro: String,
    keepalive: Keepalive,
    allow_elevated_wsl: bool,
    network: NetworkChoice,
    harden: bool,
    allow_from: Vec<String>,
    allow_wide: bool,
    activate: bool,
    yes: bool,
    elevate: Elevate,
}

/// Whether and how the process may obtain administrator rights.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Elevate {
    allowed: bool,
    is_child: bool,
    invoker_sid: Option<String>,
    exe_sha256: Option<String>,
    log: Option<String>,
}

impl Elevate {
    fn new(no_elevate: bool, child: &ChildArgs) -> Self {
        Self {
            allowed: !no_elevate,
            is_child: child.elevated_child,
            invoker_sid: child.invoker_sid.clone(),
            exe_sha256: child.exe_sha256.clone(),
            log: child.elevated_log.clone(),
        }
    }
}

/// Run the parsed command.
pub fn run(cli: &Cli, r: Renderer) -> Result<(), SetupError> {
    match &cli.command {
        Command::Install {
            client,
            host,
            dry_run,
            native,
            authorized_key,
            port,
            distro,
            keepalive,
            allow_elevated_wsl,
            network,
            harden: _,
            no_harden,
            allow_from,
            allow_wide,
            no_activate,
            no_elevate,
            yes,
            profile,
            child,
        } => install(
            r,
            &profile.profile,
            &InstallRequest {
                components: selected_components(*client, *host),
                native: *native,
                authorized_key: authorized_key.clone(),
                dry_run: *dry_run,
                port: resolve_port(*native, *port)?,
                distro: distro.clone(),
                keepalive: *keepalive,
                allow_elevated_wsl: *allow_elevated_wsl,
                network: *network,
                harden: !*no_harden,
                allow_from: allow_from.clone(),
                allow_wide: *allow_wide,
                activate: !*no_activate,
                yes: *yes,
                elevate: Elevate::new(*no_elevate, child),
            },
        ),
        Command::Uninstall {
            client,
            host,
            no_activate,
            no_elevate,
            profile,
            child,
            relaunched,
        } => {
            let components = if *client || *host {
                selected_components(*client, *host)
            } else {
                vec![Component::Host, Component::Client]
            };
            let elevate = Elevate::new(*no_elevate, child);
            let result = uninstall(
                r,
                &profile.profile,
                &components,
                !*no_activate,
                &elevate,
                relaunched.as_deref(),
            );
            if relaunched.is_none() && !child.elevated_child {
                keep_window_open(r, result.as_ref().err());
            }
            result
        }
        Command::Tune {
            memory,
            swap,
            processors,
            nested_virtualization,
            distro,
            yes,
            dry_run,
            profile,
        } => {
            let req = tune::TuneRequest {
                memory: memory.as_deref().map(tune::parse_size).transpose()?,
                swap: swap.as_deref().map(tune::parse_size).transpose()?,
                processors: processors.map(tune::parse_processors).transpose()?,
                nested_virtualization: *nested_virtualization,
            };
            run_tune(r, &profile.profile, distro, &req, *yes, *dry_run)
        }
        Command::Status { profile } => status(r, &profile.profile),
    }
}

/// goway's default remote root inside the distro (relative to the default user's home).
const DEFAULT_REMOTE_ROOT: &str = ".cache/goway";

/// `goway-setup tune`: journal the `.wslconfig` edits, then offer the WSL restart that applies them.
fn run_tune(
    r: Renderer,
    profile: &str,
    distro: &str,
    req: &tune::TuneRequest,
    yes: bool,
    dry_run: bool,
) -> Result<(), SetupError> {
    if req.is_empty() {
        return Err(SetupError::BadTuneValue(
            "nothing to change: pass --memory, --swap, --processors or --nested-virtualization"
                .to_owned(),
        ));
    }
    let layout = Layout::from_environment(profile)?;
    let home = dirs::home_dir().ok_or(SetupError::NoLocalAppData)?;
    let plan = tune::tune_plan(&tune::wslconfig_path(&home), req);
    if dry_run {
        r.plan_component(".wslconfig tuning", &plan, None);
        return Ok(());
    }
    if !cfg!(windows) {
        return Err(SetupError::HostNeedsWindows);
    }
    let mut sys = HostSystem::new(distro);
    let jobs = sys.goway_jobs_running(DEFAULT_REMOTE_ROOT)?;
    if jobs > 0 && !yes {
        return Err(SetupError::TuneJobsRunning(jobs));
    }
    let journal = tune::apply_tune(&mut LocalSystem, &layout.tune_journal_path, &plan)?;
    tracing::info!(entries = journal.entries.len(), "tuned .wslconfig");
    r.notice(&format!(
        "changed {} .wslconfig setting(s); `goway-setup uninstall --host` restores the previous values",
        plan.len()
    ));
    offer_restart(
        r,
        &layout,
        &mut sys,
        host::restart_need(&journal),
        jobs == 0,
    );
    Ok(())
}

/// Say that the new values need `wsl --shutdown`, and run it only after an explicit yes, with no
/// goway jobs running. WSL is brought back through the Limited logon keepalive task, never with
/// `wsl.exe -d` from here: whatever starts WSL decides the token of its interop (see SECURITY.md).
fn offer_restart(
    r: Renderer,
    layout: &Layout,
    sys: &mut HostSystem,
    need: host::RestartNeed,
    idle: bool,
) {
    let Some(command) = need.command(&sys.distro) else {
        return;
    };
    let console = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let task = host::task_name(&layout.profile, Keepalive::Logon);
    let after = format!(
        "WSL then starts again through the `{task}` task (or open a normal, non-administrator terminal and run `wsl -d {}`; never start WSL from an administrator terminal)",
        sys.distro
    );
    if !idle || !console {
        r.notice(&format!(
            "the new values apply after a WSL restart. {} When you are ready, run `{command}` from a normal terminal. {after}",
            need.consequence()
        ));
        return;
    }
    let question = format!(
        "The new values apply after `{command}`. {} {after}. Restart WSL now? [y/N] ",
        need.consequence()
    );
    if !ask_yes_no(r, &question) {
        r.notice(&format!(
            "not restarting; when you are ready, run: {command}"
        ));
        return;
    }
    restart_wsl(r, need, &sys.distro, &command);
    match sys.start_task(&task) {
        Ok(()) => r.notice(&format!(
            "started the `{task}` task; WSL comes back limited"
        )),
        Err(e) => {
            tracing::warn!(error = %e, task, "could not start the keepalive task");
            r.notice(&format!(
                "could not start the `{task}` task ({e}); it starts at your next logon, or run `wsl -d {}` from a normal terminal",
                sys.distro
            ));
        }
    }
}

/// Revert the user-level `.wslconfig` tuning (it needs no administrator rights).
fn uninstall_tune(r: Renderer, layout: &Layout) -> Result<bool, SetupError> {
    if !layout.tune_journal_path.exists() {
        return Ok(false);
    }
    let journal = goway_journal::Journal::load(&layout.tune_journal_path)?;
    let need = host::restart_need(&journal);
    tune::revert_tune(&mut LocalSystem, &layout.tune_journal_path)?;
    r.notice("restored the previous .wslconfig settings");
    if let Some(command) = need.command("") {
        r.notice(&format!(
            "they apply after the next WSL restart ({command})"
        ));
    }
    Ok(true)
}

/// When Windows opened this console just for us (Add/Remove Programs runs the uninstall entry
/// that way), keep it open until the user has read the result; otherwise it vanishes at once.
fn keep_window_open(r: Renderer, error: Option<&SetupError>) {
    if !std::io::stdin().is_terminal() || !crate::windows::owns_console_alone() {
        return;
    }
    if let Some(e) = error {
        r.error(e);
    }
    r.prompt("Press Enter to close this window. ");
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
}

fn staging_dir() -> PathBuf {
    std::env::temp_dir().join(format!("goway-setup-{}", std::process::id()))
}

fn current_exe() -> Result<PathBuf, SetupError> {
    std::env::current_exe().map_err(|e| SetupError::io(Path::new("<current exe>"), e))
}

fn install(r: Renderer, profile: &str, req: &InstallRequest) -> Result<(), SetupError> {
    let layout = Layout::from_environment(profile)?;
    // A key given as a file is read here, with this user's own rights, and passed on (to the
    // elevated re-run) as the validated line, never as a path an administrator token would open.
    let resolved;
    let req = match &req.authorized_key {
        Some(argument) if req.native => {
            let line = native::key_from_argument(argument, |p| std::fs::read_to_string(p).ok())?;
            resolved = InstallRequest {
                authorized_key: Some(line),
                ..req.clone()
            };
            &resolved
        }
        _ => req,
    };
    let wants_host = req.components.contains(&Component::Host);
    let mut staged: Option<stage::StagedExe> = None;
    if req.elevate.is_child && req.components.contains(&Component::Client) {
        return Err(SetupError::ClientNeverElevated);
    }
    if wants_host {
        host::validate_distro(&req.distro)?;
        for cidr in &req.allow_from {
            host::validate_allow_from_with(cidr, req.allow_wide)?;
        }
        if !req.dry_run && !cfg!(windows) {
            return Err(SetupError::HostNeedsWindows);
        }
        if req.elevate.is_child && !req.dry_run {
            let want = req.elevate.exe_sha256.as_deref().ok_or_else(|| {
                SetupError::NeedsAdmin(
                    "the elevated re-run was not given the digest of the exe its parent locked"
                        .to_owned(),
                )
            })?;
            stage::verify_image(&current_exe()?, want)?;
        }
        if !req.dry_run {
            staged = stage_if_elevating(req)?;
            if !req.native {
                precheck_wsl(&req.distro)?;
            }
        }
        if req.elevate.is_child {
            enter_elevated_child(&layout, &req.elevate)?;
        }
    }
    for component in &req.components {
        match component {
            Component::Client => install_client(r, &layout, req.dry_run)?,
            Component::Host if req.dry_run => {
                dry_run_host(r, &layout, req)?;
            }
            Component::Host => {
                let exe = match &staged {
                    Some(copy) => copy.path.clone(),
                    None => current_exe()?,
                };
                let mut args = install_child_args(profile, req);
                if let Some(copy) = &staged {
                    args.extend(["--exe-sha256".to_owned(), copy.sha256.clone()]);
                }
                if let Some(code) = relaunch_host_elevated(
                    r,
                    "the host install",
                    &layout,
                    &exe,
                    args,
                    &req.elevate,
                )? {
                    finish_elevated(code)?;
                } else {
                    install_host(r, &layout, req)?;
                }
                if !req.elevate.is_child {
                    if req.native {
                        show_native_block(r, &layout);
                    } else {
                        after_host_install(r, &layout, req);
                    }
                }
            }
        }
    }
    Ok(())
}

/// Stage a locked private copy of this exe when the install is going to ask UAC to relaunch it
/// (see [`stage`]); `None` when it already runs elevated or cannot elevate.
fn stage_if_elevating(req: &InstallRequest) -> Result<Option<stage::StagedExe>, SetupError> {
    if req.elevate.is_child
        || !req.elevate.allowed
        || elevate::is_elevated()
        || !elevate::can_prompt()
    {
        return Ok(None);
    }
    stage::stage(&current_exe()?).map(Some)
}

/// Stop before anything changes (and before any administrator prompt) when WSL or the distro is
/// missing, printing the exact steps to set it up.
fn precheck_wsl(distro: &str) -> Result<(), SetupError> {
    let probe = probe_wsl(&ProcessRunner, wsl_exe_present());
    let check = check_wsl(&probe, distro);
    match wsl_steps(&check, distro) {
        Some(steps) => {
            tracing::warn!(?check, "install stopped before changing anything: no WSL");
            Err(SetupError::WslMissing(steps))
        }
        None => Ok(()),
    }
}

/// What follows a finished host install in the user's own (non-elevated) process: the WSL
/// restart question, then the block that tells the user what to run on the main laptop. Failures
/// here never fail the install, which is already done.
fn after_host_install(r: Renderer, layout: &Layout, req: &InstallRequest) {
    let (need, network) = match app::load_journal(&layout.host_view()) {
        Ok(Some(journal)) => (
            host::restart_need(&journal),
            host::network_in_journal(&journal),
        ),
        Ok(None) => (host::RestartNeed::default(), NetworkMode::Mirrored),
        Err(e) => {
            tracing::error!(error = %e, "could not read the host journal for the restart check");
            (host::RestartNeed::default(), NetworkMode::Mirrored)
        }
    };
    let console = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let action = host::restart_action(need, req.yes, req.activate, console);
    tracing::info!(?need, ?action, "WSL restart decision");
    if let Some(command) = need.command(&req.distro) {
        match action {
            host::RestartAction::Nothing => {}
            host::RestartAction::PrintCommand => r.notice(&format!(
                "WSL must restart for a change to apply. {} When you are ready, run: {command}",
                need.consequence()
            )),
            host::RestartAction::Ask => {
                if ask_yes_no(
                    r,
                    &format!(
                        "WSL must restart for a change to apply. {} Restart WSL now? [y/N] ",
                        need.consequence()
                    ),
                ) {
                    restart_wsl(r, need, &req.distro, &command);
                } else {
                    r.notice(&format!(
                        "not restarting; when you are ready, run: {command}"
                    ));
                }
            }
        }
    }
    show_helper_block(r, &req.distro, req.port, network);
}

/// Ask a yes/no question on the console; anything but y or yes is no.
fn ask_yes_no(r: Renderer, prompt: &str) -> bool {
    r.prompt(prompt);
    let mut answer = String::new();
    match std::io::stdin().read_line(&mut answer) {
        Ok(_) => host::is_yes(&answer),
        Err(e) => {
            tracing::warn!(error = %e, "could not read the answer; taking it as no");
            false
        }
    }
}

/// Run the WSL restart `command` stands for (`wsl --shutdown` or `wsl --terminate <distro>`).
fn restart_wsl(r: Renderer, need: host::RestartNeed, distro: &str, command: &str) {
    let args = if need.shutdown {
        vec!["--shutdown".to_owned()]
    } else {
        vec!["--terminate".to_owned(), distro.to_owned()]
    };
    let inv = Invocation {
        program: tool_path(Tool::Wsl),
        args,
        stdin: None,
    };
    tracing::warn!(command, "restarting WSL at the user's request");
    match ProcessRunner.run(&inv) {
        Ok(out) if out.success() => r.notice("WSL was restarted"),
        Ok(out) => r.notice(&format!(
            "WSL did not restart ({}); run it yourself: {command}",
            out.error_text()
        )),
        Err(e) => r.notice(&format!(
            "could not run wsl.exe ({e}); run it yourself: {command}"
        )),
    }
}

/// Read the helper's name, host key fingerprint and Linux user from this laptop and print the
/// block with the one command to run on the main laptop.
fn show_helper_block(r: Renderer, distro: &str, port: u16, network: NetworkMode) {
    let sys = HostSystem::new(distro);
    let user = match sys.default_user() {
        Ok(u) => u,
        Err(e) => {
            tracing::error!(error = %e, "could not read the distro's default user");
            r.notice("could not read the Linux user from WSL; run `goway-setup.exe status --host` again in a minute");
            return;
        }
    };
    let fingerprint = sys.host_key_fingerprint().unwrap_or_else(|e| {
        tracing::error!(error = %e, "could not read the host key fingerprint");
        None
    });
    let info = HelperInfo {
        device_name: std::env::var("COMPUTERNAME").unwrap_or_default(),
        fingerprint,
        user,
        port,
        network,
    };
    r.block(&next_steps(&info));
}

/// The arguments of the elevated re-run of `install`: the host component only, with every
/// setting spelled out (nothing is copied from the parent's raw command line).
fn install_child_args(profile: &str, req: &InstallRequest) -> Vec<String> {
    let keepalive = clap::ValueEnum::to_possible_value(&req.keepalive)
        .map_or_else(|| "logon".to_owned(), |v| v.get_name().to_owned());
    let network = clap::ValueEnum::to_possible_value(&req.network)
        .map_or_else(|| "auto".to_owned(), |v| v.get_name().to_owned());
    let mut args: Vec<String> = [
        "install",
        "--host",
        "--profile",
        profile,
        "--port",
        &req.port.to_string(),
        "--distro",
        &req.distro,
        "--keepalive",
        &keepalive,
        "--network",
        &network,
    ]
    .map(str::to_owned)
    .to_vec();
    if req.native {
        args.push("--native".to_owned());
        if let Some(key) = &req.authorized_key {
            args.extend(["--authorized-key".to_owned(), key.clone()]);
        }
    }
    if req.allow_elevated_wsl {
        args.push("--allow-elevated-wsl".to_owned());
    }
    if !req.harden {
        args.push("--no-harden".to_owned());
    }
    for cidr in &req.allow_from {
        args.extend(["--allow-from".to_owned(), cidr.clone()]);
    }
    if req.allow_wide {
        args.push("--allow-wide".to_owned());
    }
    if !req.activate {
        args.push("--no-activate".to_owned());
    }
    args
}

fn install_client(r: Renderer, layout: &Layout, dry_run: bool) -> Result<(), SetupError> {
    let exe = current_exe()?;
    let setup_bytes = std::fs::read(&exe).map_err(|e| SetupError::io(&exe, e))?;
    let staging = staging_dir();
    let sources = Sources {
        goway_source: staging.join("goway.exe"),
        goway_digest: sha256_hex(crate::PAYLOAD),
        setup_source: exe,
        setup_digest: sha256_hex(&setup_bytes),
    };
    let plan = build(
        layout,
        &[Component::Client],
        &sources,
        env!("CARGO_PKG_VERSION"),
    );
    if dry_run {
        r.plan(layout, &plan);
        return Ok(());
    }
    if crate::PAYLOAD.is_empty() {
        return Err(SetupError::NoPayload);
    }
    std::fs::create_dir_all(&staging).map_err(|e| SetupError::io(&staging, e))?;
    std::fs::write(&sources.goway_source, crate::PAYLOAD)
        .map_err(|e| SetupError::io(&sources.goway_source, e))?;
    let result = app::install(&mut LocalSystem, layout, &plan);
    if let Err(e) = std::fs::remove_dir_all(&staging) {
        tracing::warn!(path = %staging.display(), error = %e, "could not remove staging directory");
    }
    let journal = result?;
    broadcast_environment_change();
    r.installed(layout, journal.entries.len());
    Ok(())
}

fn host_params(req: &InstallRequest, network: NetworkMode) -> Result<HostParams, SetupError> {
    let home = dirs::home_dir().ok_or(SetupError::NoLocalAppData)?;
    Ok(HostParams {
        network,
        port: req.port,
        distro: req.distro.clone(),
        keepalive: req.keepalive,
        harden: req.harden,
        allow_from: req.allow_from.clone(),
        home,
    })
}

/// Print the host plan; on Windows with a reachable distro, probe the machine (read-only) so the
/// plan reflects the real facts and marks what is already in place.
fn dry_run_host(r: Renderer, layout: &Layout, req: &InstallRequest) -> Result<(), SetupError> {
    if req.native {
        return dry_run_native(r, layout, req);
    }
    let label = format!("host component of profile {}", layout.profile);
    let sys = HostSystem::new(&req.distro);
    let home = dirs::home_dir().ok_or(SetupError::NoLocalAppData)?;
    let reachable = cfg!(windows)
        && match sys.distro_reachable() {
            Ok(up) => up,
            // The elevated guard refuses to start WSL: a dry run changes nothing, so it still
            // prints the plan (from assumed facts) and says what a real install needs.
            Err(SystemError::InvalidState(why)) => {
                tracing::info!(%why, "dry run: distro not running, planning from assumed facts");
                r.notice(&format!(
                    "WSL distro {} is not running; a real install will ask you to start it from a normal (non-administrator) terminal. Planning from assumed facts",
                    req.distro
                ));
                false
            }
            Err(e) => return Err(e.into()),
        };
    if reachable {
        let facts = sys.probe(&home)?;
        let network = host::resolve_network(req.network, &facts)?;
        let params = host_params(req, network)?;
        host::check_boot_keepalive(req.keepalive, &facts, req.allow_elevated_wsl)?;
        r.notice(&format!("network mode: {}", network.as_str()));
        host::check_relay_port(network, params.port, &facts.portproxy)?;
        let plan = host_plan(layout, &params, &facts);
        let holds = plan
            .iter()
            .map(|c| still_applied(c, &sys))
            .collect::<Result<Vec<_>, _>>()?;
        r.plan_component(&label, &plan, Some(&holds));
        warn_public_networks(r, &sys);
    } else {
        let facts = HostFacts::assumed();
        let network = host::resolve_network(req.network, &facts)?;
        let params = host_params(req, network)?;
        r.notice(&format!("network mode: {}", network.as_str()));
        let plan = host_plan(layout, &params, &facts);
        r.plan_component(&label, &plan, None);
    }
    Ok(())
}

fn install_host(r: Renderer, layout: &Layout, req: &InstallRequest) -> Result<(), SetupError> {
    let view = layout.host_view();
    if req.dry_run {
        return dry_run_host(r, layout, req);
    }
    if req.native {
        return install_native(r, layout, req);
    }
    app::ensure_not_installed(&view)?;
    let mut sys = HostSystem::new(&req.distro);
    if !sys.distro_reachable()? {
        return Err(SetupError::DistroUnreachable(req.distro.clone()));
    }
    if !sys.systemd_running()? {
        return Err(SetupError::SystemdOff {
            distro: req.distro.clone(),
        });
    }
    let home = dirs::home_dir().ok_or(SetupError::NoLocalAppData)?;
    let facts = sys.probe(&home)?;
    let network = host::resolve_network(req.network, &facts)?;
    host::check_relay_port(network, req.port, &facts.portproxy)?;
    host::check_boot_keepalive(req.keepalive, &facts, req.allow_elevated_wsl)?;
    let params = host_params(req, network)?;
    let plan = host_plan(layout, &params, &facts);
    app::save_settings(
        layout,
        &HostSettings {
            distro: params.distro.clone(),
            port: params.port,
            allow_from: params.allow_from.clone(),
            network,
            native: None,
        },
    )?;
    let journal = match app::install(&mut sys, &view, &plan) {
        Ok(j) => j,
        Err(e) => {
            if matches!(e, SetupError::InstallFailed { .. }) {
                app::remove_settings(layout);
            }
            return Err(e);
        }
    };
    match current_exe().and_then(|exe| admin::install_protected_exe(&layout.admin_dir, &exe)) {
        Ok(_) => {}
        Err(e) => {
            tracing::error!(error = %e, "could not keep a protected copy of the setup exe");
            r.notice("could not keep an administrator-only copy of goway-setup; a later uninstall must be started from an elevated terminal");
        }
    }
    r.host_installed(layout, &params.distro, params.port, journal.entries.len());
    if network == NetworkMode::Nat {
        r.notice(&host::nat_notice(params.port));
    }
    if params.keepalive == Keepalive::Boot && !facts.interop_disabled {
        r.notice(host::BOOT_KEEPALIVE_NOTICE);
    }
    exposure_warnings(r, layout, &params, &facts, &sys);
    if req.activate {
        if host::sshd_changed(&journal) {
            let how = sys.activate_sshd(params.port)?;
            tracing::info!(?how, "sshd activated");
        }
        for task in host::created_tasks(&journal) {
            sys.start_task(task)?;
        }
    } else {
        r.notice("sshd and the keepalive task were not activated (--no-activate)");
    }
    Ok(())
}

/// The native plan's parameters for this request and probed account.
fn native_params(req: &InstallRequest, account: native::KeyAccount) -> native::NativeParams {
    native::NativeParams {
        allow_from: req.allow_from.clone(),
        authorized_key: req.authorized_key.clone(),
        account,
        shell: tool_path(Tool::PowerShell),
    }
}

/// Print the native plan for a dry run. It changes nothing and probes nothing (the capability
/// query needs administrator rights), so the plan assumes a machine without OpenSSH Server.
fn dry_run_native(r: Renderer, layout: &Layout, req: &InstallRequest) -> Result<(), SetupError> {
    let home = dirs::home_dir().ok_or(SetupError::NoLocalAppData)?;
    let account = native::KeyAccount {
        name: std::env::var("USERNAME").unwrap_or_else(|_| "USER".to_owned()),
        sid: "S-1-5-21-0-0-0-1000".to_owned(),
        admin: true,
        profile_dir: home,
    };
    let plan = native::native_plan(
        layout,
        &native_params(req, account),
        &native::NativeFacts::assumed(),
    );
    r.notice("dry run assumes a machine without OpenSSH Server and an administrator account; an install probes the real state first");
    r.plan_component(
        &format!("native host component of profile {}", layout.profile),
        &plan,
        None,
    );
    Ok(())
}

/// Install the native host component: OpenSSH Server, firewall, shell, key and service.
fn install_native(r: Renderer, layout: &Layout, req: &InstallRequest) -> Result<(), SetupError> {
    let view = layout.host_view();
    app::ensure_not_installed(&view)?;
    let mut sys = HostSystem::new(DEFAULT_DISTRO);
    let (facts, account) = sys.probe_native()?;
    let params = native_params(req, account.clone());
    let plan = native::native_plan(layout, &params, &facts);
    app::save_settings(
        layout,
        &HostSettings {
            distro: DEFAULT_DISTRO.to_owned(),
            port: NATIVE_PORT,
            allow_from: req.allow_from.clone(),
            network: NetworkMode::Mirrored,
            native: Some(native::NativeSettings {
                authorized_key: req.authorized_key.clone(),
                account,
            }),
        },
    )?;
    let journal = match app::install(&mut sys, &view, &plan) {
        Ok(j) => j,
        Err(e) => {
            if matches!(e, SetupError::InstallFailed { .. }) {
                app::remove_settings(layout);
            }
            return Err(e);
        }
    };
    match current_exe().and_then(|exe| admin::install_protected_exe(&layout.admin_dir, &exe)) {
        Ok(_) => {}
        Err(e) => {
            tracing::error!(error = %e, "could not keep a protected copy of the setup exe");
            r.notice("could not keep an administrator-only copy of goway-setup; a later uninstall must be started from an elevated terminal");
        }
    }
    r.native_installed(layout, journal.entries.len());
    if let Some(note) = native::shell_replaced_notice(&facts, &params.shell) {
        r.notice(&note);
    }
    if let Some(warning) = native::open_rule_warning(&facts) {
        r.warning(&warning);
    }
    warn_public_networks(r, &sys);
    Ok(())
}

/// Print the block with the one command to run on the main laptop, for a native helper.
fn show_native_block(r: Renderer, layout: &Layout) {
    let settings = match app::load_settings(layout) {
        Ok(Some(s)) => s,
        Ok(None) => return,
        Err(e) => {
            tracing::error!(error = %e, "could not read the host settings");
            return;
        }
    };
    let Some(native_settings) = settings.native else {
        return;
    };
    let sys = HostSystem::new(DEFAULT_DISTRO);
    let fingerprint = sys
        .host_public_key(&native::host_key_path(layout))
        .unwrap_or_else(|e| {
            tracing::error!(error = %e, "could not read the host public key");
            None
        })
        .and_then(|k| native::fingerprint(&k));
    let key_file = native_settings.authorized_key.as_ref().map(|_| {
        native_settings
            .account
            .keys_file(&native::program_data(layout))
            .display()
            .to_string()
    });
    r.block(&crate::helper::native_next_steps(
        &crate::helper::NativeInfo {
            device_name: std::env::var("COMPUTERNAME").unwrap_or_default(),
            fingerprint,
            account: native_settings.account.name.clone(),
            key_authorized: native_settings.authorized_key.is_some(),
            key_file,
        },
    ));
}

/// Loud follow-ups about who can reach sshd: password login left on, and Public networks where
/// the (Private and Domain only) firewall rules do not apply.
fn exposure_warnings(
    r: Renderer,
    layout: &Layout,
    params: &HostParams,
    facts: &HostFacts,
    sys: &HostSystem,
) {
    if !params.harden {
        r.notice("sshd password login was left on because of --no-harden");
    } else if !facts.authorized_keys {
        r.warning(&format!(
            "sshd password login is still ON: the distro's default user has no authorized key yet, so it was not disabled. \
             Next step: from your main laptop run `goway ssh setup <this host>`, then run `goway doctor <this host> --fix --rsudo` \
             (or uninstall and rerun `goway-setup install --host --profile {}`) to turn password login off.",
            layout.profile
        ));
    }
    warn_public_networks(r, sys);
}

/// Warn about every connected network Windows classifies as Public.
fn warn_public_networks(r: Renderer, sys: &HostSystem) {
    match sys.public_networks() {
        Ok(names) => {
            for name in names {
                r.warning(&public_network_warning(&name));
            }
        }
        Err(e) => tracing::warn!(error = %e, "could not query the network profiles"),
    }
}

fn uninstall(
    r: Renderer,
    profile: &str,
    components: &[Component],
    activate: bool,
    elevate: &Elevate,
    relaunched: Option<&Path>,
) -> Result<(), SetupError> {
    let layout = Layout::from_environment(profile)?;
    if elevate.is_child {
        if components.contains(&Component::Client) {
            return Err(SetupError::ClientNeverElevated);
        }
        enter_elevated_child(&layout, elevate)?;
    }
    notice_legacy_host_state(r, &layout);
    let mut order = components.to_vec();
    order.sort_by_key(|c| std::cmp::Reverse(*c));
    let mut found = false;
    for component in order {
        found |= match component {
            Component::Host => {
                let tuned = !elevate.is_child && uninstall_tune(r, &layout)?;
                uninstall_host_entry(r, &layout, activate, elevate)? || tuned
            }
            Component::Client => uninstall_client(r, &layout, relaunched)?,
        };
    }
    if !found {
        r.nothing_installed(&layout);
    }
    Ok(())
}

/// Uninstall the host component, elevating only this part when needed.
///
/// The host journal lives in the administrator-only directory, so removing it always needs
/// administrator rights. When not elevated, the elevated re-run starts the protected copy of
/// the setup exe from that directory, never the user-writable installed one.
fn uninstall_host_entry(
    r: Renderer,
    layout: &Layout,
    activate: bool,
    elevate: &Elevate,
) -> Result<bool, SetupError> {
    if app::load_journal(&layout.host_view())?.is_none() {
        return Ok(false);
    }
    if !cfg!(windows) {
        return Err(SetupError::HostNeedsWindows);
    }
    if !elevate::is_elevated() {
        let protected = admin::protected_exe(&layout.admin_dir);
        if !protected.is_file() {
            return Err(SetupError::NeedsAdmin(format!(
                "the host uninstall: no administrator-only copy of goway-setup at {}; start goway-setup from a terminal opened with Run as administrator",
                protected.display()
            )));
        }
        let mut args: Vec<String> = ["uninstall", "--host", "--profile", &layout.profile]
            .map(str::to_owned)
            .to_vec();
        if !activate {
            args.push("--no-activate".to_owned());
        }
        if let Some(code) =
            relaunch_host_elevated(r, "the host uninstall", layout, &protected, args, elevate)?
        {
            finish_elevated(code)?;
            return Ok(true);
        }
    }
    uninstall_host(r, layout, activate, elevate.log.as_deref())
}

/// Uninstall the host component; `Ok(false)` when it has no journal.
///
/// Runs elevated. The journal and settings come from the verified administrator-only
/// directory, and the journal is replayed only if every entry is one the host plan for those
/// settings could have produced.
fn uninstall_host(
    r: Renderer,
    layout: &Layout,
    activate: bool,
    keep_log: Option<&str>,
) -> Result<bool, SetupError> {
    let view = layout.host_view();
    if app::load_journal(&view)?.is_none() {
        return Ok(false);
    }
    if !cfg!(windows) {
        return Err(SetupError::HostNeedsWindows);
    }
    let settings = app::load_settings(layout)?.unwrap_or_default();
    let home = dirs::home_dir().ok_or(SetupError::NoLocalAppData)?;
    let mut sys = HostSystem::new(&settings.distro);
    let report = app::uninstall_checked(&mut sys, &view, Retry::ONCE, |journal| {
        host::validate_journal(journal, layout, &settings, &home)
    })?;
    let Some(report) = report else {
        return Ok(false);
    };
    if activate && host::dropin_in_journal(&report.journal, &layout.profile) {
        sys.deactivate_sshd(settings.port)?;
    }
    app::remove_settings(layout);
    purge_admin_state(layout, keep_log);
    r.uninstalled_component(layout, "host component of profile", &report);
    Ok(true)
}

/// Remove the administrator-only state; what cannot go while this process runs (its own exe
/// or log) is removed by a helper shortly after it exits.
fn purge_admin_state(layout: &Layout, keep_log: Option<&str>) {
    let exe = current_exe().unwrap_or_default();
    if !admin::purge(&layout.admin_dir, &layout.admin_root, &exe, keep_log) {
        schedule_dir_removal(&layout.admin_dir, &layout.admin_root);
    }
}

/// The elevated entry point of a re-run (`--elevated-child`).
///
/// Everything after this acts with an administrator token, so it must only ever consume data
/// from a location a non-administrator cannot write: the administrator-only state directory
/// (verified, created here with a protected ACL), the arguments given on the UAC command
/// line, and the system's own tools by absolute path. It also refuses to run for a different
/// account than the one that asked, because profile and WSL distros are per user.
// frob:invariant INV-ELEV-001
fn enter_elevated_child(layout: &Layout, elevate: &Elevate) -> Result<(), SetupError> {
    if !elevate::is_elevated() {
        return Err(SetupError::NeedsAdmin(
            "the elevated re-run is still not elevated".to_owned(),
        ));
    }
    let wrong_user = |why: &str| {
        SetupError::NeedsAdmin(format!(
            "the elevated re-run {why}; run goway-setup from an elevated terminal of your own account instead"
        ))
    };
    let Some(expected) = &elevate.invoker_sid else {
        return Err(wrong_user("was not told who started it"));
    };
    let me =
        crate::sysapi::current_user_sid().map_err(|e| SetupError::io(Path::new("<token>"), e))?;
    if &me != expected {
        tracing::error!(%me, %expected, "elevated as a different account");
        return Err(wrong_user(
            "runs as a different account than the one that started it",
        ));
    }
    app::prepare_admin_dir(layout)?;
    if let Some(name) = &elevate.log {
        if !admin::valid_log_name(name) {
            return Err(SetupError::UntrustedState {
                path: name.clone(),
                reason: "not a valid elevated log name".to_owned(),
            });
        }
        admin::remove_old_logs(&layout.admin_dir, Some(name));
        let path = layout.admin_dir.join(name);
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| SetupError::io(&path, e))?;
        elevate::redirect_output(file).map_err(|e| SetupError::io(&path, e))?;
    }
    tracing::info!("running elevated for the host component");
    Ok(())
}

/// Re-run the host component elevated through UAC when this process is not elevated.
///
/// `Ok(None)` means this process is already elevated and the caller proceeds in-process;
/// `Ok(Some(code))` means the elevated copy of `exe` did the work (its output printed) and
/// finished with `code`. Only `args` (the host component) are passed; the client component
/// never runs elevated.
fn relaunch_host_elevated(
    r: Renderer,
    what: &str,
    layout: &Layout,
    exe: &Path,
    mut args: Vec<String>,
    elevate: &Elevate,
) -> Result<Option<u32>, SetupError> {
    if elevate::is_elevated() {
        tracing::info!("running with administrator rights");
        return Ok(None);
    }
    if !elevate.allowed || elevate.is_child || !elevate::can_prompt() {
        let why = if elevate.is_child {
            "the elevated re-run is still not elevated"
        } else if !elevate.allowed {
            "--no-elevate was given; start goway-setup from a terminal opened with Run as administrator"
        } else {
            "this process is not in the active desktop session (it runs as a service, a boot task or in another account's session), so Windows cannot ask for permission; start goway-setup from an elevated terminal (an administrator account over SSH already is)"
        };
        return Err(SetupError::NeedsAdmin(format!("{what}: {why}")));
    }
    let sid =
        crate::sysapi::current_user_sid().map_err(|e| SetupError::io(Path::new("<token>"), e))?;
    let log = admin::new_log_name();
    args.extend([
        "--elevated-child".to_owned(),
        "--invoker-sid".to_owned(),
        sid,
        "--elevated-log".to_owned(),
        log.clone(),
    ]);
    r.elevating(what);
    let code = elevate::run_elevated(exe, &elevate::command_line(&args)).map_err(|e| {
        SetupError::NeedsAdmin(format!("{what}: Windows did not grant elevation ({e})"))
    })?;
    // The log sits in the administrator-only directory; read it only if that checks out.
    match app::verify_admin_dir(layout) {
        Ok(true) => match std::fs::read_to_string(layout.admin_dir.join(&log)) {
            Ok(text) => r.passthrough(&text),
            Err(e) => tracing::warn!(error = %e, "no output from the elevated run"),
        },
        Ok(false) => tracing::warn!("the elevated run left no state directory"),
        Err(e) => tracing::error!(error = %e, "not reading the elevated log"),
    }
    Ok(Some(code))
}

/// Turn the exit code of an elevated re-run into this run's result.
fn finish_elevated(code: u32) -> Result<(), SetupError> {
    if code == 0 {
        Ok(())
    } else {
        Err(SetupError::ElevatedRunFailed(code))
    }
}

/// Uninstall the client component; `Ok(false)` when it has no journal.
fn uninstall_client(
    r: Renderer,
    layout: &Layout,
    relaunched: Option<&Path>,
) -> Result<bool, SetupError> {
    let exe = current_exe()?;
    if relaunched.is_none()
        && let Some(journal) = app::load_journal(layout)?
        && app::runs_from_installed_file(&journal, &exe)
    {
        relaunch(r, layout, &exe)?;
        return Ok(true);
    }
    let outcome = app::uninstall(&mut LocalSystem, layout, RETRY);
    if let Some(dir) = relaunched {
        schedule_self_delete(&exe, dir);
    }
    match outcome? {
        None => Ok(false),
        Some(report) => {
            broadcast_environment_change();
            r.uninstalled(layout, &report);
            Ok(true)
        }
    }
}

/// Copy `from` to `to`, which must not exist yet.
fn copy_new(from: &Path, to: &Path) -> Result<(), SetupError> {
    let mut src = std::fs::File::open(from).map_err(|e| SetupError::io(from, e))?;
    let mut dst = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to)
        .map_err(|e| SetupError::io(to, e))?;
    std::io::copy(&mut src, &mut dst).map_err(|e| SetupError::io(to, e))?;
    Ok(())
}

/// Copy this exe to the temp directory and continue there: a running exe cannot delete itself.
///
/// The copy runs detached (it must outlive this process, which exits at once so the installed
/// exe is unlocked) with its output in `uninstall.log` beside it; it deletes itself when done.
fn relaunch(r: Renderer, layout: &Layout, exe: &Path) -> Result<(), SetupError> {
    let dir = app::relaunch_dir(&std::env::temp_dir(), std::process::id());
    // `create_dir` (not `_all`) and exclusive file creation: a directory or link planted at the
    // predictable temp path makes this fail instead of being written through.
    std::fs::create_dir(&dir).map_err(|e| SetupError::io(&dir, e))?;
    let copy = dir.join("goway-setup.exe");
    copy_new(exe, &copy)?;
    let log_path = dir.join("uninstall.log");
    let log = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&log_path)
        .map_err(|e| SetupError::io(&log_path, e))?;
    let log_err = log.try_clone().map_err(|e| SetupError::io(&log_path, e))?;
    r.relaunching(&copy, &log_path);
    tracing::info!(copy = %copy.display(), "relaunching uninstaller from temp");
    let mut command = std::process::Command::new(&copy);
    command
        .args([
            "uninstall",
            "--client",
            "--profile",
            &layout.profile,
            "--relaunched",
        ])
        .arg(&dir)
        .stdin(std::process::Stdio::null())
        .stdout(log)
        .stderr(log_err);
    spawn_detached(&mut command).map_err(|e| SetupError::io(&copy, e))?;
    Ok(())
}

fn status(r: Renderer, profile: &str) -> Result<(), SetupError> {
    let layout = Layout::from_environment(profile)?;
    notice_legacy_host_state(r, &layout);
    let mut any = false;
    if let Some(journal) = app::load_journal(&layout)? {
        any = true;
        r.status(&layout, &app::status(&LocalSystem, &journal)?);
    }
    let view = layout.host_view();
    if let Some(journal) = app::load_journal(&view)? {
        any = true;
        let settings = app::load_settings(&layout)?.unwrap_or_default();
        let sys = HostSystem::new(&settings.distro);
        r.status(&view, &app::status(&sys, &journal)?);
        if settings.native.is_some() {
            show_native_block(r, &layout);
        } else {
            show_helper_block(r, &settings.distro, settings.port, settings.network);
        }
    }
    if !any {
        r.not_installed(&layout);
    }
    Ok(())
}

/// Tell the user about a host journal an older goway-setup left in their (user-writable)
/// profile: it is never read or replayed, because an elevated process must not trust it.
fn notice_legacy_host_state(r: Renderer, layout: &Layout) {
    let legacy = layout.state_dir.join("host-journal.json");
    if legacy.exists() {
        tracing::warn!(path = %legacy.display(), "ignoring a host journal in the user profile");
        r.notice(&format!(
            "ignoring {}: host state now lives in {} where only administrators can write; remove the old file after checking the host by hand",
            legacy.display(),
            layout.admin_dir.display()
        ));
    }
}
