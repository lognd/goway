//! C and C++ builds stay warm: `CMake` compiler launchers and the CPM source
//! cache are pointed at per-repository caches, never over the user's settings.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

mod common;

use common::{World, world};

const SHOW: &str = "echo C=${CMAKE_C_COMPILER_LAUNCHER-unset} X=${CMAKE_CXX_COMPILER_LAUNCHER-unset} \
CPM=${CPM_SOURCE_CACHE-unset} FC=${FETCHCONTENT_BASE_DIR-unset} SD=${SCCACHE_DIR-unset} CD=${CCACHE_DIR-unset}";

/// Put an executable stand-in for `name` in the world's bin directory.
fn fake_tool(bin: &Path, name: &str) {
    let path = bin.join(name);
    std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Run `SHOW` through goway with a home that has no cargo environment, so
/// only the fake tools in the world's bin directory can be found.
fn show(w: &World, extra: &[&str], path_tail: &str) -> String {
    let mut args = vec!["run"];
    args.extend_from_slice(extra);
    args.extend(["--", "sh", "-c", SHOW]);
    let path = format!("{}:{path_tail}", w.bin.display());
    let out = w
        .goway(&args)
        .env("HOME", &w.root)
        .env("PATH", path)
        .env_remove("CMAKE_C_COMPILER_LAUNCHER")
        .env_remove("CMAKE_CXX_COMPILER_LAUNCHER")
        .env_remove("CPM_SOURCE_CACHE")
        .env_remove("CCACHE_DIR")
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Whether a real sccache or ccache sits in the system directories (the
/// tests that need none of them are then skipped).
fn system_has_cache_tool() -> bool {
    ["/usr/bin", "/bin"].iter().any(|d| {
        ["sccache", "ccache"]
            .iter()
            .any(|t| Path::new(d).join(t).exists())
    })
}

#[test]
fn sccache_becomes_the_cmake_launcher_with_a_per_repository_cache() {
    let w = world();
    fake_tool(&w.bin, "sccache");
    let text = show(&w, &[], "/usr/bin:/bin");
    assert!(text.contains("C=sccache X=sccache"), "{text}");
    assert!(text.contains("/cache/"), "per-repository cache dir: {text}");
    assert!(text.contains("/cpm FC=unset"), "{text}");
    assert!(text.contains("SD="), "{text}");
    assert!(!text.contains("SD=unset"), "sccache dir set: {text}");
}

#[test]
fn ccache_is_used_when_sccache_is_absent() {
    if system_has_cache_tool() {
        return;
    }
    let w = world();
    fake_tool(&w.bin, "ccache");
    let text = show(&w, &[], "/usr/bin:/bin");
    assert!(text.contains("C=ccache X=ccache"), "{text}");
    assert!(!text.contains("CD=unset"), "ccache dir set: {text}");
    assert!(text.contains("SD=unset"), "{text}");
}

#[test]
fn no_cache_tool_means_no_launcher_but_the_cpm_cache_is_still_shared() {
    if system_has_cache_tool() {
        return;
    }
    let w = world();
    let text = show(&w, &[], "/usr/bin:/bin");
    assert!(text.contains("C=unset X=unset"), "{text}");
    assert!(text.contains("/cpm FC=unset"), "{text}");
}

#[test]
fn settings_of_the_user_are_left_untouched() {
    let w = world();
    fake_tool(&w.bin, "sccache");
    let text = show(
        &w,
        &[
            "-e",
            "CMAKE_C_COMPILER_LAUNCHER=mine",
            "-e",
            "CMAKE_CXX_COMPILER_LAUNCHER=",
            "-e",
            "CPM_SOURCE_CACHE=/my/cpm",
            "-e",
            "FETCHCONTENT_BASE_DIR=/my/deps",
        ],
        "/usr/bin:/bin",
    );
    assert!(text.contains("C=mine X= CPM=/my/cpm FC=/my/deps"), "{text}");
}
