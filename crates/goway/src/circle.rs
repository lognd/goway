//! `goway circle`: pair machines into a circle with two pasted texts.
//!
//! The new machine prints a card (`gwc1.`), an admin admits it and prints a welcome
//! (`gww1.`), the new machine joins from the welcome. Only public keys travel. The root of trust
//! is a fingerprint a person compares on both screens: the card's node-key fingerprint when
//! admitting, an admin-key fingerprint when joining. Nothing here logs or prints a private key.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use goway_circle::{
    Card, Kind, Op, Role, Signer, SshKeygenSigner, SshKeygenVerifier, State, Store, Welcome,
    ensure_keypair, fold, fold_pinned, key_fingerprint, read_public_key, valid_roles,
};

use crate::changelog;
use crate::cli::{
    CircleAdmitArgs, CircleCardArgs, CircleCommand, CircleInitArgs, CircleJoinArgs,
    CircleRevokeArgs, CircleRolesArgs, CircleSelect,
};
use crate::error::{Error, Result};
use crate::paths::Paths;
use crate::render::{self, Renderer};

/// Where sshd's ed25519 host key lives on a Linux helper.
const SSHD_HOST_KEY_PUB: &str = "/etc/ssh/ssh_host_ed25519_key.pub";

/// The directory that holds this machine's circle keys and circles.
fn circle_dir(paths: &Paths) -> PathBuf {
    paths.config_dir.join("circle")
}

/// The directory holding one directory per circle.
fn circles_dir(paths: &Paths) -> PathBuf {
    circle_dir(paths).join("circles")
}

fn node_key_path(paths: &Paths) -> PathBuf {
    circle_dir(paths).join("node_ed25519")
}

fn admin_key_path(paths: &Paths) -> PathBuf {
    circle_dir(paths).join("admin_ed25519")
}

/// A [`goway_circle::FileSink`] that records each write in the config dir's change log.
struct JournalSink {
    log_dir: PathBuf,
}

impl goway_circle::FileSink for JournalSink {
    fn write(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        changelog::write_file_in(&self.log_dir, path, bytes).map_err(std::io::Error::other)
    }
}

fn sink(paths: &Paths) -> JournalSink {
    JournalSink {
        log_dir: paths.config_dir.clone(),
    }
}

/// Load or create a key pair under the circle dir; tightens the ACL of a new key on Windows.
fn key_pair(path: &Path, renderer: Renderer) -> Result<goway_circle::KeyPair> {
    let (pair, created) = ensure_keypair(path, &|p| {
        crate::sshsetup::restrict_key_acl(p, renderer);
    })?;
    if created {
        tracing::info!(key = %path.display(), "circle key created");
        renderer.note(format!("made a new key at {}", path.display()));
    }
    Ok(pair)
}

fn signer_for(pair: &goway_circle::KeyPair) -> Result<SshKeygenSigner> {
    Ok(SshKeygenSigner::new(
        pair.private.clone(),
        pair.public.clone(),
    )?)
}

/// This machine's name as a valid circle label.
fn machine_name() -> String {
    let raw = crate::repo::client_name();
    let first = raw.split('.').next().unwrap_or_default();
    let clean: String = first
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .take(40)
        .collect();
    if clean.is_empty() || clean == "unknown" {
        "node".to_owned()
    } else {
        clean
    }
}

/// The fingerprint of this machine's sshd host key, when it can be read.
fn detected_host_fp() -> Option<String> {
    let key = read_public_key(Path::new(SSHD_HOST_KEY_PUB)).ok()?;
    key_fingerprint(&key)
}

fn fingerprint_of(key: &str) -> String {
    key_fingerprint(key).unwrap_or_else(|| "(unreadable key)".to_owned())
}

fn role_set(roles: &str) -> BTreeSet<Role> {
    roles
        .split(',')
        .filter_map(|r| match r {
            "lead" => Some(Role::Lead),
            "helper" => Some(Role::Helper),
            _ => None,
        })
        .collect()
}

fn usage(message: impl Into<String>) -> Error {
    Error::Usage(message.into())
}

