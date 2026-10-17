//! `goway circle admin|renew|policy`: admin set changes (single and two-signature), certificate
//! renewal timing and the access policy, on real `ssh-keygen` keys in temp dirs.

use std::path::{Path, PathBuf};

use goway::access::{needs_renewal, run_policy, run_renew};
use goway::circle::{admit, init, join, make_card};
use goway::circle_admin::{Proposed, admin_card, cosign, propose};
use goway::cli::{
    CircleAdminCardArgs, CircleAdminChangeArgs, CircleAdmitArgs, CircleCardArgs, CircleCosignArgs,
    CircleInitArgs, CircleJoinArgs, CirclePolicyArgs, CircleRenewArgs,
};
use goway::paths::Paths;
use goway::render::{ColorWhen, Renderer};
use goway_circle::{Kind, SshKeygenVerifier, State, Store, fold};

fn quiet() -> Renderer {
    Renderer::new(ColorWhen::Never)
}

fn paths(dir: &Path) -> Paths {
    Paths {
        config_dir: dir.join("config"),
        state_dir: dir.join("state"),
        runtime_dir: None,
    }
}

fn state_of(p: &Paths) -> State {
    let base = p.config_dir.join("circle").join("circles");
    let ops = Store::find(&base, None).unwrap().ops().unwrap();
    fold(&ops, &SshKeygenVerifier).unwrap()
}

/// Copy `from`'s roster files over `to`'s (a stand-in for the sync that spreads rosters).
fn sync(from: &Paths, to: &Paths) {
    let base = |p: &Paths| p.config_dir.join("circle").join("circles");
    let src = std::fs::read_dir(base(from))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let dst = base(to).join(src.file_name().unwrap());
    std::fs::create_dir_all(&dst).unwrap();
    for f in std::fs::read_dir(&src).unwrap() {
        let f = f.unwrap().path();
        std::fs::copy(&f, dst.join(f.file_name().unwrap())).unwrap();
    }
}

struct World {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    founder: Paths,
}

fn world() -> World {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let founder = paths(&root.join("lap"));
    init(
        &founder,
        quiet(),
        &CircleInitArgs {
            name: "home".to_owned(),
            machine: Some("lap".to_owned()),
            passphrase: false,
        },
    )
    .unwrap();
    World {
        _tmp: tmp,
        root,
        founder,
    }
}

/// A new machine `name` that joins as a lead and is also admitted under its admin key.
fn new_admin_candidate(w: &World, name: &str) -> Paths {
    let p = paths(&w.root.join(name));
    let card = make_card(
        &p,
        quiet(),
        &CircleCardArgs {
            role: "lead".to_owned(),
            name: Some(name.to_owned()),
            host_fp: Some(String::new()),
            qr: false,
        },
    )
    .unwrap();
    let admit_card = |c: &goway_circle::Card, fp: &str| {
        admit(
            &w.founder,
            quiet(),
            &CircleAdmitArgs {
                card: c.encode(),
                fingerprint: Some(fp.to_owned()),
                circle: None,
            },
        )
        .unwrap()
    };
    let welcome = admit_card(&card.card, &card.fingerprint);
    let join_args = |wel: &goway_circle::Welcome| CircleJoinArgs {
        welcome: wel.encode(),
        fingerprint: Some(welcome.admin_fingerprint.clone()),
    };
    join(&p, quiet(), &join_args(&welcome.welcome)).unwrap();
    let (acard, afp) = admin_card(
        &p,
        quiet(),
        &CircleAdminCardArgs {
            name: Some(name.to_owned()),
        },
    )
    .unwrap();
    let second = admit_card(&acard, &afp);
    join(&p, quiet(), &join_args(&second.welcome)).unwrap();
    p
}

fn change(name: &str, cosigner: Option<&str>) -> CircleAdminChangeArgs {
    CircleAdminChangeArgs {
        name: name.to_owned(),
        cosigner: cosigner.map(str::to_owned),
        out: None,
        circle: None,
    }
}

// frob:tests crates/goway/src/circle_admin.rs::propose
#[test]
fn one_admin_adds_and_removes_another_with_a_single_signature() {
    let w = world();
    let bee = new_admin_candidate(&w, "bee");
    sync(&w.founder, &bee);
    let out = propose(
        &w.founder,
        quiet(),
        Kind::AddAdmin,
        &change("bee-admin", None),
    )
    .unwrap();
    assert_eq!(out, Proposed::Applied);
    assert_eq!(state_of(&w.founder).admins.len(), 2);
    let again = propose(
        &w.founder,
        quiet(),
        Kind::AddAdmin,
        &change("bee-admin", None),
    );
    assert!(again.is_err(), "adding an admin twice is refused");
    propose(
        &w.founder,
        quiet(),
        Kind::RemoveAdmin,
        &change("bee-admin", None),
    )
    .unwrap();
    let state = state_of(&w.founder);
    assert_eq!(state.admins.len(), 1);
    assert_eq!(state.epoch, 1, "removing an admin bumps the epoch");
}

