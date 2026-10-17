//! `goway circle revoke --now` and the expiry / last-seen columns of `goway circle list`.

use std::cell::RefCell;
use std::path::Path;

use goway::circle::{admit, init, join, list_rows, make_card, revoke_with};
use goway::circle_push::{Pusher, ago};
use goway::cli::{
    CircleAdmitArgs, CircleCardArgs, CircleInitArgs, CircleJoinArgs, CircleRevokeArgs,
    CircleSelect,
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
}

impl Pusher for Fake {
    fn push(&self, host: &str) -> Result<(), String> {
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
