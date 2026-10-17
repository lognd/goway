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

    /// Print tables as labelled lines, for screen readers (also `GOWAY_PLAIN=1`).
    #[arg(long, global = true)]
    pub plain: bool,

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
    Status {
        /// Probe static host facts (GPUs, features) again instead of using the daily cache.
        #[arg(long)]
        refresh: bool,
    },
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
    /// List or undo every change goway made on a machine (lists with no subcommand).
    Changes {
        /// What to do with the change log; lists it when omitted.
        #[command(subcommand)]
        command: Option<ChangesCommand>,
    },
    /// Show where goway keeps its files.
    #[command(subcommand)]
    Config(ConfigCommand),
    /// Pair machines into a circle by pasting two short texts (card and welcome).
    #[command(subcommand)]
    Circle(CircleCommand),
}

impl Command {
    /// The verb's name, for logs (arguments may hold secrets).
    pub fn verb(&self) -> &'static str {
        match self {
            Self::Add(_) => "add",
            Self::Uninstall(_) => "uninstall",
            Self::Run(_) => "run",
            Self::Status { .. } => "status",
            Self::Gc(_) => "gc",
            Self::Doctor(_) => "doctor",
            Self::Host(_) => "host",
            Self::Ssh(_) => "ssh",
            Self::Changes { .. } => "changes",
            Self::Config(_) => "config",
            Self::Circle(_) => "circle",
        }
    }
}

