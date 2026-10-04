//! `goway ssh setup HOST [--undo]`: make key login to a host work, reversibly.
//!
//! The walk-through:
//! 1. Find the host (same resolution as every other verb). If key login
//!    already works, nothing changes.
//! 2. Pick a public key: the agent's first key, else a default identity
//!    that has a `.pub`, else create goway's own key in goway's config dir
//!    (never in `~/.ssh`; on Windows its ACL is reduced to the user).
//! 3. Log in once with a password, check the machine is the intended Linux
//!    host, and ensure `~/.ssh` (700) and the key line in
//!    `authorized_keys` (600), tagged `goway:<id>`.
//! 4. Verify key-only login, record goway's own key in the host config,
//!    and add the host to the pool if it was not there.
//!
//! Every change goes through the install journal (local and remote), and
//! the record is kept in the config dir, so `--undo` reverts exactly these
//! changes: the tagged line, modes and directories it created, goway's key
//! if it created it, and the config entries it added.

use std::path::{Path, PathBuf};

use goway_journal::{Change, Journal, LocalSystem, ResourceKind, revert};
use serde::{Deserialize, Serialize};

use crate::cli::SshSetupArgs;
use crate::config::{self, Config, HostConfig};
use crate::error::{Error, Result};
use crate::hosts;
use crate::paths::Paths;
use crate::remotesys::RemoteSystem;
use crate::render::Renderer;
use crate::resolve::{self, Lookup, ProbeResult, Prober, SshProber};
use crate::spawn::CommandExt as _;
use crate::ssh::{self, Failure, KeyPolicy, Target, attempts};
use crate::state::State;

/// What one setup changed, kept for `--undo`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    /// Host name.
    pub host: String,
    /// Address used.
    pub address: String,
    /// Port used.
    pub port: u16,
    /// User, if given.
    pub user: Option<String>,
    /// Private key goway offers for this host, when it created one.
    pub identity: Option<String>,
    /// Local changes (goway's key).
    pub local: Journal,
    /// Changes on the host (`~/.ssh`, `authorized_keys`).
    pub remote: Journal,
    /// Whether setup set `identity` in the host config.
    pub identity_set: bool,
    /// Whether setup added the host to the pool.
    pub host_added: bool,
}

/// Where the record of `host` lives.
pub fn record_path(paths: &Paths, host: &str) -> PathBuf {
    paths
        .config_dir
        .join(format!("ssh-setup-{}.json", host.to_ascii_lowercase()))
}

fn setup_err(host: &str, reason: impl Into<String>) -> Error {
    Error::HostAdd {
        name: host.to_owned(),
        reason: reason.into(),
    }
}

fn sys_err(host: &str, e: impl std::fmt::Display) -> Error {
    setup_err(host, e.to_string())
}

/// Treats "key refused" as "found": the host answered with a host key.
struct Reach(SshProber);

impl Prober for Reach {
    fn probe(&self, target: &Target, policy: KeyPolicy, remote: &str) -> ProbeResult {
        match self.0.probe(target, policy, remote) {
            Err((Failure::AuthRefused, _)) => Ok(String::new()),
            other => other,
        }
    }
}

/// The scratch `known_hosts` of this setup run, emptied first: a leftover
/// file (an earlier crash, a reused process id) must never seed this run's
/// trust decision.
fn fresh_scratch(paths: &Paths) -> PathBuf {
    let scratch = paths
        .state_dir
        .join(format!("known_hosts.setup.{}", std::process::id()));
    let _ = std::fs::remove_file(&scratch);
    scratch
}

/// goway's own key pair in its config dir, if an earlier setup made one:
/// the public key line and the private key's path.
fn own_key(paths: &Paths) -> Option<(String, String)> {
    let private = paths.config_dir.join("id_ed25519");
    let mut public = private.as_os_str().to_owned();
    public.push(".pub");
    let key = std::fs::read_to_string(PathBuf::from(public)).ok()?;
    let key = key.trim().to_owned();
    (private.is_file() && key.starts_with("ssh-"))
        .then(|| (key, private.to_string_lossy().into_owned()))
}

/// The public key line to authorize: the agent's first key, else the first
/// default identity with a `.pub` next to it.
pub fn existing_public_key(address: &str, port: u16) -> Option<String> {
    if let Ok(out) = std::process::Command::new("ssh-add")
        .arg("-L")
        .stdin(std::process::Stdio::null())
        .output_locked()
        && out.status.success()
        && let Some(line) = String::from_utf8_lossy(&out.stdout).lines().next()
        && line.starts_with("ssh-")
    {
        return Some(line.trim().to_owned());
    }
    let eff = crate::sshenv::effective(address, port)?;
    eff.identity_files.iter().find_map(|private| {
        let mut public = private.as_os_str().to_owned();
        public.push(".pub");
        std::fs::read_to_string(PathBuf::from(public))
            .ok()
            .map(|s| s.trim().to_owned())
            .filter(|s| s.starts_with("ssh-") || s.starts_with("ecdsa-"))
    })
}

