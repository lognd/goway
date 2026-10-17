//! Roster state and the deterministic fold over a set of operations.

use std::collections::{BTreeMap, BTreeSet};

use tracing::{debug, info, warn};

use crate::crypto::Verifier;
use crate::error::CircleError;
use crate::op::{Kind, MAX_OP_BYTES, MAX_OPS, Op};
use crate::pairing::valid_name;

/// Longest host fingerprint a roster op may carry.
const MAX_FP: usize = 128;

/// True for an empty fingerprint or a short token with no space, quote, comma or control byte.
///
/// Fingerprints reach `authorized_keys` and `known_hosts` text, so anything that could break out
/// of a quoted option or add a line is refused when the op is applied.
fn safe_fp(fp: &str) -> bool {
    fp.len() <= MAX_FP
        && fp
            .bytes()
            .all(|b| b.is_ascii_graphic() && !matches!(b, b'"' | b',' | b'\\'))
}

/// What a member may do: be used as a login source, or be sent work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    /// Runs jobs on helpers; receives an access certificate.
    Lead,
    /// Receives source trees and runs jobs.
    Helper,
}

/// One roster member, keyed by its node public key in [`State::members`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// Human label; identity is the key.
    pub name: String,
    /// Granted roles (never empty).
    pub roles: BTreeSet<Role>,
    /// Pinned sshd host-key fingerprint.
    pub host_fp: String,
}

/// Folded roster state; a pure function of the operation set.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    /// Circle name; empty until a genesis applies.
    pub circle: String,
    /// Admin node keys.
    pub admins: BTreeSet<String>,
    /// Current members by node key.
    pub members: BTreeMap<String, Member>,
    /// Revoked keys, never re-admitted.
    pub tombstones: BTreeSet<String>,
    /// Bumped on removal; invalidates outstanding certificates.
    pub epoch: u64,
    /// Ids of every operation the replay reached (applied or ignored).
    pub applied: BTreeSet<String>,
}

/// Parse a comma-separated, non-empty list of known roles.
fn parse_roles(text: &str) -> Option<BTreeSet<Role>> {
    let roles: Option<BTreeSet<Role>> = text
        .split(',')
        .map(|r| match r {
            "lead" => Some(Role::Lead),
            "helper" => Some(Role::Helper),
            _ => None,
        })
        .collect();
    roles.filter(|r| !r.is_empty())
}

impl State {
    /// True when `signers` hold enough distinct admin signatures to change admins.
    fn admin_quorum(&self, signers: &[String]) -> bool {
        let good = signers
            .iter()
            .filter(|s| self.admins.contains(*s))
            .collect::<BTreeSet<_>>();
        good.len() >= if self.admins.len() >= 3 { 2 } else { 1 }
    }

    /// Apply one signature-checked op if valid at this point; return whether it took effect.
    fn apply(&mut self, op: &Op, forced: bool) -> bool {
        let short = &op.id()[..8];
        let ok = self.apply_inner(op, forced);
        if ok {
            info!(op = short, kind = ?op.kind, "op applied");
        } else {
            debug!(op = short, kind = ?op.kind, "op ignored");
        }
        ok
    }

    /// Found the circle if none exists and the body names the signer as its admin.
    fn apply_genesis(&mut self, op: &Op, signer: &str) -> bool {
        let get = |k: &str| op.body.get(k).map(String::as_str);
        let (Some(circle), Some(admin), Some(name)) = (get("circle"), get("admin"), get("name"))
        else {
            return false;
        };
        if !self.circle.is_empty() || !valid_name(circle) || !valid_name(name) || admin != signer {
            return false;
        }
        circle.clone_into(&mut self.circle);
        self.admins.insert(signer.to_owned());
        self.members.insert(
            signer.to_owned(),
            Member {
                name: name.to_owned(),
                roles: BTreeSet::from([Role::Lead]),
                host_fp: String::new(),
            },
        );
        true
    }

    /// Add or remove an admin; needs the admin quorum.
    fn apply_admin_change(&mut self, op: &Op, forced: bool) -> bool {
        let Some(key) = op.body.get("key").map(String::as_str) else {
            return false;
        };
        if !forced && !self.admin_quorum(&op.signers) {
            return false;
        }
        if op.kind == Kind::AddAdmin {
            let is_lead = self
                .members
                .get(key)
                .is_some_and(|m| m.roles.contains(&Role::Lead));
            if is_lead {
                self.admins.insert(key.to_owned());
            }
            return is_lead;
        }
        if !self.admins.contains(key) || self.admins.len() < 2 {
            return false;
        }
        self.admins.remove(key);
        self.epoch += 1;
        true
    }

