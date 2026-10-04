//! The remote probe's host facts, run through the real script locally.

use goway::facts::{kv, parse_live, parse_static};

fn probe(args: &[&str]) -> String {
    let home = tempfile::tempdir().unwrap();
    let mut a = vec!["state"];
    a.extend_from_slice(args);
    let out = std::process::Command::new("sh")
        .args(["-c", &goway::remote::invocation("probe", &a)])
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(goway::remote::split_frame(out.stdout).1).unwrap()
}

// frob:tests crates/goway/src/facts.rs::parse_live
#[test]
fn the_probe_reports_live_ram_and_static_facts_on_request() {
    let plain = kv(&probe(&[]));
    assert!(
        parse_static(&plain).is_none(),
        "static facts only on request"
    );
    let live = parse_live(&plain);
    assert!(live.mem_total.is_some() && live.mem_avail.is_some());
    let full = kv(&probe(&["static", "disk"]));
    let hw = parse_static(&full).expect("static=1 marks the facts");
    assert!(hw.gpus.len() < 64);
    assert!(goway::pool::parse_probe(&probe(&["static"])).is_some());
}