/// Read the public key the user chose with `--key`. Only `.pub` files are
/// read, so a private key is never opened by mistake.
fn read_public_key(host: &str, path: &Path) -> Result<String> {
    if path.extension().is_none_or(|e| e != "pub") {
        return Err(setup_err(
            host,
            format!(
                "--key must name a public key file ending in .pub, not {}",
                path.display()
            ),
        ));
    }
    let text = std::fs::read_to_string(path).map_err(|e| Error::io("read", path, e))?;
    let line = text.lines().next().unwrap_or_default().trim().to_owned();
    if line.starts_with("ssh-") || line.starts_with("ecdsa-") || line.starts_with("sk-") {
        Ok(line)
    } else {
        Err(setup_err(
            host,
            format!("{} is not an ssh public key", path.display()),
        ))
    }
}

/// The `icacls` arguments that leave only `principal` on a private key: the grant comes first,
/// so a failed grant never leaves a key with its inheritance removed and nobody allowed. The
/// principal is a SID spelled `*S-1-5-...` (never a name taken from the environment).
pub fn icacls_args(path: &Path, principal: &str) -> Vec<String> {
    vec![
        path.display().to_string(),
        "/grant:r".to_owned(),
        format!("{principal}:F"),
        "/inheritance:r".to_owned(),
    ]
}

/// The user SID in the CSV output of `whoami /user /fo csv /nh` (`"DOMAIN\\user","S-1-5-21-..."`),
/// as the `*SID` form icacls accepts.
pub fn parse_whoami_sid(text: &str) -> Option<String> {
    let sid = text
        .lines()
        .find_map(|l| l.trim().rsplit(',').next())?
        .trim()
        .trim_matches('"');
    let valid = sid.starts_with("S-1-")
        && sid.ends_with(|c: char| c.is_ascii_digit())
        && sid.len() <= 184
        && sid
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, 'S' | '-'));
    valid.then(|| format!("*{sid}"))
}

/// A system tool by absolute path (`%SystemRoot%\System32`), never found through `PATH`.
fn system32(tool: &str) -> PathBuf {
    let root =
        std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    root.join("System32").join(tool)
}

/// The invoking user's SID from the token (through `whoami`), not from `%USERNAME%`.
fn current_user_sid() -> Option<String> {
    let out = std::process::Command::new(system32("whoami.exe"))
        .args(["/user", "/fo", "csv", "/nh"])
        .stdin(std::process::Stdio::null())
        .output_locked()
        .ok()?;
    parse_whoami_sid(&String::from_utf8_lossy(&out.stdout))
}

/// On Windows, OpenSSH refuses a private key others can read: keep only
/// the current user on goway's key.
fn restrict_key_acl(path: &Path, renderer: Renderer) {
    if !cfg!(windows) {
        return;
    }
    let Some(principal) = current_user_sid() else {
        renderer.warn(format_args!(
            "could not tell who you are to restrict the ACL of {}; run `icacls <key> /grant:r \"%USERNAME%\":F /inheritance:r` yourself",
            path.display()
        ));
        return;
    };
    let args = icacls_args(path, &principal);
    let ok = std::process::Command::new(system32("icacls.exe"))
        .args(&args)
        .stdin(std::process::Stdio::null())
        .output_locked()
        .is_ok_and(|o| o.status.success());
    if ok {
        tracing::info!(key = %path.display(), "key ACL restricted to the user");
    } else {
        renderer.warn(format_args!(
            "could not restrict the ACL of {}; run: icacls {}",
            path.display(),
            args.join(" ")
        ));
    }
}

/// `authorized_keys` options for goway's key: it needs a shell and a tty
/// (sudo), nothing else.
const KEY_OPTIONS: &str = "no-agent-forwarding,no-port-forwarding,no-X11-forwarding";

/// The plan on the host: `~/.ssh` 700, the tagged, restricted key line, the
/// file 600.
pub fn remote_plan(home: &str, key: &str, marker: &str, already: bool) -> Vec<Change> {
    let dir = PathBuf::from(home).join(".ssh");
    let file = dir.join("authorized_keys");
    let mut plan = vec![
        Change::EnsureDir { path: dir.clone() },
        Change::SetUnixMode {
            path: dir,
            mode: 0o700,
        },
    ];
    if !already {
        plan.push(Change::EnsureLine {
            path: file.clone(),
            line: format!("{KEY_OPTIONS} {key}"),
            marker: marker.to_owned(),
        });
    }
    plan.push(Change::SetUnixMode {
        path: file,
        mode: 0o600,
    });
    plan
}

/// The key's type and base64 blob, the part `authorized_keys` matches on.
fn key_blob(line: &str) -> String {
    let is_type = |w: &&str| ["ssh-", "ecdsa-", "sk-"].iter().any(|p| w.starts_with(p));
    // Skip any `authorized_keys` options in front of the key type.
    line.split_whitespace()
        .skip_while(|w| !is_type(w))
        .take(2)
        .collect::<Vec<_>>()
        .join(" ")
}