/// Require the person to confirm `expected`: from `--fingerprint`, else by typing it.
fn confirm(renderer: Renderer, given: Option<&str>, accepted: &[String], what: &str) -> Result<()> {
    let typed = if let Some(g) = given {
        g.trim().to_owned()
    } else {
        renderer.next(format!(
            "compare this with the fingerprint shown on the other machine ({what})"
        ));
        let Some(line) = render::ask("type the fingerprint to confirm: ") else {
            return Err(usage(format!(
                "cannot ask here; pass --fingerprint SHA256:... (the {what} shown on the other machine)"
            )));
        };
        line.trim().to_owned()
    };
    if accepted.contains(&typed) {
        tracing::info!(what, "fingerprint confirmed by a person");
        Ok(())
    } else {
        tracing::warn!(what, "fingerprint did not match; refusing");
        Err(usage(format!(
            "that is not the {what}; nothing was changed. Check you copied it from the other machine's screen"
        )))
    }
}

/// Load the circle store and its folded state.
fn load(paths: &Paths, select: Option<&str>) -> Result<(Store, Vec<Op>, State)> {
    let store = Store::find(&circles_dir(paths), select)?;
    let ops = store.ops()?;
    let state = fold(&ops, &SshKeygenVerifier)?;
    Ok((store, ops, state))
}

/// Append `op` to `ops` and commit when `applied` holds on the resulting state.
fn append_checked(
    paths: &Paths,
    store: &Store,
    mut ops: Vec<Op>,
    op: Op,
    applied: impl Fn(&State) -> bool,
    refusal: &str,
) -> Result<(Vec<Op>, State)> {
    ops.push(op);
    let state = fold(&ops, &SshKeygenVerifier)?;
    if !applied(&state) {
        tracing::warn!(circle = store.id(), "operation did not apply: {refusal}");
        return Err(usage(refusal));
    }
    store.commit(&ops, &sink(paths))?;
    Ok((ops, state))
}

/// What `goway circle card` produced.
#[derive(Debug)]
pub struct CardOut {
    /// The card.
    pub card: Card,
    /// Fingerprint of the node key, for the admin to compare.
    pub fingerprint: String,
}

/// Make this machine's card, creating its node key when missing.
pub fn make_card(paths: &Paths, renderer: Renderer, args: &CircleCardArgs) -> Result<CardOut> {
    let node = key_pair(&node_key_path(paths), renderer)?;
    let roles = match args.role.as_str() {
        "both" => "lead,helper".to_owned(),
        other => other.to_owned(),
    };
    let host_fp = args
        .host_fp
        .clone()
        .or_else(detected_host_fp)
        .unwrap_or_default();
    let card = Card {
        v: 1,
        key: node.public.clone(),
        roles,
        name: args.name.clone().unwrap_or_else(machine_name),
        host_fp,
    };
    card.validate()?;
    tracing::info!(name = %card.name, roles = %card.roles, "card made");
    Ok(CardOut {
        fingerprint: fingerprint_of(&card.key),
        card,
    })
}

/// What `goway circle init` produced.
#[derive(Debug)]
pub struct InitOut {
    /// The short circle id.
    pub id: String,
    /// Fingerprint of the admin key; joiners compare this.
    pub admin_fingerprint: String,
}

