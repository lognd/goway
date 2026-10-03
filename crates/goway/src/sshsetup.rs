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
use crate::ssh::{self, Failure, KeyPolicy, Target};
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
    if let Ok(out) = std::process::Command::new("ssh-add").arg("-L").output()
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
        .output()
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
        .output()
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
#[allow(clippy::too_many_lines)] // one linear walk-through, easier to audit in one place
pub fn setup(
    paths: &Paths,
    renderer: Renderer,
    args: &SshSetupArgs,
    lookup: &dyn Lookup,
) -> Result<u8> {
    if args.undo {
        return undo(paths, renderer, &args.host);
    }
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
    let host = configured.clone().unwrap_or_else(|| HostConfig {
        name: name.to_owned(),
        address: args.address.clone(),
        port: args.port,
        user: args.user.clone(),
        max_jobs: None,
        priority: None,
        max_load: None,
        identity: None,
        labels: Vec::new(),
        gpu_jobs: None,
    });
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

    // 2. One password login: check the machine, then authorize the key.
    renderer.note(format_args!(
        "logging in to {name} with a password once (ssh will ask) to authorize the key"
    ));
    let mut remote_sys = RemoteSystem {
        target: target.clone(),
        settings: settings.clone(),
        password: true,
    };
    let facts = remote_sys
        .output("uname -s; uname -n; printf '%s\\n' \"$HOME\"; cat ~/.ssh/authorized_keys 2>/dev/null || true")
        .map_err(|e| sys_err(name, e))?;
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

    // 3. Verify key-only login.
    let check_target = Target {
        identity: identity.as_ref().map(PathBuf::from),
        ..target.clone()
    };
    // A fresh connection: the password login's multiplexed master must not
    // make the key-only check pass.
    let verified = SshProber {
        settings: ssh::Settings {
            control_dir: None,
            ..settings.clone()
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
        local,
        remote,
        identity_set: false,
        host_added: false,
    };
    if configured.is_none() {
        let mut stored = host.clone();
        stored.identity.clone_from(&identity);
        // Pin the key confirmed above: no second trust-on-first-use round.
        hosts::register(paths, &stored, &target.address, target.port, &scratch)?;
        record.host_added = true;
    } else if identity.is_some() {
        config::set_host_identity(&paths.config_file(), name, identity.as_deref())?;
        record.identity_set = true;
    }
    let _ = std::fs::remove_file(&scratch);
    let text = serde_json::to_string_pretty(&record).map_err(|e| sys_err(name, e))?;
    config::write_atomic(&record_file, text.as_bytes())?;
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
