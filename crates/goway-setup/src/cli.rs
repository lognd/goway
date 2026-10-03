//! Command line and command dispatch.

use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand};
use goway_journal::{LocalSystem, sha256_hex, still_applied};

use crate::app::{self, Retry};
use crate::elevate;
use crate::error::SetupError;
use crate::host::{
    self, DEFAULT_DISTRO, DEFAULT_PORT, HostFacts, HostParams, HostSettings, Keepalive, host_plan,
};
use crate::hostsys::HostSystem;
use crate::layout::{DEFAULT_PROFILE, Layout};
use crate::plan::{Component, Sources, build};
use crate::render::{ColorWhen, Renderer};
use crate::windows::{broadcast_environment_change, schedule_self_delete, spawn_detached};

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
        /// Host: TCP port the WSL sshd listens on.
        #[arg(long, default_value_t = DEFAULT_PORT)]
        port: u16,
        /// Host: WSL distro to set up.
        #[arg(long, default_value = DEFAULT_DISTRO)]
        distro: String,
        /// Host: when the keepalive task starts the distro.
        #[arg(long, value_enum, default_value_t)]
        keepalive: Keepalive,
        /// Host: disable password logins for sshd (only when the default user has an authorized key).
        #[arg(long)]
        harden: bool,
        /// Host: do not reload or restart sshd or start the keepalive task afterwards.
        #[arg(long)]
        no_activate: bool,
        /// Host: fail instead of asking Windows (UAC) for administrator rights.
        #[arg(long)]
        no_elevate: bool,
        /// Internal: this is the elevated re-run; never elevate again.
        #[arg(long, hide = true)]
        elevated_child: bool,
        /// Profile selection.
        #[command(flatten)]
        profile: ProfileArg,
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
        /// Internal: this is the elevated re-run; never elevate again.
        #[arg(long, hide = true)]
        elevated_child: bool,
        /// Profile selection.
        #[command(flatten)]
        profile: ProfileArg,
        /// Internal: this is the relaunched temporary copy; delete its directory when done.
        #[arg(long, hide = true, value_name = "DIR")]
        relaunched: Option<PathBuf>,
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

/// Everything `install` was asked for, gathered from the flags.
#[derive(Debug, Clone)]
struct InstallRequest {
    components: Vec<Component>,
    dry_run: bool,
    port: u16,
    distro: String,
    keepalive: Keepalive,
    harden: bool,
    activate: bool,
    elevate: Elevate,
}

/// Whether and how the process may obtain administrator rights.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Elevate {
    allowed: bool,
    is_child: bool,
}

/// Run the parsed command.
pub fn run(cli: &Cli, r: Renderer) -> Result<(), SetupError> {
    match &cli.command {
        Command::Install {
            client,
            host,
            dry_run,
            port,
            distro,
            keepalive,
            harden,
            no_activate,
            no_elevate,
            elevated_child,
            profile,
        } => install(
            r,
            &profile.profile,
            &InstallRequest {
                components: selected_components(*client, *host),
                dry_run: *dry_run,
                port: *port,
                distro: distro.clone(),
                keepalive: *keepalive,
                harden: *harden,
                activate: !*no_activate,
                elevate: Elevate {
                    allowed: !*no_elevate,
                    is_child: *elevated_child,
                },
            },
        ),
        Command::Uninstall {
            client,
            host,
            no_activate,
            no_elevate,
            elevated_child,
            profile,
            relaunched,
        } => {
            let components = if *client || *host {
                selected_components(*client, *host)
            } else {
                vec![Component::Host, Component::Client]
            };
            let elevate = Elevate {
                allowed: !*no_elevate,
                is_child: *elevated_child,
            };
            uninstall(
                r,
                &profile.profile,
                &components,
                !*no_activate,
                elevate,
                relaunched.as_deref(),
            )
        }
        Command::Status { profile } => status(r, &profile.profile),
    }
}

fn staging_dir() -> PathBuf {
    std::env::temp_dir().join(format!("goway-setup-{}", std::process::id()))
}

fn current_exe() -> Result<PathBuf, SetupError> {
    std::env::current_exe().map_err(|e| SetupError::io(Path::new("<current exe>"), e))
}