/// Found a circle: genesis signed by this machine's admin key, then this machine's node key.
pub fn init(paths: &Paths, renderer: Renderer, args: &CircleInitArgs) -> Result<InitOut> {
    let admin = key_pair(&admin_key_path(paths), renderer)?;
    let node = key_pair(&node_key_path(paths), renderer)?;
    let machine = args.machine.clone().unwrap_or_else(machine_name);
    let admin_name = format!("{machine}-admin");
    if !goway_circle::valid_name(&admin_name) {
        return Err(usage(format!("`{machine}` is too long for a machine name")));
    }
    let admin_signer = signer_for(&admin)?;
    let genesis = Op::signed(
        Kind::Genesis,
        &[
            ("circle", args.name.as_str()),
            ("admin", &admin.public),
            ("name", &admin_name),
        ],
        &[],
        &[&admin_signer as &dyn Signer],
    )?;
    let store = Store::for_genesis(&circles_dir(paths), &genesis.id());
    if store.dir().join("ops.jsonl").is_file() {
        return Err(usage(format!(
            "this machine already founded circle {}; use `goway circle list`",
            store.id()
        )));
    }
    let host_fp = detected_host_fp().unwrap_or_default();
    let roles = if host_fp.is_empty() {
        "lead"
    } else {
        "lead,helper"
    };
    let admission = Op::signed(
        Kind::Admit,
        &[
            ("key", node.public.as_str()),
            ("name", machine.as_str()),
            ("roles", roles),
            ("host_fp", host_fp.as_str()),
        ],
        &[genesis.id()],
        &[&admin_signer as &dyn Signer],
    )?;
    let ops = vec![genesis, admission];
    let state = fold(&ops, &SshKeygenVerifier)?;
    if !state.members.contains_key(&node.public) {
        return Err(usage("the first admission did not apply"));
    }
    store.commit(&ops, &sink(paths))?;
    tracing::info!(circle = store.id(), name = %args.name, "circle founded");
    Ok(InitOut {
        id: store.id().to_owned(),
        admin_fingerprint: fingerprint_of(&admin.public),
    })
}

/// What `goway circle admit` produced.
#[derive(Debug)]
pub struct AdmitOut {
    /// The welcome to paste on the new machine.
    pub welcome: Welcome,
    /// Fingerprint of this admin key, for the new machine to compare.
    pub admin_fingerprint: String,
}

/// Admit the machine behind `card_text`; needs this machine's admin key to be an admin.
pub fn admit(paths: &Paths, renderer: Renderer, args: &CircleAdmitArgs) -> Result<AdmitOut> {
    let card = Card::decode(&args.card)?;
    let fp = fingerprint_of(&card.key);
    renderer.headline(format!("card from `{}` asks for {}", card.name, card.roles));
    renderer.line(format!("  node key fingerprint: {fp}"));
    if !card.host_fp.is_empty() {
        renderer.line(format!("  host key fingerprint: {}", card.host_fp));
    }
    let (store, ops, state) = load(paths, args.circle.as_deref())?;
    let admin_path = admin_key_path(paths);
    if !admin_path.is_file() {
        return Err(usage(
            "this machine has no admin key; run `goway circle admit` on an admin machine",
        ));
    }
    let admin = key_pair(&admin_path, renderer)?;
    if !state.admins.contains(&admin.public) {
        return Err(usage(
            "this machine's admin key is not an admin of that circle; run admit on an admin machine",
        ));
    }
    if state.members.contains_key(&card.key) {
        return Err(usage(format!("`{}` is already a member", card.name)));
    }
    confirm(
        renderer,
        args.fingerprint.as_deref(),
        &[fp],
        "node key fingerprint",
    )?;
    let signer = signer_for(&admin)?;
    let op = Op::signed(
        Kind::Admit,
        &[
            ("key", card.key.as_str()),
            ("name", card.name.as_str()),
            ("roles", card.roles.as_str()),
            ("host_fp", card.host_fp.as_str()),
        ],
        &goway_circle::heads(&ops).into_iter().collect::<Vec<_>>(),
        &[&signer as &dyn Signer],
    )?;
    let key = card.key.clone();
    let (ops, state) = append_checked(
        paths,
        &store,
        ops,
        op,
        |s| s.members.contains_key(&key),
        "the roster refused this admission (the key may have been revoked before)",
    )?;
    let genesis = ops
        .iter()
        .find(|o| o.kind == Kind::Genesis)
        .map(Op::id)
        .unwrap_or_default();
    tracing::info!(circle = store.id(), member = %card.name, "member admitted");
    Ok(AdmitOut {
        welcome: Welcome {
            v: 1,
            circle: state.circle,
            genesis,
            ops,
        },
        admin_fingerprint: fingerprint_of(&admin.public),
    })
}

/// What `goway circle join` produced.
#[derive(Debug)]
pub struct JoinOut {
    /// The short circle id.
    pub id: String,
    /// Members of the circle after joining.
    pub members: usize,
}

