//! `goway circle`: pair machines into a circle with two pasted texts.
//!
//! The new machine prints a card (`gwc1.`), an admin admits it and prints a welcome
//! (`gww1.`), the new machine joins from the welcome. Only public keys travel. The root of trust
//! is a fingerprint a person compares on both screens: the card's node-key fingerprint when
//! admitting, an admin-key fingerprint when joining. Nothing here logs or prints a private key.

use std::collections::BTreeSet;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use std::path::{Path, PathBuf};

use goway_circle::{
    Card, FixedPassphrase, Kind, Op, Passphrase, PassphraseSource, Role, SecretToolKeystore,
    Signer, SshKeygenSigner, SshKeygenVerifier, State, Store, TtyPrompt, Welcome,
    KeystorePassphrase, ensure_keypair_with, fold, fold_pinned, key_fingerprint, key_is_encrypted,
    read_public_key, valid_roles,
};

use crate::changelog;
use crate::cli::{
    CircleAdmitArgs, CircleCardArgs, CircleCommand, CircleInitArgs, CircleJoinArgs,
    CircleRevokeArgs, CircleRolesArgs, CircleSelect, CircleSyncArgs,
};
use crate::error::{Error, Result};
use crate::paths::Paths;
use crate::render::{self, Renderer};

/// Where sshd's ed25519 host key lives on a Linux helper.
const SSHD_HOST_KEY_PUB: &str = "/etc/ssh/ssh_host_ed25519_key.pub";

/// The directory that holds this machine's circle keys and circles.
pub(crate) fn circle_dir(paths: &Paths) -> PathBuf {
    paths.config_dir.join("circle")
}

/// The directory holding one directory per circle.
pub(crate) fn circles_dir(paths: &Paths) -> PathBuf {
    circle_dir(paths).join("circles")
}

pub(crate) fn node_key_path(paths: &Paths) -> PathBuf {
    circle_dir(paths).join("node_ed25519")
}

/// True when a key is set up at `path`: a private file, or just a `.pub` for an agent or FIDO key.
pub(crate) fn has_key(path: &Path) -> bool {
    path.is_file() || pub_of(path).is_file()
}