/// Arguments of `goway run`.
#[derive(Debug, Args)]
#[allow(clippy::struct_excessive_bools)] // independent command-line flags
pub struct RunArgs {
    /// Run on this host instead of the least-loaded one.
    #[arg(long, value_parser = host_name)]
    pub host: Option<String>,
    /// Keep the remote work directory after the run.
    #[arg(long)]
    pub keep: bool,
    /// Give the helper's copy a `.git` (HEAD, index) matching this work
    /// tree, for tests that ask git about the repository; also
    /// `with_git = true` in goway.toml. No remotes, credentials or hooks.
    #[arg(long)]
    pub with_git: bool,
    /// Also send this ref (a full name such as refs/heads/experimental,
    /// repeatable; also `with_refs` in goway.toml) with the helper's `.git`,
    /// at the same commit with its tree readable. Implies --with-git.
    #[arg(long = "with-ref", value_name = "REF")]
    pub with_refs: Vec<String>,
    /// Write host, arch, address and exit code as JSON to this file.
    #[arg(long, env = "GOWAY_REPORT")]
    pub report: Option<PathBuf>,
    /// Split the run across N hosts: each runs part i of N (nextest gets
    /// `--partition count:i/N`; every command sees `GOWAY_SHARD` and
    /// `GOWAY_SHARD_COUNT`). Output lines are prefixed with the host.
    #[arg(long, value_name = "N", conflicts_with = "host", value_parser = clap::value_parser!(u16).range(1..))]
    pub shard: Option<u16>,
    /// Let hosts of every OS take this run (by default only the laptop's OS
    /// family does); also `cross_os = true` in goway.toml.
    #[arg(long, conflicts_with = "each_os")]
    pub any_os: bool,
    /// Skip the recorded memory and disk footprint checks for this run (also
    /// `--needs mem>=0`): a stale or inflated record never holds a run back.
    #[arg(long)]
    pub ignore_footprint: bool,
    /// Run the command once on one host of each OS in the pool, in
    /// parallel, with `[host os]` output prefixes and a per-OS summary.
    #[arg(long, conflicts_with_all = ["host", "shard"])]
    pub each_os: bool,
    /// How the command's output is shown: `safe` strips terminal control
    /// sequences except colors when a stream is a terminal; `raw` passes
    /// every byte through. Pipes and files are never filtered.
    #[arg(long, value_enum, default_value_t, env = "GOWAY_OUTPUT")]
    pub output: crate::termfilter::OutputMode,
    /// Hardware the host must have (repeatable or comma separated): `gpu`,
    /// `gpu=cuda`, `gpu=rocm`, `gpu-mem>=8G`, `cuda>=12.1`, `mem>=16G`,
    /// `cores>=8`, `arch=x86_64`, `os=linux`, `cpu=avx512f`, `kvm`, `docker`,
    /// `disk>=50G`, `label=NAME`. Hosts that lack any are never used; when
    /// none qualifies goway exits 125 and lists what each host lacks.
    #[arg(long, value_name = "TERM", value_delimiter = ',')]
    pub needs: Vec<String>,
    /// Hardware that makes a host score better (same terms as `--needs`);
    /// a host that lacks it is still used when it is the best otherwise.
    #[arg(long, value_name = "TERM", value_delimiter = ',')]
    pub prefers: Vec<String>,
    /// How long to wait in goway's local queue when every host that fits is
    /// at its job limit or short of memory for one more job (such as 90s,
    /// 5m); `0s` fails at once with exit 125.
    #[arg(long, value_name = "DURATION", default_value = "5m", value_parser = wait_duration)]
    pub wait: std::time::Duration,
    /// Skip the extra copy verification a repository gets after a proven
    /// mismatch, for this run only (changed files are still checked).
    #[arg(long)]
    pub trust_copy: bool,
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
#[allow(clippy::struct_excessive_bools)] // independent command-line switches
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
    /// With --fix --rsudo, also offer the optional hardening (such as turning
    /// off ssh password login), asked separately from the tool installs.
    #[arg(long, requires = "rsudo")]
    pub harden: bool,
    /// With --fix --rsudo on a Windows host: a Windows administrator account whose key is in
    /// the host's `administrators_authorized_keys`, so goway elevates through its Windows ssh
    /// (otherwise a UAC prompt, else the exact command to run as administrator).
    #[arg(long, value_name = "USER", requires = "rsudo")]
    pub windows_admin: Option<String>,
    /// With --fix, say explicitly that every configured host is fixed (the
    /// default when no HOST is named); refuses a HOST.
    #[arg(long, requires = "fix", conflicts_with = "host")]
    pub all_hosts: bool,
    /// Also list the passing checks (by default only problems are shown).
    #[arg(long)]
    pub all: bool,
    /// Explain one check in full: what was found, why, and the exact fix.
    #[arg(long, value_name = "CHECK", conflicts_with = "fix")]
    pub explain: Option<String>,
    /// For a `CMake` project: also run one traced configure of it on each
    /// helper (in goway's own labelled scratch directory there, removed
    /// afterwards) to see every package it looks for. Never runs here.
    #[arg(long, conflicts_with = "fix")]
    pub configure: bool,
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
    /// Also install what this laptop is missing (ssh client, git) with sudo here
    /// (on Windows, through its own UAC prompt).
    #[arg(long)]
    pub lsudo: bool,
    /// Do not ask before the --rsudo / --lsudo changes.
    #[arg(long, short = 'y')]
    pub yes: bool,
    /// Do not try a password: print goway's key and the commands to add it by hand on the helper.
    #[arg(long)]
    pub no_password: bool,
    /// With --rsudo, for a Windows-side step: a Windows administrator account whose key is in
    /// the helper's `administrators_authorized_keys`, so goway elevates through its Windows ssh
    /// without anyone at the helper (otherwise a UAC prompt on its desktop, else the exact
    /// command to run as administrator).
    #[arg(long, value_name = "USER", requires = "rsudo")]
    pub windows_admin: Option<String>,
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
    /// Only print the commands that set a Windows machine up as an OpenSSH
    /// helper (install the server, authorize goway's key, PowerShell shell);
    /// nothing is probed or changed.
    #[arg(long)]
    pub windows: bool,
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
#[allow(clippy::struct_excessive_bools)] // independent command-line switches
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
    /// Do not try a password: print goway's key and the commands to add it by hand on the host.
    #[arg(long, conflicts_with = "undo")]
    pub no_password: bool,
    /// For a native Windows host: run its administrator step (authorizing the key) for you,
    /// through its Windows ssh as `--windows-admin`, else through a UAC prompt on its desktop;
    /// no password is ever typed into goway.
    #[arg(long, conflicts_with = "undo")]
    pub rsudo: bool,
    /// With --rsudo: a Windows administrator account whose key is in the helper's
    /// `administrators_authorized_keys` (unattended elevation).
    #[arg(long, value_name = "USER", requires = "rsudo")]
    pub windows_admin: Option<String>,
    /// Answer yes to the questions (try the password, run the administrator step).
    #[arg(long, short = 'y', conflicts_with = "undo")]
    pub yes: bool,
}