/// Join a circle from `welcome_text` after the admin fingerprint is confirmed.
pub fn join(paths: &Paths, renderer: Renderer, args: &CircleJoinArgs) -> Result<JoinOut> {
    let welcome = Welcome::decode(&args.welcome)?;
    let node_path = node_key_path(paths);
    if !node_path.is_file() {
        return Err(usage(
            "this machine has no node key; run `goway circle card` first and give the card to an admin",
        ));
    }
    let node = key_pair(&node_path, renderer)?;
    let state = fold_pinned(&welcome.ops, &SshKeygenVerifier, Some(&welcome.genesis))?;
    if state.circle.is_empty() {
        return Err(usage("the welcome does not found a circle"));
    }
    if !state.members.contains_key(&node.public) {
        return Err(usage(
            "the welcome does not admit this machine's node key; was it made from this machine's card?",
        ));
    }
    let admin_fps: Vec<String> = state.admins.iter().map(|k| fingerprint_of(k)).collect();
    renderer.headline(format!(
        "welcome to circle `{}` ({} members)",
        state.circle,
        state.members.len()
    ));
    for fp in &admin_fps {
        renderer.line(format!("  admin key fingerprint: {fp}"));
    }
    confirm(
        renderer,
        args.fingerprint.as_deref(),
        &admin_fps,
        "admin key fingerprint",
    )?;
    let store = Store::for_genesis(&circles_dir(paths), &welcome.genesis);
    store.commit(&welcome.ops, &sink(paths))?;
    tracing::info!(circle = store.id(), "circle joined");
    Ok(JoinOut {
        id: store.id().to_owned(),
        members: state.members.len(),
    })
}

/// Find the one member called `name`.
fn member_key(state: &State, name: &str) -> Result<String> {
    let mut hits = state
        .members
        .iter()
        .filter(|(_, m)| m.name == name)
        .map(|(k, _)| k.clone());
    match (hits.next(), hits.next()) {
        (Some(key), None) => Ok(key),
        (None, _) => Err(usage(format!(
            "no member named `{name}`; see `goway circle list`"
        ))),
        (Some(_), Some(_)) => Err(usage(format!(
            "more than one member is named `{name}`; revoke by renaming is not possible yet"
        ))),
    }
}

#[allow(clippy::too_many_arguments)] // one call shape shared by revoke and roles
/// An admin-signed op `kind` on member `name`, committed when `applied` holds.
fn admin_op(
    paths: &Paths,
    renderer: Renderer,
    select: Option<&str>,
    name: &str,
    kind: Kind,
    extra: &[(&str, &str)],
    applied: impl Fn(&State, &str) -> bool,
    refusal: &str,
) -> Result<()> {
    let (store, ops, state) = load(paths, select)?;
    let admin_path = admin_key_path(paths);
    if !admin_path.is_file() {
        return Err(usage(
            "this machine has no admin key; run this on an admin machine",
        ));
    }
    let admin = key_pair(&admin_path, renderer)?;
    if !state.admins.contains(&admin.public) {
        return Err(usage(
            "this machine's admin key is not an admin of that circle",
        ));
    }
    let key = member_key(&state, name)?;
    let signer = signer_for(&admin)?;
    let mut body = vec![("key", key.as_str())];
    body.extend_from_slice(extra);
    let op = Op::signed(
        kind,
        &body,
        &goway_circle::heads(&ops).into_iter().collect::<Vec<_>>(),
        &[&signer as &dyn Signer],
    )?;
    append_checked(paths, &store, ops, op, |s| applied(s, &key), refusal)?;
    tracing::info!(circle = store.id(), member = name, ?kind, "roster changed");
    Ok(())
}