fn install(r: Renderer, profile: &str, req: &InstallRequest) -> Result<(), SetupError> {
    let layout = Layout::from_environment(profile)?;
    let wants_host = req.components.contains(&Component::Host);
    if wants_host {
        host::validate_distro(&req.distro)?;
        if !req.dry_run && !cfg!(windows) {
            return Err(SetupError::HostNeedsWindows);
        }
        if !req.dry_run
            && let Some(code) = ensure_admin(r, "the host install", req.elevate)?
        {
            return finish_elevated(code);
        }
    }
    for component in &req.components {
        match component {
            Component::Client => install_client(r, &layout, req.dry_run)?,
            Component::Host => install_host(r, &layout, req)?,
        }
    }
    Ok(())
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

fn host_params(req: &InstallRequest) -> Result<HostParams, SetupError> {
    let home = dirs::home_dir().ok_or(SetupError::NoLocalAppData)?;
    Ok(HostParams {
        port: req.port,
        distro: req.distro.clone(),
        keepalive: req.keepalive,
        harden: req.harden,
        home,
    })
}

/// Print the host plan; on Windows with a reachable distro, probe the machine (read-only) so the
/// plan reflects the real facts and marks what is already in place.
fn dry_run_host(r: Renderer, layout: &Layout, params: &HostParams) -> Result<(), SetupError> {
    let label = format!("host component of profile {}", layout.profile);
    let sys = HostSystem::new(&params.distro);
    if cfg!(windows) && sys.distro_reachable()? {
        let facts = sys.probe()?;
        let plan = host_plan(layout, params, &facts);
        let holds = plan
            .iter()
            .map(|c| still_applied(c, &sys))
            .collect::<Result<Vec<_>, _>>()?;
        r.plan_component(&label, &plan, Some(&holds));
    } else {
        let plan = host_plan(layout, params, &HostFacts::assumed());
        r.plan_component(&label, &plan, None);
    }
    Ok(())
}

fn install_host(r: Renderer, layout: &Layout, req: &InstallRequest) -> Result<(), SetupError> {
    let params = host_params(req)?;
    let view = layout.host_view();
    if req.dry_run {
        return dry_run_host(r, layout, &params);
    }
    app::ensure_not_installed(&view)?;
    let mut sys = HostSystem::new(&params.distro);
    if !sys.distro_reachable()? {
        return Err(SetupError::DistroUnreachable(params.distro));
    }
    if !sys.systemd_running()? {
        return Err(SetupError::SystemdOff {
            distro: params.distro,
        });
    }
    let facts = sys.probe()?;
    if params.harden && !facts.authorized_keys {
        r.notice("not hardening sshd: the distro's default user has no authorized key yet (run `goway ssh setup` first)");
    }
    let plan = host_plan(layout, &params, &facts);
    app::save_settings(
        layout,
        &HostSettings {
            distro: params.distro.clone(),
            port: params.port,
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
    r.host_installed(layout, &params.distro, params.port, journal.entries.len());
    if req.activate {
        if host::sshd_changed(&journal) {
            let how = sys.activate_sshd(params.port)?;
            tracing::info!(?how, "sshd activated");
        }
        if let Some(task) = host::created_task(&journal) {
            sys.start_task(task)?;
        }
    } else {
        r.notice("sshd and the keepalive task were not activated (--no-activate)");
    }
    for notice in host::restart_notices(&journal) {
        r.notice(&notice);
    }
    Ok(())
}

fn uninstall(
    r: Renderer,
    profile: &str,
    components: &[Component],
    activate: bool,
    elevate: Elevate,
    relaunched: Option<&Path>,
) -> Result<(), SetupError> {
    let layout = Layout::from_environment(profile)?;
    let mut order = components.to_vec();
    order.sort_by_key(|c| std::cmp::Reverse(*c));
    if order.contains(&Component::Host)
        && let Some(journal) = app::load_journal(&layout.host_view())?
        && host::needs_admin(&journal)
    {
        if !cfg!(windows) {
            return Err(SetupError::HostNeedsWindows);
        }
        if let Some(code) = ensure_admin(r, "the host uninstall", elevate)? {
            return finish_elevated(code);
        }
    }
    let mut found = false;
    for component in order {
        found |= match component {
            Component::Host => uninstall_host(r, &layout, activate)?,
            Component::Client => uninstall_client(r, &layout, relaunched)?,
        };
    }
    if !found {
        r.nothing_installed(&layout);
    }
    Ok(())
}

/// Uninstall the host component; `Ok(false)` when it has no journal.
fn uninstall_host(r: Renderer, layout: &Layout, activate: bool) -> Result<bool, SetupError> {
    let view = layout.host_view();
    if app::load_journal(&view)?.is_none() {
        return Ok(false);
    }
    if !cfg!(windows) {
        return Err(SetupError::HostNeedsWindows);
    }
    let settings = app::load_settings(layout)?.unwrap_or(HostSettings {
        distro: DEFAULT_DISTRO.to_owned(),
        port: DEFAULT_PORT,
    });
    let mut sys = HostSystem::new(&settings.distro);
    let report = app::uninstall(&mut sys, &view, Retry::ONCE)?;
    let Some(report) = report else {
        return Ok(false);
    };
    if activate && host::dropin_in_journal(&report.journal, &layout.profile) {
        sys.deactivate_sshd(settings.port)?;
    }
    app::remove_settings(layout);
    r.uninstalled_component(layout, "host component of profile", &report);
    Ok(true)
}

/// Make sure this process is elevated, relaunching through UAC when allowed.
///
/// `Ok(None)` means elevated and the caller proceeds; `Ok(Some(code))` means the elevated copy
/// already did the work (its output printed) and finished with `code`.
fn ensure_admin(r: Renderer, what: &str, elevate: Elevate) -> Result<Option<u32>, SetupError> {
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
            "this is not an interactive desktop session, so Windows cannot ask for permission; start goway-setup from an elevated terminal (an administrator account over SSH already is)"
        };
        return Err(SetupError::NeedsAdmin(format!("{what}: {why}")));
    }
    r.elevating(what);
    let exe = current_exe()?;
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    args.push("--elevated-child".to_owned());
    let log = std::env::temp_dir().join(format!("goway-setup-elevated-{}.log", std::process::id()));
    let code =
        elevate::run_elevated(&elevate::elevated_parameters(&exe, &args, &log)).map_err(|e| {
            SetupError::NeedsAdmin(format!("{what}: Windows did not grant elevation ({e})"))
        })?;
    match std::fs::read_to_string(&log) {
        Ok(text) => r.passthrough(&text),
        Err(e) => {
            tracing::warn!(path = %log.display(), error = %e, "no output from the elevated run");
        }
    }
    let _ = std::fs::remove_file(&log);
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

/// Copy this exe to the temp directory and continue there: a running exe cannot delete itself.
///
/// The copy runs detached (it must outlive this process, which exits at once so the installed
/// exe is unlocked) with its output in `uninstall.log` beside it; it deletes itself when done.
fn relaunch(r: Renderer, layout: &Layout, exe: &Path) -> Result<(), SetupError> {
    let dir = app::relaunch_dir(&std::env::temp_dir(), std::process::id());
    std::fs::create_dir_all(&dir).map_err(|e| SetupError::io(&dir, e))?;
    let copy = dir.join("goway-setup.exe");
    std::fs::copy(exe, &copy).map_err(|e| SetupError::io(&copy, e))?;
    let log_path = dir.join("uninstall.log");
    let log = std::fs::File::create(&log_path).map_err(|e| SetupError::io(&log_path, e))?;
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
    let mut any = false;
    if let Some(journal) = app::load_journal(&layout)? {
        any = true;
        r.status(&layout, &app::status(&LocalSystem, &journal)?);
    }
    let view = layout.host_view();
    if let Some(journal) = app::load_journal(&view)? {
        any = true;
        let settings = app::load_settings(&layout)?.unwrap_or(HostSettings {
            distro: DEFAULT_DISTRO.to_owned(),
            port: DEFAULT_PORT,
        });
        let sys = HostSystem::new(&settings.distro);
        r.status(&view, &app::status(&sys, &journal)?);
    }
    if !any {
        r.not_installed(&layout);
    }
    Ok(())
}
