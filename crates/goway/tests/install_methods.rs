//! `goway uninstall` recognises how it was installed (cargo, uv, pipx, a
//! virtual environment) from where the program lives, names the exact
//! removal command in its plan, and runs that command last.
//!
//! Each test copies the real goway binary into a fake layout under a
//! temporary `$HOME` and puts a stub of the installing tool first on PATH.
//! The stub records its arguments instead of removing anything.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A fake machine: a home directory, a directory of stub tools, a log the stubs write.
struct Machine {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    bin: PathBuf,
    log: PathBuf,
}

impl Machine {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let bin = tmp.path().join("tools");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        let log = tmp.path().join("calls.log");
        Self {
            _tmp: tmp,
            home,
            bin,
            log,
        }
    }

    /// Put a stub `name` on PATH that logs its arguments and exits with `code`.
    fn stub(&self, name: &str, code: i32) {
        let file = self.bin.join(name);
        std::fs::write(
            &file,
            format!(
                "#!/bin/sh\necho \"$0 $*\" >> '{}'\nexit {code}\n",
                self.log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Copy the goway binary to `home/<rel>` and return the copy.
    fn install_at(&self, rel: &str) -> PathBuf {
        let dest = self.home.join(rel);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::copy(env!("CARGO_BIN_EXE_goway"), &dest).unwrap();
        dest
    }

    /// Write goway's config so the uninstall has something to remove.
    fn config(&self) -> PathBuf {
        let dir = self.home.join(".config/goway");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("config.toml");
        std::fs::write(&file, "").unwrap();
        file
    }

    /// Run `goway <args>` from `exe` with only this machine's PATH and home.
    fn run(&self, exe: &Path, args: &[&str]) -> Output {
        Command::new(exe)
            .args(["--color", "never"])
            .args(args)
            .env_clear()
            .env("HOME", &self.home)
            // macOS ignores XDG variables, so name the config dir outright.
            .env("GOWAY_CONFIG_DIR", self.home.join(".config/goway"))
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .output()
            .unwrap()
    }

    /// What the stubs were called with, one line per call.
    fn calls(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// The plan names the command, the confirmation runs it, and the config is gone first.
fn expect_method(rel: &str, tool: &str, args: &str) {
    let m = Machine::new();
    m.stub(tool, 0);
    let exe = m.install_at(rel);
    let config = m.config();
    let want = format!("{} {args}", m.bin.join(tool).display());

    let dry = m.run(&exe, &["uninstall", "--dry-run"]);
    assert!(dry.status.success(), "{}", text(&dry));
    assert!(
        text(&dry).contains(&want),
        "plan lacks `{want}`:\n{}",
        text(&dry)
    );
    assert_eq!(m.calls(), "", "a dry run must not run anything");
    assert!(config.exists());

    let out = m.run(&exe, &["uninstall", "--yes"]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(m.calls().trim(), want, "{}", text(&out));
    assert!(!config.exists(), "the config goes before the program");
}

// frob:tests crates/goway/src/uninstall.rs::plan_removal
#[test]
fn cargo_install_is_removed_with_cargo_uninstall() {
    expect_method(".cargo/bin/goway", "cargo", "uninstall goway");
}

// frob:tests crates/goway/src/uninstall.rs::plan_removal
#[test]
fn uv_tool_install_is_removed_with_uv_tool_uninstall() {
    expect_method(
        ".local/share/uv/tools/goway/bin/goway",
        "uv",
        "tool uninstall goway",
    );
}

// frob:tests crates/goway/src/uninstall.rs::plan_removal
#[test]
fn pipx_install_is_removed_with_pipx_uninstall() {
    expect_method(
        ".local/share/pipx/venvs/goway/bin/goway",
        "pipx",
        "uninstall goway",
    );
}

// frob:tests crates/goway/src/uninstall.rs::plan_removal
#[test]
fn the_older_pipx_location_is_recognised_too() {
    expect_method(
        ".local/pipx/venvs/goway/bin/goway",
        "pipx",
        "uninstall goway",
    );
}

// frob:tests crates/goway/src/uninstall.rs::plan_removal
#[test]
fn a_virtual_environment_runs_its_own_pip() {
    let m = Machine::new();
    let exe = m.install_at("proj/.venv/bin/goway");
    let venv = exe.parent().unwrap().parent().unwrap();
    std::fs::write(venv.join("pyvenv.cfg"), "home = /usr/bin\n").unwrap();
    // the venv's python is a stub that logs, standing in for `python -m pip`
    let python = venv.join("bin/python");
    std::fs::write(
        &python,
        format!("#!/bin/sh\necho \"$0 $*\" >> '{}'\n", m.log.display()),
    )
    .unwrap();
    std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755)).unwrap();
    let config = m.config();
    let out = m.run(&exe, &["uninstall", "--yes"]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(
        m.calls().trim(),
        format!("{} -m pip uninstall --yes goway", python.display())
    );
    assert!(!config.exists());
}

// frob:tests crates/goway/src/uninstall.rs::plan_removal
#[test]
fn a_failing_removal_command_fails_the_uninstall() {
    let m = Machine::new();
    m.stub("cargo", 3);
    let exe = m.install_at(".cargo/bin/goway");
    let out = m.run(&exe, &["uninstall", "--yes"]);
    assert!(!out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains("cargo uninstall goway"),
        "{}",
        text(&out)
    );
}

// frob:tests crates/goway/src/uninstall.rs::plan_removal
#[test]
fn a_tool_missing_from_path_is_named_not_run() {
    let m = Machine::new();
    let exe = m.install_at(".local/share/uv/tools/goway/bin/goway");
    let out = m.run(&exe, &["uninstall", "--yes"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains("uv tool uninstall goway"),
        "{}",
        text(&out)
    );
    assert!(text(&out).contains("not on PATH"), "{}", text(&out));
}

// frob:tests crates/goway/src/uninstall.rs::plan_removal
#[test]
fn an_unknown_location_gets_a_clear_message() {
    let m = Machine::new();
    let exe = m.install_at("Downloads/goway");
    let out = m.run(&exe, &["uninstall", "--yes"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("could not tell how"), "{}", text(&out));
    assert_eq!(m.calls(), "");
    assert!(exe.exists(), "an unknown install is never deleted");
}