/// `goway ssh setup` (or `--undo`).
pub fn setup(
    paths: &Paths,
    renderer: Renderer,
    args: &SshSetupArgs,
    lookup: &dyn Lookup,
) -> Result<u8> {
    setup_with(paths, renderer, args, lookup, args.yes)
}

/// Removes goway's temporary prompt `known_hosts` copy when setup ends, however it ends.
struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The login name shown to the user: the configured one, else this account's own.
fn account_name(target: &Target) -> String {
    target
        .user
        .clone()
        .or_else(|| std::env::var("USER").ok().filter(|u| !u.is_empty()))
        .or_else(|| std::env::var("USERNAME").ok().filter(|u| !u.is_empty()))
        .unwrap_or_else(|| "your account".to_owned())
}

/// Whether to try a password login: on a terminal goway first explains what it is about to do
/// and asks whether the user knows the password (goway cannot tell a blank password from a
/// wrong one, so it asks up front); `assume_yes` and scripts skip the question.
fn knows_password(renderer: Renderer, name: &str, user: &str, assume_yes: bool) -> bool {
    if assume_yes || !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        return true;
    }
    renderer.note(format_args!(
        "To let this laptop log in to {name} without a password from now on, goway puts a key on {name} once."
    ));
    renderer.note(format_args!(
        "(If {user} has no password, or you are not sure, answer n: goway shows three lines to paste on {name} instead.)"
    ));
    let answer = crate::render::ask(&format!(
        "Do you know the password of {user} on {name}? [Y/n] "
    ));
    // No answer (end of input) is the safe side: no password attempt.
    answer.is_some_and(|a| !matches!(a.trim(), "n" | "N" | "no" | "No" | "NO"))
}