/// `goway circle` verbs.
#[derive(Debug, Subcommand)]
pub enum CircleCommand {
    /// Found a circle on this machine; this machine becomes its first admin and lead.
    Init(CircleInitArgs),
    /// Print this machine's card (`gwc1.`...) to paste into `goway circle admit` on an admin.
    Card(CircleCardArgs),
    /// Admit the machine behind a pasted card and print the welcome to paste back.
    Admit(CircleAdmitArgs),
    /// Join a circle from a pasted welcome (`gww1.`...).
    Join(CircleJoinArgs),
    /// Admit a machine on the local network: show a code, compare it on both screens.
    PairHost(CirclePairHostArgs),
    /// Join a circle from an admin that is running `pair-host` on the local network.
    PairJoin(CirclePairJoinArgs),
    /// Show the members of the circle.
    List(CircleSelect),
    /// Remove a member for good (admin only).
    Revoke(CircleRevokeArgs),
    /// Change a member's roles (admin only).
    Roles(CircleRolesArgs),
    /// Merge a roster pushed by an admin (`revoke --now` runs this on helpers).
    Sync(CircleSyncArgs),
    /// Make this machine trust the circle's admins: write the `cert-authority` lines.
    Access(CircleAccessArgs),
    /// Mint (or renew) a lead's 14-day access certificate (admin only).
    Cert(CircleCertArgs),
    /// Pin a roster member's ssh host key from a scan, only if it matches the roster.
    Pin(CirclePinArgs),
    /// Add or remove an admin, with a second admin's signature once there are three.
    #[command(subcommand)]
    Admin(CircleAdminCommand),
    /// Renew this machine's (or a lead's) access certificate once half its life has passed.
    Renew(CircleRenewArgs),
    /// Set this helper's access policy: `open` (every lead) or `listed` (named leads only).
    Policy(CirclePolicyArgs),
}

/// `goway circle admin` verbs.
#[derive(Debug, Subcommand)]
pub enum CircleAdminCommand {
    /// Print a card for this machine's admin key, to be admitted like any member first.
    Card(CircleAdminCardArgs),
    /// Promote a member (admitted with an admin-key card) to admin.
    Add(CircleAdminChangeArgs),
    /// Demote an admin.
    Remove(CircleAdminChangeArgs),
    /// Add this admin's signature to a proposal file and apply it.
    Cosign(CircleCosignArgs),
    /// Make this machine's admin key on a FIDO2 token (`ssh-keygen -t ed25519-sk`); touch it when it blinks.
    Keygen,
}

/// Arguments of `goway circle admin card`.
#[derive(Debug, Args)]
pub struct CircleAdminCardArgs {
    /// This machine's name; `-admin` is appended (default: its host name).
    #[arg(long, value_parser = circle_label)]
    pub name: Option<String>,
}

/// Arguments of `goway circle admin add` and `remove`.
#[derive(Debug, Args)]
pub struct CircleAdminChangeArgs {
    /// Name of the member whose admin key changes status.
    #[arg(value_parser = circle_label)]
    pub name: String,
    /// The second admin (member name of its admin key); needed when there are 3 or more admins.
    #[arg(long, value_parser = circle_label)]
    pub cosigner: Option<String>,
    /// Write the half-signed proposal here instead of applying it (with `--cosigner`).
    #[arg(long)]
    pub out: Option<std::path::PathBuf>,
    /// Which circle.
    #[arg(long)]
    pub circle: Option<String>,
}

/// Arguments of `goway circle admin cosign`.
#[derive(Debug, Args)]
pub struct CircleCosignArgs {
    /// The proposal file written by `admin add|remove --out`.
    pub file: std::path::PathBuf,
    /// Which circle.
    #[arg(long)]
    pub circle: Option<String>,
}

/// Arguments of `goway circle renew`.
#[derive(Debug, Args)]
pub struct CircleRenewArgs {
    /// The lead to renew (default: this machine).
    #[arg(value_parser = circle_label)]
    pub name: Option<String>,
    /// Renew even when more than half of the certificate's life is left.
    #[arg(long)]
    pub force: bool,
    /// Which circle.
    #[arg(long)]
    pub circle: Option<String>,
}

/// Arguments of `goway circle policy`.
#[derive(Debug, Args)]
pub struct CirclePolicyArgs {
    /// `open` accepts every lead of the circle; `listed` only the leads named with `--lead`.
    #[arg(value_parser = ["open", "listed"])]
    pub mode: String,
    /// A lead to accept (repeatable; required for `listed`).
    #[arg(long = "lead", value_parser = circle_label)]
    pub lead: Vec<String>,
    /// The `authorized_keys` file to edit (default: the user's, or the Windows admin file).
    #[arg(long)]
    pub file: Option<std::path::PathBuf>,
    /// Edit `%ProgramData%\ssh\administrators_authorized_keys` (a Windows administrator account).
    #[arg(long)]
    pub windows_admin: bool,
    /// Which circle.
    #[arg(long)]
    pub circle: Option<String>,
}