    /// `forced` skips the signer-admin and quorum checks for a removal already proven valid in
    /// its own causal past (see [`resolve`]).
    fn apply_inner(&mut self, op: &Op, forced: bool) -> bool {
        let get = |k: &str| op.body.get(k).map(String::as_str);
        let Some(signer) = op.signers.first() else {
            return false;
        };
        if op.kind == Kind::Genesis {
            return self.apply_genesis(op, signer);
        }
        if self.circle.is_empty() {
            return false;
        }
        let is_admin = forced || self.admins.contains(signer);
        match op.kind {
            Kind::Genesis => false,
            Kind::Admit => {
                let (Some(key), Some(name), Some(roles), Some(host_fp)) =
                    (get("key"), get("name"), get("roles"), get("host_fp"))
                else {
                    return false;
                };
                let Some(roles) = parse_roles(roles) else {
                    return false;
                };
                if !is_admin || self.tombstones.contains(key) {
                    return false;
                }
                if !valid_name(name) || !safe_fp(host_fp) {
                    warn!(
                        op = &op.id()[..8],
                        "admit refused: unsafe name or fingerprint"
                    );
                    return false;
                }
                self.members.insert(
                    key.to_owned(),
                    Member {
                        name: name.to_owned(),
                        roles,
                        host_fp: host_fp.to_owned(),
                    },
                );
                true
            }
            Kind::Revoke => {
                let Some(key) = get("key") else { return false };
                if !is_admin || key == signer {
                    return false;
                }
                // Revoking an admin is an admin change: it needs the same quorum.
                if !forced && self.admins.contains(key) && !self.admin_quorum(&op.signers) {
                    return false;
                }
                self.members.remove(key);
                self.admins.remove(key);
                self.tombstones.insert(key.to_owned());
                self.epoch += 1;
                true
            }
            Kind::SetRoles => {
                let (Some(key), Some(roles)) = (get("key"), get("roles")) else {
                    return false;
                };
                let Some(roles) = parse_roles(roles) else {
                    return false;
                };
                if !is_admin || self.admins.contains(key) && !roles.contains(&Role::Lead) {
                    return false;
                }
                let Some(member) = self.members.get_mut(key) else {
                    return false;
                };
                member.roles = roles;
                true
            }
            Kind::AddAdmin | Kind::RemoveAdmin => self.apply_admin_change(op, forced),
            Kind::RotateHostKey => {
                let (Some(host_fp), Some(member)) = (get("host_fp"), self.members.get_mut(signer))
                else {
                    return false;
                };
                if !safe_fp(host_fp) {
                    warn!(op = &op.id()[..8], "rotate refused: unsafe fingerprint");
                    return false;
                }
                host_fp.clone_into(&mut member.host_fp);
                true
            }
        }
    }
}

/// Replay `ops` deterministically; the result depends only on the op set.
pub fn fold(ops: &[Op], verifier: &dyn Verifier) -> Result<State, CircleError> {
    fold_pinned(ops, verifier, None)
}

/// Like [`fold`], but when `genesis_id` is given only that genesis op can found the circle.
pub fn fold_pinned(
    ops: &[Op],
    verifier: &dyn Verifier,
    genesis_id: Option<&str>,
) -> Result<State, CircleError> {
    if ops.len() > MAX_OPS {
        return Err(CircleError::TooManyOps {
            count: ops.len(),
            max: MAX_OPS,
        });
    }
    let mut pending: BTreeMap<String, &Op> = BTreeMap::new();
    for op in ops {
        let id = op.id();
        if pending.contains_key(&id) {
            continue;
        }
        if op.encoded_len() > MAX_OP_BYTES {
            warn!(op = &id[..8], "op rejected: too large");
        } else if !op.sig_ok(verifier) {
            warn!(op = &id[..8], "op rejected: bad signature");
        } else if op.kind == Kind::Genesis && genesis_id.is_some_and(|g| g != id) {
            warn!(op = &id[..8], "op rejected: not the pinned genesis");
        } else {
            pending.insert(id, op);
        }
    }
    let (excluded, forced) = resolve(&pending);
    Ok(replay(&pending, &excluded, &forced))
}

/// What a removal proved about itself when judged against its own causal past.
struct Verdict {
    /// Key the removal takes out.
    target: String,
    /// True when the target was an admin in that past (an admin-versus-admin removal).
    admin_target: bool,
    /// Strict ancestors of the removal.
    anc: BTreeSet<String>,
}

/// Strict ancestors of `id`, or `None` when any of them has a parent that is not in `ops`.
fn ancestors(ops: &BTreeMap<String, &Op>, id: &str) -> Option<BTreeSet<String>> {
    let mut seen = BTreeSet::new();
    let mut stack: Vec<&String> = ops.get(id)?.parents.iter().collect();
    while let Some(p) = stack.pop() {
        if seen.insert(p.clone()) {
            stack.extend(&ops.get(p)?.parents);
        }
    }
    Some(seen)
}

