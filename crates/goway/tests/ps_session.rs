//! The `session` verb of `remote.ps1` and goway's client for it: many verbs
//! over one PowerShell process. Like the other contract tests it drives
//! whatever PowerShell the machine has (`GOWAY_PWSH`, `pwsh`, `powershell`
//! on Windows) and passes trivially without one.

use std::path::{Path, PathBuf};
use std::process::Command;

use goway::remote;
use goway::session::Session;

fn pwsh() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(p) = std::env::var_os("GOWAY_PWSH") {
        candidates.push(PathBuf::from(p));
    }
    candidates.push(PathBuf::from("pwsh"));
    if cfg!(windows) {
        candidates.push(PathBuf::from("powershell"));
    }
    candidates.into_iter().find(|c| {
        Command::new(c)
            .args(["-NoProfile", "-Command", "exit 0"])
            .output()
            .is_ok_and(|o| o.status.success())
    })
}

fn session_command(ps: &Path, script: &Path) -> Command {
    let words = vec![script.to_string_lossy().into_owned(), "session".to_owned()];
    let source = format!("{}; exit $LASTEXITCODE", goway::transport::ps_call(&words));
    let mut c = Command::new(ps);
    c.args(goway::transport::POWERSHELL_FLAGS)
        .arg(goway::transport::encoded_command(&source));
    c
}

macro_rules! session {
    () => {{
        let Some(ps) = pwsh() else { return };
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("remote.ps1");
        std::fs::write(&script, remote::SCRIPT_PS).unwrap();
        let root = dir.path().join("root");
        let s = Session::start(session_command(&ps, &script)).expect("session starts");
        (dir, root, s)
    }};
}

const CAP: usize = 1 << 20;

// frob:tests crates/goway/src/session.rs::Session
#[test]
fn one_process_serves_many_verbs_with_their_own_exit_codes_and_streams() {
    let (_dir, root, mut s) = session!();
    let r = s.call(&["ping"], b"", CAP).unwrap();
    assert_eq!(
        (r.code, String::from_utf8_lossy(&r.stdout).trim()),
        (0, "goway-remote ok")
    );
    let root = root.to_string_lossy().into_owned();
    // A verb that reads its input and one that ends with a failure code.
    let r = s.call(&["envfile", &root, "run1"], b"A=1\0", CAP).unwrap();
    assert_eq!(
        r.code,
        125,
        "no work dir yet: {}",
        String::from_utf8_lossy(&r.stderr)
    );
    assert!(String::from_utf8_lossy(&r.stderr).contains("goway-remote"));
    // The process is still there after a failing verb.
    let r = s.call(&["ping"], b"", CAP).unwrap();
    assert_eq!(r.code, 0);
    let r = s.call(&["nope"], b"", CAP).unwrap();
    assert_eq!(r.code, 125);
    assert!(String::from_utf8_lossy(&r.stderr).contains("unknown verb"));
}

// frob:tests crates/goway/src/session.rs::Session
#[test]
fn run_and_nested_sessions_are_refused() {
    let (_dir, root, mut s) = session!();
    let root = root.to_string_lossy().into_owned();
    for verb in ["run", "session"] {
        let r = s.call(&[verb, &root], b"", CAP).unwrap();
        assert_eq!(r.code, 125, "{verb}");
        assert!(
            String::from_utf8_lossy(&r.stderr).contains("not allowed"),
            "{verb}"
        );
    }
    assert_eq!(s.call(&["ping"], b"", CAP).unwrap().code, 0);
}

// frob:tests crates/goway/src/session.rs::Session
#[test]
fn locks_are_released_between_calls() {
    let (_dir, root, mut s) = session!();
    let root = root.to_string_lossy().into_owned();
    // manifest takes the seed lock and answers a (here empty) listing; twice,
    // so a lock held across calls would make the second one wait or fail.
    for _ in 0..2 {
        let r = s.call(&["manifest", &root, "r1/w1"], b"", CAP).unwrap();
        assert_eq!(r.code, 0, "{}", String::from_utf8_lossy(&r.stderr));
    }
}

// frob:tests crates/goway/src/session.rs::Session
#[test]
fn a_session_that_cannot_start_reports_why() {
    let mut cmd = Command::new(if cfg!(windows) { "cmd" } else { "sh" });
    cmd.args(if cfg!(windows) {
        vec!["/c", "echo no 1>&2"]
    } else {
        vec!["-c", "echo no >&2"]
    });
    let err = Session::start(cmd).unwrap_err();
    assert!(err.contains("no"), "{err}");
}
