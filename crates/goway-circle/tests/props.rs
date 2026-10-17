//! Property tests: convergence under any op ordering, and unauthorized ops never change state.
#![allow(clippy::cast_possible_truncation)]

mod common;

use common::{admit, forge, genesis, run, sign, simple};
use goway_circle::{Kind, Op, State};
use proptest::prelude::*;

const KEYS: [&str; 5] = ["A", "B", "C", "D", "E"];

/// A recipe for one op: kind selector, signer picks, target key, parent picks.
type Recipe = (u8, usize, usize, usize, usize, Vec<usize>);

fn recipe() -> impl Strategy<Value = Recipe> {
    (
        0u8..7,
        0..KEYS.len(),
        0..KEYS.len(),
        0..KEYS.len(),
        0usize..3,
        prop::collection::vec(any::<usize>(), 0..3),
    )
}

/// Build a DAG of ops from recipes; parents are earlier ops (sometimes a dangling id).
fn build(recipes: &[Recipe]) -> Vec<Op> {
    let mut ops = vec![genesis("A")];
    for (sel, s1, s2, target, role, picks) in recipes {
        let mut parents: Vec<String> = picks.iter().map(|p| ops[p % ops.len()].id()).collect();
        if picks.len() == 2 && picks[0] % 5 == 0 {
            parents.push("dangling".into());
        }
        let key = KEYS[*target];
        let signers: Vec<&str> = if s1 == s2 {
            vec![KEYS[*s1]]
        } else {
            vec![KEYS[*s1], KEYS[*s2]]
        };
        let roles = ["lead", "helper", "lead,helper"][*role];
        let op = match sel {
            0 | 1 => admit(signers[0], key, roles, &parents),
            2 => simple(Kind::Revoke, &signers[..1], key, &parents),
            3 => sign(
                Kind::SetRoles,
                &[("key", key), ("roles", roles)],
                &parents,
                &signers[..1],
            ),
            4 => simple(Kind::AddAdmin, &signers, key, &parents),
            5 => simple(Kind::RemoveAdmin, &signers, key, &parents),
            _ => sign(
                Kind::RotateHostKey,
                &[("host_fp", "x")],
                &parents,
                &signers[..1],
            ),
        };
        ops.push(op);
    }
    ops
}

fn without_applied(mut s: State) -> State {
    s.applied.clear();
    s
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Any shuffle, duplication or merge split of the same op set folds to the same state.
    #[test]
    fn fold_converges_under_any_ordering(
        recipes in prop::collection::vec(recipe(), 0..14),
        seed in any::<u64>(),
    ) {
        let ops = build(&recipes);
        let want = run(&ops);
        let mut shuffled = ops.clone();
        let mut x = seed | 1;
        for i in (1..shuffled.len()).rev() {
            x ^= x << 13; x ^= x >> 7; x ^= x << 17;
            shuffled.swap(i, (x as usize) % (i + 1));
        }
        prop_assert_eq!(&run(&shuffled), &want);
        let half = shuffled.len() / 2;
        let merged = goway_circle::merge(&shuffled[..half], &shuffled[half / 2..]);
        let mut all = merged;
        all.extend(ops.iter().cloned());
        prop_assert_eq!(&run(&all), &want);
    }

    /// An outsider's op (never an admin, never a member) or a forged one changes no state.
    #[test]
    fn unauthorized_ops_never_change_state(
        recipes in prop::collection::vec(recipe(), 0..14),
        sel in 0u8..6,
        target in 0..KEYS.len(),
        pick in any::<usize>(),
    ) {
        let ops = build(&recipes);
        let base = without_applied(run(&ops));
        let parent = vec![ops[pick % ops.len()].id()];
        let key = KEYS[target];
        let outsider = match sel {
            0 => admit("Z", key, "lead", &parent),
            1 => simple(Kind::Revoke, &["Z"], key, &parent),
            2 => sign(Kind::SetRoles, &[("key", key), ("roles", "lead")], &parent, &["Z"]),
            3 => simple(Kind::AddAdmin, &["Z"], key, &parent),
            4 => simple(Kind::RemoveAdmin, &["Z"], key, &parent),
            _ => sign(Kind::RotateHostKey, &[("host_fp", "evil")], &parent, &["Z"]),
        };
        // A forged admit that claims to be signed by the founding admin.
        let forged = forge(&admit("A", "Z", "lead", &parent));
        let mut more = ops.clone();
        more.extend([outsider, forged]);
        prop_assert_eq!(without_applied(run(&more)), base);
    }
}