pub(crate) fn admin_key_path(paths: &Paths) -> PathBuf {
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
pub(crate) fn key_pair(path: &Path, renderer: Renderer) -> Result<goway_circle::KeyPair> {
    key_pair_with(path, renderer, None)
}

/// Like [`key_pair`], but a key made now is encrypted with `pass` when one is given.
pub(crate) fn key_pair_with(
    path: &Path,
    renderer: Renderer,
    pass: Option<&Passphrase>,
) -> Result<goway_circle::KeyPair> {
    let (pair, created) = ensure_keypair_with(
        path,
        &|p| {
            crate::sshsetup::restrict_key_acl(p, renderer);
        },
        pass,
    )?;
    if created {
        tracing::info!(key = %path.display(), "circle key created");
        renderer.note(format!("made a new key at {}", path.display()));
    }
    Ok(pair)
}

/// A signer for `pair`; an encrypted key gets its passphrase from the OS keystore, else a prompt.
pub(crate) fn signer_for(pair: &goway_circle::KeyPair) -> Result<SshKeygenSigner> {
    let account = pair.private.display().to_string();
    signer_with(pair, &KeystorePassphrase::new(&SecretToolKeystore, TtyPrompt, account))
}

/// A signer for `pair` that asks `source` for the passphrase only when the key is encrypted.
pub(crate) fn signer_with(
    pair: &goway_circle::KeyPair,
    source: &dyn PassphraseSource,
) -> Result<SshKeygenSigner> {
    let signer = SshKeygenSigner::new(pair.private.clone(), pair.public.clone())?;
    if !key_is_encrypted(&pair.private) {
        return Ok(signer);
    }
    let prompt = format!("Passphrase for {}: ", pair.private.display());
    tracing::info!(key = %pair.private.display(), "key is encrypted; asking for its passphrase");
    Ok(signer.with_passphrase(source.passphrase(&prompt)?))
}

/// This machine's name as a valid circle label.
pub(crate) fn machine_name() -> String {
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

pub(crate) fn fingerprint_of(key: &str) -> String {
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

pub(crate) fn usage(message: impl Into<String>) -> Error {
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
pub(crate) fn load(paths: &Paths, select: Option<&str>) -> Result<(Store, Vec<Op>, State)> {
    let store = Store::find(&circles_dir(paths), select)?;
    let ops = store.ops()?;
    let state = fold(&ops, &SshKeygenVerifier)?;
    Ok((store, ops, state))
}

/// Append `op` to `ops` and commit when `applied` holds on the resulting state.
pub(crate) fn append_checked(
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
    let pass = if args.passphrase {
        Some(TtyPrompt.passphrase("New passphrase for the circle keys: ")?)
    } else {
        None
    };
    let admin = key_pair_with(&admin_key_path(paths), renderer, pass.as_ref())?;
    let node = key_pair_with(&node_key_path(paths), renderer, pass.as_ref())?;
    let machine = args.machine.clone().unwrap_or_else(machine_name);
    let admin_name = format!("{machine}-admin");
    if !goway_circle::valid_name(&admin_name) {
        return Err(usage(format!("`{machine}` is too long for a machine name")));
    }
    let admin_signer = match &pass {
        Some(p) => signer_with(&admin, &FixedPassphrase(p.clone()))?,
        None => signer_for(&admin)?,
    };
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
    if !has_key(&admin_path) {
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
    join_welcome(paths, renderer, &welcome, &|admin_fps| {
        confirm(
            renderer,
            args.fingerprint.as_deref(),
            admin_fps,
            "admin key fingerprint",
        )
    })
}

/// Verify `welcome` for this machine, ask `accept` to approve the admin fingerprints, then store.
pub(crate) fn join_welcome(
    paths: &Paths,
    renderer: Renderer,
    welcome: &Welcome,
    accept: &dyn Fn(&[String]) -> Result<()>,
) -> Result<JoinOut> {
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
    accept(&admin_fps)?;
    let store = Store::for_genesis(&circles_dir(paths), &welcome.genesis);
    store.commit(&welcome.ops, &sink(paths))?;
    tracing::info!(circle = store.id(), "circle joined");
    Ok(JoinOut {
        id: store.id().to_owned(),
        members: state.members.len(),
    })
}

/// Fingerprint of this machine's admin key, when it is an admin of the selected circle.
pub(crate) fn admin_ident(
    paths: &Paths,
    renderer: Renderer,
    circle: Option<&str>,
) -> Result<String> {
    let (_, _, state) = load(paths, circle)?;
    let admin_path = admin_key_path(paths);
    if !has_key(&admin_path) {
        return Err(usage(
            "this machine has no admin key; pair from an admin machine",
        ));
    }
    let admin = key_pair(&admin_path, renderer)?;
    if !state.admins.contains(&admin.public) {
        return Err(usage(
            "this machine's admin key is not an admin of that circle",
        ));
    }
    Ok(fingerprint_of(&admin.public))
}

/// Find the one member called `name`.
pub(crate) fn member_key(state: &State, name: &str) -> Result<String> {
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
    if !has_key(&admin_path) {
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

/// Remove `args.name` from the circle for good; with `--now`, also push the new epoch to helpers.
pub fn revoke(paths: &Paths, renderer: Renderer, args: &CircleRevokeArgs) -> Result<()> {
    revoke_with(
        paths,
        renderer,
        args,
        &crate::circle_push::SshPusher {
            paths: paths.clone(),
        },
    )
}

/// [`revoke`] with the helper-reaching step injected (tests pass a fake).
pub fn revoke_with(
    paths: &Paths,
    renderer: Renderer,
    args: &CircleRevokeArgs,
    pusher: &dyn crate::circle_push::Pusher,
) -> Result<()> {
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
    if args.now {
        push_epoch(paths, renderer, args.circle.as_deref(), pusher)?;
    } else {
        renderer.next("pass --now next time, or run `goway circle access` on each helper, so old certificates stop working");
    }
    Ok(())
}

/// Send the roster to every other helper, ask it to refresh its access lines, and report who could not be reached.
fn push_epoch(
    paths: &Paths,
    renderer: Renderer,
    circle: Option<&str>,
    pusher: &dyn crate::circle_push::Pusher,
) -> Result<()> {
    let (store, ops, state) = load(paths, circle)?;
    let me = read_public_key(&pub_of(&node_key_path(paths))).ok();
    let hosts = crate::circle_push::helper_names(&state, me.as_deref());
    let text: String = ops.iter().map(|o| format!("{}\n", o.to_line())).collect();
    let offer = crate::circle_push::RosterOffer::new(store.id(), &text);
    let report = crate::circle_push::push_all(&hosts, &offer, pusher);
    crate::circle_push::record_seen(paths, store.id(), &report.reached, now_secs())?;
    crate::circle_push::record_reported(paths, store.id(), &report.expiries)?;
    tracing::info!(
        circle = store.id(),
        epoch = state.epoch,
        reached = report.reached.len(),
        unreachable = report.unreachable.len(),
        "epoch pushed"
    );
    renderer.ok(format!(
        "epoch {} pushed to {} helper(s)",
        state.epoch,
        report.reached.len()
    ));
    for (name, why) in &report.unreachable {
        renderer.warn(format_args!(
            "`{name}` was not reached ({why}); it stays exposed until it runs `goway circle access`"
        ));
    }
    Ok(())
}

/// The offered roster text: `arg` itself, or all of `stdin` when `arg` is the stdin mark.
fn offered_text(arg: &str, stdin: &mut dyn std::io::Read) -> Result<String> {
    if arg != crate::circle_push::STDIN_MARK {
        return Ok(arg.to_owned());
    }
    let mut text = String::new();
    stdin
        .read_to_string(&mut text)
        .map_err(|e| usage(format!("could not read the roster from stdin: {e}")))?;
    tracing::debug!(bytes = text.len(), "read the offered roster from stdin");
    Ok(text)
}

/// `goway circle sync`: merge an offered roster (url-safe base64 `ops.jsonl`) into the stored one.
///
/// The merge is a union by op id, folded and verified, and the store refuses a result that
/// lacks a head it already saw, so a stale offer can never roll this machine back.
pub fn sync_roster(paths: &Paths, renderer: Renderer, args: &CircleSyncArgs) -> Result<()> {
    let ops = offered_text(&args.ops, &mut std::io::stdin().lock())?;
    let bytes = URL_SAFE_NO_PAD
        .decode(ops.trim().as_bytes())
        .map_err(|e| usage(format!("the offered roster is not valid base64: {e}")))?;
    let text = String::from_utf8(bytes).map_err(|_| usage("the offered roster is not text"))?;
    let offered = goway_circle::parse_ops(&text)?;
    let (store, ops, _) = load(paths, args.circle.as_deref())?;
    let merged = goway_circle::merge(&ops, &offered);
    let state = fold(&merged, &SshKeygenVerifier)?;
    if merged.len() == ops.len() {
        tracing::debug!(circle = store.id(), "offered roster adds nothing");
    } else {
        store.commit(&merged, &sink(paths))?;
        tracing::info!(
            circle = store.id(),
            added = merged.len() - ops.len(),
            epoch = state.epoch,
            "roster adopted from a push"
        );
    }
    renderer.ok(format!(
        "roster at epoch {} ({} operations)",
        state.epoch,
        merged.len()
    ));
    Ok(())
}

/// Seconds since the unix epoch (0 if the clock is before it).
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
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
        [
            "name",
            "roles",
            "fingerprint",
            "host key",
            "cert expires",
            "last seen",
            "notes",
        ]
        .map(str::to_owned)
        .to_vec(),
    ];
    let seen = crate::circle_push::last_seen(paths, store.id());
    let now = now_secs();
    let own_expiry = own_cert_expiry(paths);
    let issued = crate::circle_push::issued_expiry(paths, store.id());
    let reported = crate::circle_push::reported_expiry(paths, store.id());
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
            if own.as_deref() == Some(key.as_str()) {
                own_expiry.map_or_else(|| "none".to_owned(), |t| format!("unix {t}"))
            } else {
                match (issued.get(&m.name), reported.get(&m.name)) {
                    (Some(i), Some(r)) if r > i => format!("unix {r} (reported)"),
                    (Some(i), _) => format!("unix {i} (issued here)"),
                    (None, Some(r)) => format!("unix {r} (reported)"),
                    (None, None) => "-".to_owned(),
                }
            },
            if own.as_deref() == Some(key.as_str()) {
                "now".to_owned()
            } else {
                crate::circle_push::ago(seen.get(&m.name).copied(), now)
            },
            notes.join(", "),
        ]);
    }
    tracing::debug!(circle = store.id(), epoch = state.epoch, "listed members");
    Ok(rows)
}

/// Expiry (unix seconds) of this machine's lead certificate, if one is written and readable.
pub(crate) fn own_cert_expiry(paths: &Paths) -> Option<u64> {
    let mut file = node_key_path(paths).into_os_string();
    file.push("-cert.pub");
    let text = std::fs::read_to_string(PathBuf::from(file)).ok()?;
    goway_circle::parse_cert(&text).ok().map(|c| c.valid_before)
}

pub(crate) fn pub_of(private: &Path) -> PathBuf {
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
            let encoded = out.card.encode();
            renderer.line(&encoded);
            if args.qr {
                crate::lanpair::print_qr(renderer, &encoded)?;
            }
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
        CircleCommand::PairHost(args) => crate::lanpair::pair_host_cli(paths, renderer, args)?,
        CircleCommand::PairJoin(args) => crate::lanpair::pair_join_cli(paths, renderer, args)?,
        CircleCommand::List(select) => {
            renderer.table(&list_rows(paths, renderer, select)?);
        }
        CircleCommand::Sync(args) => sync_roster(paths, renderer, args)?,
        CircleCommand::Access(args) => crate::access::run_access(paths, renderer, args)?,
        CircleCommand::Cert(args) => crate::access::run_cert(paths, renderer, args)?,
        CircleCommand::Pin(args) => crate::access::run_pin(paths, renderer, args)?,
        CircleCommand::Revoke(args) => revoke(paths, renderer, args)?,
        CircleCommand::Roles(args) => set_roles(paths, renderer, args)?,
        CircleCommand::Admin(cmd) => crate::circle_admin::run_admin(paths, renderer, cmd)?,
        CircleCommand::Renew(args) => crate::access::run_renew(paths, renderer, args)?,
        CircleCommand::Policy(args) => crate::access::run_policy(paths, renderer, args)?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::ColorWhen;

    // frob:tests crates/goway/src/circle.rs::has_key
    #[test]
    fn a_public_only_key_counts_as_set_up() {
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("admin");
        assert!(!has_key(&key));
        std::fs::write(pub_of(&key), "ssh-ed25519 AAAA\n").unwrap();
        assert!(has_key(&key));
    }

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
            qr: false,
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
                passphrase: false,
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

    // frob:tests crates/goway/src/circle.rs::run
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
                .any(|r| r[0] == "box" && r[6].contains("this machine"))
        );
        assert!(
            rows.iter()
                .any(|r| r[0] == "lap-admin" && r[6].contains("admin"))
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
                now: false,
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
                passphrase: false,
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

    // frob:tests crates/goway/src/circle.rs::offered_text
    #[test]
    fn the_stdin_mark_reads_the_roster_from_stdin() {
        let mut input: &[u8] = b"abc";
        assert_eq!(offered_text("-", &mut input).unwrap(), "abc");
        assert_eq!(offered_text("xyz", &mut std::io::empty()).unwrap(), "xyz");
    }

    // frob:tests crates/goway/src/circle.rs::signer_with
    #[test]
    fn an_encrypted_key_signs_only_with_its_passphrase() {
        let dir = tempfile::tempdir().unwrap();
        let pass = Passphrase::new("correct horse battery".into()).unwrap();
        let key = dir.path().join("k");
        let Ok((pair, _)) = ensure_keypair_with(&key, &|_| {}, Some(&pass)) else {
            return; // no ssh-keygen here
        };
        let signer = signer_with(&pair, &FixedPassphrase(pass)).unwrap();
        assert!(signer.sign(b"hello").is_ok());
        let wrong = Passphrase::new("not the one".into()).unwrap();
        let bad = signer_with(&pair, &FixedPassphrase(wrong)).unwrap();
        assert!(bad.sign(b"hello").is_err());
    }

    // frob:tests crates/goway/src/circle.rs::signer_with
    #[test]
    fn a_plain_key_never_asks_for_a_passphrase() {
        struct Never;
        impl PassphraseSource for Never {
            fn passphrase(&self, _p: &str) -> std::result::Result<Passphrase, goway_circle::CryptoError> {
                panic!("a plain key must not prompt");
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let Ok((pair, _)) = ensure_keypair_with(&dir.path().join("k"), &|_| {}, None) else {
            return;
        };
        assert!(signer_with(&pair, &Never).is_ok());
    }

    #[test]
    fn machine_name_is_a_valid_label() {
        assert!(goway_circle::valid_name(&machine_name()));
    }
}
