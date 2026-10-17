//! `goway circle access|cert|pin`: turn the circle roster into ssh trust.
//!
//! A helper runs `access` to trust the circle's admins through `cert-authority` lines (so a new
//! lead needs no helper edit); an admin runs `cert` to mint a lead's 14-day certificate; `pin`
//! records a roster member's host key, but only when its fingerprint equals the roster's.
//! The roster also feeds the host list ([`roster_hosts`]); manual `[[host]]` entries win.
//! frob:ticket 01M5443BQZFS1KV0S4JQ5WP11P

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use goway_circle::{
    Policy, Role, State, authority_lines, merge_authorized_keys, mint_lead_cert, parse_cert,
    pin_line, read_public_key,
};

use crate::changelog;
use crate::circle::{admin_key_path, has_key, load, member_key, node_key_path, pub_of, usage};
use crate::cli::{CircleAccessArgs, CircleCertArgs, CirclePinArgs, CirclePolicyArgs, CircleRenewArgs};
use crate::config::HostConfig;
use crate::error::{Error, Result};
use crate::paths::Paths;
use crate::render::Renderer;
use crate::spawn::CommandExt as _;

/// Where the saved access policy of circle `id` lives.
fn policy_path(paths: &Paths, id: &str) -> PathBuf {
    crate::circle::circle_dir(paths).join(format!("access-{id}.policy"))
}

/// Text form of a policy: `open`, or `listed` followed by one lead per line.
fn policy_text(policy: &Policy) -> String {
    match policy {
        Policy::Open => "open\n".to_owned(),
        Policy::Listed(names) => {
            let mut out = String::from("listed\n");
            for n in names {
                out.push_str(n);
                out.push('\n');
            }
            out
        }
    }
}

/// Parse [`policy_text`]; anything unreadable falls back to the stricter empty `listed`.
fn parse_policy(text: &str) -> Policy {
    let mut lines = text.lines();
    match lines.next() {
        Some("open") => Policy::Open,
        _ => Policy::Listed(lines.map(str::to_owned).collect::<BTreeSet<_>>()),
    }
}

/// The saved policy of circle `id`, `open` when none was saved.
fn saved_policy(paths: &Paths, id: &str) -> Policy {
    std::fs::read_to_string(policy_path(paths, id))
        .map(|t| parse_policy(&t))
        .unwrap_or(Policy::Open)
}

/// The `authorized_keys` file `args` selects.
fn authorized_keys_file(args: &CircleAccessArgs) -> Result<PathBuf> {
    if let Some(file) = &args.file {
        return Ok(file.clone());
    }
    if args.windows_admin {
        let data = std::env::var_os("ProgramData")
            .ok_or_else(|| usage("%ProgramData% is not set; pass --file"))?;
        return Ok(PathBuf::from(data)
            .join("ssh")
            .join("administrators_authorized_keys"));
    }
    dirs::home_dir()
        .map(|h| h.join(".ssh").join("authorized_keys"))
        .ok_or_else(|| usage("no home directory; pass --file"))
}

