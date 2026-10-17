//! Rust equivalents of `docs/model/test_circle_model.py` plus strictness and cap checks.
#![allow(clippy::many_single_char_names, clippy::cast_possible_truncation)]

mod common;

use std::collections::BTreeSet;

use common::{admit, chain, fake, forge, genesis, run, sign, simple};
use goway_circle::{
    CERT_TTL_SECS, CertError, CircleError, Kind, MAX_OPS, Op, Role, accept_roster, fold,
    fold_pinned, heads, helper_accepts, issue_cert, merge, parse_ops,
};
use proptest::prelude::*;

fn roles(list: &[Role]) -> BTreeSet<Role> {
    list.iter().copied().collect()
}

// frob:ticket 01M5443BNG19E2ZT3TZ2ZV2BCK
// frob:tests crates/goway-circle/src/state.rs::fold
#[test]
fn admit_by_admin_applies() {
    let (g, l, h) = chain();
    let s = run(&[g, l, h]);
    let keys: Vec<&str> = s.members.keys().map(String::as_str).collect();
    assert_eq!(keys, ["A", "H", "L"]);
    assert_eq!(s.members["H"].roles, roles(&[Role::Helper]));
}

#[test]
fn admit_by_non_admin_is_ignored() {
    let (g, l, _) = chain();
    let bad = admit("L", "X", "lead", &[l.id()]);
    assert!(!run(&[g, l, bad]).members.contains_key("X"));
}

// frob:ticket 01M5443BNG19E2ZT3TZ2ZV2BCK
// frob:tests crates/goway-circle/src/op.rs::Op
#[test]
fn forged_signature_is_ignored() {
    let (g, l, _) = chain();
    let forged = forge(&admit("A", "X", "lead", &[l.id()]));
    assert!(!run(&[g, l, forged]).members.contains_key("X"));
}

#[test]
fn unknown_roles_are_ignored() {
    let g = genesis("A");
    let bad = admit("A", "X", "root", &[g.id()]);
    assert!(!run(&[g, bad]).members.contains_key("X"));
}

#[test]
fn order_independence() {
    let (g, l, h) = chain();
    let r = simple(Kind::Revoke, &["A"], "L", &[h.id()]);
    let ops = vec![g, l, h, r];
    let want = run(&ops);
    // All 24 permutations.
    let mut idx = [0usize, 1, 2, 3];
    let mut count = 0;
    permute(&mut idx, 0, &mut |p| {
        let perm: Vec<Op> = p.iter().map(|&i| ops[i].clone()).collect();
        assert_eq!(run(&perm), want);
        count += 1;
    });
    assert_eq!(count, 24);
}

fn permute(a: &mut [usize], k: usize, f: &mut impl FnMut(&[usize])) {
    if k == a.len() {
        f(a);
        return;
    }
    for i in k..a.len() {
        a.swap(k, i);
        permute(a, k + 1, f);
        a.swap(k, i);
    }
}

#[test]
fn revoked_key_cannot_be_readmitted() {
    let (g, l, h) = chain();
    let r = simple(Kind::Revoke, &["A"], "L", &[h.id()]);
    let again = admit("A", "L", "lead", &[r.id()]);
    let s = run(&[g, l, h, r, again]);
    assert!(!s.members.contains_key("L"));
    assert!(s.tombstones.contains("L"));
}

#[test]
fn concurrent_revoke_beats_admit_of_same_key() {
    let (g, l, h) = chain();
    let r = simple(Kind::Revoke, &["A"], "H", &[h.id()]);
    let a = sign(
        Kind::Admit,
        &[
            ("key", "H"),
            ("name", "again"),
            ("roles", "helper"),
            ("host_fp", "fp-H"),
        ],
        &[h.id()],
        &["A"],
    );
    let s = run(&[g, l, h, r, a]);
    assert!(!s.members.contains_key("H"));
}

