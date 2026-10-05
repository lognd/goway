//! GPU slots: a run that needs a GPU holds one GPU lock while it runs,
//! through the fake-ssh world with a fake `nvidia-smi` on the PATH.
#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt as _;

/// A world whose host has `gpus` fake NVIDIA GPUs.
fn gpu_world(gpus: usize) -> common::World {
    let w = common::world();
    let script = format!(
        r#"#!/bin/sh
case "$*" in
  *--query-gpu=index*) i=0; while [ $i -lt {gpus} ]; do echo $i; i=$((i+1)); done ;;
  *--query-gpu=name*) i=0; while [ $i -lt {gpus} ]; do echo "Fake GPU, 8192, 555.1"; i=$((i+1)); done ;;
  *) echo "| NVIDIA-SMI 555.1   Driver Version: 555.1   CUDA Version: 12.5 |" ;;
esac
"#
    );
    let path = w.bin.join("nvidia-smi");
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    w
}

fn gpu_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("GPU=").map(str::to_owned))
        .unwrap_or_else(|| panic!("no GPU= line: {}", String::from_utf8_lossy(&out.stdout)))
}

fn err(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

const SHOW: &str = "echo GPU=$CUDA_VISIBLE_DEVICES,$ROCR_VISIBLE_DEVICES";

// frob:tests crates/goway/src/run.rs::gpu_words
#[test]
fn a_gpu_run_names_its_gpu_and_a_cpu_run_does_not() {
    let w = gpu_world(2);
    let out = w.run(&["run", "--needs", "gpu", "--", "sh", "-c", SHOW]);
    assert!(out.status.success(), "{}", err(&out));
    assert_eq!(gpu_of(&out), "0,0");
    assert!(err(&out).contains("using GPU 0"), "{}", err(&out));
    let out = w.run(&["run", "--", "sh", "-c", SHOW]);
    assert_eq!(gpu_of(&out), ",", "a run without a GPU need sets nothing");
    // A variable the user set stays theirs; the other is still named.
    let out = w.run(&[
        "run",
        "--needs",
        "gpu",
        "--env",
        "CUDA_VISIBLE_DEVICES=1",
        "--",
        "sh",
        "-c",
        SHOW,
    ]);
    assert_eq!(gpu_of(&out), "1,0");
}

// frob:tests crates/goway/src/run.rs::gpu_words
#[test]
fn concurrent_gpu_runs_get_different_gpus_and_the_third_waits_then_runs() {
    let w = gpu_world(2);
    let hold = |w: &common::World| {
        w.hold(
            &["--needs", "gpu", "--ignore-footprint"],
            "echo GPU=$CUDA_VISIBLE_DEVICES",
        )
    };
    let a = hold(&w);
    a.wait_started();
    let b = hold(&w);
    b.wait_started();
    let c = hold(&w);
    c.wait_stderr("all 2 GPU(s) are busy");
    a.release();
    b.release();
    let (a, b, c) = (a.finish(), b.finish(), c.finish());
    for o in [&a, &b, &c] {
        assert!(o.status.success(), "{}", err(o));
    }
    assert_eq!((gpu_of(&a), gpu_of(&b)), ("0".to_owned(), "1".to_owned()));
    assert!(["0", "1"].contains(&gpu_of(&c).as_str()));
    assert!(!err(&a).contains("busy") && !err(&b).contains("busy"));
}

// frob:tests crates/goway/src/config.rs::gpu_jobs_of
#[test]
fn gpu_jobs_lets_several_runs_share_each_gpu() {
    let w = gpu_world(1);
    let config = std::fs::read_to_string(w.config.join("config.toml")).unwrap();
    std::fs::write(
        w.config.join("config.toml"),
        config.replace(
            "address = \"127.0.0.1\"",
            "address = \"127.0.0.1\"\ngpu_jobs = 2",
        ),
    )
    .unwrap();
    let hold = |w: &common::World| {
        w.hold(
            &["--needs", "gpu", "--ignore-footprint"],
            "echo GPU=$CUDA_VISIBLE_DEVICES",
        )
    };
    let a = hold(&w);
    a.wait_started();
    let b = hold(&w);
    b.wait_started();
    let c = hold(&w);
    // Two share the one GPU at once; the third has to wait for a slot.
    c.wait_stderr("all 1 GPU(s) are busy (up to 2 run(s) each)");
    a.release();
    b.release();
    let outs = [a.finish(), b.finish(), c.finish()];
    for o in &outs {
        assert!(o.status.success(), "{}", err(o));
        assert_eq!(gpu_of(o), "0");
    }
    assert!(!err(&outs[0]).contains("busy") && !err(&outs[1]).contains("busy"));
}

// frob:tests crates/goway/src/run.rs::gpu_words
#[test]
fn the_lock_is_released_however_the_run_ends() {
    let w = gpu_world(1);
    // A failing command, then a killed one: the next run gets the only GPU at once.
    let out = w.run(&["run", "--needs", "gpu", "--", "sh", "-c", "exit 7"]);
    assert_eq!(out.status.code(), Some(7));
    let mut held = w.hold(&["--needs", "gpu"], "true");
    held.wait_started();
    // SIGKILL cannot be handled: only the kernel releasing the flock can free the slot.
    held.kill_client();
    // The fake remote runs on until released; the slot frees when that shell
    // ends and the kernel drops its flock, not before.
    held.release();
    let mut got = None;
    common::wait_for("the killed run's GPU to free", || {
        let out = w.run(&[
            "run",
            "--needs",
            "gpu",
            "--",
            "sh",
            "-c",
            "echo GPU=$CUDA_VISIBLE_DEVICES",
        ]);
        if out.status.success() && !err(&out).contains("busy") {
            got = Some(gpu_of(&out));
        }
        got.is_some()
    });
    assert_eq!(got.as_deref(), Some("0"));
}

// frob:tests crates/goway/src/config.rs::validate
#[test]
fn a_bad_gpu_jobs_is_a_config_error() {
    let w = gpu_world(1);
    let config = std::fs::read_to_string(w.config.join("config.toml")).unwrap();
    std::fs::write(
        w.config.join("config.toml"),
        config.replace(
            "address = \"127.0.0.1\"",
            "address = \"127.0.0.1\"\ngpu_jobs = 0",
        ),
    )
    .unwrap();
    let out = w.run(&["run", "--", "true"]);
    assert_eq!(out.status.code(), Some(125));
    assert!(
        err(&out).contains("gpu_jobs of host `local` must be 1-64"),
        "{}",
        err(&out)
    );
}

/// A held run never outlives its test: dropping it releases it, and even a
/// test that dies without running Drop (killed by the harness) leaves a
/// poller that ends by itself once the test's world directory is gone.
#[cfg(target_os = "linux")]
#[test]
fn held_runs_never_outlive_their_test() {
    let w = gpu_world(1);
    let held = w.hold(&["--needs", "gpu"], "true");
    held.wait_started();
    let marker = w.root.join("held-").display().to_string();
    assert!(common::process_mentions(&marker), "the poller runs");
    drop(held);
    common::wait_for("the dropped run's poller to end", || {
        !common::process_mentions(&marker)
    });

    // A test killed without Drop: the guard never runs, the directory goes.
    let w = gpu_world(1);
    let marker = w.root.join("held-").display().to_string();
    let held = w.hold(&["--needs", "gpu"], "true");
    held.wait_started();
    std::mem::forget(held);
    let root = w.root.clone();
    drop(w);
    assert!(!root.exists());
    common::wait_for("the orphaned poller to notice the vanished world", || {
        !common::process_mentions(&marker)
    });
}
