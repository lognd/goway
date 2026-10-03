//! The command line: one place that defines every verb and flag.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::render::ColorWhen;

/// Host names name files, ssh key aliases and config entries, so they are
/// checked here, before anything happens: 1-63 letters, digits, `-`, `_`.
fn host_name(s: &str) -> Result<String, String> {
    if crate::config::valid_name(s) {
        Ok(s.to_owned())
    } else {
        Err(format!(
            "`{s}` is not a host name: use 1-63 letters, digits, `-` or `_`, not starting with `-` (the Windows device name works, e.g. Helios)"
        ))
    }
}

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
    /// Add a helper laptop: key login, pinned identity, toolchain, all in one.
    Add(AddArgs),
    /// Remove everything goway added, on every helper and on this laptop.
    Uninstall(UninstallArgs),
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

impl Command {
    /// The verb's name, for logs (arguments may hold secrets).
    pub fn verb(&self) -> &'static str {
        match self {
            Self::Add(_) => "add",
            Self::Uninstall(_) => "uninstall",
            Self::Run(_) => "run",
            Self::Status => "status",
            Self::Gc(_) => "gc",
            Self::Doctor(_) => "doctor",
            Self::Host(_) => "host",
            Self::Ssh(_) => "ssh",
            Self::Config(_) => "config",
        }
    }
}

/// Arguments of `goway run`.
#[derive(Debug, Args)]
pub struct RunArgs {
    /// Run on this host instead of the least-loaded one.
    #[arg(long, value_parser = host_name)]
    pub host: Option<String>,
    /// Keep the remote work directory after the run.
    #[arg(long)]
    pub keep: bool,
    /// Write host, arch, address and exit code as JSON to this file.
    #[arg(long, env = "GOWAY_REPORT")]
    pub report: Option<PathBuf>,
    /// Split the run across N hosts: each runs part i of N (nextest gets
    /// `--partition count:i/N`; every command sees `GOWAY_SHARD` and
    /// `GOWAY_SHARD_COUNT`). Output lines are prefixed with the host.
    #[arg(long, value_name = "N", conflicts_with = "host", value_parser = clap::value_parser!(u16).range(1..))]
    pub shard: Option<u16>,
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
    #[arg(long, value_parser = host_name)]
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
    #[arg(value_parser = host_name)]
    pub host: Option<String>,
    /// Run the fixes that need no root; explain the ones that do.
    #[arg(long)]
    pub fix: bool,
    /// With --fix, also run the fixes that need root on the host: listed
    /// with reasons, confirmed once, then run in one interactive sudo
    /// session there (its sudo asks for the password; goway never sees it).
    #[arg(long, alias = "sudo", requires = "fix")]
    pub rsudo: bool,
    /// Do not ask before running root fixes (with --rsudo).
    #[arg(long, short = 'y')]
    pub yes: bool,
}

/// Arguments of `goway uninstall`.
#[derive(Debug, Args)]
#[allow(clippy::struct_excessive_bools)] // independent command-line switches
pub struct UninstallArgs {
    /// Remove it everywhere without asking (otherwise: list, then ask).
    #[arg(long)]
    pub everywhere: bool,
    /// Answer yes to the questions.
    #[arg(long, short = 'y')]
    pub yes: bool,
    /// Only list what would be removed.
    #[arg(long)]
    pub dry_run: bool,
    /// Also undo goway's administrator changes on helpers (one sudo session each).
    #[arg(long)]
    pub rsudo: bool,
}

/// Arguments of `goway add`.
#[derive(Debug, Args)]
#[allow(clippy::struct_excessive_bools)] // independent command-line switches
pub struct AddArgs {
    /// The helper's name (its Windows device name works, e.g. Helios).
    #[arg(value_parser = host_name)]
    pub host: String,
    /// The helper's host key fingerprint, as its installer printed it.
    #[arg(long, value_name = "SHA256:...")]
    pub fingerprint: Option<String>,
    /// The Linux user on the helper (the password you are asked for is this user's).
    #[arg(long)]
    pub user: Option<String>,
    /// An address to try first (name or IP); not needed on most networks.
    #[arg(long)]
    pub address: Option<String>,
    /// The ssh port on the helper (default 2222).
    #[arg(long)]
    pub port: Option<u16>,
    /// The public key to authorize (a `.pub` file).
    #[arg(long, value_name = "FILE.pub")]
    pub key: Option<std::path::PathBuf>,
    /// Also make the changes that need administrator rights on the helper
    /// (listed, confirmed once, one sudo password typed into its sudo).
    #[arg(long)]
    pub rsudo: bool,
    /// Also install what this laptop is missing (ssh client, git) with sudo here.
    #[arg(long)]
    pub lsudo: bool,
    /// Do not ask before the --rsudo / --lsudo changes.
    #[arg(long, short = 'y')]
    pub yes: bool,
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
        #[arg(value_parser = host_name)]
        name: String,
    },
}

/// Arguments of `goway host add`.
#[derive(Debug, Args)]
pub struct HostAddArgs {
    /// The host's name (its identity; also tried as `NAME.local`).
    #[arg(value_parser = host_name)]
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
    /// The host key fingerprint to trust (`SHA256:...`, shown on the host
    /// by `ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub`); without it
    /// goway asks you to confirm the key at the terminal.
    #[arg(long, value_name = "SHA256:...")]
    pub fingerprint: Option<String>,
}

/// `goway ssh` verbs.
#[derive(Debug, Subcommand)]
pub enum SshCommand {
    /// Make key login to a host work: pick or create a key, authorize it
    /// with one password login, fix permissions; reversible with --undo.
    Setup(SshSetupArgs),
}

/// Arguments of `goway ssh setup`.
#[derive(Debug, Args)]
pub struct SshSetupArgs {
    /// The host name (configured or new).
    #[arg(value_parser = host_name)]
    pub host: String,
    /// Undo exactly what a previous setup of this host changed.
    #[arg(long)]
    pub undo: bool,
    /// For a new host: an address to try first (name or IP).
    #[arg(long, conflicts_with = "undo")]
    pub address: Option<String>,
    /// For a new host: the ssh port (default: the configured default, 2222).
    #[arg(long, conflicts_with = "undo")]
    pub port: Option<u16>,
    /// For a new host: the remote user.
    #[arg(long, conflicts_with = "undo")]
    pub user: Option<String>,
    /// The public key to authorize (a `.pub` file); default: the ssh
    /// agent's first key, else your default identity, else a new goway key.
    #[arg(long, value_name = "FILE.pub", conflicts_with = "undo")]
    pub key: Option<std::path::PathBuf>,
    /// For a new host: the host key fingerprint to trust (`SHA256:...`);
    /// without it goway asks you to confirm the key before any password.
    #[arg(long, value_name = "SHA256:...", conflicts_with = "undo")]
    pub fingerprint: Option<String>,
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
    fn host_names_are_checked_before_anything_runs() {
        for bad in ["a*", "../x", "-x", "a b", ""] {
            assert!(
                Cli::try_parse_from(["goway", "host", "add", bad]).is_err(),
                "{bad}"
            );
            assert!(
                Cli::try_parse_from(["goway", "ssh", "setup", bad]).is_err(),
                "{bad}"
            );
            assert!(
                Cli::try_parse_from(["goway", "run", "--host", bad, "--", "x"]).is_err(),
                "{bad}"
            );
        }
        assert!(Cli::try_parse_from(["goway", "host", "add", "Orion-Notebook"]).is_ok());
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
