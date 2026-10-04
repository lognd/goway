//! `CMake`'s own interfaces on a helper, through the fake-ssh world: the File
//! API query every run leaves in a slot tree's build directories, the
//! replies read back from there, and one traced configure in a labelled
//! scratch directory. The real replies come from real `CMake` runs (see
//! `tests/fixtures/cmake_api`); the configure tests run real `cmake` and are
//! skipped where it is not installed.
#![cfg(unix)]

mod common;

use std::path::Path;

use goway::cmakeapi::{self, Presence};
use goway::remote::{Call, split_frame};

fn cmake_installed() -> bool {
    std::process::Command::new("cmake")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn copy_project(from: &str, to: &Path) {
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(from);
    for e in std::fs::read_dir(src).unwrap() {
        let e = e.unwrap();
        std::fs::copy(e.path(), to.join(e.file_name())).unwrap();
    }
}

fn job(w: &common::World, extra: &[&str], script: &str) -> std::process::Output {
    let mut args = vec!["run"];
    args.extend_from_slice(extra);
    args.extend(["--", "sh", "-c", script]);
    w.run(&args)
}

/// Run `call` the way a helper call runs: through the script on this machine.
fn call(w: &common::World, call: &Call) -> Vec<u8> {
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(call.bash())
        .env("HOME", &w.root)
        // Compiler caches inherited from an outer goway job point at that
        // job's sccache server, whose temp dir is not this test's.
        .env_remove("SCCACHE_SERVER_UDS")
        .env_remove("SCCACHE_SERVER_PORT")
        .env_remove("CMAKE_C_COMPILER_LAUNCHER")
        .env_remove("CMAKE_CXX_COMPILER_LAUNCHER")
        .env_remove("RUSTC_WRAPPER")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    split_frame(out.stdout).1
}

fn remote_root(w: &common::World) -> String {
    w.remote.to_string_lossy().into_owned()
}

// frob:tests crates/goway/src/cmakeapi.rs::replies_call
#[test]
fn a_run_leaves_the_file_api_query_in_the_build_directory_of_a_cmake_project() {
    let w = common::world();
    copy_project("cmakeplain", &w.repo);
    let o = job(
        &w,
        &[],
        "cat build/.cmake/api/v1/query/client-goway/query.json",
    );
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let text = String::from_utf8_lossy(&o.stdout);
    for kind in ["codemodel", "cache", "cmakeFiles", "toolchains"] {
        assert!(text.contains(kind), "{text}");
    }
    // Not a CMake project: nothing is created.
    let w = common::world();
    let o = job(&w, &[], "ls -A");
    assert!(!String::from_utf8_lossy(&o.stdout).contains("build"));
}

// frob:tests crates/goway/src/cmakeapi.rs::replies_call
#[test]
fn a_directory_with_the_projects_own_files_is_not_a_build_directory_but_an_existing_cmake_cache_is()
{
    let w = common::world();
    copy_project("cmakeplain", &w.repo);
    std::fs::create_dir(w.repo.join("build")).unwrap();
    std::fs::write(w.repo.join("build/notes.txt"), "mine\n").unwrap();
    std::fs::create_dir(w.repo.join("cmake-build-debug")).unwrap();
    std::fs::write(w.repo.join("cmake-build-debug/CMakeCache.txt"), "# cache\n").unwrap();
    let o = job(
        &w,
        &[],
        "ls -A build cmake-build-debug/.cmake/api/v1/query/client-goway",
    );
    let text = String::from_utf8_lossy(&o.stdout);
    assert!(
        text.contains("notes.txt") && !text.contains(".cmake\n"),
        "{text}"
    );
    assert!(text.contains("query.json"), "{text}");
}

// frob:tests crates/goway/src/cmakeapi.rs::replies_call
#[test]
fn the_newest_replies_in_a_slot_tree_come_back_framed_and_parse() {
    let w = common::world();
    copy_project("cmakeplain", &w.repo);
    let fx =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cmake_api/cmakeplain/reply");
    std::fs::create_dir(w.repo.join("fx")).unwrap();
    for e in std::fs::read_dir(fx).unwrap() {
        let e = e.unwrap();
        std::fs::copy(e.path(), w.repo.join("fx").join(e.file_name())).unwrap();
    }
    // Stand in for a configure: put its replies where CMake writes them.
    let o = job(
        &w,
        &[],
        "mkdir -p build/.cmake/api/v1/reply && cp fx/*.json build/.cmake/api/v1/reply/",
    );
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let repo = goway::repo::Repo::discover(&w.repo).unwrap();
    let bytes = call(&w, &cmakeapi::replies_call(&remote_root(&w), &repo.id));
    let bundle = cmakeapi::parse_bundle(&bytes).unwrap();
    assert_eq!(bundle.builds.len(), 1, "{bundle:?}");
    assert!(
        bundle.builds[0].label.ends_with("/build"),
        "{}",
        bundle.builds[0].label
    );
    let replies = cmakeapi::read_replies(&bundle.builds[0].files).unwrap();
    assert_eq!(replies.cmake_version.as_deref(), Some("3.28.3"));
    assert!(
        replies
            .packages
            .iter()
            .any(|p| p.name == "Catch2" && !p.found)
    );
    // A repository nobody ran in: just the header.
    let none = call(
        &w,
        &cmakeapi::replies_call(&remote_root(&w), "0123456789abcdef"),
    );
    assert_eq!(
        cmakeapi::parse_bundle(&none).unwrap(),
        cmakeapi::Bundle::default()
    );
}

/// Sync `project` into a kept run and return its run id (the work dir the configure verb reads).
fn kept_run(w: &common::World, project: &str) -> String {
    copy_project(project, &w.repo);
    let o = job(w, &["--keep"], "true");
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let dirs = w.work_dirs();
    assert_eq!(dirs.len(), 1, "{dirs:?}");
    dirs[0].clone()
}

// frob:tests crates/goway/src/cmakeapi.rs::configure_call
#[test]
fn a_traced_configure_runs_in_the_work_dir_reports_replies_and_leaves_nothing_behind() {
    if !cmake_installed() {
        return;
    }
    let w = common::world();
    let run_id = kept_run(&w, "cmakeplain");
    let bytes = call(
        &w,
        &cmakeapi::configure_call(&remote_root(&w), &run_id, 120),
    );
    let bundle = cmakeapi::parse_bundle(&bytes).unwrap();
    assert_eq!(
        bundle.rc,
        Some(0),
        "{}",
        String::from_utf8_lossy(&bundle.files["stderr"])
    );
    let trace =
        cmakeapi::parse_trace(&String::from_utf8_lossy(&bundle.files["trace.jsonl"])).unwrap();
    let names: Vec<_> = trace.find_packages().into_iter().map(|p| p.name).collect();
    assert_eq!(names, ["Threads", "PkgConfig", "Catch2"], "{names:?}");
    let configure = bundle
        .builds
        .iter()
        .find(|b| b.label == "configure")
        .unwrap();
    let replies = cmakeapi::read_replies(&configure.files).unwrap();
    assert!(!replies.compilers.is_empty());
    let a = cmakeapi::analyse(Some(&replies), Some(&trace), None, bundle.rc);
    assert!(a.system.iter().all(|n| n.name != "Threads"));
    assert!(
        w.work_dirs().is_empty(),
        "the scratch work dir is removed afterwards: {:?}",
        w.work_dirs()
    );
}

// frob:tests crates/goway/src/cmakeapi.rs::analyse
#[test]
fn a_find_package_of_a_library_the_host_lacks_fails_naming_the_package_and_its_install_command() {
    if !cmake_installed() {
        return;
    }
    let w = common::world();
    let run_id = kept_run(&w, "cmakefind");
    let bytes = call(
        &w,
        &cmakeapi::configure_call(&remote_root(&w), &run_id, 120),
    );
    let bundle = cmakeapi::parse_bundle(&bytes).unwrap();
    let stderr = String::from_utf8_lossy(&bundle.files["stderr"]).into_owned();
    let trace =
        cmakeapi::parse_trace(&String::from_utf8_lossy(&bundle.files["trace.jsonl"])).unwrap();
    let a = cmakeapi::analyse(None, Some(&trace), Some(&stderr), bundle.rc);
    let gtest = a
        .system
        .iter()
        .find(|n| n.name == "GTest")
        .expect("find_package(GTest) is in the trace");
    if bundle.rc == Some(0) {
        // This machine has GoogleTest installed: nothing is missing.
        assert_eq!(gtest.presence, Presence::Present);
        return;
    }
    assert_eq!(gtest.presence, Presence::Missing, "{stderr}");
    assert_eq!(gtest.package.unwrap().apt, "libgtest-dev");
    assert!(w.work_dirs().is_empty());
}

fn doctor(w: &common::World, args: &[&str]) -> (String, String) {
    let mut all = vec!["doctor"];
    all.extend_from_slice(args);
    let o = w.run(&all);
    (
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

// frob:tests crates/goway/src/doctor/cmakecheck.rs::host_checks
#[test]
fn doctor_configure_names_the_missing_package_with_its_install_command_and_leaves_no_scratch_dir() {
    if !cmake_installed() {
        return;
    }
    let w = common::world();
    copy_project("cmakefind", &w.repo);
    let (out, err) = doctor(&w, &["--configure", "--all"]);
    assert!(out.contains("cmake: GTest"), "{out}\n{err}");
    if !out.contains("found (") {
        // GoogleTest is not installed here, as on both real helpers.
        assert!(out.contains("libgtest-dev"), "{out}");
        assert!(out.contains("FAIL"), "{out}");
    }
    assert!(err.contains("configuring on"), "{err}");
    assert!(
        w.work_dirs().is_empty(),
        "the snapshot's work dir is removed: {:?}",
        w.work_dirs()
    );
}

// frob:tests crates/goway/src/doctor/cmakecheck.rs::host_checks
#[test]
fn doctor_reads_the_replies_a_normal_configure_left_without_running_anything() {
    let w = common::world();
    copy_project("cmakeplain", &w.repo);
    let fx =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cmake_api/cmakeplain/reply");
    std::fs::create_dir(w.repo.join("fx")).unwrap();
    for e in std::fs::read_dir(fx).unwrap() {
        let e = e.unwrap();
        std::fs::copy(e.path(), w.repo.join("fx").join(e.file_name())).unwrap();
    }
    let o = job(
        &w,
        &[],
        "mkdir -p build/.cmake/api/v1/reply && cp fx/*.json build/.cmake/api/v1/reply/",
    );
    assert_eq!(o.status.code(), Some(0));
    let (out, err) = doctor(&w, &["--all"]);
    assert!(out.contains("CMake replies"), "{out}\n{err}");
    assert!(out.contains("cmake 3.28.3"), "{out}");
    assert!(
        out.contains("cmake: Catch2"),
        "an optional package CMake did not find: {out}"
    );
    assert!(out.contains("pkg-config"), "{out}");
    assert!(!err.contains("configuring on"), "{err}");
}

// frob:tests crates/goway/src/doctor.rs::doctor
#[test]
fn configure_in_a_project_without_cmakelists_says_so_and_runs_nothing() {
    let w = common::world();
    let (_, err) = doctor(&w, &["--configure"]);
    assert!(err.contains("not a CMake project"), "{err}");
    assert!(w.work_dirs().is_empty());
}

// frob:tests crates/goway/src/cmakeapi.rs::configure_call
#[test]
fn a_scratch_dir_a_killed_configure_left_behind_is_an_ordinary_labelled_work_dir_gc_removes() {
    let w = common::world();
    let run_id = kept_run(&w, "cmakeplain");
    // What a killed configure leaves: the snapshot's work dir and its scratch trees.
    let work = w.remote.join("work").join(&run_id);
    std::fs::create_dir_all(work.join("cfg-build")).unwrap();
    assert!(
        work.join("meta.json").is_file(),
        "labelled with repository and worktree"
    );
    let old = std::process::Command::new("touch")
        .args(["-d", "2000-01-01"])
        .arg(work.join("meta.json"))
        .arg(&work)
        .status()
        .unwrap();
    assert!(old.success());
    assert_eq!(job(&w, &[], "true").status.code(), Some(0));
    common::wait_for("gc to remove the expired scratch", || !work.exists());
}
