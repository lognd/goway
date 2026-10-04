//! A helper that drops off the network mid-job (a laptop that suspends
//! anyway) is reported as asleep with goway's own exit code, never as a
//! failure of the command; a command that itself exits 255 keeps its code.
#![cfg(unix)]

mod common;

/// A fake ssh that, for the `run` call only, hangs up with ssh's 255 while the
/// marker file `asleep` exists, and from then on refuses every call (the
/// host no longer answers).
fn sleepy_ssh(root: &std::path::Path) -> String {
    format!(
        r#"#!/bin/sh
while [ $# -gt 0 ]; do
  case "$1" in
    -o|-p|-l) shift 2 ;;
    --) shift; shift; break ;;
    *) shift ;;
  esac
done
line="$1"
if [ -e '{root}/dead' ]; then
  echo "ssh: connect to host helios port 22: Connection timed out" >&2
  exit 255
fi
if [ -e '{root}/asleep' ]; then
  verb=$(printf %s "$line" | sed -n 's/.* \([A-Za-z0-9+\/=]\{{40,\}}\) .*/\1/p' | base64 -d | sed -n 's/^set -- \([a-z-]*\).*/\1/p')
  if [ "$verb" = run ]; then
    : > '{root}/dead'
    echo "client_loop: send disconnect: Broken pipe" >&2
    exit 255
  fi
fi
exec sh -c "$line"
"#,
        root = root.display()
    )
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

// frob:tests crates/goway/src/run.rs::run
#[test]
fn a_host_that_stops_answering_mid_job_is_reported_asleep_not_as_a_failed_command() {
    let dir = tempfile::tempdir().unwrap();
    let w = common::world_with_ssh(&sleepy_ssh(dir.path()));
    // The fake reads its marker files from its own directory, not the world's.
    std::fs::write(dir.path().join("asleep"), "").unwrap();
    let out = w.run(&["run", "--", "sh", "-c", "echo hi"]);
    assert_eq!(out.status.code(), Some(125), "{}", text(&out.stderr));
    let err = text(&out.stderr);
    assert!(err.contains("asleep"), "{err}");
    assert!(err.contains("not a failure of your command"), "{err}");
}

// frob:tests crates/goway/src/run.rs::run
#[test]
fn a_command_that_exits_255_on_a_host_that_still_answers_keeps_its_exit_code() {
    let dir = tempfile::tempdir().unwrap();
    let w = common::world_with_ssh(&sleepy_ssh(dir.path()));
    let out = w.run(&["run", "--", "sh", "-c", "exit 255"]);
    assert_eq!(out.status.code(), Some(255), "{}", text(&out.stderr));
    assert!(!text(&out.stderr).contains("asleep"));
}
