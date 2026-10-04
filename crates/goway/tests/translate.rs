//! Portable command translation end to end: the `resolve` verb of `remote.sh` (and, where
//! PowerShell exists, `remote.ps1`) and `goway run` through the fake-ssh world. The rules under
//! test: only argv[0] changes, only PATH directories outside goway's state are searched,
//! doubt means no translation, and a guess is never run.
#![cfg(unix)]

mod common;

use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const SCRIPT_SH: &str = include_str!("../src/remote.sh");

fn exe(dir: &Path, name: &str, body: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

/// `resolve` of remote.sh with `spec` on stdin, PATH as given, in `cwd`.
fn resolve(home: &Path, cwd: &Path, path: &str, spec: &str) -> String {
    let mut child = Command::new("bash")
        .args(["-c", SCRIPT_SH, "goway", "resolve", ".cache/goway", "run1"])
        .env("HOME", home)
        .env("PATH", path)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(spec.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn system_path() -> String {
    "/usr/bin:/bin".to_owned()
}

// frob:ticket 01M42FN011Z3NBG491XAHHZCF5
// frob:tests crates/goway/src/remote.rs::SCRIPT_SH
#[test]
fn resolution_searches_absolute_path_directories_only_never_the_cwd_or_the_work_tree() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    let cwd = t.path().join("cwd");
    let bin = t.path().join("bin");
    exe(&cwd, "mytool", "exit 0");
    exe(&bin, "realtool", "exit 0");
    // A look-alike inside goway's own state (where the synced tree lives).
    let state_bin = home.join(".cache/goway/work/run1/tree/bin");
    exe(&state_bin, "mytool", "exit 0");
    let spec = "0;bare;mytool;;\n";
    // Relative and empty PATH entries, the current directory, and the work tree: none counts.
    let path = format!(".::rel:bin:{}:{}", state_bin.display(), system_path());
    assert_eq!(
        resolve(&home, &cwd, &path, spec),
        "none;nothing found on PATH"
    );
    // An absolute directory outside goway's state does, and the answer is the absolute path.
    let path = format!("{}:{}", bin.display(), system_path());
    let ok = resolve(&home, &cwd, &path, "0;bare;realtool;-x,y;\n");
    assert_eq!(ok, format!("ok;{};-x,y", bin.join("realtool").display()));
    // A symlink on PATH that leads into goway's state does not count either.
    let linkdir = t.path().join("links");
    std::fs::create_dir_all(&linkdir).unwrap();
    std::os::unix::fs::symlink(state_bin.join("mytool"), linkdir.join("mytool")).unwrap();
    let path = format!("{}:{}", linkdir.display(), system_path());
    assert_eq!(
        resolve(&home, &cwd, &path, spec),
        "none;nothing found on PATH"
    );
}

// frob:ticket 01M42FN011Z3NBG491XAHHZCF5
// frob:tests crates/goway/src/remote.rs::SCRIPT_SH
#[test]
fn doubt_means_no_translation_and_the_first_certain_tier_wins() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    let bin = t.path().join("bin");
    exe(&bin, "a-tool", "exit 0");
    exe(&bin, "b-tool", "exit 0");
    exe(&bin, "picky", "[ \"$1\" = bad ] && exit 1; exit 0");
    let path = format!("{}:{}", bin.display(), system_path());
    let r = |spec: &str| resolve(&home, t.path(), &path, spec);
    // Two usable candidates in one tier: ambiguous.
    assert_eq!(
        r("0;bare;a-tool;;\n0;bare;b-tool;;\n"),
        "none;several candidates match"
    );
    // The earlier tier wins over a later one.
    assert!(r("1;bare;b-tool;;\n0;bare;a-tool;;\n").contains("a-tool"));
    // A failing self-check rejects the candidate; a passing one keeps it.
    assert_eq!(r("0;bare;picky;;bad\n"), "none;nothing found on PATH");
    assert!(r("0;bare;picky;;ok\n").starts_with("ok;"));
    // The program as given means "leave the command alone".
    assert_eq!(r("0;same;a-tool;;\n1;bare;b-tool;;\n"), "same");
    // Unsafe names, tree paths (a Windows convention) and junk are never matched.
    for bad in [
        "0;bare;../a-tool;;\n",
        "0;bare;a/tool;;\n",
        "0;tree;a-tool;;\n",
        "junk\n",
        "",
        "x;bare;a-tool;;\n",
    ] {
        assert!(r(bad).starts_with("none;"), "{bad:?}: {}", r(bad));
    }
}

fn goway_toml(w: &common::World, text: &str) {
    std::fs::write(w.repo.join("goway.toml"), text).unwrap();
}

fn stderr(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

// frob:ticket 01M42FN011Z3NBG491XAHHZCF5
// frob:tests crates/goway/src/run.rs::translated_command
#[test]
fn a_translate_entry_runs_the_resolved_program_names_it_and_records_it() {
    let w = common::world();
    exe(&w.bin, "realtool", "echo \"realtool got: $*\"");
    goway_toml(
        &w,
        "[translate]\nmytool = { linux = \"realtool\", macos = \"realtool\" }\n",
    );
    let report = w.root.join("report.json");
    let out = w.run(&[
        "run",
        "--report",
        report.to_str().unwrap(),
        "--",
        "mytool",
        "a b",
        "--flag",
    ]);
    let err = stderr(&out);
    assert!(out.status.success(), "{err}");
    // Only argv[0] changed: the other words arrive byte for byte.
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("realtool got: a b --flag"),
        "{}\n{err}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(err.contains("local: mytool -> realtool"), "{err}");
    let r: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report).unwrap()).unwrap();
    assert_eq!(r["translation"]["requested"], "mytool");
    assert!(
        r["translation"]["actual"]
            .as_str()
            .unwrap()
            .ends_with("/realtool"),
        "{r}"
    );
    assert_eq!(r["command"], serde_json::json!(["mytool", "a b", "--flag"]));
}

