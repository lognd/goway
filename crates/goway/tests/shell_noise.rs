//! A helper whose shell startup files print text, and whose login shell is
//! fish- or csh-like, must not corrupt goway's protocol: a fake ssh prints a
//! greeting before the command and refuses command lines a fish or csh
//! parser would choke on (newlines, backslashes, bangs).
#![cfg(unix)]

mod common;

/// Ignores ssh options, greets like a noisy `.bashrc`, then runs the command
/// line the way a strict login shell would: refusing what fish or csh reject.
const NOISY_FISH_SSH: &str = r#"#!/bin/sh
while [ $# -gt 0 ]; do
  case "$1" in
    -o|-p|-l) shift 2 ;;
    --) shift; shift; break ;;
    *) shift ;;
  esac
done
line="$1"
echo "Welcome to helios! 3 updates are pending."
echo "stderr greeting" >&2
case "$line" in
  *'
'*|*\\*|*'!'*) echo "fish: Unexpected end of string, quotes are not balanced" >&2; exit 127 ;;
esac
exec sh -c "$line"
"#;

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

// frob:tests crates/goway/src/remote.rs::invocation
// frob:tests crates/goway/src/remote.rs::Framed
#[test]
fn a_noisy_startup_file_and_a_fish_like_login_shell_leave_the_run_output_exact() {
    let w = common::world_with_ssh(NOISY_FISH_SSH);
    // Quotes, a backslash and a bang in the command reach bash unchanged.
    let out = w.run(&[
        "run",
        "--",
        "sh",
        "-c",
        r"echo it\'s; printf '%s\n' 'a\b' 'wow!'",
    ]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout), "it's\na\\b\nwow!\n");
    assert!(
        !text(&out.stdout).contains("Welcome"),
        "{}",
        text(&out.stdout)
    );
}

// frob:tests crates/goway/src/remote.rs::split_frame
#[test]
fn the_exit_code_of_the_command_survives_a_noisy_helper() {
    let w = common::world_with_ssh(NOISY_FISH_SSH);
    let out = w.run(&["run", "--", "sh", "-c", "echo hi; exit 7"]);
    assert_eq!(out.status.code(), Some(7), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout), "hi\n");
}

// frob:tests crates/goway/src/resolve.rs::noise_fact
// frob:tests crates/goway/src/doctor.rs::host_checks
#[test]
fn doctor_warns_about_noisy_startup_files_with_the_guard_to_add() {
    let w = common::world_with_ssh(NOISY_FISH_SSH);
    let out = w.run(&["doctor", "local"]);
    let all = format!("{}{}", text(&out.stdout), text(&out.stderr));
    assert!(all.contains("shell startup"), "{all}");
    assert!(all.contains("Welcome to helios!"), "{all}");
    assert!(
        all.contains("case $- in *i*) ;; *) return ;; esac"),
        "{all}"
    );
    // A quiet helper gets no such warning.
    let quiet = common::world().run(&["doctor", "local"]);
    let all = format!("{}{}", text(&quiet.stdout), text(&quiet.stderr));
    assert!(!all.contains("shell startup"), "{all}");
}
