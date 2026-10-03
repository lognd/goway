//! Command line and command dispatch.

use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand};
use goway_journal::{LocalSystem, sha256_hex};

use crate::app::{self, Retry};
use crate::error::SetupError;
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
        /// Show the changes without making them.
        #[arg(long)]
        dry_run: bool,
        /// Profile selection.
        #[command(flatten)]
        profile: ProfileArg,
    },
    /// Undo everything the install did by replaying its journal backwards.
    Uninstall {
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
pub fn selected_components(client: bool) -> Vec<Component> {
    let _ = client; // `--client` is the only component today, so naming it or not selects it.
    vec![Component::Client]
}

/// Run the parsed command.
pub fn run(cli: &Cli, r: Renderer) -> Result<(), SetupError> {
    match &cli.command {
        Command::Install {
            client,
            dry_run,
            profile,
        } => install(r, &profile.profile, &selected_components(*client), *dry_run),
        Command::Uninstall {
            profile,
            relaunched,
        } => uninstall(r, &profile.profile, relaunched.as_deref()),
        Command::Status { profile } => status(r, &profile.profile),
    }
}

fn staging_dir() -> PathBuf {
    std::env::temp_dir().join(format!("goway-setup-{}", std::process::id()))
}

fn current_exe() -> Result<PathBuf, SetupError> {
    std::env::current_exe().map_err(|e| SetupError::io(Path::new("<current exe>"), e))
}

fn install(
    r: Renderer,
    profile: &str,
    components: &[Component],
    dry_run: bool,
) -> Result<(), SetupError> {
    let layout = Layout::from_environment(profile)?;
    let exe = current_exe()?;
    let setup_bytes = std::fs::read(&exe).map_err(|e| SetupError::io(&exe, e))?;
    let staging = staging_dir();
    let sources = Sources {
        goway_source: staging.join("goway.exe"),
        goway_digest: sha256_hex(crate::PAYLOAD),
        setup_source: exe,
        setup_digest: sha256_hex(&setup_bytes),
    };
    let plan = build(&layout, components, &sources, env!("CARGO_PKG_VERSION"));
    if dry_run {
        r.plan(&layout, &plan);
        return Ok(());
    }
    if crate::PAYLOAD.is_empty() {
        return Err(SetupError::NoPayload);
    }
    std::fs::create_dir_all(&staging).map_err(|e| SetupError::io(&staging, e))?;
    std::fs::write(&sources.goway_source, crate::PAYLOAD)
        .map_err(|e| SetupError::io(&sources.goway_source, e))?;
    let result = app::install(&mut LocalSystem, &layout, &plan);
    if let Err(e) = std::fs::remove_dir_all(&staging) {
        tracing::warn!(path = %staging.display(), error = %e, "could not remove staging directory");
    }
    let journal = result?;
    broadcast_environment_change();
    r.installed(&layout, journal.entries.len());
    Ok(())
}

fn uninstall(r: Renderer, profile: &str, relaunched: Option<&Path>) -> Result<(), SetupError> {
    let layout = Layout::from_environment(profile)?;
    let exe = current_exe()?;
    if relaunched.is_none()
        && let Some(journal) = app::load_journal(&layout)?
        && app::runs_from_installed_file(&journal, &exe)
    {
        return relaunch(r, &layout, &exe);
    }
    let outcome = app::uninstall(&mut LocalSystem, &layout, RETRY);
    if let Some(dir) = relaunched {
        schedule_self_delete(&exe, dir);
    }
    match outcome? {
        None => r.nothing_installed(&layout),
        Some(report) => {
            broadcast_environment_change();
            r.uninstalled(&layout, &report);
        }
    }
    Ok(())
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
        .args(["uninstall", "--profile", &layout.profile, "--relaunched"])
        .arg(&dir)
        .stdin(std::process::Stdio::null())
        .stdout(log)
        .stderr(log_err);
    spawn_detached(&mut command).map_err(|e| SetupError::io(&copy, e))?;
    Ok(())
}

fn status(r: Renderer, profile: &str) -> Result<(), SetupError> {
    let layout = Layout::from_environment(profile)?;
    match app::load_journal(&layout)? {
        None => r.not_installed(&layout),
        Some(journal) => {
            let rows = app::status(&LocalSystem, &journal)?;
            r.status(&layout, &rows);
        }
    }
    Ok(())
}
