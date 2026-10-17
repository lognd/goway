"""Tests for docs/model/circle_model.py: the rules the Rust crate must match."""

from __future__ import annotations

import itertools
import logging
import random
import unittest

import circle_model as m

log = logging.getLogger("goway.circle_model.test")


def genesis(admin: str = "A") -> m.Op:
    """Return a genesis op founding circle 'home' with one admin."""
    return m.sign("genesis", {"circle": "home", "admin": admin, "name": "a"}, (), (admin,))


def admit(signer: str, key: str, roles: str, parents: tuple, name: str = "") -> m.Op:
    """Return an admit op."""
    body = {"key": key, "name": name or key, "roles": roles, "host_fp": "fp-" + key}
    return m.sign("admit", body, parents, (signer,))


class RosterTests(unittest.TestCase):
    """Authorization, convergence, revocation and rollback."""

    def chain(self):
        """Genesis then admit lead L and helper H; return ops and ids."""
        g = genesis()
        l = admit("A", "L", "lead", (g.id,))
        h = admit("A", "H", "helper", (l.id,))
        return g, l, h

    def test_admit_by_admin_applies(self):
        g, l, h = self.chain()
        s = m.fold({g, l, h})
        self.assertEqual(set(s.members), {"A", "L", "H"})
        self.assertEqual(s.members["H"]["roles"], {"helper"})

    def test_admit_by_non_admin_is_ignored(self):
        g, l, _ = self.chain()
        bad = admit("L", "X", "lead", (l.id,))
        self.assertNotIn("X", m.fold({g, l, bad}).members)

    def test_forged_signature_is_ignored(self):
        g, l, _ = self.chain()
        good = admit("A", "X", "lead", (l.id,))
        forged = m.Op(good.kind, good.body, good.parents, good.signers, (("A", "00"),))
        self.assertNotIn("X", m.fold({g, l, forged}).members)

    def test_unknown_roles_are_ignored(self):
        g = genesis()
        self.assertNotIn("X", m.fold({g, admit("A", "X", "root", (g.id,))}).members)

    def test_order_independence(self):
        g, l, h = self.chain()
        r = m.sign("revoke", {"key": "L"}, (h.id,), ("A",))
        ops = [g, l, h, r]
        want = m.fold(set(ops))
        for perm in itertools.permutations(ops):
            got = m.fold(set(perm))
            self.assertEqual(got.members, want.members)
            self.assertEqual(got.tombstones, want.tombstones)
            self.assertEqual(got.epoch, want.epoch)

    def test_revoked_key_cannot_be_readmitted(self):
        g, l, h = self.chain()
        r = m.sign("revoke", {"key": "L"}, (h.id,), ("A",))
        again = admit("A", "L", "lead", (r.id,))
        s = m.fold({g, l, h, r, again})
        self.assertNotIn("L", s.members)
        self.assertIn("L", s.tombstones)

    def test_concurrent_revoke_beats_admit_of_same_key(self):
        g, l, h = self.chain()
        r = m.sign("revoke", {"key": "H"}, (h.id,), ("A",))
        a = admit("A", "H", "helper", (h.id,), name="again")
        s = m.fold({g, l, h, r, a})
        self.assertNotIn("H", s.members)

    def test_revoke_bumps_epoch_and_kills_old_cert(self):
        g, l, h = self.chain()
        before = m.fold({g, l, h})
        cert = m.issue_cert(before, "A", "L", now=0)
        self.assertTrue(m.helper_accepts(before, cert, now=1))
        r = m.sign("revoke", {"key": "H"}, (h.id,), ("A",))
        after = m.fold({g, l, h, r})
        self.assertFalse(m.helper_accepts(after, cert, now=1))
        fresh = m.issue_cert(after, "A", "L", now=0)
        self.assertTrue(m.helper_accepts(after, fresh, now=1))

    def test_cert_expires(self):
        g, l, _ = self.chain()
        s = m.fold({g, l})
        cert = m.issue_cert(s, "A", "L", now=0)
        self.assertTrue(m.helper_accepts(s, cert, now=m.CERT_TTL_DAYS))
        self.assertFalse(m.helper_accepts(s, cert, now=m.CERT_TTL_DAYS + 1))

    def test_helper_only_never_gets_a_lead_cert(self):
        g, l, h = self.chain()
        s = m.fold({g, l, h})
        self.assertIsNone(m.issue_cert(s, "A", "H", now=0))

    def test_non_admin_cannot_issue_certs(self):
        g, l, _ = self.chain()
        s = m.fold({g, l})
        self.assertIsNone(m.issue_cert(s, "L", "L", now=0))

    def test_cert_from_removed_admin_is_rejected(self):
        g, l, _ = self.chain()
        add = m.sign("add_admin", {"key": "L"}, (l.id,), ("A",))
        s = m.fold({g, l, add})
        cert = m.issue_cert(s, "L", "L", now=0)
        self.assertTrue(m.helper_accepts(s, cert, now=1))
        rem = m.sign("remove_admin", {"key": "L"}, (add.id,), ("A",))
        s2 = m.fold({g, l, add, rem})
        self.assertFalse(m.helper_accepts(s2, cert, now=1))

    def test_helper_only_cannot_become_admin(self):
        g, l, h = self.chain()
        add = m.sign("add_admin", {"key": "H"}, (h.id,), ("A",))
        self.assertNotIn("H", m.fold({g, l, h, add}).admins)

    def test_admin_cannot_revoke_itself_or_drop_to_zero_admins(self):
        g = genesis()
        self_revoke = m.sign("revoke", {"key": "A"}, (g.id,), ("A",))
        self.assertIn("A", m.fold({g, self_revoke}).admins)
        sole = m.sign("remove_admin", {"key": "A"}, (g.id,), ("A",))
        self.assertIn("A", m.fold({g, sole}).admins)

    def test_third_admin_needs_two_signatures(self):
        g = genesis("A")
        ops = [g]
        parent = g.id
        for key in ("B", "C", "D"):
            o = admit("A", key, "lead", (parent,))
            ops.append(o)
            parent = o.id
        for key in ("B", "C"):
            o = m.sign("add_admin", {"key": key}, (parent,), ("A",))
            ops.append(o)
            parent = o.id
        # Three admins now (A, B, C): one signature is no longer enough.
        one = m.sign("add_admin", {"key": "D"}, (parent,), ("A",))
        self.assertNotIn("D", m.fold(set(ops) | {one}).admins)
        two = m.sign("add_admin", {"key": "D"}, (parent,), ("A", "B"))
        self.assertIn("D", m.fold(set(ops) | {two}).admins)

    def test_revoking_an_admin_needs_quorum_at_three_admins(self):
        g = genesis("A")
        ops = [g]
        parent = g.id
        for key in ("B", "C", "X"):
            o = admit("A", key, "lead", (parent,))
            ops.append(o)
            parent = o.id
        for key in ("B", "C"):
            o = m.sign("add_admin", {"key": key}, (parent,), ("A",))
            ops.append(o)
            parent = o.id
        one = m.sign("revoke", {"key": "C"}, (parent,), ("A",))
        self.assertIn("C", m.fold(set(ops) | {one}).admins)
        two = m.sign("revoke", {"key": "C"}, (parent,), ("A", "B"))
        s = m.fold(set(ops) | {two})
        self.assertNotIn("C", s.admins)
        self.assertIn("C", s.tombstones)
        # A non-admin member still needs only one admin.
        plain = m.sign("revoke", {"key": "X"}, (parent,), ("A",))
        self.assertIn("X", m.fold(set(ops) | {plain}).tombstones)

    def test_missing_parent_is_ignored_not_fatal(self):
        g = genesis()
        orphan = admit("A", "X", "lead", ("deadbeef",))
        s = m.fold({g, orphan})
        self.assertNotIn("X", s.members)

    def test_rotate_host_key_is_self_signed_only(self):
        g, l, h = self.chain()
        mine = m.sign("rotate_host_key", {"host_fp": "new"}, (h.id,), ("H",))
        theirs = m.sign("rotate_host_key", {"host_fp": "evil"}, (h.id,), ("L",))
        s = m.fold({g, l, h, mine})
        self.assertEqual(s.members["H"]["host_fp"], "new")
        s = m.fold({g, l, h, theirs})
        self.assertEqual(s.members["H"]["host_fp"], "fp-H")

    def test_rollback_is_refused(self):
        g, l, h = self.chain()
        known = m.heads({g, l, h})
        self.assertFalse(m.accept_roster(known, {g, l}))
        self.assertTrue(m.accept_roster(known, {g, l, h}))

    def test_merge_is_commutative_and_idempotent(self):
        g, l, h = self.chain()
        a, b = {g, l}, {g, h, l}
        self.assertEqual(m.merge(a, b), m.merge(b, a))
        self.assertEqual(m.merge(a, a), a)

    def test_random_interleavings_converge(self):
        rng = random.Random(7)
        g = genesis()
        ops = [g]
        parent = g.id
        for i in range(12):
            key = f"K{i}"
            roles = rng.choice(["lead", "helper", "lead,helper"])
            o = admit("A", key, roles, (parent,))
            ops.append(o)
            parent = o.id
            if rng.random() < 0.3:
                r = m.sign("revoke", {"key": f"K{rng.randrange(i + 1)}"}, (parent,), ("A",))
                ops.append(r)
                parent = r.id
        want = m.fold(set(ops))
        for _ in range(50):
            sub_a = {o for o in ops if rng.random() < 0.7} | {g}
            sub_b = {o for o in ops if rng.random() < 0.7} | {g}
            merged = m.merge(sub_a, sub_b) | set(ops)
            got = m.fold(merged)
            self.assertEqual(got.members, want.members)
            self.assertEqual(got.tombstones, want.tombstones)


if __name__ == "__main__":
    logging.basicConfig(level=logging.WARNING)
    unittest.main()