// frob:ticket 01M42FN011Z3NBG491XAHHZCF5
// frob:tests crates/goway/src/run.rs::translated_command
#[test]
fn a_run_with_no_certain_equivalent_stops_before_the_command_starts() {
    let w = common::world();
    let marker = w.root.join("ran");
    goway_toml(
        &w,
        "[translate]\nmytool = { linux = \"no-such-tool-xyz\", macos = \"no-such-tool-xyz\" }\n",
    );
    let out = w.run(&[
        "run",
        "--",
        "mytool",
        &format!("--touch={}", marker.display()),
    ]);
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(125), "{err}");
    assert!(err.contains("no certain equivalent"), "{err}");
    assert!(err.contains("does not run a guess"), "{err}");
    assert!(!marker.exists());
    // A program without an entry is never sent to the host at all.
    let out = w.run(&["run", "--", "true"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!stderr(&out).contains(" -> "), "{}", stderr(&out));
    // A bad [translate] entry is a config error naming the file.
    goway_toml(
        &w,
        "[translate]\nmytool = { windows = \"C:\\\\x\\\\evil.exe\\\\..\\\\..\" }\n",
    );
    let out = w.run(&["run", "--", "true"]);
    assert_eq!(out.status.code(), Some(125));
    assert!(stderr(&out).contains("goway.toml"), "{}", stderr(&out));
    goway_toml(&w, "[translate]\nmytool = { beos = \"x\" }\n");
    let out = w.run(&["run", "--", "true"]);
    assert_eq!(out.status.code(), Some(125));
    assert!(stderr(&out).contains("beos"), "{}", stderr(&out));
}

/// The PowerShell these tests drive, if there is one (the CI jobs have it).
fn pwsh() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(p) = std::env::var_os("GOWAY_PWSH") {
        candidates.push(PathBuf::from(p));
    }
    candidates.push(PathBuf::from("pwsh"));
    candidates.into_iter().find(|c| {
        Command::new(c)
            .args(["-NoProfile", "-Command", "exit 0"])
            .output()
            .is_ok_and(|o| o.status.success())
    })
}