/// Write this circle's `cert-authority` lines into the chosen `authorized_keys`.
///
/// Lines of this circle from an older epoch are replaced, so a revoke closes the door on
/// old certificates the next time this runs.
pub fn run_access(paths: &Paths, renderer: Renderer, args: &CircleAccessArgs) -> Result<()> {
    let (store, _, state) = load(paths, args.circle.as_deref())?;
    let policy = if args.open {
        Policy::Open
    } else if args.lead.is_empty() {
        saved_policy(paths, store.id())
    } else {
        Policy::Listed(args.lead.iter().cloned().collect())
    };
    for name in match &policy {
        Policy::Listed(n) => n.iter().cloned().collect(),
        Policy::Open => Vec::new(),
    } {
        member_key(&state, &name)?;
    }
    let file = authorized_keys_file(args)?;
    let existing = match std::fs::read_to_string(&file) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(Error::io("read", &file, e)),
    };
    let lines = authority_lines(&state, &policy);
    let merged = merge_authorized_keys(&existing, &state, &lines);
    changelog::write_file_in(&paths.config_dir, &file, merged.as_bytes())?;
    changelog::write_file_in(
        &paths.config_dir,
        &policy_path(paths, store.id()),
        policy_text(&policy).as_bytes(),
    )?;
    tracing::info!(circle = store.id(), epoch = state.epoch, file = %file.display(), lines = lines.len(), "cert-authority lines written");
    renderer.ok(format!(
        "{} now trusts {} (epoch {}, {} line(s) in {})",
        "this machine",
        match &policy {
            Policy::Open => "every lead of the circle".to_owned(),
            Policy::Listed(n) => format!("{} named lead(s)", n.len()),
        },
        state.epoch,
        lines.len(),
        file.display()
    ));
    renderer.next("run `goway circle access` again after a revoke so old certificates stop working here");
    Ok(())
}

/// Mint the lead certificate for `args.name` (default: this machine) with this machine's admin key.
///
/// For this machine the cert is written next to the node key, where ssh picks it up; for
/// another lead the cert text is printed to hand over.
pub fn run_cert(paths: &Paths, renderer: Renderer, args: &CircleCertArgs) -> Result<()> {
    let (_, _, state) = load(paths, args.circle.as_deref())?;
    let admin_key = admin_key_path(paths);
    if !has_key(&admin_key) {
        return Err(usage("this machine has no admin key; run this on an admin machine"));
    }
    let admin_pub = read_public_key(&pub_of(&admin_key))?;
    let node_pub = read_public_key(&pub_of(&node_key_path(paths)))?;
    let (subject, own) = match &args.name {
        Some(name) => {
            let key = member_key(&state, name)?;
            let own = key == node_pub;
            (key, own)
        }
        None => (node_pub, true),
    };
    let text = mint_lead_cert(&state, &admin_pub, &admin_key, &subject)?;
    let parsed = parse_cert(&text)?;
    if own {
        let file = {
            let mut p = node_key_path(paths).into_os_string();
            p.push("-cert.pub");
            PathBuf::from(p)
        };
        changelog::write_file_in(&paths.config_dir, &file, text.as_bytes())?;
        renderer.ok(format!("lead certificate written to {}", file.display()));
    } else {
        renderer.line(text.trim_end());
        renderer.note("save this as <node key>-cert.pub next to the lead's node key");
    }
    renderer.note(format!(
        "valid until unix time {} (about 14 days); run this again to renew",
        parsed.valid_before
    ));
    Ok(())
}

/// True when a certificate valid until `valid_before` has less than half of its life left at `now`.
pub fn needs_renewal(valid_before: u64, now: u64) -> bool {
    valid_before.saturating_sub(now) <= goway_circle::CERT_TTL_SECS / 2
}

/// Renew the lead certificate of `args.name` (default: this machine) once half its life has passed.
///
/// Without `--force`, this machine's own fresh certificate is left alone. Minting needs this
/// machine's admin key, so a machine without one must ask an admin to run `cert` or `renew`.
pub fn run_renew(paths: &Paths, renderer: Renderer, args: &CircleRenewArgs) -> Result<()> {
    if !args.force && args.name.is_none() {
        let mut file = node_key_path(paths).into_os_string();
        file.push("-cert.pub");
        if let Ok(text) = std::fs::read_to_string(PathBuf::from(file)) {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            if let Ok(cert) = parse_cert(&text) {
                if !needs_renewal(cert.valid_before, now) {
                    tracing::info!(valid_before = cert.valid_before, "certificate still fresh");
                    renderer.ok("the certificate has more than half of its life left; nothing renewed");
                    renderer.next("pass --force to renew anyway");
                    return Ok(());
                }
            }
        }
    }
    tracing::info!(name = ?args.name, force = args.force, "renewing certificate");
    run_cert(
        paths,
        renderer,
        &CircleCertArgs {
            name: args.name.clone(),
            circle: args.circle.clone(),
        },
    )
}