/// Remove `args.name` from the circle for good.
pub fn revoke(paths: &Paths, renderer: Renderer, args: &CircleRevokeArgs) -> Result<()> {
    if !args.yes {
        let ok = render::ask(&format!(
            "revoke `{}` for good? it can never be re-admitted [y/N] ",
            args.name
        ))
        .is_some_and(|a| matches!(a.trim(), "y" | "Y" | "yes"));
        if !ok {
            return Err(usage("not revoked (pass --yes to revoke without asking)"));
        }
    }
    admin_op(
        paths,
        renderer,
        args.circle.as_deref(),
        &args.name,
        Kind::Revoke,
        &[],
        |s, key| s.tombstones.contains(key),
        "the roster refused this revoke (an admin member needs the admin quorum)",
    )?;
    renderer.ok(format!("revoked `{}`", args.name));
    Ok(())
}

/// Change the roles of `args.name`.
pub fn set_roles(paths: &Paths, renderer: Renderer, args: &CircleRolesArgs) -> Result<()> {
    if !valid_roles(&args.roles) {
        return Err(usage("roles must be lead, helper or lead,helper"));
    }
    let want = role_set(&args.roles);
    admin_op(
        paths,
        renderer,
        args.circle.as_deref(),
        &args.name,
        Kind::SetRoles,
        &[("roles", args.roles.as_str())],
        |s, key| s.members.get(key).is_some_and(|m| m.roles == want),
        "the roster refused this role change (an admin must keep the lead role)",
    )?;
    renderer.ok(format!("`{}` now has roles {}", args.name, args.roles));
    Ok(())
}

/// Rows of the member table (header first).
pub fn list_rows(
    paths: &Paths,
    renderer: Renderer,
    select: &CircleSelect,
) -> Result<Vec<Vec<String>>> {
    let (store, _ops, state) = load(paths, select.circle.as_deref())?;
    let own = node_key_path(paths)
        .with_extension("pub")
        .exists()
        .then(|| read_public_key(&pub_of(&node_key_path(paths))).ok())
        .flatten();
    let _ = renderer;
    let mut rows = vec![
        ["name", "roles", "fingerprint", "host key", "notes"]
            .map(str::to_owned)
            .to_vec(),
    ];
    for (key, m) in &state.members {
        let roles: Vec<&str> = m
            .roles
            .iter()
            .map(|r| match r {
                Role::Lead => "lead",
                Role::Helper => "helper",
            })
            .collect();
        let mut notes = Vec::new();
        if state.admins.contains(key) {
            notes.push("admin");
        }
        if own.as_deref() == Some(key.as_str()) {
            notes.push("this machine");
        }
        rows.push(vec![
            m.name.clone(),
            roles.join(","),
            fingerprint_of(key),
            m.host_fp.clone(),
            notes.join(", "),
        ]);
    }
    tracing::debug!(circle = store.id(), epoch = state.epoch, "listed members");
    Ok(rows)
}

fn pub_of(private: &Path) -> PathBuf {
    let mut name = private.as_os_str().to_owned();
    name.push(".pub");
    PathBuf::from(name)
}