// frob:tests crates/goway/src/circle_admin.rs::cosign
#[test]
fn three_admins_need_a_second_signature_to_add_a_fourth() {
    let w = world();
    let bee = new_admin_candidate(&w, "bee");
    let cat = new_admin_candidate(&w, "cat");
    let dog = new_admin_candidate(&w, "dog");
    sync(&w.founder, &bee);
    for who in ["bee-admin", "cat-admin"] {
        propose(&w.founder, quiet(), Kind::AddAdmin, &change(who, None)).unwrap();
    }
    assert_eq!(state_of(&w.founder).admins.len(), 3);
    sync(&w.founder, &bee);
    sync(&w.founder, &cat);
    sync(&w.founder, &dog);

    let alone = propose(
        &w.founder,
        quiet(),
        Kind::AddAdmin,
        &change("dog-admin", None),
    );
    assert!(alone.is_err(), "one signature is not a quorum");
    let Proposed::NeedsCosign(line) = propose(
        &w.founder,
        quiet(),
        Kind::AddAdmin,
        &change("dog-admin", Some("bee-admin")),
    )
    .unwrap() else {
        panic!("expected a half-signed proposal");
    };
    assert_eq!(
        state_of(&w.founder).admins.len(),
        3,
        "a half-signed op changes nothing"
    );
    let file = w.root.join("proposal.jsonl");
    std::fs::write(&file, format!("{line}\n")).unwrap();

    let wrong = cosign(
        &cat,
        quiet(),
        &CircleCosignArgs {
            file: file.clone(),
            circle: None,
        },
    );
    assert!(
        wrong.is_err(),
        "an admin who is not a named signer cannot finish it"
    );
    cosign(&bee, quiet(), &CircleCosignArgs { file, circle: None }).unwrap();
    assert_eq!(state_of(&bee).admins.len(), 4);
}

// frob:tests crates/goway/src/access.rs::needs_renewal
#[test]
fn renewal_starts_at_half_of_the_ttl() {
    let ttl = goway_circle::CERT_TTL_SECS;
    assert!(!needs_renewal(1_000 + ttl, 1_000));
    assert!(!needs_renewal(1_000 + ttl / 2 + 1, 1_000));
    assert!(needs_renewal(1_000 + ttl / 2, 1_000));
    assert!(needs_renewal(1_000, 5_000), "an expired cert needs renewal");
}

// frob:tests crates/goway/src/access.rs::run_renew
#[test]
fn renew_leaves_a_fresh_certificate_alone_unless_forced() {
    let w = world();
    let cert = {
        let mut p = w
            .founder
            .config_dir
            .join("circle")
            .join("node_ed25519")
            .into_os_string();
        p.push("-cert.pub");
        PathBuf::from(p)
    };
    let args = |force| CircleRenewArgs {
        name: None,
        force,
        circle: None,
    };
    run_renew(&w.founder, quiet(), &args(false)).unwrap();
    assert!(cert.is_file(), "no certificate yet, so renew mints one");
    std::fs::write(
        &cert,
        std::fs::read_to_string(&cert).unwrap() + "# marker\n",
    )
    .unwrap_or(());
    let before = std::fs::read_to_string(&cert).unwrap();
    run_renew(&w.founder, quiet(), &args(false)).unwrap();
    assert_eq!(
        std::fs::read_to_string(&cert).unwrap(),
        before,
        "fresh cert is untouched"
    );
    run_renew(&w.founder, quiet(), &args(true)).unwrap();
    assert_ne!(
        std::fs::read_to_string(&cert).unwrap(),
        before,
        "--force reissues"
    );
}

fn policy(w: &World, mode: &str, lead: &[&str]) -> goway::error::Result<()> {
    run_policy(
        &w.founder,
        quiet(),
        &CirclePolicyArgs {
            mode: mode.to_owned(),
            lead: lead.iter().map(|s| (*s).to_owned()).collect(),
            file: Some(w.root.join("authorized_keys")),
            windows_admin: false,
            circle: None,
        },
    )
}

// frob:tests crates/goway/src/access.rs::run_policy
#[test]
fn policy_switches_between_open_and_listed() {
    let w = world();
    let keys = w.root.join("authorized_keys");
    policy(&w, "listed", &["lap"]).unwrap();
    assert!(
        std::fs::read_to_string(&keys)
            .unwrap()
            .contains(":lead:lap")
    );
    policy(&w, "open", &[]).unwrap();
    assert!(!std::fs::read_to_string(&keys).unwrap().contains(":lead:"));
    assert!(policy(&w, "listed", &[]).is_err(), "listed needs a lead");
    assert!(policy(&w, "open", &["lap"]).is_err(), "open takes no lead");
    assert!(
        policy(&w, "listed", &["ghost"]).is_err(),
        "unknown lead refused"
    );
}

/// Write an executable fake `ssh-keygen` that makes a key pair at the `-f` path (no device).
#[cfg(unix)]
fn fake_keygen(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt as _;
    let script = dir.join("fake-ssh-keygen");
    let body = "#!/bin/sh\nwhile [ $# -gt 0 ]; do case \"$1\" in -f) f=\"$2\";; esac; shift; done\n\
        echo secret > \"$f\"; echo 'sk-ssh-ed25519@openssh.com AAAAGnNrLXNzaC1lZDI1NTE5QG9wZW5zc2guY29t c' > \"$f.pub\"\n";
    std::fs::write(&script, body).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

// frob:tests crates/goway/src/circle_admin.rs::keygen
#[cfg(unix)]
#[test]
fn keygen_makes_the_admin_key_once() {
    let tmp = tempfile::tempdir().unwrap();
    let p = paths(&tmp.path().join("lap"));
    let tool = fake_keygen(tmp.path());
    let (pair, fp) = goway::circle_admin::keygen(&p, quiet(), &tool).unwrap();
    assert_eq!(
        pair.private,
        p.config_dir.join("circle").join("admin_ed25519")
    );
    assert!(pair.public.starts_with("sk-ssh-ed25519@openssh.com "));
    assert!(fp.starts_with("SHA256:"));
    assert!(goway::circle_admin::keygen(&p, quiet(), &tool).is_err());
}