/// Arguments of `goway circle sync`.
#[derive(Debug, Args)]
pub struct CircleSyncArgs {
    /// The roster (`ops.jsonl` text) as url-safe base64 without padding, or `-` to read it from stdin.
    pub ops: String,
    /// Which circle.
    #[arg(long)]
    pub circle: Option<String>,
}

/// Arguments of `goway circle access`.
#[derive(Debug, Args)]
pub struct CircleAccessArgs {
    /// Accept only this lead (repeatable); default is every lead of the circle (open).
    #[arg(long = "lead", value_parser = circle_label)]
    pub lead: Vec<String>,
    /// Go back to accepting every lead of the circle.
    #[arg(long, conflicts_with = "lead")]
    pub open: bool,
    /// The `authorized_keys` file to edit (default: the user's, or the Windows admin file).
    #[arg(long)]
    pub file: Option<std::path::PathBuf>,
    /// Edit `%ProgramData%\ssh\administrators_authorized_keys` (a Windows administrator account).
    #[arg(long)]
    pub windows_admin: bool,
    /// Which circle.
    #[arg(long)]
    pub circle: Option<String>,
}

/// Arguments of `goway circle cert`.
#[derive(Debug, Args)]
pub struct CircleCertArgs {
    /// The lead to certify (default: this machine).
    #[arg(value_parser = circle_label)]
    pub name: Option<String>,
    /// Which circle.
    #[arg(long)]
    pub circle: Option<String>,
}

/// Arguments of `goway circle pin`.
#[derive(Debug, Args)]
pub struct CirclePinArgs {
    /// Name of the roster member.
    #[arg(value_parser = circle_label)]
    pub name: String,
    /// Where to scan its ssh host key (default: the member name).
    #[arg(long)]
    pub address: Option<String>,
    /// The ssh port to scan.
    #[arg(long, default_value_t = 22)]
    pub port: u16,
    /// Which circle.
    #[arg(long)]
    pub circle: Option<String>,
}

/// Which circle a verb works on when this machine holds more than one.
#[derive(Debug, Args)]
pub struct CircleSelect {
    /// Id (or unique prefix) of the circle; optional when there is only one.
    #[arg(long)]
    pub circle: Option<String>,
}

/// Arguments of `goway circle init`.
#[derive(Debug, Args)]
pub struct CircleInitArgs {
    /// Name of the circle.
    #[arg(default_value = "home", value_parser = circle_label)]
    pub name: String,
    /// This machine's name in the circle (default: its host name).
    #[arg(long, value_parser = circle_label)]
    pub machine: Option<String>,
    /// Encrypt the new admin and node keys with a passphrase asked for on the terminal.
    #[arg(long)]
    pub passphrase: bool,
}

/// Arguments of `goway circle card`.
#[derive(Debug, Args)]
pub struct CircleCardArgs {
    /// Roles to ask for: lead, helper or both.
    #[arg(long, default_value = "both", value_parser = ["lead", "helper", "both"])]
    pub role: String,
    /// This machine's name in the circle (default: its host name).
    #[arg(long, value_parser = circle_label)]
    pub name: Option<String>,
    /// Fingerprint of this machine's sshd host key (default: read from /etc/ssh when possible).
    #[arg(long)]
    pub host_fp: Option<String>,
    /// Also print the card as a QR code (ASCII) to scan from the admin's machine.
    #[arg(long)]
    pub qr: bool,
}

/// Arguments of `goway circle admit`.
#[derive(Debug, Args)]
pub struct CircleAdmitArgs {
    /// The pasted card.
    pub card: String,
    /// The fingerprint the new machine shows; admits without asking when it matches the card.
    #[arg(long)]
    pub fingerprint: Option<String>,
    /// Which circle to admit into.
    #[arg(long)]
    pub circle: Option<String>,
}

/// Arguments of `goway circle join`.
#[derive(Debug, Args)]
pub struct CircleJoinArgs {
    /// The pasted welcome.
    pub welcome: String,
    /// The admin fingerprint the admin shows; joins without asking when it is in the welcome.
    #[arg(long)]
    pub fingerprint: Option<String>,
}

