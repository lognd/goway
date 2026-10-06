//! `--with-git`: the helper's copy gets a `.git` whose HEAD, index and
//! status match the laptop's work tree, and nothing private.
#![cfg(unix)]

mod common;

use std::process::Command;

use common::{World, git, world};

fn local(w: &World, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(&w.repo)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn remote(w: &World, extra: &[&str], script: &str) -> String {
    let mut args = vec!["run"];
    args.extend_from_slice(extra);
    args.extend(["--", "sh", "-c", script]);
    let out = w.run(&args);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A repository with history, a staged change, an edit and an untracked file.
fn prepare(w: &World) {
    for (k, v) in [("user.email", "t@example.com"), ("user.name", "t")] {
        git(&w.repo, &["config", k, v]);
    }
    git(&w.repo, &["add", "."]);
    git(&w.repo, &["commit", "-qm", "one"]);
    std::fs::write(w.repo.join("second.txt"), "second\n").unwrap();
    git(&w.repo, &["add", "."]);
    git(&w.repo, &["commit", "-qm", "two"]);
    std::fs::write(w.repo.join("hello.txt"), "edited\n").unwrap();
    std::fs::write(w.repo.join("new.txt"), "new\n").unwrap();
    git(&w.repo, &["add", "second.txt"]);
}

// frob:tests crates/goway/src/gitmeta.rs::prepare
#[test]
fn head_index_and_status_match_the_laptops() {
    let w = world();
    prepare(&w);
    let probe = "git rev-parse HEAD; git status --porcelain=v1; git ls-files; git symbolic-ref HEAD; git log --oneline";
    let there = remote(&w, &["--with-git"], probe);
    let here = format!(
        "{}{}{}{}{}",
        local(&w, &["rev-parse", "HEAD"]),
        local(&w, &["status", "--porcelain=v1"]),
        local(&w, &["ls-files"]),
        local(&w, &["symbolic-ref", "HEAD"]),
        // shallow: only the HEAD commit's line
        local(&w, &["log", "--oneline", "-1"]),
    );
    assert_eq!(there, here);
}

#[test]
fn without_the_flag_there_is_no_git_directory() {
    let w = world();
    prepare(&w);
    assert_eq!(
        remote(&w, &[], "test -e .git && echo yes || echo no"),
        "no\n"
    );
}

#[test]
fn project_file_can_ask_for_it() {
    let w = world();
    prepare(&w);
    std::fs::write(w.repo.join("goway.toml"), "with_git = true\n").unwrap();
    let out = remote(&w, &[], "git rev-parse --is-inside-work-tree");
    assert_eq!(out, "true\n");
}

// frob:tests crates/goway/src/gitmeta.rs::prepare
#[test]
fn credentials_remotes_and_hooks_never_travel() {
    let w = world();
    prepare(&w);
    git(
        &w.repo,
        &[
            "remote",
            "add",
            "origin",
            "https://user:token123@example.com/r.git",
        ],
    );
    git(&w.repo, &["config", "credential.helper", "store"]);
    std::fs::write(w.repo.join(".git/hooks/pre-commit"), "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::write(w.repo.join(".git-credentials"), "https://u:p@example.com\n").unwrap();
    let there = remote(
        &w,
        &["--with-git"],
        "cat .git/config; ls .git; ls .git/hooks 2>&1; ls -a | grep credentials; true",
    );
    assert!(!there.contains("token123"), "{there}");
    assert!(!there.contains("credential"), "{there}");
    assert!(!there.contains("origin"), "{there}");
    assert!(!there.contains("pre-commit"), "{there}");
}

#[test]
fn a_moved_head_rebuilds_and_an_unchanged_one_sends_nothing_new() {
    let w = world();
    prepare(&w);
    let first = remote(&w, &["--with-git"], "git rev-parse HEAD");
    let again = remote(&w, &["--with-git"], "git rev-parse HEAD");
    assert_eq!(first, again);
    git(&w.repo, &["commit", "-qam", "three"]);
    let moved = remote(&w, &["--with-git"], "git rev-parse HEAD");
    assert_eq!(moved, local(&w, &["rev-parse", "HEAD"]));
    assert_ne!(first, moved);
}

// frob:tests crates/goway/src/gitmeta.rs::prepare
#[test]
fn committed_secret_looking_files_are_not_in_the_helpers_objects() {
    let w = world();
    std::fs::write(w.repo.join(".env"), "TOKEN=placeholder-value\n").unwrap();
    prepare(&w);
    let there = remote(
        &w,
        &["--with-git"],
        "git cat-file -p HEAD:.env 2>&1; git cat-file -p HEAD:hello.txt",
    );
    assert!(!there.contains("placeholder-value"), "{there}");
    assert!(
        there.contains("hello") || there.contains("edited"),
        "{there}"
    );
}

/// A second branch whose tree has a file the checked-out one lacks.
fn side_branch(w: &World) -> String {
    git(&w.repo, &["checkout", "-qb", "ledger"]);
    std::fs::create_dir_all(w.repo.join("tickets")).unwrap();
    std::fs::write(w.repo.join("tickets/t1.md"), "ledger entry\n").unwrap();
    git(&w.repo, &["add", "tickets"]);
    git(&w.repo, &["commit", "-qm", "ledger"]);
    let sha = local(w, &["rev-parse", "HEAD"]).trim().to_owned();
    git(&w.repo, &["checkout", "-q", "-"]);
    sha
}

// frob:tests crates/goway/src/gitmeta.rs::prepare
#[test]
fn a_named_ref_arrives_at_the_same_commit_with_its_tree_readable() {
    let w = world();
    prepare(&w);
    let sha = side_branch(&w);
    let there = remote(
        &w,
        &["--with-ref", "refs/heads/ledger"],
        "git rev-parse refs/heads/ledger; git show refs/heads/ledger:tickets/t1.md; git branch --list; git remote -v; git for-each-ref --format='%(refname)'",
    );
    assert!(
        there.starts_with(&format!("{sha}\nledger entry\n")),
        "{there}"
    );
    assert!(there.contains("refs/heads/ledger"), "{there}");
    assert!(!there.contains("origin"), "{there}");
}

// frob:tests crates/goway/src/gitmeta.rs::prepare
#[test]
fn only_the_requested_ref_and_head_are_sent() {
    let w = world();
    prepare(&w);
    side_branch(&w);
    git(&w.repo, &["branch", "other"]);
    let there = remote(
        &w,
        &["--with-ref", "refs/heads/ledger"],
        "git for-each-ref --format='%(refname)'; git log --oneline refs/heads/ledger | wc -l",
    );
    assert!(!there.contains("other"), "{there}");
    assert!(there.ends_with("1\n"), "history leaked: {there}");
}

#[test]
fn project_file_can_list_refs() {
    let w = world();
    prepare(&w);
    side_branch(&w);
    std::fs::write(
        w.repo.join("goway.toml"),
        "with_refs = [\"refs/heads/ledger\"]\n",
    )
    .unwrap();
    let out = remote(&w, &[], "git show refs/heads/ledger:tickets/t1.md");
    assert_eq!(out, "ledger entry\n");
}

// frob:tests crates/goway/src/gitmeta.rs::resolve_refs
#[test]
fn a_missing_or_malformed_ref_is_refused() {
    let w = world();
    prepare(&w);
    for bad in ["refs/heads/nope", "ledger"] {
        let out = w.run(&["run", "--with-ref", bad, "--", "true"]);
        assert!(!out.status.success(), "{bad}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("--with-ref"), "{err}");
    }
}