// frob:ticket 01M5443BNG19E2ZT3TZ2ZV2BCK
// frob:tests crates/goway-circle/src/cert.rs::principal
#[test]
fn revoke_bumps_epoch_and_kills_old_cert() {
    let (g, l, h) = chain();
    let before = run(&[g.clone(), l.clone(), h.clone()]);
    let cert = issue_cert(&before, "A", "L", 0).unwrap();
    assert!(helper_accepts(&before, &cert, 1));
    let r = simple(Kind::Revoke, &["A"], "H", &[h.id()]);
    let after = run(&[g, l, h, r]);
    assert!(!helper_accepts(&after, &cert, 1));
    let fresh = issue_cert(&after, "A", "L", 0).unwrap();
    assert!(helper_accepts(&after, &fresh, 1));
}

// frob:ticket 01M5443BNG19E2ZT3TZ2ZV2BCK
// frob:tests crates/goway-circle/src/cert.rs::helper_accepts
#[test]
fn cert_expires() {
    let (g, l, _) = chain();
    let s = run(&[g, l]);
    let cert = issue_cert(&s, "A", "L", 0).unwrap();
    assert_eq!(cert.principal, "goway-lead:home:e0");
    assert!(helper_accepts(&s, &cert, CERT_TTL_SECS));
    assert!(!helper_accepts(&s, &cert, CERT_TTL_SECS + 1));
}

#[test]
fn helper_only_never_gets_a_lead_cert() {
    let (g, l, h) = chain();
    let s = run(&[g, l, h]);
    assert_eq!(issue_cert(&s, "A", "H", 0), Err(CertError::NotLead));
    assert_eq!(issue_cert(&s, "A", "nobody", 0), Err(CertError::NotMember));
}

// frob:ticket 01M5443BNG19E2ZT3TZ2ZV2BCK
// frob:tests crates/goway-circle/src/cert.rs::issue_cert
#[test]
fn non_admin_cannot_issue_certs() {
    let (g, l, _) = chain();
    let s = run(&[g, l]);
    assert_eq!(issue_cert(&s, "L", "L", 0), Err(CertError::NotAdmin));
}

#[test]
fn cert_from_removed_admin_is_rejected() {
    let (g, l, _) = chain();
    let add = simple(Kind::AddAdmin, &["A"], "L", &[l.id()]);
    let s = run(&[g.clone(), l.clone(), add.clone()]);
    let cert = issue_cert(&s, "L", "L", 0).unwrap();
    assert!(helper_accepts(&s, &cert, 1));
    let rem = simple(Kind::RemoveAdmin, &["A"], "L", &[add.id()]);
    let s2 = run(&[g, l, add, rem]);
    assert!(!helper_accepts(&s2, &cert, 1));
}

#[test]
fn helper_only_cannot_become_admin() {
    let (g, l, h) = chain();
    let add = simple(Kind::AddAdmin, &["A"], "H", &[h.id()]);
    assert!(!run(&[g, l, h, add]).admins.contains("H"));
}

#[test]
fn admin_cannot_revoke_itself_or_drop_to_zero_admins() {
    let g = genesis("A");
    let self_revoke = simple(Kind::Revoke, &["A"], "A", &[g.id()]);
    assert!(run(&[g.clone(), self_revoke]).admins.contains("A"));
    let sole = simple(Kind::RemoveAdmin, &["A"], "A", &[g.id()]);
    assert!(run(&[g, sole]).admins.contains("A"));
}

// frob:ticket 01M5443BNG19E2ZT3TZ2ZV2BCK
// frob:tests crates/goway-circle/src/state.rs::fold
#[test]
fn revoking_an_admin_needs_quorum_at_three_admins() {
    let g = genesis("A");
    let mut ops = vec![g.clone()];
    let mut parent = g.id();
    for key in ["B", "C", "X"] {
        let o = admit("A", key, "lead", &[parent]);
        parent = o.id();
        ops.push(o);
    }
    for key in ["B", "C"] {
        let o = simple(Kind::AddAdmin, &["A"], key, &[parent]);
        parent = o.id();
        ops.push(o);
    }
    let with = |extra: Op| {
        let mut v = ops.clone();
        v.push(extra);
        run(&v)
    };
    let one = with(simple(Kind::Revoke, &["A"], "C", &[parent.clone()]));
    assert!(one.admins.contains("C"));
    let two = with(simple(Kind::Revoke, &["A", "B"], "C", &[parent.clone()]));
    assert!(!two.admins.contains("C") && two.tombstones.contains("C"));
    let plain = with(simple(Kind::Revoke, &["A"], "X", &[parent]));
    assert!(plain.tombstones.contains("X"));
}