/// Set the helper's access policy (`open` or `listed`) and rewrite its `cert-authority` lines.
pub fn run_policy(paths: &Paths, renderer: Renderer, args: &CirclePolicyArgs) -> Result<()> {
    let open = args.mode == "open";
    if open && !args.lead.is_empty() {
        return Err(usage("`policy open` takes no --lead; use `policy listed`"));
    }
    if !open && args.lead.is_empty() {
        return Err(usage("`policy listed` needs at least one --lead NAME"));
    }
    tracing::info!(mode = %args.mode, leads = args.lead.len(), "setting access policy");
    run_access(
        paths,
        renderer,
        &CircleAccessArgs {
            lead: args.lead.clone(),
            open,
            file: args.file.clone(),
            windows_admin: args.windows_admin,
            circle: args.circle.clone(),
        },
    )
}

/// `ssh-keyscan` for one ed25519 host key.
fn keyscan(address: &str, port: u16) -> Result<String> {
    let out = std::process::Command::new("ssh-keyscan")
        .args(["-t", "ed25519", "-p", &port.to_string(), address])
        .stdin(std::process::Stdio::null())
        .output_locked()
        .map_err(|e| Error::io("run ssh-keyscan", "ssh-keyscan", e))?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Pin `args.name`'s host key into goway's `known_hosts` when the scan matches the roster.
pub fn run_pin(paths: &Paths, renderer: Renderer, args: &CirclePinArgs) -> Result<()> {
    pin_with(paths, renderer, args, &keyscan)
}

/// [`run_pin`] with the scanner injected so tests need no network.
pub fn pin_with(
    paths: &Paths,
    renderer: Renderer,
    args: &CirclePinArgs,
    scan: &dyn Fn(&str, u16) -> Result<String>,
) -> Result<()> {
    let (_, _, state) = load(paths, args.circle.as_deref())?;
    let key = member_key(&state, &args.name)?;
    let expected = &state.members[&key].host_fp;
    if expected.is_empty() {
        return Err(usage(format!(
            "the roster holds no host key fingerprint for `{}`",
            args.name
        )));
    }
    let address = args.address.as_deref().unwrap_or(&args.name);
    let scanned = scan(address, args.port)?;
    let alias = format!("goway-{}", args.name);
    let entry = pin_line(&alias, &scanned, expected)?;
    let known_hosts = paths.known_hosts();
    crate::hosts::forget_key(&known_hosts, &alias);
    let mut current = std::fs::read_to_string(&known_hosts).unwrap_or_default();
    if !current.is_empty() && !current.ends_with('\n') {
        current.push('\n');
    }
    current.push_str(&entry);
    current.push('\n');
    changelog::write_file(&known_hosts, current.as_bytes())?;
    tracing::info!(alias, "roster host key pinned");
    renderer.ok(format!(
        "pinned `{}` (fingerprint matches the roster)",
        args.name
    ));
    Ok(())
}

/// Hosts the roster adds to the pool: every other helper, named after its member name.
///
/// Best effort: a missing or unreadable circle adds nothing. ssh offers the node key and its
/// adjacent certificate; the host key is checked against `known_hosts` as for any host.
pub fn roster_hosts(config_dir: &Path) -> Vec<HostConfig> {
    let paths = Paths {
        config_dir: config_dir.to_path_buf(),
        state_dir: PathBuf::new(),
        runtime_dir: None,
    };
    let Ok((_, _, state)) = load(&paths, None) else {
        return Vec::new();
    };
    let Ok(me) = read_public_key(&pub_of(&node_key_path(&paths))) else {
        return Vec::new();
    };
    helper_hosts(&state, &me, &node_key_path(&paths))
}

/// Pure core of [`roster_hosts`].
fn helper_hosts(state: &State, me: &str, identity: &Path) -> Vec<HostConfig> {
    state
        .members
        .iter()
        .filter(|(key, m)| key.as_str() != me && m.roles.contains(&Role::Helper))
        .map(|(_, m)| HostConfig {
            name: m.name.clone(),
            identity: Some(identity.display().to_string()),
            labels: vec!["circle".to_owned()],
            ..HostConfig::default()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circle;
    use crate::cli::{CircleAdmitArgs, CircleCardArgs, CircleInitArgs, CircleJoinArgs, CircleRevokeArgs};
    use crate::render::ColorWhen;

    fn quiet() -> Renderer {
        Renderer::new(ColorWhen::Never)
    }

    fn paths(dir: &Path) -> Paths {
        Paths {
            config_dir: dir.join("config"),
            state_dir: dir.join("state"),
            runtime_dir: None,
        }
    }

    struct Rig {
        _tmp: tempfile::TempDir,
        admin: Paths,
        helper: Paths,
        keys: PathBuf,
        host_pub: String,
    }

    /// A circle with an admin machine `lap` and a joined machine `box`.
    fn rig() -> Rig {
        let tmp = tempfile::tempdir().unwrap();
        let admin = paths(&tmp.path().join("a"));
        let helper = paths(&tmp.path().join("b"));
        let out = circle::init(
            &admin,
            quiet(),
            &CircleInitArgs {
                name: "home".to_owned(),
                machine: Some("lap".to_owned()),
            },
        )
        .unwrap();
        let (host, _) = goway_circle::ensure_keypair(&tmp.path().join("hostkey"), &|_| {}).unwrap();
        let host_fp = goway_circle::key_fingerprint(&host.public).unwrap();
        let card = circle::make_card(
            &helper,
            quiet(),
            &CircleCardArgs {
                role: "both".to_owned(),
                name: Some("box".to_owned()),
                host_fp: Some(host_fp),
            },
        )
        .unwrap();
        let welcome = circle::admit(
            &admin,
            quiet(),
            &CircleAdmitArgs {
                card: card.card.encode(),
                fingerprint: Some(card.fingerprint.clone()),
                circle: None,
            },
        )
        .unwrap();
        circle::join(
            &helper,
            quiet(),
            &CircleJoinArgs {
                welcome: welcome.welcome.encode(),
                fingerprint: Some(out.admin_fingerprint),
            },
        )
        .unwrap();
        let keys = tmp.path().join("authorized_keys");
        Rig {
            _tmp: tmp,
            admin,
            helper,
            keys,
            host_pub: host.public,
        }
    }

    fn access(r: &Rig, lead: &[&str], open: bool) -> Result<()> {
        run_access(
            &r.admin,
            quiet(),
            &CircleAccessArgs {
                lead: lead.iter().map(|s| (*s).to_owned()).collect(),
                open,
                file: Some(r.keys.clone()),
                windows_admin: false,
                circle: None,
            },
        )
    }

    // frob:tests crates/goway/src/access.rs::run_access
    #[test]
    fn access_writes_a_restricted_line_and_a_revoke_replaces_it() {
        let r = rig();
        std::fs::write(&r.keys, "ssh-ed25519 KEEP me@x\n").unwrap();
        access(&r, &[], false).unwrap();
        let text = std::fs::read_to_string(&r.keys).unwrap();
        assert!(text.contains("ssh-ed25519 KEEP me@x"));
        assert!(text.contains("cert-authority,principals=\"goway-lead:home:e0\""));
        assert!(text.contains("no-port-forwarding"));
        circle::revoke(
            &r.admin,
            quiet(),
            &CircleRevokeArgs {
                name: "box".to_owned(),
                yes: true,
                circle: None,
            },
        )
        .unwrap();
        access(&r, &[], false).unwrap();
        let text = std::fs::read_to_string(&r.keys).unwrap();
        assert!(text.contains("goway-lead:home:e1"), "{text}");
        assert!(!text.contains("goway-lead:home:e0"), "{text}");
        assert!(text.contains("KEEP"));
    }

    // frob:tests crates/goway/src/access.rs::run_access
    #[test]
    fn the_saved_listed_policy_survives_a_refresh_and_unknown_leads_are_refused() {
        let r = rig();
        assert!(access(&r, &["ghost"], false).is_err());
        access(&r, &["box"], false).unwrap();
        access(&r, &[], false).unwrap();
        let text = std::fs::read_to_string(&r.keys).unwrap();
        assert!(text.contains("goway-lead:home:e0:lead:box"), "{text}");
        access(&r, &[], true).unwrap();
        let text = std::fs::read_to_string(&r.keys).unwrap();
        assert!(!text.contains(":lead:box"));
    }

    // frob:tests crates/goway/src/access.rs::run_cert
    #[test]
    fn an_admin_mints_a_cert_and_a_non_admin_cannot() {
        let r = rig();
        let cert = |p: &Paths, name: Option<&str>| {
            run_cert(
                p,
                quiet(),
                &CircleCertArgs {
                    name: name.map(str::to_owned),
                    circle: None,
                },
            )
        };
        cert(&r.admin, None).unwrap();
        let file = format!("{}-cert.pub", node_key_path(&r.admin).display());
        let parsed = parse_cert(&std::fs::read_to_string(file).unwrap()).unwrap();
        assert!(parsed.principals.contains(&"goway-lead:home:e0".to_owned()));
        assert!(cert(&r.admin, Some("box")).is_ok());
        assert!(cert(&r.admin, Some("nobody")).is_err());
        let err = cert(&r.helper, None).unwrap_err();
        assert!(err.to_string().contains("no admin key"), "{err}");
    }

    // frob:tests crates/goway/src/access.rs::pin_with
    #[test]
    fn pinning_writes_known_hosts_only_when_the_roster_fingerprint_matches() {
        let r = rig();
        let args = CirclePinArgs {
            name: "box".to_owned(),
            address: Some("10.0.0.9".to_owned()),
            port: 22,
            circle: None,
        };
        let other = read_public_key(&pub_of(&node_key_path(&r.helper))).unwrap();
        let wrong = move |_: &str, _: u16| Ok(format!("10.0.0.9 {other}\n"));
        assert!(pin_with(&r.admin, quiet(), &args, &wrong).is_err());
        assert!(!r.admin.known_hosts().exists());
        let host = r.host_pub.clone();
        let right = move |_: &str, _: u16| Ok(format!("10.0.0.9 {host}\n"));
        pin_with(&r.admin, quiet(), &args, &right).unwrap();
        let known = std::fs::read_to_string(r.admin.known_hosts()).unwrap();
        assert_eq!(known, format!("goway-box {}\n", r.host_pub));
    }

    // frob:tests crates/goway/src/access.rs::roster_hosts
    #[test]
    fn roster_lists_the_other_helpers_only() {
        let r = rig();
        let hosts = roster_hosts(&r.admin.config_dir);
        let names: Vec<_> = hosts.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, ["box"]);
        assert!(roster_hosts(&r._tmp.path().join("nothing")).is_empty());
    }

    // frob:tests crates/goway/src/config.rs::merge_roster
    #[test]
    fn a_manual_host_wins_over_the_roster() {
        let r = rig();
        let file = r.admin.config_file();
        let merged = crate::config::Config::load(&file).unwrap();
        assert_eq!(merged.hosts.len(), 1);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "[[host]]\nname = \"box\"\naddress = \"1.2.3.4\"\n").unwrap();
        let merged = crate::config::Config::load(&file).unwrap();
        assert_eq!(merged.hosts.len(), 1);
        assert_eq!(merged.hosts[0].address.as_deref(), Some("1.2.3.4"));
    }
}
