//! `goway circle admin`: change the set of admins, with a second signature once there are three.
//!
//! With one or two admins a single admin applies the change at once. With three or more the
//! roster needs two distinct admin signatures, and signers are part of an operation's id, so the
//! proposer names the second admin up front, writes a half-signed proposal (`--out`), and that
//! admin finishes it with `admin cosign FILE` on their machine.

use std::path::Path;

use goway_circle::{Card, Kind, Op, Signer, State, generate_sk_keypair, parse_ops};

use crate::changelog;
use crate::circle::{
    admin_key_path, append_checked, fingerprint_of, has_key, key_pair, load, machine_name,
    member_key, signer_for, usage,
};
use crate::cli::{
    CircleAdminCardArgs, CircleAdminChangeArgs, CircleAdminCommand, CircleCosignArgs,
};
use crate::error::{Error, Result};
use crate::paths::Paths;
use crate::render::Renderer;

/// Admin signatures a change of admins needs in `state`.
fn quorum(state: &State) -> usize {
    if state.admins.len() >= 3 { 2 } else { 1 }
}

/// This machine's admin key pair, checked to be an admin of `state`.
fn own_admin(
    paths: &Paths,
    renderer: Renderer,
    state: &State,
) -> Result<(goway_circle::KeyPair, goway_circle::SshKeygenSigner)> {
    let path = admin_key_path(paths);
    if !has_key(&path) {
        return Err(usage(
            "this machine has no admin key; run this on an admin machine",
        ));
    }
    let pair = key_pair(&path, renderer)?;
    if !state.admins.contains(&pair.public) {
        return Err(usage(
            "this machine's admin key is not an admin of that circle",
        ));
    }
    let signer = signer_for(&pair)?;
    Ok((pair, signer))
}

/// A card for this machine's admin key, named `<machine>-admin`, to be admitted as a lead.
pub fn admin_card(
    paths: &Paths,
    renderer: Renderer,
    args: &CircleAdminCardArgs,
) -> Result<(Card, String)> {
    let admin = key_pair(&admin_key_path(paths), renderer)?;
    let name = format!("{}-admin", args.name.clone().unwrap_or_else(machine_name));
    let card = Card {
        v: 1,
        key: admin.public.clone(),
        roles: "lead".to_owned(),
        name,
        host_fp: String::new(),
    };
    card.validate()?;
    tracing::info!(name = %card.name, "admin card made");
    let fp = fingerprint_of(&card.key);
    Ok((card, fp))
}

/// Make this machine's `ed25519-sk` admin key by running `program` (`ssh-keygen`); never overwrites.
// frob:ticket 01M54VV1JY0C7FDCHM9A8Q2EJJ
pub fn keygen(
    paths: &Paths,
    renderer: Renderer,
    program: &Path,
) -> Result<(goway_circle::KeyPair, String)> {
    let path = admin_key_path(paths);
    let pair = generate_sk_keypair(program, &path, &|p| {
        crate::sshsetup::restrict_key_acl(p, renderer);
    })?;
    tracing::info!(key = %path.display(), "hardware admin key made");
    let fp = fingerprint_of(&pair.public);
    Ok((pair, fp))
}

/// What a proposal did: applied to the roster, or written half-signed to a file.
#[derive(Debug, PartialEq, Eq)]
pub enum Proposed {
    /// The roster now includes the change.
    Applied,
    /// The proposal needs the second admin; this is the one-line op to hand over.
    NeedsCosign(String),
}