#[test]
fn third_admin_needs_two_signatures() {
    let g = genesis("A");
    let mut ops = vec![g.clone()];
    let mut parent = g.id();
    for key in ["B", "C", "D"] {
        let o = admit("A", key, "lead", &[parent]);
        parent = o.id();
        ops.push(o);
    }
    for key in ["B", "C"] {
        let o = simple(Kind::AddAdmin, &["A"], key, &[parent]);
        parent = o.id();
        ops.push(o);
    }
    let one = simple(Kind::AddAdmin, &["A"], "D", &[parent.clone()]);
    let mut with_one = ops.clone();
    with_one.push(one);
    assert!(!run(&with_one).admins.contains("D"));
    let two = simple(Kind::AddAdmin, &["A", "B"], "D", &[parent.clone()]);
    let mut with_two = ops.clone();
    with_two.push(two);
    assert!(run(&with_two).admins.contains("D"));
    // The same admin listed twice is one signature, and does not even verify.
    let dup = simple(Kind::AddAdmin, &["A", "A"], "D", &[parent]);
    ops.push(dup);
    assert!(!run(&ops).admins.contains("D"));
}

#[test]
fn missing_parent_is_ignored_not_fatal() {
    let g = genesis("A");
    let orphan = admit("A", "X", "lead", &["deadbeef".to_owned()]);
    assert!(!run(&[g, orphan]).members.contains_key("X"));
}

#[test]
fn rotate_host_key_is_self_signed_only() {
    let (g, l, h) = chain();
    let rot =
        |who: &str, fp: &str| sign(Kind::RotateHostKey, &[("host_fp", fp)], &[h.id()], &[who]);
    let s = run(&[g.clone(), l.clone(), h.clone(), rot("H", "new")]);
    assert_eq!(s.members["H"].host_fp, "new");
    let s = run(&[g, l, h.clone(), rot("L", "evil")]);
    assert_eq!(s.members["H"].host_fp, "fp-H");
}

// frob:ticket 01M5443BNG19E2ZT3TZ2ZV2BCK
// frob:tests crates/goway-circle/src/state.rs::accept_roster
// frob:tests crates/goway-circle/src/state.rs::heads
#[test]
fn rollback_is_refused() {
    let (g, l, h) = chain();
    let known = heads(&[g.clone(), l.clone(), h.clone()]);
    assert!(matches!(
        accept_roster(&known, &[g.clone(), l.clone()]),
        Err(CircleError::Rollback { .. })
    ));
    assert!(accept_roster(&known, &[g, l, h]).is_ok());
}

// frob:ticket 01M5443BNG19E2ZT3TZ2ZV2BCK
// frob:tests crates/goway-circle/src/state.rs::merge
#[test]
fn merge_is_commutative_and_idempotent() {
    let (g, l, h) = chain();
    let a = vec![g.clone(), l.clone()];
    let b = vec![g, h, l];
    let ids = |v: Vec<Op>| v.iter().map(Op::id).collect::<BTreeSet<_>>();
    assert_eq!(ids(merge(&a, &b)), ids(merge(&b, &a)));
    assert_eq!(ids(merge(&a, &a)), ids(a.clone()));
    assert_eq!(merge(&a, &a).len(), a.len());
}

