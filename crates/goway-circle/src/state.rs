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
    fn apply(&mut self, op: &Op) -> bool {
        let short = &op.id()[..8];
        let ok = self.apply_inner(op);
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
    fn apply_admin_change(&mut self, op: &Op) -> bool {
        let Some(key) = op.body.get("key").map(String::as_str) else {
            return false;
        };
        if !self.admin_quorum(&op.signers) {
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

    fn apply_inner(&mut self, op: &Op) -> bool {
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
        let is_admin = self.admins.contains(signer);
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
                if self.admins.contains(key) && !self.admin_quorum(&op.signers) {
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
            Kind::AddAdmin | Kind::RemoveAdmin => self.apply_admin_change(op),
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
            state.apply(op);
            pending.remove(&id);
            done.insert(id);
        }
    }
    state.applied = done;
    Ok(state)
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
