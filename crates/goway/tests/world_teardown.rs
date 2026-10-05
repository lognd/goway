//! A test world does not leave the sccache server its runs started.
#![cfg(unix)]

mod common;

// frob:ticket 01M451F9V3WBZM37PF85HN8AZK
// frob:tests crates/goway/tests/common/mod.rs::stop_sccache_servers
#[test]
fn a_dropped_world_leaves_no_sccache_server_of_its_own() {
    if !cfg!(target_os = "linux")
        || std::process::Command::new("sccache")
            .arg("--version")
            .output()
            .is_err()
    {
        return;
    }
    let w = common::world();
    let root = w.root.clone();
    let out = w.run(&["run", "--", "sh", "-c", "echo dir=$SCCACHE_DIR"]);
    assert!(out.status.success(), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("dir="),
        "sccache was not configured: {out:?}"
    );
    common::wait_for("the run's sccache server to be up", || {
        !common::sccache_servers_under(&root).is_empty()
    });
    drop(w);
    assert!(common::sccache_servers_under(&root).is_empty());
}