/// Ops to skip and removals to force, derived from the removals already judged in `verdicts`.
///
/// A key that a valid removal takes out loses every op it signed that does not causally precede
/// that removal, so it cannot backdate its parents to act before its own revocation. Removals of
/// admins by admins stay in force even when the other side is removed too (fail closed).
fn derive(
    ops: &BTreeMap<String, &Op>,
    verdicts: &BTreeMap<String, Verdict>,
) -> (BTreeSet<String>, BTreeSet<String>) {
    let forced: BTreeSet<String> = verdicts
        .iter()
        .filter(|(id, v)| v.admin_target && ops.contains_key(*id))
        .map(|(id, _)| id.clone())
        .collect();
    let mut excluded = BTreeSet::new();
    for (rid, v) in verdicts.iter().filter(|(id, _)| ops.contains_key(*id)) {
        for (id, op) in ops {
            if id != rid
                && op.kind != Kind::Genesis
                && !v.anc.contains(id)
                && !forced.contains(id)
                && op.signers.contains(&v.target)
            {
                excluded.insert(id.clone());
            }
        }
    }
    (excluded, forced)
}

/// Judge every removal against the state of its own causal past, smallest past first.
fn resolve(ops: &BTreeMap<String, &Op>) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut removals: Vec<(String, BTreeSet<String>)> = ops
        .iter()
        .filter(|(_, o)| o.kind.is_removing())
        .filter_map(|(id, _)| ancestors(ops, id).map(|a| (id.clone(), a)))
        .collect();
    removals.sort_by(|(ia, a), (ib, b)| (a.len(), ia).cmp(&(b.len(), ib)));
    let mut verdicts: BTreeMap<String, Verdict> = BTreeMap::new();
    for (id, anc) in removals {
        let op = ops[&id];
        let Some(target) = op.body.get("key") else {
            continue;
        };
        let past: BTreeMap<String, &Op> = anc.iter().map(|a| (a.clone(), ops[a])).collect();
        let (excluded, forced) = derive(&past, &verdicts);
        let mut state = replay(&past, &excluded, &forced);
        let admin_target = state.admins.contains(target);
        if state.apply(op, false) {
            debug!(op = &id[..8], "removal valid in its causal past");
            verdicts.insert(
                id,
                Verdict {
                    target: target.clone(),
                    admin_target,
                    anc,
                },
            );
        }
    }
    derive(ops, &verdicts)
}

/// Kahn-layer replay of `ops`, skipping `excluded` ops and forcing the `forced` removals.
fn replay(
    ops: &BTreeMap<String, &Op>,
    excluded: &BTreeSet<String>,
    forced: &BTreeSet<String>,
) -> State {
    let mut pending: BTreeMap<String, &Op> = ops.clone();
    let mut state = State::default();
    let mut done: BTreeSet<String> = BTreeSet::new();
    while !pending.is_empty() {
        let mut ready: Vec<(&String, &Op)> = pending
            .iter()
            .filter(|(_, o)| o.parents.iter().all(|p| done.contains(p)))
            .map(|(id, o)| (id, *o))
            .collect();
        if ready.is_empty() {
            warn!(
                count = pending.len(),
                "ops have missing parents and are ignored"
            );
            break;
        }
        ready.sort_by(|(ia, a), (ib, b)| {
            (!a.kind.is_removing(), ia).cmp(&(!b.kind.is_removing(), ib))
        });
        let ids: Vec<(String, &Op)> = ready.into_iter().map(|(i, o)| (i.clone(), o)).collect();
        for (id, op) in ids {
            if excluded.contains(&id) {
                debug!(
                    op = &id[..8],
                    "op excluded: signed by a key removed earlier"
                );
            } else {
                state.apply(op, forced.contains(&id));
            }
            pending.remove(&id);
            done.insert(id);
        }
    }
    state.applied = done;
    state
}

/// Merge two op sets: union by id, verified at fold time.
pub fn merge(a: &[Op], b: &[Op]) -> Vec<Op> {
    let mut seen = BTreeSet::new();
    a.iter()
        .chain(b)
        .filter(|o| seen.insert(o.id()))
        .cloned()
        .collect()
}

/// Ids of ops that are no other op's parent.
pub fn heads(ops: &[Op]) -> BTreeSet<String> {
    let parents: BTreeSet<&String> = ops.iter().flat_map(|o| &o.parents).collect();
    ops.iter()
        .map(Op::id)
        .filter(|id| !parents.contains(id))
        .collect()
}

/// Anti-rollback: accept only a roster containing every head seen before.
pub fn accept_roster(known_heads: &BTreeSet<String>, offered: &[Op]) -> Result<(), CircleError> {
    let ids: BTreeSet<String> = offered.iter().map(Op::id).collect();
    let missing: Vec<String> = known_heads.difference(&ids).cloned().collect();
    if missing.is_empty() {
        Ok(())
    } else {
        warn!(
            missing = missing.len(),
            "rollback refused: offered roster lacks known heads"
        );
        Err(CircleError::Rollback { missing })
    }
}