/// Promote (`kind` `AddAdmin`) or demote (`RemoveAdmin`) member `args.name`.
pub fn propose(
    paths: &Paths,
    renderer: Renderer,
    kind: Kind,
    args: &CircleAdminChangeArgs,
) -> Result<Proposed> {
    let (store, ops, state) = load(paths, args.circle.as_deref())?;
    let (me, signer) = own_admin(paths, renderer, &state)?;
    let key = member_key(&state, &args.name)?;
    let adding = kind == Kind::AddAdmin;
    if adding == state.admins.contains(&key) {
        return Err(usage(if adding {
            format!("`{}` is already an admin", args.name)
        } else {
            format!("`{}` is not an admin", args.name)
        }));
    }
    let parents = goway_circle::heads(&ops).into_iter().collect::<Vec<_>>();
    let body = [("key", key.as_str())];
    let applied = |s: &State| s.admins.contains(&key) == adding;
    let refusal = "the roster refused this admin change (the member must be a lead; the last admin cannot be removed)";
    if quorum(&state) == 1 {
        let op = Op::signed(kind, &body, &parents, &[&signer as &dyn Signer])?;
        append_checked(paths, &store, ops, op, applied, refusal)?;
        tracing::info!(circle = store.id(), member = %args.name, ?kind, "admin set changed");
        return Ok(Proposed::Applied);
    }
    let Some(cosigner) = &args.cosigner else {
        return Err(usage(
            "with 3 or more admins a second admin must sign: pass --cosigner NAME and --out FILE",
        ));
    };
    let other = member_key(&state, cosigner)?;
    if other == me.public || !state.admins.contains(&other) {
        return Err(usage(format!("`{cosigner}` is not another admin")));
    }
    let mut op = Op::draft(kind, &body, &parents, &[me.public.clone(), other]);
    op.cosign(&signer)?;
    tracing::info!(circle = store.id(), member = %args.name, ?kind, cosigner = %cosigner, "proposal half-signed");
    Ok(Proposed::NeedsCosign(op.to_line()))
}

/// Finish the half-signed proposal in `args.file` with this admin's signature and apply it.
pub fn cosign(paths: &Paths, renderer: Renderer, args: &CircleCosignArgs) -> Result<()> {
    let (store, ops, state) = load(paths, args.circle.as_deref())?;
    let (_, signer) = own_admin(paths, renderer, &state)?;
    let text = read_proposal(&args.file)?;
    let mut parsed = parse_ops(&text)?;
    let (Some(mut op), None) = (parsed.pop(), parsed.first()) else {
        return Err(usage("the proposal file must hold exactly one operation"));
    };
    if !matches!(op.kind, Kind::AddAdmin | Kind::RemoveAdmin) {
        return Err(usage("only admin changes can be cosigned"));
    }
    if !op.cosign(&signer)? {
        return Err(usage("this admin is not a pending signer of that proposal"));
    }
    let key = op.body.get("key").cloned().unwrap_or_default();
    let adding = op.kind == Kind::AddAdmin;
    append_checked(
        paths,
        &store,
        ops,
        op,
        |s| s.admins.contains(&key) == adding,
        "the roster refused this proposal (stale, wrongly signed or not allowed)",
    )?;
    tracing::info!(circle = store.id(), adding, "proposal cosigned and applied");
    renderer.ok("admin change applied");
    Ok(())
}

fn read_proposal(file: &Path) -> Result<String> {
    std::fs::read_to_string(file).map_err(|e| Error::io("read", file, e))
}

/// Run one `goway circle admin` verb.
pub fn run_admin(paths: &Paths, renderer: Renderer, command: &CircleAdminCommand) -> Result<()> {
    match command {
        CircleAdminCommand::Card(args) => {
            let (card, fp) = admin_card(paths, renderer, args)?;
            renderer.line(card.encode());
            renderer.note(format!("admin key fingerprint: {fp}"));
            renderer.next("paste it into `goway circle admit` on an admin, then `goway circle admin add NAME`");
        }
        CircleAdminCommand::Keygen => {
            let (pair, fp) = keygen(paths, renderer, Path::new("ssh-keygen"))?;
            renderer.ok(format!("admin key made at {}", pair.private.display()));
            renderer.note(format!("admin key fingerprint: {fp}"));
            renderer.next("run `goway circle admin card` to get its card");
        }
        CircleAdminCommand::Add(args) => report(paths, renderer, Kind::AddAdmin, args)?,
        CircleAdminCommand::Remove(args) => report(paths, renderer, Kind::RemoveAdmin, args)?,
        CircleAdminCommand::Cosign(args) => cosign(paths, renderer, args)?,
    }
    Ok(())
}

/// Propose and tell the user what happened.
fn report(
    paths: &Paths,
    renderer: Renderer,
    kind: Kind,
    args: &CircleAdminChangeArgs,
) -> Result<()> {
    match propose(paths, renderer, kind, args)? {
        Proposed::Applied => renderer.ok(format!("admin change for `{}` applied", args.name)),
        Proposed::NeedsCosign(line) => {
            if let Some(out) = &args.out {
                changelog::write_file_in(&paths.config_dir, out, format!("{line}\n").as_bytes())?;
                renderer.ok(format!("proposal written to {}", out.display()));
            } else {
                renderer.line(line);
            }
            renderer.next("on the second admin run `goway circle admin cosign FILE`");
        }
    }
    Ok(())
}
