//! `goway circle revoke --now` and the expiry / last-seen columns of `goway circle list`.

use std::cell::RefCell;
use std::path::Path;

use goway::circle::{admit, init, join, list_rows, make_card, revoke_with, sync_roster};
use goway::circle_push::{Pusher, RosterOffer, ago};
use goway::cli::{
    CircleAdmitArgs, CircleCardArgs, CircleInitArgs, CircleJoinArgs, CircleRevokeArgs,
    CircleSelect, CircleSyncArgs,
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
fn offer_command_carries_the_roster_and_is_size_capped() {
    let offer = RosterOffer::new("abc", "line\n");
    let cmd = offer.remote_command().unwrap();
    assert!(cmd.starts_with("goway circle sync --circle abc "), "{cmd}");
    assert!(
        cmd.ends_with("&& goway circle access --circle abc"),
        "{cmd}"
    );
    assert!(
        RosterOffer::new("abc", &"x".repeat(200_000))
            .remote_command()
            .is_err()
    );
}