/// Run one `goway circle` verb.
pub fn run(paths: &Paths, renderer: Renderer, command: &CircleCommand) -> Result<()> {
    match command {
        CircleCommand::Init(args) => {
            let out = init(paths, renderer, args)?;
            renderer.ok(format!("circle `{}` founded (id {})", args.name, out.id));
            renderer.line(format!("admin key fingerprint: {}", out.admin_fingerprint));
            renderer.next(
                "on each new machine run `goway circle card`, then `goway circle admit CARD` here",
            );
        }
        CircleCommand::Card(args) => {
            let out = make_card(paths, renderer, args)?;
            renderer.line(out.card.encode());
            renderer.note(format!("node key fingerprint: {}", out.fingerprint));
            renderer.next("paste the card into `goway circle admit` on an admin machine; it will ask for that fingerprint");
        }
        CircleCommand::Admit(args) => {
            let out = admit(paths, renderer, args)?;
            renderer.line(out.welcome.encode());
            renderer.note(format!("admin key fingerprint: {}", out.admin_fingerprint));
            renderer.next("paste the welcome into `goway circle join` on the new machine; it will ask for that fingerprint");
        }
        CircleCommand::Join(args) => {
            let out = join(paths, renderer, args)?;
            renderer.ok(format!(
                "joined circle {} ({} members)",
                out.id, out.members
            ));
        }
        CircleCommand::List(select) => {
            renderer.table(&list_rows(paths, renderer, select)?);
        }
        CircleCommand::Revoke(args) => revoke(paths, renderer, args)?,
        CircleCommand::Roles(args) => set_roles(paths, renderer, args)?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::ColorWhen;

    fn paths(dir: &Path) -> Paths {
        Paths {
            config_dir: dir.join("config"),
            state_dir: dir.join("state"),
            runtime_dir: None,
        }
    }

    fn quiet() -> Renderer {
        Renderer::new(ColorWhen::Never)
    }

    fn card_args(name: &str) -> CircleCardArgs {
        CircleCardArgs {
            role: "helper".to_owned(),
            name: Some(name.to_owned()),
            host_fp: Some("SHA256:abc".to_owned()),
        }
    }

    struct Pair {
        _tmp: tempfile::TempDir,
        admin: Paths,
        joiner: Paths,
        id: String,
        admin_fp: String,
    }

    fn pair() -> Pair {
        let tmp = tempfile::tempdir().unwrap();
        let admin = paths(&tmp.path().join("a"));
        let joiner = paths(&tmp.path().join("b"));
        let out = init(
            &admin,
            quiet(),
            &CircleInitArgs {
                name: "home".to_owned(),
                machine: Some("lap".to_owned()),
            },
        )
        .unwrap();
        Pair {
            _tmp: tmp,
            admin,
            joiner,
            id: out.id,
            admin_fp: out.admin_fingerprint,
        }
    }

    fn admit_args(card: &CardOut, fp: Option<&str>) -> CircleAdmitArgs {
        CircleAdmitArgs {
            card: card.card.encode(),
            fingerprint: fp.map(str::to_owned),
            circle: None,
        }
    }

    // frob:tests circle::run
    #[test]
    fn two_paste_pairing_joins_a_machine() {
        let p = pair();
        let card = make_card(&p.joiner, quiet(), &card_args("box")).unwrap();
        let admitted = admit(
            &p.admin,
            quiet(),
            &admit_args(&card, Some(&card.fingerprint)),
        )
        .unwrap();
        assert_eq!(admitted.admin_fingerprint, p.admin_fp);
        let out = join(
            &p.joiner,
            quiet(),
            &CircleJoinArgs {
                welcome: admitted.welcome.encode(),
                fingerprint: Some(p.admin_fp.clone()),
            },
        )
        .unwrap();
        assert_eq!(out.id, p.id);
        assert_eq!(out.members, 3);
        let rows = list_rows(&p.joiner, quiet(), &CircleSelect { circle: None }).unwrap();
        assert!(
            rows.iter()
                .any(|r| r[0] == "box" && r[4].contains("this machine"))
        );
        assert!(
            rows.iter()
                .any(|r| r[0] == "lap-admin" && r[4].contains("admin"))
        );
    }

    #[test]
    fn admit_with_a_wrong_fingerprint_changes_nothing() {
        let p = pair();
        let card = make_card(&p.joiner, quiet(), &card_args("box")).unwrap();
        let err = admit(&p.admin, quiet(), &admit_args(&card, Some("SHA256:nope"))).unwrap_err();
        assert!(
            err.to_string().contains("not the node key fingerprint"),
            "{err}"
        );
        let rows = list_rows(&p.admin, quiet(), &CircleSelect { circle: None }).unwrap();
        assert!(!rows.iter().any(|r| r[0] == "box"));
    }

    #[test]
    fn join_with_a_wrong_admin_fingerprint_stores_nothing() {
        let p = pair();
        let card = make_card(&p.joiner, quiet(), &card_args("box")).unwrap();
        let admitted = admit(
            &p.admin,
            quiet(),
            &admit_args(&card, Some(&card.fingerprint)),
        )
        .unwrap();
        let err = join(
            &p.joiner,
            quiet(),
            &CircleJoinArgs {
                welcome: admitted.welcome.encode(),
                fingerprint: Some("SHA256:evil".to_owned()),
            },
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("not the admin key fingerprint"),
            "{err}"
        );
        assert!(Store::find(&circles_dir(&p.joiner), None).is_err());
    }

    #[test]
    fn a_welcome_for_another_machine_is_refused() {
        let p = pair();
        let other = tempfile::tempdir().unwrap();
        let stranger = paths(other.path());
        let card = make_card(&p.joiner, quiet(), &card_args("box")).unwrap();
        let admitted = admit(
            &p.admin,
            quiet(),
            &admit_args(&card, Some(&card.fingerprint)),
        )
        .unwrap();
        make_card(&stranger, quiet(), &card_args("eve")).unwrap();
        let err = join(
            &stranger,
            quiet(),
            &CircleJoinArgs {
                welcome: admitted.welcome.encode(),
                fingerprint: Some(p.admin_fp.clone()),
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("does not admit"), "{err}");
    }

    #[test]
    fn admit_refuses_a_machine_that_is_not_an_admin() {
        let p = pair();
        let card = make_card(&p.joiner, quiet(), &card_args("box")).unwrap();
        let admitted = admit(
            &p.admin,
            quiet(),
            &admit_args(&card, Some(&card.fingerprint)),
        )
        .unwrap();
        join(
            &p.joiner,
            quiet(),
            &CircleJoinArgs {
                welcome: admitted.welcome.encode(),
                fingerprint: Some(p.admin_fp.clone()),
            },
        )
        .unwrap();
        let third = tempfile::tempdir().unwrap();
        let card3 = make_card(&paths(third.path()), quiet(), &card_args("c3")).unwrap();
        let err = admit(
            &p.joiner,
            quiet(),
            &admit_args(&card3, Some(&card3.fingerprint)),
        )
        .unwrap_err();
        assert!(err.to_string().contains("admin"), "{err}");
    }

    #[test]
    fn revoke_and_roles_change_the_roster() {
        let p = pair();
        let card = make_card(&p.joiner, quiet(), &card_args("box")).unwrap();
        admit(
            &p.admin,
            quiet(),
            &admit_args(&card, Some(&card.fingerprint)),
        )
        .unwrap();
        set_roles(
            &p.admin,
            quiet(),
            &CircleRolesArgs {
                name: "box".to_owned(),
                roles: "lead,helper".to_owned(),
                circle: None,
            },
        )
        .unwrap();
        let rows = list_rows(&p.admin, quiet(), &CircleSelect { circle: None }).unwrap();
        assert!(rows.iter().any(|r| r[0] == "box" && r[1] == "lead,helper"));
        revoke(
            &p.admin,
            quiet(),
            &CircleRevokeArgs {
                name: "box".to_owned(),
                yes: true,
                circle: None,
            },
        )
        .unwrap();
        let rows = list_rows(&p.admin, quiet(), &CircleSelect { circle: None }).unwrap();
        assert!(!rows.iter().any(|r| r[0] == "box"));
        // A revoked key cannot be admitted again.
        let err = admit(
            &p.admin,
            quiet(),
            &admit_args(&card, Some(&card.fingerprint)),
        )
        .unwrap_err();
        assert!(err.to_string().contains("refused"), "{err}");
    }

    #[test]
    fn init_twice_is_refused() {
        let p = pair();
        let err = init(
            &p.admin,
            quiet(),
            &CircleInitArgs {
                name: "home".to_owned(),
                machine: Some("lap".to_owned()),
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("already"), "{err}");
    }

    #[test]
    fn keys_are_private_and_changes_are_journaled() {
        let p = pair();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            for key in [node_key_path(&p.admin), admin_key_path(&p.admin)] {
                let mode = std::fs::metadata(&key).unwrap().permissions().mode();
                assert_eq!(mode & 0o077, 0, "{}", key.display());
            }
        }
        let log = std::fs::read_to_string(changelog::journal_path(&p.admin.config_dir)).unwrap();
        assert!(log.contains("ops.jsonl"), "{log}");
        assert!(!log.contains("PRIVATE KEY"));
    }

    #[test]
    fn machine_name_is_a_valid_label() {
        assert!(goway_circle::valid_name(&machine_name()));
    }
}