#[test]
fn random_interleavings_converge() {
    // Deterministic LCG stand-in for the model's seeded RNG; the proptest suite covers the rest.
    let mut seed = 7u64;
    let mut next = move |n: u64| {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (seed >> 33) % n
    };
    let g = genesis("A");
    let mut ops = vec![g.clone()];
    let mut parent = g.id();
    for i in 0..12u64 {
        let r = ["lead", "helper", "lead,helper"][next(3) as usize];
        let o = admit("A", &format!("K{i}"), r, &[parent]);
        parent = o.id();
        ops.push(o);
        if next(10) < 3 {
            let rv = simple(
                Kind::Revoke,
                &["A"],
                &format!("K{}", next(i + 1)),
                &[parent],
            );
            parent = rv.id();
            ops.push(rv);
        }
    }
    let want = run(&ops);
    for _ in 0..50 {
        let pick = |next: &mut dyn FnMut(u64) -> u64| -> Vec<Op> {
            ops.iter()
                .filter(|_| next(10) < 7)
                .cloned()
                .chain([g.clone()])
                .collect()
        };
        let a = pick(&mut next);
        let b = pick(&mut next);
        let mut merged = merge(&a, &b);
        merged.extend(ops.iter().cloned());
        let got = run(&merged);
        assert_eq!(got.members, want.members);
        assert_eq!(got.tombstones, want.tombstones);
    }
}

// ---- beyond the model: strictness, caps, pinning ----

#[test]
fn op_roundtrips_through_strict_jsonl() {
    let (g, l, h) = chain();
    let text: String = [&g, &l, &h].iter().map(|o| o.to_line() + "\n").collect();
    let back = parse_ops(&text).unwrap();
    assert_eq!(back, vec![g, l, h]);
}

#[test]
fn parse_rejects_unknown_fields_and_bad_ids() {
    let g = genesis("A");
    let line = g.to_line();
    let extra = line.replacen('{', "{\"extra\":1,", 1);
    assert!(matches!(
        parse_ops(&extra),
        Err(CircleError::Parse { line: 1, .. })
    ));
    let bad_id = line.replacen(&g.id(), &"0".repeat(64), 1);
    assert!(matches!(
        parse_ops(&bad_id),
        Err(CircleError::IdMismatch { line: 1 })
    ));
    let unknown_kind = line.replacen("genesis", "bogus", 1);
    assert!(parse_ops(&unknown_kind).is_err());
}

// frob:ticket 01M5443BNG19E2ZT3TZ2ZV2BCK
// frob:tests crates/goway-circle/src/op.rs::parse_ops
#[test]
fn caps_are_enforced() {
    let g = genesis("A");
    let many = vec![g.clone(); MAX_OPS + 1];
    assert!(matches!(
        fold(&many, &fake("v")),
        Err(CircleError::TooManyOps { .. })
    ));
    let big = "x".repeat(70_000);
    let huge = sign(
        Kind::Admit,
        &[
            ("key", "X"),
            ("name", &big),
            ("roles", "lead"),
            ("host_fp", "f"),
        ],
        &[g.id()],
        &["A"],
    );
    assert!(!run(&[g.clone(), huge.clone()]).members.contains_key("X"));
    assert!(matches!(
        parse_ops(&huge.to_line()),
        Err(CircleError::OpTooLarge { .. })
    ));
}

#[test]
fn id_is_stable_canonical_json_hash() {
    let a = sign(
        Kind::Admit,
        &[
            ("key", "K"),
            ("name", "n"),
            ("roles", "lead"),
            ("host_fp", "f"),
        ],
        &[],
        &["A"],
    );
    let b = sign(
        Kind::Admit,
        &[
            ("host_fp", "f"),
            ("roles", "lead"),
            ("name", "n"),
            ("key", "K"),
        ],
        &[],
        &["A"],
    );
    assert_eq!(a.id(), b.id());
    assert_eq!(a.id().len(), 64);
}

// frob:ticket 01M5443BNG19E2ZT3TZ2ZV2BCK
// frob:tests crates/goway-circle/src/state.rs::fold_pinned
#[test]
fn rival_genesis_cannot_hijack_a_pinned_circle() {
    let mine = genesis("A");
    let rival = sign(
        Kind::Genesis,
        &[("circle", "home"), ("admin", "Z"), ("name", "z")],
        &[],
        &["Z"],
    );
    let ops = [mine.clone(), rival];
    let s = fold_pinned(&ops, &fake("v"), Some(&mine.id())).unwrap();
    assert!(s.admins.contains("A") && !s.admins.contains("Z"));
}

proptest! {
    #[test]
    fn folding_any_prefix_never_panics(n in 0usize..5) {
        let (g, l, h) = chain();
        let all = [g, l, h];
        let _ = run(&all[..n.min(3)]);
    }
}
