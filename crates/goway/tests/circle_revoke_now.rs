//! `goway circle revoke --now` and the expiry / last-seen columns of `goway circle list`.

use std::cell::RefCell;
use std::path::Path;

use goway::access::run_cert;
use goway::circle::{admit, init, join, list_rows, make_card, revoke_with, set_roles, sync_roster};
use goway::circle_push::{Pusher, RosterOffer, ago, issued_expiry, record_expiry};
use goway::cli::{
    CircleAdmitArgs, CircleCardArgs, CircleCertArgs, CircleInitArgs, CircleJoinArgs,
    CircleRevokeArgs, CircleRolesArgs, CircleSelect, CircleSyncArgs,
};
use goway::paths::Paths;
use goway::render::{ColorWhen, Renderer};

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

/// Answers for the hosts in `up`, refuses the rest, and remembers every call.
struct Fake {
    up: Vec<&'static str>,
    calls: RefCell<Vec<String>>,
    offers: RefCell<Vec<RosterOffer>>,
}

impl Pusher for Fake {
    fn push(&self, host: &str, offer: &RosterOffer) -> Result<(), String> {
        self.offers.borrow_mut().push(offer.clone());
        self.calls.borrow_mut().push(host.to_owned());
        if self.up.contains(&host) {
            Ok(())
        } else {
            Err("no route".to_owned())
        }
    }
}

/// A circle with a founder and helpers `names`, all joined.
fn circle(root: &Path, names: &[&str]) -> Paths {
    let founder = paths(&root.join("lap"));
    init(
        &founder,
        quiet(),
        &CircleInitArgs {
            name: "home".to_owned(),
            machine: Some("lap".to_owned()),
        },
    )
    .unwrap();
    for name in names {
        let p = paths(&root.join(name));
        let card = make_card(
            &p,
            quiet(),
            &CircleCardArgs {
                role: "helper".to_owned(),
                name: Some((*name).to_owned()),
                host_fp: Some(String::new()),
            },
        )
        .unwrap();
        let w = admit(
            &founder,
            quiet(),
            &CircleAdmitArgs {
                card: card.card.encode(),
                fingerprint: Some(card.fingerprint),
                circle: None,
            },
        )
        .unwrap();
        join(
            &p,
            quiet(),
            &CircleJoinArgs {
                welcome: w.welcome.encode(),
                fingerprint: Some(w.admin_fingerprint),
            },
        )
        .unwrap();
    }
    founder
}

fn revoke_args(name: &str, now: bool) -> CircleRevokeArgs {
    CircleRevokeArgs {
        name: name.to_owned(),
        yes: true,
        now,
        circle: None,
    }
}

// frob:tests crates/goway/src/circle.rs::revoke_with
#[test]
fn revoke_now_pushes_to_reachable_helpers_and_records_last_seen() {
    let tmp = tempfile::tempdir().unwrap();
    let founder = circle(tmp.path(), &["a", "b", "c"]);
    let fake = Fake {
        up: vec!["a"],
        calls: RefCell::new(Vec::new()),
        offers: RefCell::new(Vec::new()),
    };
    revoke_with(&founder, quiet(), &revoke_args("c", true), &fake).unwrap();
    let mut calls = fake.calls.borrow().clone();
    calls.sort();
    assert_eq!(calls, ["a", "b"], "the revoked member is not contacted");
    let rows = list_rows(&founder, quiet(), &CircleSelect { circle: None }).unwrap();
    assert_eq!(rows[0][4..6], ["cert expires", "last seen"]);
    let seen = |name: &str| rows.iter().find(|r| r[0] == name).unwrap()[5].clone();
    assert_eq!(seen("a"), "just now");
    assert_eq!(seen("b"), "never");
    assert_eq!(seen("lap"), "now");
}

// frob:tests crates/goway/src/circle.rs::revoke_with
#[test]
fn revoke_without_now_contacts_nobody() {
    let tmp = tempfile::tempdir().unwrap();
    let founder = circle(tmp.path(), &["a"]);
    let fake = Fake {
        up: vec!["a"],
        calls: RefCell::new(Vec::new()),
        offers: RefCell::new(Vec::new()),
    };
    revoke_with(&founder, quiet(), &revoke_args("a", false), &fake).unwrap();
    assert!(fake.calls.borrow().is_empty());
}

// frob:tests crates/goway/src/circle_push.rs::ago
#[test]
fn ago_words() {
    assert_eq!(ago(None, 100), "never");
    assert_eq!(ago(Some(100), 130), "just now");
    assert_eq!(ago(Some(0), 300), "5m ago");
    assert_eq!(ago(Some(0), 7200), "2h ago");
    assert_eq!(ago(Some(0), 200_000), "2d ago");
}