/// Run the ssh setup; `assume_yes` skips the password question for callers that already confirmed (`goway add --yes`).
#[allow(clippy::too_many_lines)] // one linear walk-through, easier to audit in one place
pub fn setup_with(
    paths: &Paths,
    renderer: Renderer,
    args: &SshSetupArgs,
    lookup: &dyn Lookup,
    assume_yes: bool,
) -> Result<u8> {
    if args.undo {
        return undo(paths, renderer, &args.host);
    }
    // The user asked for this setup, so every login it makes is a user-initiated
    // step in the failed-login ledger (see `ssh::attempts`), never blocked by
    // the cap on automatic probing.
    let _step = attempts::user_step();
    let name = args.host.as_str();
    let record_file = record_path(paths, name);
    if record_file.exists() {
        return Err(setup_err(
            name,
            format!(
                "already set up (record {}); run `goway ssh setup {name} --undo` first",
                record_file.display()
            ),
        ));
    }
    let config = Config::load(&paths.config_file())?;
    let configured = config.host(name).ok().cloned();
    let mut host = configured.clone().unwrap_or_else(|| HostConfig {
        name: name.to_owned(),
        address: args.address.clone(),
        port: args.port,
        user: args.user.clone(),
        ..HostConfig::default()
    });
    // A rerun after the key was pasted offers goway's own key to the first
    // probe, so it succeeds instead of costing one more failed login.
    if configured.is_none() && args.key.is_none() && host.identity.is_none() {
        host.identity = own_key(paths).map(|(_, private)| private);
    }
    // A configured host is pinned: check its key strictly. A new one is
    // reached with a scratch known_hosts and verified by hostname below.
    let scratch = fresh_scratch(paths);
    let mut settings = ssh::Settings::from_paths(paths);
    let policy = if configured.is_some() {
        KeyPolicy::Strict
    } else {
        settings.known_hosts.clone_from(&scratch);
        KeyPolicy::AcceptNew
    };
    let mut state = State::load(&paths.state_file())?;
    let found = resolve::resolve(
        &config,
        &host,
        &mut state,
        lookup,
        &Reach(SshProber {
            settings: settings.clone(),
        }),
        policy,
        "echo goway-key-login-ok",
    )?;
    let target = found.target.clone();
    // A new host's key is confirmed before anything else happens, in
    // particular before ssh can ask for a password.
    if configured.is_none()
        && let Err(e) = hosts::confirm_key(
            renderer,
            name,
            &target.address,
            &scratch,
            args.fingerprint.as_deref(),
        )
    {
        let _ = std::fs::remove_file(&scratch);
        return Err(e);
    }
    if found.output.contains("goway-key-login-ok") {
        renderer.ok(format_args!(
            "key login to {name} at {} already works; nothing to change",
            target.address
        ));
        if configured.is_none() {
            hosts::register(paths, &host, &target.address, target.port, &scratch)?;
        }
        let _ = std::fs::remove_file(&scratch);
        return Ok(0);
    }
    renderer.headline(format_args!(
        "{name} at {}:{} answers but refuses key login; setting it up",
        target.address, target.port
    ));

    // 1. The key.
    let mut local = Journal::generate();
    // goway's own key from an earlier setup is preferred: the agent's first
    // key may be unrelated to goway (a work or GitHub key).
    let own = own_key(paths);
    let chosen = match &args.key {
        Some(path) => Some(read_public_key(name, path)?),
        None => own
            .as_ref()
            .map(|(key, _)| key.clone())
            .or_else(|| existing_public_key(&target.address, target.port)),
    };
    let (public_key, identity) = if let Some(key) = chosen {
        renderer.note(format_args!("using your existing key {}", key_blob(&key)));
        let identity = own
            .filter(|(own_key, _)| args.key.is_none() && *own_key == key)
            .map(|(_, private)| private);
        (key, identity)
    } else {
        let private = paths.config_dir.join("id_ed25519");
        let comment = format!("goway@{}", crate::repo::client_name());
        let plan = [
            Change::EnsureDir {
                path: paths.config_dir.clone(),
            },
            Change::EnsureResource {
                kind: ResourceKind::SshKeyPair,
                name: private.to_string_lossy().into_owned(),
                spec: comment,
            },
        ];
        local = goway_journal::apply(&plan, &mut LocalSystem).map_err(|e| sys_err(name, e))?;
        restrict_key_acl(&private, renderer);
        renderer.note(format_args!(
            "created goway's own key {}",
            private.display()
        ));
        let mut public = private.as_os_str().to_owned();
        public.push(".pub");
        let key = std::fs::read_to_string(PathBuf::from(public))
            .map_err(|e| Error::io("read", &private, e))?
            .trim()
            .to_owned();
        (key, Some(private.to_string_lossy().into_owned()))
    };

    // A native Windows host takes its key from `goway-setup`, run there once
    // (administrator): no password login, no shell script.
    if host.os == config::Os::Windows {
        let key_target = Target {
            identity: identity.as_ref().map(PathBuf::from),
            ..target.clone()
        };
        HandInstall {
            renderer,
            name,
            public_key: &public_key,
            target: &key_target,
            settings: &settings,
            windows: true,
            elevate: args.rsudo.then_some(Elevate {
                admin: args.windows_admin.as_deref(),
                yes: assume_yes,
            }),
        }
        .run(&account_name(&target))?;
        return finish_setup(
            paths,
            renderer,
            &Finish {
                name,
                record_file: &record_file,
                target: &target,
                settings: &settings,
                identity,
                local,
                remote: Journal::generate(),
                configured: configured.is_some(),
                host: &host,
                scratch: &scratch,
            },
        );
    }

    // 2. One password login (or the key added by hand): check the machine,
    // then authorize the key.
    let facts_script = "uname -s; uname -n; printf '%s\\n' \"$HOME\"; cat ~/.ssh/authorized_keys 2>/dev/null || true";
    let key_target = Target {
        identity: identity.as_ref().map(PathBuf::from),
        ..target.clone()
    };
    let by_hand = HandInstall {
        renderer,
        name,
        public_key: &public_key,
        target: &key_target,
        settings: &settings,
        windows: false,
        elevate: None,
    };
    let user = account_name(&target);
    let password_allowed = attempts::permit(name, attempts::Origin::User);
    if let Err(blocked) = &password_allowed {
        renderer.warn(format_args!(
            "Not asking for a password on {name}: {blocked}."
        ));
    }
    let try_password = !args.no_password
        && password_allowed.is_ok()
        && knows_password(renderer, name, &user, assume_yes);
    let mut remote_sys = RemoteSystem {
        target: target.clone(),
        settings: settings.clone(),
        password: try_password,
        prompt: None,
    };
    let prompt_hosts = RemoveOnDrop(
        paths
            .state_dir
            .join(format!("known_hosts.prompt.{}", std::process::id())),
    );
    if try_password {
        // ssh names the host in its prompt after the key alias; show what the user typed.
        remote_sys.prompt =
            ssh::PasswordPrompt::create(name, &settings.known_hosts, &prompt_hosts.0)
                .map_err(|e| tracing::warn!(error = %e, "no custom password prompt"))
                .ok();
        renderer.note(format_args!(
            "ssh now asks for {user}'s password on {name} (typing is hidden; goway never sees or stores it)."
        ));
    } else {
        if args.no_password {
            renderer.note(format_args!(
                "--no-password: not asking for a password; the key is added by hand"
            ));
        } else {
            renderer.note(format_args!(
                "No password: goway will show you what to paste on {name} instead."
            ));
        }
        by_hand.run(&user)?;
        remote_sys.target = key_target.clone();
    }
    let first = remote_sys.output(facts_script);
    let facts = match first {
        Err(e)
            if remote_sys.password
                && ssh::classify_failure(&e.to_string()) == Failure::AuthRefused =>
        {
            tracing::warn!(host = name, error = %e, "password login refused; key to be added by hand");
            attempts::record_failure(name, attempts::Origin::User);
            renderer.warn(format_args!("That password did not work on {name}."));
            renderer.note(format_args!(
                "Most likely {user} has no password set (common with automatic login) or {name} only allows key logins; \
                 other causes are in docs/troubleshooting.md, \"goway add says Permission denied\"."
            ));
            renderer.headline(format_args!("Let's do it the other way:"));
            by_hand.run(&user)?;
            remote_sys.password = false;
            remote_sys.target = key_target.clone();
            remote_sys
                .output(facts_script)
                .map_err(|e| sys_err(name, e))?
        }
        other => other.map_err(|e| sys_err(name, e))?,
    };
    let mut lines = facts.lines();
    let (os, hostname, home) = (
        lines.next().unwrap_or_default(),
        lines.next().unwrap_or_default(),
        lines.next().unwrap_or_default().to_owned(),
    );
    let trusted_address = configured.is_some()
        || args
            .address
            .as_deref()
            .is_some_and(|a| a.parse::<std::net::IpAddr>().is_ok());
    if os != "Linux" {
        return Err(setup_err(
            name,
            format!("{} is {os}, not Linux", target.address),
        ));
    }
    if !trusted_address && !hosts::hostname_matches(hostname, name, args.address.as_deref()) {
        return Err(setup_err(
            name,
            format!(
                "{} is `{hostname}`, not `{name}` (pass --address with its IP to trust it)",
                target.address
            ),
        ));
    }
    let blob = key_blob(&public_key);
    let already = lines.any(|l| key_blob(l) == blob);
    let marker = format!("goway:{}", local_marker());
    let plan = remote_plan(&home, &public_key, &marker, already);
    let remote = match goway_journal::apply(&plan, &mut remote_sys) {
        Ok(journal) => journal,
        Err(e) => {
            let mut partial = e.journal.clone();
            let _ = revert(&mut partial, &mut remote_sys);
            let _ = revert(&mut local, &mut LocalSystem);
            return Err(sys_err(
                name,
                format!("authorizing the key failed and was rolled back: {e}"),
            ));
        }
    };

    finish_setup(
        paths,
        renderer,
        &Finish {
            name,
            record_file: &record_file,
            target: &target,
            settings: &settings,
            identity,
            local,
            remote,
            configured: configured.is_some(),
            host: &host,
            scratch: &scratch,
        },
    )
}

