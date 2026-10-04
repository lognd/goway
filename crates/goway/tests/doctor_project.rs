//! `goway doctor` run inside a project checks what that project needs and
//! nothing else, through the fake-ssh world (the fake host is this machine,
//! so which tools exist varies; the tests look at which rows appear and at
//! the verdicts that cannot depend on the machine).
#![cfg(unix)]

mod common;

fn doctor(w: &common::World) -> (std::process::Output, String, String) {
    let out = w.run(&["doctor"]);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    (out, stdout, stderr)
}

fn has_row(table: &str, name: &str) -> bool {
    table
        .lines()
        .any(|l| l.split_whitespace().next() == Some(name))
}

// frob:tests crates/goway/src/doctor/projneeds.rs::analyse
#[test]
fn a_cmake_project_is_checked_for_cmake_and_a_compiler_and_never_for_cargo() {
    let w = common::world();
    std::fs::write(
        w.repo.join("CMakeLists.txt"),
        "cmake_minimum_required(VERSION 3.20)\nproject(x LANGUAGES CXX)\n",
    )
    .unwrap();
    let (_, table, err) = doctor(&w);
    assert!(err.contains("project needs: C/C++"), "{err}");
    for row in ["cmake", "c++", "make", "ccache"] {
        assert!(has_row(&table, row), "{row} missing\n{table}");
    }
    for row in ["cargo", "cargo-nextest", "sccache", "node"] {
        assert!(!has_row(&table, row), "{row} is not needed\n{table}");
    }
}

// frob:tests crates/goway/src/doctor/projneeds.rs::analyse
#[test]
fn a_rust_project_keeps_the_cargo_checks_and_a_node_project_gets_its_own() {
    let w = common::world();
    std::fs::write(w.repo.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
    let (_, table, _) = doctor(&w);
    assert!(has_row(&table, "cargo"), "{table}");
    std::fs::remove_file(w.repo.join("Cargo.toml")).unwrap();
    std::fs::write(w.repo.join("package.json"), "{}").unwrap();
    std::fs::write(w.repo.join("pnpm-lock.yaml"), "").unwrap();
    let (_, table, err) = doctor(&w);
    assert!(err.contains("project needs: Node"), "{err}");
    assert!(
        has_row(&table, "node") && has_row(&table, "pnpm"),
        "{table}"
    );
    assert!(!has_row(&table, "cargo"), "{table}");
}

// frob:tests crates/goway/src/doctor/projneeds.rs::analyse
// frob:tests crates/goway/src/doctor/projneeds.rs::checks
#[test]
fn goway_toml_toolchain_is_checked_and_a_missing_tool_fails_the_run() {
    let w = common::world();
    std::fs::write(
        w.repo.join("goway.toml"),
        "[toolchain]\ntools = [\"goway-no-such-tool\"]\nsh = \">=999\"\n",
    )
    .unwrap();
    let (out, table, _) = doctor(&w);
    let row = |name: &str| {
        table
            .lines()
            .find(|l| l.split_whitespace().next() == Some(name))
            .unwrap_or_else(|| panic!("{name} row missing\n{table}"))
            .to_owned()
    };
    assert!(row("goway-no-such-tool").contains("FAIL"), "{table}");
    assert!(
        row("sh").contains("FAIL") && row("sh").contains("needs >=999"),
        "{table}"
    );
    assert_eq!(out.status.code(), Some(1));
}

// frob:tests crates/goway/src/doctor/projneeds.rs::analyse
#[test]
fn a_bad_tool_name_in_goway_toml_is_a_config_error_not_a_command() {
    let w = common::world();
    std::fs::write(
        w.repo.join("goway.toml"),
        "[toolchain]\ntools = [\"x; touch pwned\"]\n",
    )
    .unwrap();
    let (out, _, err) = doctor(&w);
    assert_eq!(out.status.code(), Some(125), "{err}");
    assert!(err.contains("not a tool name"), "{err}");
    assert!(!w.repo.join("pwned").exists());
}
