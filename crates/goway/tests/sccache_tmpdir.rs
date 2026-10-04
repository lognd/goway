//! A shared sccache server left running under a deleted temp directory (the
//! state an older goway produced) is detected and restarted before a run.
#![cfg(unix)]

mod common;

use std::path::PathBuf;
use std::process::Command;

fn have(tool: &str) -> bool {
    Command::new(tool).arg("--version").output().is_ok()
}

// frob:tests crates/goway/src/remote.rs::invocation
#[test]
fn a_server_under_a_deleted_tmpdir_is_restarted_before_the_build() {
    if !have("sccache") || !have("cc") {
        return;
    }
    let w = common::world();
    // Run 1 starts the repository's server and tells where it lives.
    let first = w.run(&[
        "run",
        "--",
        "sh",
        "-c",
        "echo uds=$SCCACHE_SERVER_UDS; echo dir=$SCCACHE_DIR",
    ]);
    let text = String::from_utf8_lossy(&first.stdout).into_owned();
    let find = |key: &str| -> PathBuf {
        text.lines()
            .find_map(|l| l.strip_prefix(key))
            .unwrap_or_else(|| panic!("no {key} in {text}"))
            .into()
    };
    let (uds, dir) = (find("uds="), find("dir="));
    let cache = dir.parent().unwrap().to_owned();
    let sccache = |tmp: Option<&std::path::Path>, arg: &str| {
        let mut c = Command::new("sccache");
        c.arg(arg)
            .env("SCCACHE_SERVER_UDS", &uds)
            .env("SCCACHE_DIR", &dir);
        if let Some(t) = tmp {
            c.env("TMPDIR", t);
        }
        c.output().unwrap()
    };
    // Replace the healthy server with one started by an "older goway": a
    // per-run TMPDIR that is then removed, and no record of a stable one.
    sccache(None, "--stop-server");
    let gone = w.root.join("per-run-tmp");
    std::fs::create_dir_all(&gone).unwrap();
    assert!(sccache(Some(&gone), "--start-server").status.success());
    std::fs::remove_dir_all(&gone).unwrap();
    let _ = std::fs::remove_file(cache.join("sccache.tmpdir"));
    let second = w.run(&[
        "run",
        "--",
        "sh",
        "-c",
        "echo 'int main(void){return 0;}' > t.c && sccache cc -c t.c -o t.o 2>&1 && echo compiled",
    ]);
    let out = String::from_utf8_lossy(&second.stdout).into_owned();
    let err = String::from_utf8_lossy(&second.stderr).into_owned();
    sccache(None, "--stop-server");
    assert!(out.contains("compiled"), "{out}{err}");
    assert!(
        err.contains("restarting the shared sccache server"),
        "{err}"
    );
}