/// What the end of a setup needs: verify key-only login, record the
/// config changes, write the record `--undo` reads.
struct Finish<'a> {
    name: &'a str,
    record_file: &'a Path,
    target: &'a Target,
    settings: &'a ssh::Settings,
    identity: Option<String>,
    local: Journal,
    remote: Journal,
    configured: bool,
    host: &'a HostConfig,
    scratch: &'a Path,
}

/// Steps 3 and 4 of a setup: check that key-only login works, store the
/// identity and pool membership, and write the undo record.
fn finish_setup(paths: &Paths, renderer: Renderer, f: &Finish<'_>) -> Result<u8> {
    let Finish {
        name,
        record_file,
        target,
        settings,
        identity,
        local,
        remote,
        configured,
        host,
        scratch,
    } = f;
    let (name, configured) = (*name, *configured);
    // 3. Verify key-only login.
    let check_target = Target {
        identity: identity.as_ref().map(PathBuf::from),
        ..(*target).clone()
    };
    // A fresh connection: the password login's multiplexed master must not
    // make the key-only check pass.
    let verified = SshProber {
        settings: ssh::Settings {
            control_dir: None,
            ..(*settings).clone()
        },
    }
    .probe(&check_target, KeyPolicy::Strict, "true")
    .is_ok();

    // 4. Config: identity and pool membership.
    let mut record = Record {
        host: name.to_owned(),
        address: target.address.clone(),
        port: target.port,
        user: target.user.clone(),
        identity: identity.clone(),
        local: local.clone(),
        remote: remote.clone(),
        identity_set: false,
        host_added: false,
    };
    if !configured {
        let mut stored = (*host).clone();
        stored.identity.clone_from(identity);
        // Pin the key confirmed above: no second trust-on-first-use round.
        hosts::register(paths, &stored, &target.address, target.port, scratch)?;
        record.host_added = true;
    } else if identity.is_some() {
        config::set_host_identity(&paths.config_file(), name, identity.as_deref())?;
        record.identity_set = true;
    }
    let _ = std::fs::remove_file(scratch);
    let text = serde_json::to_string_pretty(&record).map_err(|e| sys_err(name, e))?;
    config::write_atomic(record_file, text.as_bytes())?;
    if verified {
        renderer.ok(format_args!(
            "key login to {name} works; undo with `goway ssh setup {name} --undo`"
        ));
        Ok(0)
    } else {
        renderer.warn(format_args!(
            "the key is authorized but key login still fails; check `goway doctor {name}` (undo with --undo)"
        ));
        Ok(1)
    }
}

/// Installing goway's key on the helper by hand, for a helper whose password login is refused or
/// unusable (no password set, password login off, a second factor).
struct HandInstall<'a> {
    renderer: Renderer,
    name: &'a str,
    public_key: &'a str,
    /// The target with goway's own key as identity, so a key-only login offers it.
    target: &'a Target,
    settings: &'a ssh::Settings,
    /// A native Windows host: one `goway-setup` command instead of shell lines.
    windows: bool,
    /// Run the Windows administrator step for the user (`--rsudo`), when asked to.
    elevate: Option<Elevate<'a>>,
}

