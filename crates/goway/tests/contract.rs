//! Process-level contract tests: output discipline and exit codes.

use std::path::Path;
use std::process::Command;

fn goway() -> Command {
    Command::new(env!("CARGO_BIN_EXE_goway"))
}

/// Collect every `.rs` file under `dir`.
fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn print_macros_are_denied_outside_render() {
    let manifest =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml"))
            .unwrap();
    assert!(manifest.contains("print_stdout = \"deny\""));
    assert!(manifest.contains("print_stderr = \"deny\""));

    let mut files = Vec::new();
    rust_files(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        let allows = text.contains("allow(clippy::print_stdout");
        let is_render = file.ends_with("render.rs");
        assert_eq!(
            allows,
            is_render,
            "{} must not allow printing",
            file.display()
        );
    }
}

// frob:tests crates/goway/src/lib.rs::main_with
// frob:tests crates/goway/src/lib.rs::init_tracing
// frob:tests crates/goway/src/render.rs::Renderer.error
#[test]
fn goway_failure_exits_125_with_rendered_error() {
    let dir = tempfile::tempdir().unwrap();
    let out = goway()
        .args([
            "--color",
            "never",
            "run",
            "--host",
            "no-such-host",
            "--",
            "true",
        ])
        .env("GOWAY_CONFIG_DIR", dir.path())
        .env("GOWAY_STATE_DIR", dir.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(125));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.starts_with("goway: error:"), "{stderr}");
    assert!(out.stdout.is_empty());
}
