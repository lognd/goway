//! Round trip through the real `ssh-keygen -Y` backend; skipped when it is unavailable.

use std::process::Command;

use goway_circle::{
    Kind, Op, Signer, SshKeygenSigner, SshKeygenVerifier, Verifier, fold, valid_public_key,
};

fn keygen(dir: &std::path::Path, name: &str) -> Option<SshKeygenSigner> {
    let key = dir.join(name);
    let ok = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-f"])
        .arg(&key)
        .status()
        .ok()?
        .success();
    if !ok {
        return None;
    }
    let pubtext = std::fs::read_to_string(key.with_extension("pub")).ok()?;
    let mut parts = pubtext.split_whitespace();
    let public = format!("{} {}", parts.next()?, parts.next()?);
    SshKeygenSigner::new(key, public).ok()
}

// frob:ticket 01M5443BNG19E2ZT3TZ2ZV2BCK
// frob:tests crates/goway-circle/src/crypto.rs::SshKeygenSigner
// frob:tests crates/goway-circle/src/crypto.rs::SshKeygenVerifier
#[test]
fn ssh_keygen_signs_and_verifies_ops() {
    let dir = tempfile::tempdir().unwrap();
    let Some(admin) = keygen(dir.path(), "admin") else {
        // No usable ssh-keygen on this machine: nothing to exercise.
        return;
    };
    let pk = admin.public_key().to_owned();
    let g = Op::signed(
        Kind::Genesis,
        &[("circle", "home"), ("admin", &pk), ("name", "a")],
        &[],
        &[&admin],
    )
    .unwrap();
    assert!(g.sig_ok(&SshKeygenVerifier));
    assert!(
        fold(std::slice::from_ref(&g), &SshKeygenVerifier)
            .unwrap()
            .admins
            .contains(&pk)
    );
    let mut tampered = g.clone();
    tampered.sigs[0].sig = g.sigs[0].sig.replace('A', "B");
    assert!(!tampered.sig_ok(&SshKeygenVerifier));
    // A signature by another key does not verify under the admin key.
    let other = keygen(dir.path(), "other").unwrap();
    let sig = other.sign(b"msg").unwrap();
    assert!(!SshKeygenVerifier.verify(&pk, b"msg", &sig));
    assert!(SshKeygenVerifier.verify(other.public_key(), b"msg", &sig));
    assert!(!SshKeygenVerifier.verify(other.public_key(), b"other msg", &sig));
}

// frob:ticket 01M5443BNG19E2ZT3TZ2ZV2BCK
// frob:tests crates/goway-circle/src/crypto.rs::valid_public_key
#[test]
fn only_ed25519_single_line_keys_are_valid() {
    assert!(valid_public_key("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIabc"));
    assert!(!valid_public_key("ssh-rsa AAAAB3NzaC1yc2E"));
    assert!(!valid_public_key("ssh-ed25519 AAAA comment"));
    assert!(!valid_public_key("ssh-ed25519 AAAA\nssh-rsa BBBB"));
    assert!(!valid_public_key(""));
}