/// The command to run in an administrator PowerShell on a native Windows
/// host so it authorizes `public_key` (goway-setup records it for its own
/// uninstall). The key is one single-quoted literal (nothing in it expands in
/// PowerShell) and carries no comment: a comment is free text from a `.pub`
/// file or an ssh agent, and the administrator session must not see it.
pub fn native_setup_command(public_key: &str) -> String {
    format!(
        "goway-setup install --host --native --authorized-key {}",
        crate::transport::ps_quote(&native_key_line(public_key))
    )
}

/// The one key `goway-setup` is given: the key type and key of the first line of the `.pub` text,
/// without the comment (and so without anything a comment could smuggle into a command).
fn native_key_line(public_key: &str) -> String {
    let mut words = public_key
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace();
    [words.next(), words.next()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ")
}

/// How `--rsudo` elevates a Windows-side step during setup.
struct Elevate<'a> {
    /// A Windows administrator account for the unattended ssh route.
    admin: Option<&'a str>,
    /// Do not ask before running the step.
    yes: bool,
}

/// How many times the user may press Enter before goway gives up waiting for the key.
const HAND_ATTEMPTS: u32 = 3;

impl HandInstall<'_> {
    /// The commands to run on the helper: the key line is the same restricted one the password
    /// path installs, from the `.pub` text only.
    fn commands(&self) -> Vec<String> {
        if self.windows {
            return vec![native_setup_command(self.public_key)];
        }
        let line = format!("{KEY_OPTIONS} {} goway:{}", self.public_key, local_marker());
        vec![
            "mkdir -p ~/.ssh && chmod 700 ~/.ssh".to_owned(),
            format!("echo {} >> ~/.ssh/authorized_keys", ssh::shell_quote(&line)),
            "chmod 600 ~/.ssh/authorized_keys".to_owned(),
        ]
    }

    /// Whether key-only login with the pinned host key works now (a fresh connection).
    fn key_works(&self) -> bool {
        SshProber {
            settings: ssh::Settings {
                control_dir: None,
                ..self.settings.clone()
            },
        }
        .probe(self.target, KeyPolicy::Strict, "true")
        .is_ok()
    }

    /// With `--rsudo`: run the Windows administrator step through the best route available
    /// (administrator ssh account, else a UAC prompt on the helper's desktop). True when the key
    /// works afterwards; otherwise says why and leaves the manual command to the caller.
    fn elevated_install(&self) -> bool {
        use crate::winadmin::{Outcome, SshWinRunner, WinStep, elevate};
        let Some(want) = &self.elevate else {
            return false;
        };
        let name = self.name;
        let key = native_key_line(self.public_key);
        let step = WinStep::setup(
            &["install", "--host", "--native", "--authorized-key", &key],
            "authorizes goway's key on this Windows host",
        );
        let approved = want.yes
            || crate::render::ask(&format!(
                "Run `{}` as a Windows administrator on {name}? [y/N] ",
                step.command
            ))
            .is_some_and(|a| matches!(a.trim(), "y" | "Y" | "yes" | "Yes" | "YES"));
        if !approved {
            self.renderer
                .note("not elevating; showing the command instead");
            return false;
        }
        let runner = SshWinRunner {
            key_name: name,
            address: &self.target.address,
            port: self.target.port,
            settings: self.settings,
            // A native Windows host has no WSL to start the prompt from.
            wsl: None,
        };
        match elevate(&step, want.admin, &runner) {
            Outcome::Ran(route) => {
                tracing::info!(host = name, ?route, "windows administrator step ran");
                if self.key_works() {
                    self.renderer
                        .ok(format_args!("Key works. {name} is ready."));
                    return true;
                }
                self.renderer.warn(format_args!(
                    "the administrator step ran on {name} but the key does not work yet"
                ));
                false
            }
            Outcome::Failed(route, why) => {
                tracing::warn!(host = name, ?route, %why, "windows administrator step failed");
                self.renderer.warn(format_args!(
                    "the administrator step failed on {name}: {why}"
                ));
                false
            }
            Outcome::Manual { tried } => {
                for line in tried {
                    self.renderer.note(format_args!("not elevated: {line}"));
                }
                false
            }
        }
    }

    /// Print the commands and wait until key login works: with a terminal the user presses
    /// Enter after running them; without one goway stops with the next step.
    fn run(&self, user: &str) -> Result<()> {
        let name = self.name;
        if self.key_works() {
            self.renderer
                .ok(format_args!("goway's key already works on {name}"));
            return Ok(());
        }
        if self.windows && self.elevated_install() {
            return Ok(());
        }
        if self.windows {
            self.renderer.headline(format_args!(
                "On {name}, open PowerShell as Administrator and run this (goway-setup is the installer you used for {name}):"
            ));
        } else {
            self.renderer.headline(format_args!(
                "On {name}, open a terminal and paste these three lines:"
            ));
        }
        // Blank lines around the block and nothing else on its lines, so it copies cleanly.
        self.renderer.line("");
        for command in self.commands() {
            self.renderer.line(&command);
        }
        self.renderer.line("");
        for attempt in 1..=HAND_ATTEMPTS {
            let Some(_) = crate::render::ask("Then press Enter here (Ctrl-C to stop). ") else {
                self.renderer.next(format_args!(
                    "run the {} above on {name}, then rerun the same `goway add {name}` command",
                    if self.windows { "command" } else { "lines" }
                ));
                return Err(setup_err(
                    name,
                    "goway's key is not installed there yet, and there is no terminal to wait on",
                ));
            };
            // The user pressed Enter: this check is user-initiated and may
            // use the budget automatic probing leaves, up to the overall cap.
            if let Err(blocked) = attempts::permit(name, attempts::Origin::User) {
                self.renderer.warn(format_args!("{blocked}."));
                return Err(setup_err(
                    name,
                    "stopped checking the key to stay under the helper's failed-login limit; wait a few minutes and rerun the same `goway add` command",
                ));
            }
            if self.key_works() {
                self.renderer
                    .ok(format_args!("Key works. {name} is ready."));
                tracing::info!(host = name, attempt, "key added by hand works");
                return Ok(());
            }
            self.renderer.warn(format_args!(
                "The key still does not work on {name}. goway tried logging in as {user} with the key above. \
                 Check that the three lines were pasted whole (a typo in the pasted line is the usual cause) and that you ran them as {user} on {name}."
            ));
        }
        Err(setup_err(
            name,
            "key login still fails after the key was added by hand; check `~/.ssh` is 700 and `authorized_keys` is 600 and owned by that user",
        ))
    }
}

