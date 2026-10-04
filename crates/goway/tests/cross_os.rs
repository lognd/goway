//! Cross-OS runs through the fake-ssh world: the same-OS default with a loud hint for portable
//! runners, `--any-os`, `cross_os` in goway.toml, and `--each-os`. The Windows host in the
//! config is never reachable here (the fake remote only speaks sh); the hint is read from the
//! config, so it still names it.
#![cfg(unix)]

mod common;

const ID: &str = "01M42FJVGY91ND091THEDPP8DN";

fn world() -> common::World {
    let w = common::world();
    let path = w.config.join("config.toml");
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str("\n[[host]]\nname = \"winbox\"\nos = \"windows\"\naddress = \"192.0.2.9\"\n");
    std::fs::write(path, text).unwrap();
    w
}

fn stderr(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn toml(w: &common::World, text: &str) {
    std::fs::write(w.repo.join("goway.toml"), text).unwrap();
}

// frob:ticket 01M42FJVGY91ND091THEDPP8DN
// frob:tests crates/goway/src/project.rs::warn_cross_os
#[test]
fn a_portable_runner_gets_the_loud_hint_and_stays_on_the_laptops_os() {
    let _ = ID;
    let w = world();
    let err = stderr(&w.run(&["run", "--", "pytest", "--version"]));
    assert!(err.contains("CROSS-OS"), "{err}");
    assert!(
        err.contains("other-OS hosts that could take it: winbox"),
        "{err}"
    );
    assert!(err.contains("--any-os"), "{err}");
    assert!(err.contains("cross_os = true in goway.toml"), "{err}");
    // An unrecognized command stays same-OS without a word.
    let err = stderr(&w.run(&["run", "--", "true"]));
    assert!(!err.contains("CROSS-OS"), "{err}");
}

// frob:ticket 01M42FJVGY91ND091THEDPP8DN
// frob:tests crates/goway/src/project.rs::selection_for
#[test]
fn any_os_and_cross_os_settings_silence_the_hint() {
    let w = world();
    let err = stderr(&w.run(&["run", "--any-os", "--", "pytest", "--version"]));
    assert!(!err.contains("CROSS-OS"), "{err}");
    toml(&w, "cross_os = true\n");
    let err = stderr(&w.run(&["run", "--", "pytest", "--version"]));
    assert!(!err.contains("CROSS-OS"), "{err}");
    toml(&w, "cross_os = false\n");
    let err = stderr(&w.run(&["run", "--", "pytest", "--version"]));
    assert!(!err.contains("CROSS-OS"), "{err}");
    toml(&w, "cross_os = \"maybe\"\n");
    let out = w.run(&["run", "--", "pytest", "--version"]);
    assert_eq!(out.status.code(), Some(125));
    assert!(stderr(&out).contains("goway.toml"), "{}", stderr(&out));
}

// frob:ticket 01M42FJVGY91ND091THEDPP8DN
// frob:tests crates/goway/src/shard.rs::run_each_os
#[test]
fn each_os_runs_once_per_reachable_os_with_prefixes_and_a_summary() {
    let w = world();
    let os = common::host_os();
    let out = w.run(&["run", "--each-os", "--", "echo", "hello"]);
    let err = stderr(&out);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{err}");
    assert!(
        text.contains(&format!("[local {os}] hello")),
        "{text}\n{err}"
    );
    assert!(err.contains(&format!("{os} on local: exit 0")), "{err}");
    // A failing command fails the run with its code.
    let out = w.run(&["run", "--each-os", "--", "sh", "-c", "exit 3"]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    // --each-os and --host exclude each other.
    let out = w.run(&["run", "--each-os", "--host", "local", "--", "true"]);
    assert_eq!(out.status.code(), Some(2));
}
