//! goway inside goway is bounded: a command that recursively calls goway
//! stops at the depth limit, and automatic gc never overlaps itself.
#![cfg(unix)]

mod common;

use common::world;

// frob:tests crates/goway/src/run.rs::nested_env
#[test]
fn recursive_goway_stops_at_the_depth_limit() {
    let w = world();
    // Every level holds a build slot while it waits for the next one.
    let cfg = std::fs::read_to_string(w.config.join("config.toml")).unwrap();
    std::fs::write(
        w.config.join("config.toml"),
        cfg.replace("target_slots = 2", "target_slots = 8"),
    )
    .unwrap();
    std::fs::write(
        w.repo.join("rec.sh"),
        "echo \"depth=$GOWAY_DEPTH chain=$GOWAY_CHAIN\"\ncd \"$ORIGIN_REPO\" && exec \"$GOWAY_BIN\" --color never run --ignore-footprint -- sh rec.sh\n",
    )
    .unwrap();
    let out = w
        .goway(&["run", "--", "sh", "rec.sh"])
        .env("GOWAY_BIN", env!("CARGO_BIN_EXE_goway"))
        .env("ORIGIN_REPO", &w.repo)
        .env("GOWAY_SSH_PASS_ENV", "GOWAY_BIN,ORIGIN_REPO,PATH,HOME,GOWAY_CONFIG_DIR,GOWAY_STATE_DIR,GOWAY_WINDOWS_LOOKUP,GOWAY_SSH_PASS_ENV")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(125), "{stdout}\n{stderr}");
    for d in 1..=4 {
        assert!(stdout.contains(&format!("depth={d} ")), "{stdout}");
    }
    assert!(!stdout.contains("depth=5"), "{stdout}");
    assert!(stderr.contains("nested 4 deep"), "{stderr}");
    assert!(stderr.contains("origin >"), "names the chain: {stderr}");
}

// frob:tests crates/goway/src/run.rs::nested_env
#[test]
fn the_job_sees_depth_one_and_the_chain() {
    let w = world();
    let out = w.run(&["run", "--", "sh", "-c", "echo $GOWAY_DEPTH/$GOWAY_CHAIN"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "1/origin\n");
}

/// An expired, labelled cache entry the automatic gc would remove.
fn stale_cache(w: &common::World) -> std::path::PathBuf {
    let stale = w.remote.join("cache").join("stale-repo");
    std::fs::create_dir_all(&stale).unwrap();
    std::fs::write(stale.join("meta.json"), "{\"kind\":\"cache\"}").unwrap();
    std::fs::write(stale.join("target-0.lock"), "").unwrap();
    let ok = std::process::Command::new("touch")
        .args(["-d", "2000-01-01"])
        .arg(stale.join("meta.json"))
        .arg(&stale)
        .status()
        .unwrap();
    assert!(ok.success());
    stale
}

#[test]
fn automatic_gc_removes_an_expired_entry() {
    let w = world();
    assert_eq!(w.run(&["run", "--", "true"]).status.code(), Some(0));
    let stale = stale_cache(&w);
    assert_eq!(w.run(&["run", "--", "true"]).status.code(), Some(0));
    for _ in 0..50 {
        if !stale.exists() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    panic!("the control: automatic gc should have removed {stale:?}");
}

#[test]
fn only_one_automatic_gc_runs_per_root() {
    let w = world();
    assert_eq!(w.run(&["run", "--", "true"]).status.code(), Some(0));
    // Hold the lock as a running gc would: a later run's gc must back off.
    // Taking it first also waits out the first run's detached gc.
    let root = w.root.display();
    let mut holder = std::process::Command::new("flock")
        .arg(w.remote.join("gc.lock"))
        .args(["sh", "-c"])
        .arg(format!(
            "touch {root}/held; while [ ! -e {root}/stop ]; do sleep 0.2; done"
        ))
        .spawn()
        .unwrap();
    for _ in 0..300 {
        if w.root.join("held").exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let stale = stale_cache(&w);
    assert_eq!(w.run(&["run", "--", "true"]).status.code(), Some(0));
    std::thread::sleep(std::time::Duration::from_secs(2));
    let survived = stale.exists();
    std::fs::write(w.root.join("stop"), "").unwrap();
    holder.wait().ok();
    assert!(survived, "gc ran while the lock was held");
}