fn local_marker() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}-{}", now.as_secs(), std::process::id())
}

/// Revert a recorded setup: host side first (while the key still works),
/// then goway's key, then the config entries.
pub(crate) fn undo(paths: &Paths, renderer: Renderer, name: &str) -> Result<u8> {
    let record_file = record_path(paths, name);
    let text = std::fs::read_to_string(&record_file).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            setup_err(name, "no ssh setup recorded for this host")
        } else {
            Error::io("read", &record_file, e)
        }
    })?;
    let mut record: Record = serde_json::from_str(&text).map_err(|e| sys_err(name, e))?;
    let mut remote_sys = RemoteSystem {
        target: Target {
            name: record.host.clone(),
            address: record.address.clone(),
            port: record.port,
            user: record.user.clone(),
            identity: record.identity.as_ref().map(PathBuf::from),
        },
        settings: ssh::Settings::from_paths(paths),
        // The tagged line goes first in reverse order; later steps may
        // need a password on clients without ssh multiplexing.
        password: true,
        prompt: None,
    };
    let report = revert(&mut record.remote, &mut remote_sys).map_err(|e| sys_err(name, e))?;
    tracing::info!(?report, "remote changes reverted");
    revert(&mut record.local, &mut LocalSystem).map_err(|e| sys_err(name, e))?;
    if record.host_added {
        if Config::load(&paths.config_file())?.host(name).is_ok() {
            config::remove_host(&paths.config_file(), name)?;
            let mut state = State::load(&paths.state_file())?;
            state.forget(name);
            state.save(&paths.state_file())?;
            hosts::forget_key(&paths.known_hosts(), &config::key_alias(name));
        }
    } else if record.identity_set && Config::load(&paths.config_file())?.host(name).is_ok() {
        config::set_host_identity(&paths.config_file(), name, None)?;
    }
    std::fs::remove_file(&record_file).map_err(|e| Error::io("remove", &record_file, e))?;
    renderer.ok(format_args!("undid the ssh setup of {name}"));
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_tags_the_line_and_fixes_modes() {
        let plan = remote_plan("/home/u", "ssh-ed25519 AAAA c", "goway:1", false);
        assert_eq!(plan.len(), 4);
        assert!(matches!(&plan[2], Change::EnsureLine { marker, .. } if marker == "goway:1"));
        assert!(matches!(&plan[3], Change::SetUnixMode { mode: 0o600, .. }));
        let present = remote_plan("/home/u", "ssh-ed25519 AAAA c", "goway:1", true);
        assert!(
            !present
                .iter()
                .any(|c| matches!(c, Change::EnsureLine { .. }))
        );
    }

    #[test]
    fn windows_key_acl_grants_the_sid_before_removing_inheritance() {
        assert_eq!(
            icacls_args(
                Path::new(r"C:\Users\user\AppData\Local\goway\id_ed25519"),
                "*S-1-5-21-1-2-3-1001"
            ),
            [
                r"C:\Users\user\AppData\Local\goway\id_ed25519",
                "/grant:r",
                "*S-1-5-21-1-2-3-1001:F",
                "/inheritance:r"
            ]
        );
    }

    #[test]
    fn the_user_sid_is_read_from_whoami_csv_and_nothing_else() {
        assert_eq!(
            parse_whoami_sid("\"DESKTOP-X\\user\",\"S-1-5-21-1-2-3-1001\"\r\n").as_deref(),
            Some("*S-1-5-21-1-2-3-1001")
        );
        for bad in [
            "",
            "ERROR: nope",
            "\"a\",\"b\"",
            "\"a\",\"S-1-5;calc\"",
            "\"a\",\"S-1-\n",
        ] {
            assert_eq!(parse_whoami_sid(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn only_pub_files_are_read_as_keys() {
        let dir = tempfile::tempdir().unwrap();
        let private = dir.path().join("id_ed25519");
        std::fs::write(&private, "PRIVATE MATERIAL").unwrap();
        let e = read_public_key("h", &private).unwrap_err();
        assert!(e.to_string().contains(".pub"), "{e}");
        let public = dir.path().join("id_ed25519.pub");
        std::fs::write(&public, "ssh-ed25519 AAAA me@x\n").unwrap();
        assert_eq!(
            read_public_key("h", &public).unwrap(),
            "ssh-ed25519 AAAA me@x"
        );
        let junk = dir.path().join("junk.pub");
        std::fs::write(&junk, "hello").unwrap();
        assert!(read_public_key("h", &junk).is_err());
    }

    // frob:tests crates/goway/src/sshsetup.rs::native_setup_command
    #[test]
    fn a_native_windows_host_gets_one_goway_setup_command_with_the_key_only() {
        let cmd = native_setup_command("ssh-ed25519 AAAAC3Nza placeholder@laptop\n");
        assert_eq!(
            cmd,
            "goway-setup install --host --native --authorized-key 'ssh-ed25519 AAAAC3Nza'"
        );
    }

    // frob:tests crates/goway/src/sshsetup.rs::native_setup_command
    #[test]
    fn a_key_comment_never_reaches_the_printed_administrator_command() {
        for comment in [
            "$(Invoke-Expression 'calc')",
            "`$(x)",
            "a\"b",
            "it\u{2019}s; Remove-Item x; \u{2018}",
            "$env:USERNAME",
        ] {
            let cmd = native_setup_command(&format!("ssh-ed25519 AAAAC3Nza {comment}"));
            assert_eq!(
                cmd, "goway-setup install --host --native --authorized-key 'ssh-ed25519 AAAAC3Nza'",
                "{comment}"
            );
        }
        // Even a malformed key line is only ever one single-quoted literal.
        let cmd = native_setup_command("x$(y)\u{2019}z");
        assert!(
            cmd.ends_with("--authorized-key 'x$(y)\u{2019}\u{2019}z'"),
            "{cmd}"
        );
    }

    #[test]
    fn the_authorized_key_line_is_restricted_and_still_recognised() {
        let plan = remote_plan("/home/u", "ssh-ed25519 AAAA c", "goway:1", false);
        let Change::EnsureLine { line, .. } = &plan[2] else {
            panic!("no line change");
        };
        for option in [
            "no-agent-forwarding",
            "no-port-forwarding",
            "no-X11-forwarding",
        ] {
            assert!(
                line.starts_with(KEY_OPTIONS) && line.contains(option),
                "{line}"
            );
        }
        assert_eq!(key_blob(line), "ssh-ed25519 AAAA");
        assert_eq!(
            key_blob("from=\"1.2.3.4\" ssh-ed25519 AAAA c"),
            "ssh-ed25519 AAAA"
        );
    }

    #[test]
    fn a_stale_scratch_known_hosts_is_removed_before_use() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            config_dir: dir.path().to_owned(),
            state_dir: dir.path().to_owned(),
            runtime_dir: None,
        };
        let stale = dir
            .path()
            .join(format!("known_hosts.setup.{}", std::process::id()));
        std::fs::write(&stale, "goway-x ssh-ed25519 AAAA\n").unwrap();
        assert_eq!(fresh_scratch(&paths), stale);
        assert!(!stale.exists());
    }

    #[test]
    fn goways_own_key_is_preferred_and_becomes_the_identity() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            config_dir: dir.path().to_owned(),
            state_dir: dir.path().to_owned(),
            runtime_dir: None,
        };
        assert!(own_key(&paths).is_none());
        std::fs::write(dir.path().join("id_ed25519"), "private").unwrap();
        std::fs::write(
            dir.path().join("id_ed25519.pub"),
            "ssh-ed25519 AAAA goway@x\n",
        )
        .unwrap();
        let (key, private) = own_key(&paths).unwrap();
        assert_eq!(key, "ssh-ed25519 AAAA goway@x");
        assert!(private.ends_with("id_ed25519"));
    }

    #[test]
    fn key_blob_ignores_comments() {
        assert_eq!(
            key_blob("ssh-ed25519 AAAA me@x goway:1"),
            "ssh-ed25519 AAAA"
        );
    }
}
