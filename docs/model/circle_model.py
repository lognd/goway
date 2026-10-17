"""Executable model of the circle roster rules in docs/circles.md.

WHY: pin the authorization, convergence and revocation rules down before the
Rust crate exists. The Rust implementation must match these tests.

Signatures are modelled, not computed: a signature is the pair
(signer, sha256(op id + signer)). That checks the authorization logic, not
the cryptography (ssh-keygen -Y does that in the real implementation).
"""

from __future__ import annotations

import hashlib
import json
import logging
from dataclasses import dataclass, field
from typing import Optional

log = logging.getLogger("goway.circle_model")

MAX_OPS = 4096
CERT_TTL_DAYS = 14
ROLES = frozenset({"lead", "helper"})
# Applied first inside a causal layer, so removal wins over concurrent grants.
REMOVING = ("revoke", "remove_admin")


def _digest(text: str) -> str:
    """Return the hex SHA-256 of text."""
    return hashlib.sha256(text.encode("ascii")).hexdigest()


@dataclass(frozen=True)
class Op:
    """One signed roster operation; id covers kind, body and parents."""

    kind: str
    body: tuple
    parents: tuple
    signers: tuple
    sigs: tuple = field(default=(), compare=False)

    @property
    def id(self) -> str:
        """Content id: hash of everything except the signatures."""
        raw = json.dumps([self.kind, self.body, self.parents, self.signers])
        return _digest(raw)


def sign(kind: str, body: dict, parents: tuple, signers: tuple) -> Op:
    """Build an op and attach the modelled signatures of each signer."""
    op = Op(kind, tuple(sorted(body.items())), tuple(sorted(parents)), tuple(signers))
    sigs = tuple((s, _digest(op.id + s)) for s in signers)
    return Op(op.kind, op.body, op.parents, op.signers, sigs)


def sig_ok(op: Op) -> bool:
    """Return True when every signer has a matching signature."""
    want = {(s, _digest(op.id + s)) for s in op.signers}
    return len(op.signers) > 0 and set(op.sigs) == want


@dataclass
class State:
    """Folded roster state."""

    circle: str = ""
    admins: set = field(default_factory=set)
    members: dict = field(default_factory=dict)  # key -> {name, roles, host_fp}
    tombstones: set = field(default_factory=set)
    epoch: int = 0
    applied: set = field(default_factory=set)


def _admin_quorum(state: State, signers: tuple) -> bool:
    """Return True when signers carry enough admin signatures to change admins."""
    good = {s for s in signers if s in state.admins}
    need = 2 if len(state.admins) >= 3 else 1
    return len(good) >= need


def _apply(state: State, op: Op) -> bool:
    """Apply one op to state if valid at this point; return whether it applied."""
    if not sig_ok(op):
        log.warning("op %s rejected: bad signature", op.id[:8])
        return False
    body = dict(op.body)
    signer = op.signers[0]
    if op.kind == "genesis":
        if state.circle or body["admin"] != signer:
            return False
        state.circle = body["circle"]
        state.admins.add(signer)
        state.members[signer] = {"name": body["name"], "roles": {"lead"}, "host_fp": ""}
        return True
    if not state.circle:
        return False
    is_admin = signer in state.admins
    if op.kind == "admit":
        key = body["key"]
        roles = set(body["roles"].split(","))
        if not is_admin or key in state.tombstones or not roles <= ROLES or not roles:
            return False
        state.members[key] = {"name": body["name"], "roles": roles, "host_fp": body["host_fp"]}
        return True
    if op.kind == "revoke":
        key = body["key"]
        if not is_admin or key == signer:
            return False
        if key in state.admins and not _admin_quorum(state, op.signers):
            return False
        state.members.pop(key, None)
        state.admins.discard(key)
        state.tombstones.add(key)
        state.epoch += 1
        return True
    if op.kind == "set_roles":
        roles = set(body["roles"].split(","))
        key = body["key"]
        if not is_admin or key not in state.members or not roles <= ROLES or not roles:
            return False
        if key in state.admins and "lead" not in roles:
            return False
        state.members[key]["roles"] = roles
        return True
    if op.kind == "add_admin":
        key = body["key"]
        member = state.members.get(key)
        if not _admin_quorum(state, op.signers) or member is None or "lead" not in member["roles"]:
            return False
        state.admins.add(key)
        return True
    if op.kind == "remove_admin":
        key = body["key"]
        if not _admin_quorum(state, op.signers) or key not in state.admins or len(state.admins) < 2:
            return False
        state.admins.discard(key)
        state.epoch += 1
        return True
    if op.kind == "rotate_host_key":
        member = state.members.get(signer)
        if member is None:
            return False
        member["host_fp"] = body["host_fp"]
        return True
    log.warning("op %s rejected: unknown kind %r", op.id[:8], op.kind)
    return False


def fold(ops: set) -> State:
    """Replay ops deterministically; the result depends only on the op set."""
    state = State()
    pending = {o.id: o for o in ops if sig_ok(o)}
    if len(pending) > MAX_OPS:
        raise ValueError("too many ops")
    done: set = set()
    while pending:
        ready = [o for o in pending.values() if all(p in done for p in o.parents)]
        if not ready:
            log.warning("%d ops have missing parents and are ignored", len(pending))
            break
        ready.sort(key=lambda o: (o.kind not in REMOVING, o.id))
        for op in ready:
            _apply(state, op)
            done.add(op.id)
            del pending[op.id]
    state.applied = done
    return state


def merge(a: set, b: set) -> set:
    """Merge two op sets: union, verified at fold time."""
    return a | b


def heads(ops: set) -> set:
    """Return ids of ops that are no other op's parent."""
    parents = {p for o in ops for p in o.parents}
    return {o.id for o in ops} - parents


def accept_roster(known_heads: set, offered: set) -> bool:
    """Anti-rollback: accept only a roster containing every head seen before."""
    ids = {o.id for o in offered}
    ok = known_heads <= ids
    if not ok:
        log.error("rollback refused: offered roster lacks known heads")
    return ok


@dataclass(frozen=True)
class Cert:
    """Modelled ssh certificate: signer, subject key, principal, expiry day."""

    signer: str
    subject: str
    principal: str
    expires: int


def issue_cert(state: State, admin: str, subject: str, now: int) -> Optional[Cert]:
    """Mint a lead cert; only an admin, only for a current member with role lead."""
    member = state.members.get(subject)
    if admin not in state.admins or member is None or "lead" not in member["roles"]:
        log.info("cert refused for %s", subject)
        return None
    principal = f"goway-lead:{state.circle}:e{state.epoch}"
    return Cert(admin, subject, principal, now + CERT_TTL_DAYS)


def helper_accepts(state: State, cert: Cert, now: int) -> bool:
    """Model of a helper's authorized_keys cert-authority line for one circle."""
    return (
        cert.signer in state.admins
        and cert.principal == f"goway-lead:{state.circle}:e{state.epoch}"
        and now <= cert.expires
    )
