//! The change log: config and pinned-key edits and recorded actions are listed and undone.

use goway::changelog::{self, FILE_NAME};
use goway::config::{self, HostConfig};
use goway::hosts;
use goway_journal::{ActionKind, Outcome};

fn host(name: &str) -> HostConfig {
    HostConfig {
        name: name.to_owned(),
        port: Some(2222),
        ..HostConfig::default()
    }
}

// frob:tests crates/goway/src/changelog.rs::write_file
// frob:tests crates/goway/src/changelog.rs::undo
// frob:tests crates/goway/src/config.rs::add_host
#[test]
fn config_edits_are_recorded_and_undone_newest_first_to_the_exact_prior_text() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let original = "# my pool\n";
    std::fs::write(&path, original).unwrap();
    config::add_host(&path, &host("helios")).unwrap();
    let one = std::fs::read_to_string(&path).unwrap();
    config::add_host(&path, &host("orion")).unwrap();
    assert_eq!(changelog::rows(dir.path()).unwrap().len(), 2);

    let done = changelog::undo(dir.path(), Some(1)).unwrap();
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].number, 2);
    assert_eq!(done[0].outcome, Outcome::Restored);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), one);

    let done = changelog::undo(dir.path(), None).unwrap();
    assert_eq!(done.len(), 1, "only the one still active");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    assert!(
        !dir.path().join(FILE_NAME).exists(),
        "a fully undone log is removed"
    );
}

#[test]
fn an_edit_made_since_is_left_alone_not_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    config::add_host(&path, &host("helios")).unwrap();
    std::fs::write(&path, "# edited by hand\n").unwrap();
    let done = changelog::undo(dir.path(), None).unwrap();
    assert!(matches!(done[0].outcome, Outcome::LeftAlone(_)), "{done:?}");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# edited by hand\n"
    );
}

#[test]
fn removing_a_host_and_its_pinned_key_is_recorded_and_undoable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let known = dir.path().join("known_hosts");
    config::add_host(&path, &host("helios")).unwrap();
    std::fs::write(
        &known,
        "goway-helios ssh-ed25519 AAAAkey\ngoway-orion ssh-ed25519 AAAAother\n",
    )
    .unwrap();
    hosts::forget_key(&known, "goway-helios");
    assert_eq!(
        std::fs::read_to_string(&known).unwrap(),
        "goway-orion ssh-ed25519 AAAAother\n"
    );
    config::remove_host(&path, "helios").unwrap();
    assert!(!std::fs::read_to_string(&path).unwrap().contains("helios"));

    changelog::undo(dir.path(), None).unwrap();
    assert!(!path.exists(), "the config goway created is gone again");
    assert!(
        std::fs::read_to_string(&known)
            .unwrap()
            .contains("goway-helios")
    );
}

// frob:tests crates/goway/src/changelog.rs::record_action
#[test]
fn an_action_is_listed_with_host_and_reason_and_undo_reports_it_not_reversible() {
    let dir = tempfile::tempdir().unwrap();
    changelog::record_action(
        dir.path(),
        ActionKind::RunFix,
        "rustup-init -y",
        "helios",
        "doctor --fix",
        Some("`goway uninstall`"),
    )
    .unwrap();
    let rows = changelog::rows(dir.path()).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(!rows[0].reversible);
    for part in [
        "ran a fix command",
        "rustup-init -y",
        "helios",
        "doctor --fix",
    ] {
        assert!(rows[0].what.contains(part), "{}", rows[0].what);
    }
    let done = changelog::undo(dir.path(), None).unwrap();
    let Outcome::NotReversible(text) = &done[0].outcome else {
        panic!("{done:?}");
    };
    assert!(
        text.contains("to take it back: `goway uninstall`"),
        "{text}"
    );
}

#[test]
fn an_unrecordable_change_does_not_happen() {
    let dir = tempfile::tempdir().unwrap();
    // The change log's path is a directory, so nothing can be recorded.
    std::fs::create_dir(dir.path().join(FILE_NAME)).unwrap();
    let path = dir.path().join("config.toml");
    assert!(config::add_host(&path, &host("helios")).is_err());
    assert!(!path.exists(), "the file was not touched");
}

#[test]
fn rewriting_the_same_text_records_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.txt");
    std::fs::write(&path, "same").unwrap();
    changelog::write_file(&path, b"same").unwrap();
    assert!(changelog::rows(dir.path()).unwrap().is_empty());
}

#[cfg(unix)]
mod cli {
    use std::process::Command;

    fn goway(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_goway"))
            .args(args)
            .env("GOWAY_CONFIG_DIR", dir)
            .env("GOWAY_STATE_DIR", dir.join("state"))
            .env("NO_COLOR", "1")
            .output()
            .unwrap()
    }

    // frob:tests crates/goway/src/changelog.rs::show
    // frob:tests crates/goway/src/changelog.rs::undo_command
    #[test]
    fn goway_changes_lists_and_undoes() {
        let dir = tempfile::tempdir().unwrap();
        let empty = goway(dir.path(), &["changes"]);
        assert!(empty.status.success());
        assert!(String::from_utf8_lossy(&empty.stderr).contains("no changes are recorded"));

        super::changelog::record_action(
            dir.path(),
            super::ActionKind::StartScheduledTask,
            "keepalive",
            "localhost",
            "test",
            None,
        )
        .unwrap();
        let listed = goway(dir.path(), &["changes", "list"]);
        let text = String::from_utf8_lossy(&listed.stdout).into_owned()
            + &String::from_utf8_lossy(&listed.stderr);
        assert!(
            text.contains("keepalive") && text.contains("active"),
            "{text}"
        );

        let undone = goway(dir.path(), &["changes", "undo"]);
        let text = String::from_utf8_lossy(&undone.stdout).into_owned()
            + &String::from_utf8_lossy(&undone.stderr);
        assert!(text.contains("not undone"), "{text}");
        assert_eq!(
            undone.status.code(),
            Some(1),
            "an action is never silently skipped"
        );
    }
}
