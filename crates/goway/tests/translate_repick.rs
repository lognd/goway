//! An unpinned run that may land on any OS and meets a host where the command's translation is
//! in doubt picks again among the hosts of this machine's OS, with one note, instead of
//! stopping; a pinned run still stops. The doubtful host is a second fake-ssh host whose probe
//! says it runs Windows; its work dir is removed by the `discard` verb.
#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::Output;

/// A fake ssh like the common one; the host at 127.0.0.2 reports `os=windows` in its probe.
const SSH_WITH_A_WINDOWS_HOST: &str = r#"#!/bin/sh
host=""
while [ $# -gt 0 ]; do
  case "$1" in
    -o|-p|-l) shift 2 ;;
    --) host=$2; shift; shift; break ;;
    *) shift ;;
  esac
done
case "$host" in
  *127.0.0.2*)
    # Its probe (and nothing else prints a line like this) says Windows; the exit code survives.
    rc=$(mktemp)
    { sh -c "$1"; echo $? >"$rc"; } | sed 's/^os=linux$/os=windows/; s/^os=darwin$/os=windows/'
    code=$(cat "$rc"); rm -f "$rc"
    exit "$code" ;;
  *) exec sh -c "$1" ;;
esac
"#;

fn world() -> common::World {
    let w = common::world_with_ssh(SSH_WITH_A_WINDOWS_HOST);
    let path = w.config.join("config.toml");
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str("\n[[host]]\nname = \"far\"\naddress = \"127.0.0.2\"\nmax_jobs = 64\n");
    std::fs::write(path, text).unwrap();
    let tool = w.bin.join("realtool");
    std::fs::write(&tool, "#!/bin/sh\necho \"realtool got: $*\"\n").unwrap();
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
    // Windows has no such program; the laptop's OS has `realtool`.
    std::fs::write(
        w.repo.join("goway.toml"),
        "[translate]\nmytool = { windows = \"no-such-tool-xyz\", linux = \"realtool\", macos = \"realtool\" }\n",
    )
    .unwrap();
    w
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

// frob:ticket 01M43ETRMKFS15MWTTVZHX18YW
// frob:tests crates/goway/src/run.rs::run
#[test]
fn an_unpinned_any_os_run_in_translation_doubt_runs_on_a_host_of_the_laptops_os() {
    let w = world();
    let out = w.run(&[
        "run",
        "--any-os",
        "--prefers",
        "os=windows",
        "--",
        "mytool",
        "a b",
    ]);
    let err = stderr(&out);
    assert!(out.status.success(), "{err}");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("realtool got: a b"),
        "{}\n{err}",
        String::from_utf8_lossy(&out.stdout)
    );
    // The doubtful host was tried first, said once, and the run went to the laptop's OS.
    assert!(err.contains("picked far"), "{err}");
    assert_eq!(
        err.matches("running on a host of this machine's OS instead")
            .count(),
        1,
        "{err}"
    );
    assert!(err.contains("running on local"), "{err}");
    assert!(!err.contains("running on far"), "{err}");
    // The doubtful host's synced work dir is gone, not left for gc.
    assert!(w.work_dirs().is_empty(), "{:?}", w.work_dirs());
}

// frob:ticket 01M43ETRMKFS15MWTTVZHX18YW
// frob:tests crates/goway/src/run.rs::run
#[test]
fn a_pinned_run_in_translation_doubt_still_stops_and_leaves_no_work_dir() {
    let w = world();
    let out = w.run(&["run", "--host", "far", "--", "mytool", "a b"]);
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(125), "{err}");
    assert!(err.contains("no certain equivalent"), "{err}");
    assert!(!err.contains("instead"), "{err}");
    assert!(w.work_dirs().is_empty(), "{:?}", w.work_dirs());
    // An os= need is a pin of the OS: it stops too.
    let out = w.run(&["run", "--needs", "os=windows", "--", "mytool", "a b"]);
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(125), "{err}");
    assert!(!err.contains("instead"), "{err}");
}

/// `discard ROOT RUN_ID` of remote.sh under `home`.
fn discard(home: &Path, run_id: &str) -> Output {
    std::process::Command::new("bash")
        .args([
            goway::remote::SCRIPT_SH_PATH,
            "discard",
            ".cache/goway",
            run_id,
        ])
        .env("HOME", home)
        .output()
        .unwrap()
}

// frob:ticket 01M43ETRMKFS15MWTTVZHX18YW
// frob:tests crates/goway/src/run.rs::discard_work
#[test]
fn discard_removes_only_the_named_work_dir_and_refuses_a_bad_run_id() {
    let home = tempfile::tempdir().unwrap();
    let work = home.path().join(".cache/goway/work");
    for id in ["run-a", "run-b"] {
        std::fs::create_dir_all(work.join(id).join("tree")).unwrap();
    }
    assert!(discard(home.path(), "run-a").status.success());
    assert!(!work.join("run-a").exists());
    assert!(work.join("run-b").exists());
    // A missing dir is fine; a path-shaped id is refused and removes nothing.
    assert!(discard(home.path(), "run-a").status.success());
    assert!(!discard(home.path(), "../work/run-b").status.success());
    assert!(work.join("run-b").exists());
}