/// Arguments of `goway circle pair-host`.
#[derive(Debug, Args)]
pub struct CirclePairHostArgs {
    /// Port to listen on (default: a free one, printed).
    #[arg(long, default_value_t = 0)]
    pub port: u16,
    /// Address to listen on; a LAN address keeps the listener off other networks.
    #[arg(long, default_value = "0.0.0.0")]
    pub bind: String,
    /// Minutes to wait for the new machine before the listener closes.
    #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u64).range(1..=30))]
    pub wait: u64,
    /// Which circle to admit into.
    #[arg(long)]
    pub circle: Option<String>,
    /// LAN address to show the new machine (default: --bind when specific, else detected for --qr).
    #[arg(long)]
    pub advertise: Option<String>,
    /// Also print the pair-host address as a QR code (ASCII).
    #[arg(long)]
    pub qr: bool,
}

/// Arguments of `goway circle pair-join`.
#[derive(Debug, Args)]
pub struct CirclePairJoinArgs {
    /// Address and port the admin printed, such as 192.168.1.20:41234.
    pub address: String,
    /// Roles to ask for: lead, helper or both.
    #[arg(long, default_value = "both", value_parser = ["lead", "helper", "both"])]
    pub role: String,
    /// This machine's name in the circle (default: its host name).
    #[arg(long, value_parser = circle_label)]
    pub name: Option<String>,
    /// Fingerprint of this machine's sshd host key (default: read from /etc/ssh when possible).
    #[arg(long)]
    pub host_fp: Option<String>,
    /// Remember the admin's address as a hint for the goway host with this name.
    #[arg(long, value_parser = circle_label)]
    pub peer: Option<String>,
}

/// Arguments of `goway circle revoke`.
#[derive(Debug, Args)]
pub struct CircleRevokeArgs {
    /// Name of the member to remove.
    pub name: String,
    /// Remove without asking.
    #[arg(long)]
    pub yes: bool,
    /// Also push the new epoch to every reachable helper now, and list the ones not reached.
    #[arg(long)]
    pub now: bool,
    /// Which circle.
    #[arg(long)]
    pub circle: Option<String>,
}

/// Arguments of `goway circle roles`.
#[derive(Debug, Args)]
pub struct CircleRolesArgs {
    /// Name of the member.
    pub name: String,
    /// New roles: lead, helper or lead,helper.
    pub roles: String,
    /// Which circle.
    #[arg(long)]
    pub circle: Option<String>,
}

/// Clap parser for circle and machine names (letters, digits, `.`, `_`, `-`).
fn circle_label(text: &str) -> Result<String, String> {
    if goway_circle::valid_name(text) {
        Ok(text.to_owned())
    } else {
        Err("use 1-64 letters, digits, . _ -".to_owned())
    }
}

/// `goway changes` verbs.
#[derive(Debug, Subcommand)]
pub enum ChangesCommand {
    /// List every recorded change, oldest first.
    List,
    /// Undo the newest recorded changes (one by default), newest first.
    Undo {
        /// How many of the newest active changes to undo.
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(usize))]
        last: usize,
        /// Undo every recorded change.
        #[arg(long, conflicts_with = "last")]
        all: bool,
    },
}

/// `goway config` verbs.
#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Print the config, state and `known_hosts` paths.
    Path,
}

/// Parse `--wait` (`90s`, `5m`, `0s`).
fn wait_duration(text: &str) -> std::result::Result<std::time::Duration, String> {
    humantime::parse_duration(text)
        .map_err(|e| format!("`{text}` is not a duration like 90s or 5m: {e}"))
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
    fn circle_verbs_parse() {
        for args in [
            vec!["goway", "circle", "init"],
            vec!["goway", "circle", "init", "lab", "--machine", "orion"],
            vec!["goway", "circle", "card", "--role", "helper"],
            vec![
                "goway",
                "circle",
                "admit",
                "gwc1.AAAA",
                "--fingerprint",
                "SHA256:x",
            ],
            vec!["goway", "circle", "join", "gww1.AAAA"],
            vec!["goway", "circle", "list"],
            vec!["goway", "circle", "revoke", "orion", "--yes"],
            vec!["goway", "circle", "roles", "orion", "lead,helper"],
        ] {
            assert!(Cli::try_parse_from(&args).is_ok(), "{args:?}");
        }
        assert!(Cli::try_parse_from(["goway", "circle", "init", "bad name"]).is_err());
        assert!(Cli::try_parse_from(["goway", "circle", "card", "--role", "boss"]).is_err());
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