/// `resolve` of remote.ps1 with `spec` on stdin and PATH as given.
fn resolve_ps(ps: &Path, script: &Path, root: &Path, path: &str, spec: &str) -> String {
    let words = vec![
        script.to_string_lossy().into_owned(),
        "resolve".to_owned(),
        root.to_string_lossy().into_owned(),
        "run1".to_owned(),
    ];
    let source = format!("{}; exit $LASTEXITCODE", goway::transport::ps_call(&words));
    let mut child = Command::new(ps)
        .args(goway::transport::POWERSHELL_FLAGS)
        .arg(goway::transport::encoded_command(&source))
        .env("PATH", path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(spec.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

// frob:ticket 01M42FN011Z3NBG491XAHHZCF5
// frob:tests crates/goway/src/remote.rs::SCRIPT_PS
#[test]
fn powershell_resolves_the_same_way_path_only_tree_files_and_doubt() {
    let Some(ps) = pwsh() else { return };
    let t = tempfile::tempdir().unwrap();
    let script = t.path().join("remote.ps1");
    std::fs::write(&script, goway::remote::SCRIPT_PS).unwrap();
    let root = t.path().join("root");
    let tree = root.join("work/run1/tree");
    let bin = t.path().join("bin");
    exe(&bin, "realtool", "exit 0");
    exe(&tree.join("bin"), "mytool", "exit 0");
    exe(&tree.join("build/Debug"), "app", "exit 0");
    exe(&tree.join("build/Release"), "app", "exit 0");
    exe(&tree.join("solo/Debug"), "one", "exit 0");
    std::fs::write(tree.join("gradlew.bat"), "").unwrap();
    let sep = ":";
    let path = format!(
        ".{sep}rel{sep}{}{sep}{}",
        tree.join("bin").display(),
        "/usr/bin"
    );
    let r = |path: &str, spec: &str| resolve_ps(&ps, &script, &root, path, spec);
    // Relative entries and the work tree never count.
    assert_eq!(r(&path, "0;bare;mytool;;\n"), "none;nothing found");
    // An absolute directory outside goway's state does.
    let good = format!("{}{sep}/usr/bin", bin.display());
    assert_eq!(
        r(&good, "0;bare;realtool;-3;\n"),
        format!("ok;{};-3", bin.join("realtool").display())
    );
    // Work-tree files: a hit keeps its relative path; Debug and Release together are doubt.
    assert_eq!(r(&good, "0;tree;gradlew.bat;;\n"), "ok;.\\gradlew.bat;");
    assert_eq!(r(&good, "0;tree;build/app.exe;;\n"), "none;nothing found");
    assert_eq!(
        r(
            &good,
            "1;tree;build/Debug/app;;\n1;tree;build/Release/app;;\n"
        ),
        "none;several candidates match"
    );
    assert_eq!(
        r(
            &good,
            "1;tree;solo/Debug/one;;\n1;tree;solo/Release/one;;\n"
        ),
        "ok;.\\solo\\Debug\\one;"
    );
    for bad in [
        "0;tree;../x;;\n",
        "0;tree;/etc/passwd;;\n",
        "0;bare;a/b;;\n",
        "junk\n",
        "",
    ] {
        assert!(r(&good, bad).starts_with("none;"), "{bad:?}");
    }
}

// frob:ticket 01M42FN011Z3NBG491XAHHZCF5
// frob:tests crates/goway/src/project.rs::warn_cross_os
#[test]
fn a_translatable_program_counts_as_cross_platform_for_the_cross_os_hint() {
    let w = common::world();
    let path = w.config.join("config.toml");
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str("\n[[host]]\nname = \"winbox\"\nos = \"windows\"\naddress = \"192.0.2.9\"\n");
    std::fs::write(path, text).unwrap();
    let err = stderr(&w.run(&["run", "--", "python3", "--version"]));
    assert!(err.contains("CROSS-OS"), "{err}");
    assert!(err.contains("winbox"), "{err}");
    let err = stderr(&w.run(&["run", "--", "true"]));
    assert!(!err.contains("CROSS-OS"), "{err}");
}
