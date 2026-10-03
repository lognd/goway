//! The command line: one place that defines every verb and flag.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::render::ColorWhen;

/// goway ("go away"): run a command on another machine, natively, from the
/// current git work tree.
#[derive(Debug, Parser)]
#[command(name = "goway", version, about, long_about = None)]
pub struct Cli {
    /// More logging on stderr (-v info, -vv debug, -vvv trace).
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    /// Color goway's own output.
    #[arg(long, value_enum, default_value_t, global = true)]
    pub color: ColorWhen,

    /// The verb to run.
    #[command(subcommand)]
    pub command: Command,
}

/// Every goway verb.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Sync the work tree and run a command on a pool host.
    Run(RunArgs),
    /// Show hosts, load, running jobs and disk used by goway.
    Status,
    /// Remove stale remote work directories and caches.
    Gc(GcArgs),
    /// Check ssh, the remote toolchain and disk; print exact fixes.
    Doctor(DoctorArgs),
    /// Manage the host pool.
    #[command(subcommand)]
    Host(HostCommand),
    /// Walk through ssh key setup for a host, reversibly.
    #[command(subcommand)]
    Ssh(SshCommand),
    /// Show where goway keeps its files.
    #[command(subcommand)]
    Config(ConfigCommand),
}

/// Arguments of `goway run`.
#[derive(Debug, Args)]
pub struct RunArgs {
    /// Run on this host instead of the least-loaded one.
    #[arg(long)]
    pub host: Option<String>,
    /// Keep the remote work directory after the run.
    #[arg(long)]
    pub keep: bool,
    /// Write host, arch, address and exit code as JSON to this file.
    #[arg(long, env = "GOWAY_REPORT")]
    pub report: Option<PathBuf>,
    /// Extra environment for the remote command (KEY=VALUE, repeatable).
    #[arg(long = "env", short = 'e', value_name = "KEY=VALUE")]
    pub env: Vec<String>,
    /// The command and its arguments.
    #[arg(last = true, required = true, num_args = 1..)]
    pub command: Vec<String>,
}

/// Arguments of `goway gc`.
#[derive(Debug, Args)]
pub struct GcArgs {
    /// Only this host.
    #[arg(long)]
    pub host: Option<String>,
    /// Only entries of this repository (name or id).
    #[arg(long)]
    pub repo: Option<String>,
    /// Only entries idle for longer than this (such as 12h, 3d).
    #[arg(long, value_name = "DURATION")]
    pub older_than: Option<String>,
    /// Every unlocked entry, regardless of age.
    #[arg(long)]
    pub all: bool,
    /// List what would be removed, remove nothing.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments of `goway doctor`.
#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Only this host (default: every configured host).
    pub host: Option<String>,
    /// Run the fixes that need no root; explain the ones that do.
    #[arg(long)]
    pub fix: bool,
}

/// `goway host` verbs.
#[derive(Debug, Subcommand)]
pub enum HostCommand {
    /// Add a host: detect port and user, pin its ssh host key.
    Add(HostAddArgs),
    /// List configured hosts.
    List,
    /// Remove a host and its pinned key.
    Remove {
        /// The host name.
        name: String,
    },
}

/// Arguments of `goway host add`.
#[derive(Debug, Args)]
pub struct HostAddArgs {
    /// The host's name (its identity; also tried as `NAME.local`).
    pub name: String,
    /// An address to try first (name or IP); goway never depends on it staying valid.
    #[arg(long)]
    pub address: Option<String>,
    /// The ssh port (default: try 2222, then 22).
    #[arg(long)]
    pub port: Option<u16>,
    /// The remote user (default: from ssh config, else the local user).
    #[arg(long)]
    pub user: Option<String>,
    /// Most goway jobs at once on this host.
    #[arg(long)]
    pub max_jobs: Option<u32>,
}

/// `goway ssh` verbs.
#[derive(Debug, Subcommand)]
pub enum SshCommand {
    /// Create or pick a key and authorize it on a host.
    Setup {
        /// The host name.
        host: String,
        /// Undo exactly what a previous setup changed.
        #[arg(long)]
        undo: bool,
    },
}

/// `goway config` verbs.
#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Print the config, state and `known_hosts` paths.
    Path,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_is_well_formed() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    #[test]
    fn run_takes_command_after_double_dash() {
        let cli =
            Cli::try_parse_from(["goway", "run", "--host", "q", "--", "cargo", "-q"]).unwrap();
        let Command::Run(run) = cli.command else {
            panic!("expected run");
        };
        assert_eq!(run.host.as_deref(), Some("q"));
        assert_eq!(run.command, ["cargo", "-q"]);
    }
}
