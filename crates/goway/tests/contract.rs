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

// frob:tests crates/goway/src/lib.rs::host_list
// frob:tests crates/goway/src/lib.rs::host_remove
// frob:tests crates/goway/src/paths.rs::Paths.from_env
#[test]
fn host_list_and_remove_through_the_cli() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.toml"),
        "[[host]]\nname = \"helios\"\n\n[[host]]\nname = \"nova\"\nport = 22\n",
    )
    .unwrap();
    let run = |args: &[&str]| {
        goway()
            .arg("--color=never")
            .args(args)
            .env("GOWAY_CONFIG_DIR", dir.path())
            .env("GOWAY_STATE_DIR", dir.path())
            .output()
            .unwrap()
    };
    let out = run(&["host", "list"]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("helios  2222"), "{text}");
    assert!(text.contains("nova    22"), "{text}");

    let out = run(&["host", "remove", "helios"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&run(&["host", "list"]).stdout).into_owned();
    assert!(!text.contains("helios"), "{text}");

    let out = run(&["host", "remove", "helios"]);
    assert_eq!(out.status.code(), Some(125));

    let out = run(&["config", "path"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains(&dir.path().join("config.toml").display().to_string()),
        "{text}"
    );
}
