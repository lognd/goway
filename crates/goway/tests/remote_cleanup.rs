//! The end-of-run cleanup of the work dir is best-effort: whatever stops it
//! from removing the directory, the run exits with the command's own code.
#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt;

/// The command leaves an unremovable directory in the run's work dir (a
/// stand-in for anything that makes `rm -rf` fail, such as a detached child
/// still writing there); the run must still exit with the command's code,
/// say that gc will collect the directory, and leave no error trap trace.
#[test]
fn a_work_dir_that_cannot_be_removed_never_changes_the_exit_code() {
    let w = common::world();
    let root = w.remote.display();
    let script = format!(
        "W='{root}'/work/\"$GOWAY_RUN_ID\"; mkdir \"$W/d\" && : > \"$W/d/f\" && chmod 555 \"$W/d\"; exit 7"
    );
    let out = w.run(&["run", "--", "sh", "-c", &script]);
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    for dir in w.work_dirs() {
        let stuck = w.remote.join("work").join(dir).join("d");
        let _ = std::fs::set_permissions(stuck, std::fs::Permissions::from_mode(0o755));
    }
    assert_eq!(out.status.code(), Some(7), "{err}");
    assert!(!err.contains("failed at line"), "{err}");
    assert!(err.contains("gc will collect it"), "{err}");
}