fn sync_args(offer: &RosterOffer) -> CircleSyncArgs {
    CircleSyncArgs {
        ops: offer.ops_b64.clone(),
        circle: None,
    }
}

// frob:tests crates/goway/src/circle.rs::sync_roster
#[test]
fn pushed_roster_makes_a_stale_helper_adopt_the_revoke() {
    let tmp = tempfile::tempdir().unwrap();
    let founder = circle(tmp.path(), &["c", "a"]);
    let fake = Fake {
        up: vec!["a"],
        calls: RefCell::new(Vec::new()),
        offers: RefCell::new(Vec::new()),
    };
    revoke_with(&founder, quiet(), &revoke_args("c", true), &fake).unwrap();
    let offer = fake.offers.borrow()[0].clone();
    let helper = paths(&tmp.path().join("a"));
    let names = |p: &Paths| -> Vec<String> {
        list_rows(p, quiet(), &CircleSelect { circle: None })
            .unwrap()
            .iter()
            .map(|r| r[0].clone())
            .collect()
    };
    assert!(names(&helper).contains(&"c".to_owned()), "helper is stale");
    sync_roster(&helper, quiet(), &sync_args(&offer)).unwrap();
    assert!(!names(&helper).contains(&"c".to_owned()), "revoke adopted");
    // Offering the same roster again changes nothing.
    sync_roster(&helper, quiet(), &sync_args(&offer)).unwrap();
}

// frob:tests crates/goway/src/circle.rs::sync_roster
#[test]
fn an_old_roster_cannot_roll_a_helper_back() {
    let tmp = tempfile::tempdir().unwrap();
    let founder = circle(tmp.path(), &["c", "a"]);
    let old = {
        let dir = std::fs::read_dir(founder.config_dir.join("circle/circles")).unwrap();
        let id = dir.flatten().next().unwrap().path();
        RosterOffer::new("x", &std::fs::read_to_string(id.join("ops.jsonl")).unwrap())
    };
    let fake = Fake {
        up: vec!["a"],
        calls: RefCell::new(Vec::new()),
        offers: RefCell::new(Vec::new()),
    };
    revoke_with(&founder, quiet(), &revoke_args("c", true), &fake).unwrap();
    // The founder already holds the revoke; an older roster is a no-op, not a rollback.
    sync_roster(&founder, quiet(), &sync_args(&old)).unwrap();
    let rows = list_rows(&founder, quiet(), &CircleSelect { circle: None }).unwrap();
    assert!(!rows.iter().any(|r| r[0] == "c"));
}

// frob:tests crates/goway/src/circle_push.rs::RosterOffer
#[test]
fn offer_command_reads_the_roster_from_stdin_at_any_size() {
    let offer = RosterOffer::new("abc", &"x".repeat(200_000));
    let cmd = offer.remote_command();
    assert_eq!(
        cmd,
        "goway circle sync --circle abc - && goway circle access --circle abc"
    );
    assert!(
        offer.ops_b64.len() > 100_000,
        "the roster no longer rides the command line"
    );
}

// frob:tests crates/goway/src/circle_push.rs::record_expiry
#[test]
fn issued_expiry_is_remembered_per_member() {
    let tmp = tempfile::tempdir().unwrap();
    let p = paths(tmp.path());
    assert!(issued_expiry(&p, "c1").is_empty());
    record_expiry(&p, "c1", "a", 1000).unwrap();
    record_expiry(&p, "c1", "b", 2000).unwrap();
    record_expiry(&p, "c1", "a", 3000).unwrap();
    let got = issued_expiry(&p, "c1");
    assert_eq!(got.get("a"), Some(&3000));
    assert_eq!(got.get("b"), Some(&2000));
}

// frob:tests crates/goway/src/access.rs::run_cert
#[test]
fn list_shows_the_expiry_of_a_cert_issued_for_another_member() {
    let tmp = tempfile::tempdir().unwrap();
    let founder = circle(tmp.path(), &["a"]);
    set_roles(
        &founder,
        quiet(),
        &CircleRolesArgs {
            name: "a".to_owned(),
            roles: "lead,helper".to_owned(),
            circle: None,
        },
    )
    .unwrap();
    run_cert(
        &founder,
        quiet(),
        &CircleCertArgs {
            name: Some("a".to_owned()),
            circle: None,
        },
    )
    .unwrap();
    let rows = list_rows(&founder, quiet(), &CircleSelect { circle: None }).unwrap();
    let row = rows.iter().find(|r| r[0] == "a").unwrap();
    assert!(row[4].starts_with("unix "), "{}", row[4]);
}
