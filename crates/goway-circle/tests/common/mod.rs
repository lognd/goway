//! Shared builders for the roster tests (fake signatures, model-style helpers).
#![allow(dead_code, missing_docs)]

use goway_circle::{FakeCrypto, Kind, Op, Sig, State, fold};

pub fn fake(name: &str) -> FakeCrypto {
    FakeCrypto::new(name)
}

pub fn sign(kind: Kind, body: &[(&str, &str)], parents: &[String], signers: &[&str]) -> Op {
    let fakes: Vec<FakeCrypto> = signers.iter().map(|s| fake(s)).collect();
    let refs: Vec<&dyn goway_circle::Signer> = fakes.iter().map(|f| f as _).collect();
    Op::signed(kind, body, parents, &refs).unwrap()
}

pub fn genesis(admin: &str) -> Op {
    sign(
        Kind::Genesis,
        &[("circle", "home"), ("admin", admin), ("name", "a")],
        &[],
        &[admin],
    )
}

pub fn admit(signer: &str, key: &str, roles: &str, parents: &[String]) -> Op {
    let fp = format!("fp-{key}");
    sign(
        Kind::Admit,
        &[
            ("key", key),
            ("name", key),
            ("roles", roles),
            ("host_fp", &fp),
        ],
        parents,
        &[signer],
    )
}

pub fn simple(kind: Kind, signers: &[&str], key: &str, parents: &[String]) -> Op {
    sign(kind, &[("key", key)], parents, signers)
}

pub fn run(ops: &[Op]) -> State {
    fold(ops, &fake("v")).unwrap()
}

/// Genesis, then admit lead L and helper H, chained.
pub fn chain() -> (Op, Op, Op) {
    let g = genesis("A");
    let l = admit("A", "L", "lead", &[g.id()]);
    let h = admit("A", "H", "helper", &[l.id()]);
    (g, l, h)
}

pub fn forge(op: &Op) -> Op {
    let mut f = op.clone();
    f.sigs = vec![Sig {
        signer: op.signers[0].clone(),
        sig: "00".into(),
    }];
    f
}
