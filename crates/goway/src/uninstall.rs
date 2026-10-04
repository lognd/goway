//! `goway uninstall`: remove everything goway added, on every helper and
//! on this laptop, in an order that keeps undo possible:
//!
//! 1. each helper: goway's state directory (refused while a run is in
//!    progress), the tools `doctor --fix` installed (as recorded; root
//!    ones with `--rsudo`, system packages are listed, not removed), the
//!    key setup, and the host's entry and pinned key;
//! 2. this laptop: goway's config, keys and state;
//! 3. the goway program, removed the way it was installed: by replaying the
//!    install journal of scripts/install.sh, or by the command of the tool
//!    that put it there (`cargo uninstall`, `uv tool uninstall`,
//!    `pipx uninstall`, `pip uninstall` in a virtual environment), run
//!    last (on Windows the command is printed; the setup program's own
//!    uninstall entry removes a goway-setup install).
//!
//! Without `--everywhere` it lists exactly that and asks first.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::cli::UninstallArgs;
use crate::config::{self, Config, HostConfig};
use crate::doctor::{self, FixRunner as _, Undo};
use crate::error::{Error, Result};
use crate::paths::Paths;
use crate::remote::Call;
use crate::render::Renderer;
use crate::resolve::{self, Lookup, Prober};
use crate::ssh::{self, KeyPolicy};
use crate::state::State;
use crate::sync::Transport as _;
use crate::{hosts, sshsetup};

/// goway's own files in its config dir (everything else is left alone).
pub fn local_files(paths: &Paths) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let fixed = [
        "config.toml",
        "known_hosts",
        "known_hosts.old",
        "id_ed25519",
        "id_ed25519.pub",
    ];
    for name in fixed {
        let p = paths.config_dir.join(name);
        if p.exists() {
            out.push(p);
        }
    }
    if let Ok(entries) = std::fs::read_dir(&paths.config_dir) {
        let mut records: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                    (n.starts_with("ssh-setup-") || n.starts_with("installed-"))
                        && Path::new(n).extension().is_some_and(|e| e == "json")
                })
            })
            .collect();
        records.sort();
        out.extend(records);
    }
    let state = paths.state_file();
    if state.exists() {
        out.push(state);
    }
    out
}

/// The install journal written by scripts/install.sh.
pub fn install_journal() -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".local/state")))
        .unwrap_or_default();
    state.join("goway/install-journal")
}

/// Whether `path` is an absolute path inside `home` with no `..` component.
fn inside(home: &Path, path: &Path) -> bool {
    path.is_absolute()
        && path.starts_with(home)
        && !path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
}

/// The file the install put on PATH: this program itself, or a file named
/// `goway` inside the user's home (`GOWAY_PREFIX` defaults to `~/.local`).
fn is_installed_binary(home: &Path, exe: Option<&Path>, path: &Path) -> bool {
    exe.is_some_and(|e| e == path)
        || (inside(home, path) && path.file_name().is_some_and(|n| n == "goway"))
}

/// The one profile the install edits: `~/.profile`.
fn is_profile(home: &Path, path: &Path) -> bool {
    path == home.join(".profile")
}

/// Replace `path` with `bytes` through a temporary file in the same
/// directory, keeping the file's permissions.
fn rewrite(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension(format!("goway-uninstall.{}", std::process::id()));
    std::fs::write(&tmp, bytes).map_err(|e| Error::io("write", &tmp, e))?;
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        Error::io("write", path, e)
    })
}

/// `content` without the lines equal to `line` (terminators ignored when
/// comparing, kept on every other line, so CRLF files stay CRLF).
fn without_line(content: &[u8], line: &str) -> Vec<u8> {
    let mut kept = Vec::with_capacity(content.len());
    for l in content.split_inclusive(|b| *b == b'\n') {
        let bare = l.strip_suffix(b"\n").unwrap_or(l);
        let bare = bare.strip_suffix(b"\r").unwrap_or(bare);
        if bare != line.as_bytes() {
            kept.extend_from_slice(l);
        }
    }
    kept
}

/// Undo scripts/install.sh by replaying its journal backwards (the same
/// rules as scripts/uninstall.sh: the binary only while it is still the
/// installed build, the exact PATH line, the added newline, created
/// directories only when empty). The journal is a plain file, so it is not
/// trusted: only a file named `goway` under `home` (or this program),
/// `home/.profile`, and directories under `home` are ever touched; other
/// entries are skipped and reported. Returns what it did, for the user.
pub fn revert_install_journal(
    journal: &Path,
    home: &Path,
    exe: Option<&Path>,
) -> Result<Vec<String>> {
    let text = std::fs::read_to_string(journal).map_err(|e| Error::io("read", journal, e))?;
    let mut done = Vec::new();
    let mut dirs = Vec::new();
    let skip = |done: &mut Vec<String>, what: &str, path: &str| {
        tracing::warn!(
            entry = what,
            path,
            "install journal entry outside its scope skipped"
        );
        done.push(format!(
            "skipped journal entry for {path}: not a file the install makes"
        ));
    };
    for entry in text.lines().rev() {
        let (kind, rest) = entry.split_once(' ').unwrap_or((entry, ""));
        match kind {
            "file" => {
                let Some((path, sum)) = rest.rsplit_once(' ') else {
                    continue;
                };
                let path = Path::new(path);
                if !is_installed_binary(home, exe, path) {
                    skip(&mut done, "file", &path.display().to_string());
                    continue;
                }
                match crate::sync::file_sha256(path) {
                    Some(actual) if actual == sum => {
                        std::fs::remove_file(path).map_err(|e| Error::io("remove", path, e))?;
                        done.push(format!("removed {}", path.display()));
                    }
                    Some(_) => {
                        done.push(format!("kept {}: it changed since install", path.display()));
                    }
                    None => {}
                }
            }
            "line" => {
                let mut parts = rest.splitn(3, ' ');
                let (Some(existed), Some(profile), Some(line)) =
                    (parts.next(), parts.next(), parts.next())
                else {
                    continue;
                };
                let profile = Path::new(profile);
                if !is_profile(home, profile) {
                    skip(&mut done, "line", &profile.display().to_string());
                    continue;
                }
                let Ok(content) = std::fs::read(profile) else {
                    continue;
                };
                let kept = without_line(&content, line);
                if kept.len() == content.len() {
                    continue;
                }
                if existed == "0" && kept.is_empty() {
                    std::fs::remove_file(profile).map_err(|e| Error::io("remove", profile, e))?;
                    done.push(format!(
                        "removed {} (created by install)",
                        profile.display()
                    ));
                } else {
                    rewrite(profile, &kept)?;
                    done.push(format!("removed the PATH line from {}", profile.display()));
                }
            }
            "newline" => {
                let profile = Path::new(rest);
                if !is_profile(home, profile) {
                    skip(&mut done, "newline", rest);
                    continue;
                }
                if let Ok(mut content) = std::fs::read(profile)
                    && content.last() == Some(&b'\n')
                {
                    content.pop();
                    rewrite(profile, &content)?;
                }
            }
            "dir" => {
                let dir = PathBuf::from(rest);
                if inside(home, &dir) {
                    dirs.push(dir);
                } else {
                    skip(&mut done, "dir", rest);
                }
            }
            _ => {}
        }
    }
    std::fs::remove_file(journal).map_err(|e| Error::io("remove", journal, e))?;
    for d in dirs {
        if std::fs::remove_dir(&d).is_ok() {
            done.push(format!("removed {}", d.display()));
        }
    }
    Ok(done)
}

/// How the running goway program was installed, found from where it lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Removal {
    /// scripts/install.sh left a journal; replaying it removes the program.
    Journal,
    /// Run `argv` (its first element an absolute path) to remove the program.
    Command {
        /// The install method, for messages ("cargo install", "uv tool install", ...).
        how: &'static str,
        /// The removal command, program first.
        argv: Vec<String>,
    },
    /// The method is known but its tool is not on PATH.
    ToolMissing {
        /// The install method, for messages.
        how: &'static str,
        /// The command to run once the tool is reachable.
        command: String,
    },
    /// Not installed by any method goway knows (a plain download, a build tree).
    Unknown,
}

/// The environment variables and home directory the install locations derive from.
#[derive(Debug, Clone, Default)]
pub struct InstallEnv {
    /// The user's home directory.
    pub home: PathBuf,
    /// `CARGO_HOME`, when set.
    pub cargo_home: Option<PathBuf>,
    /// `UV_TOOL_DIR`, when set.
    pub uv_tool_dir: Option<PathBuf>,
    /// `PIPX_HOME`, when set.
    pub pipx_home: Option<PathBuf>,
    /// `XDG_DATA_HOME`, when set.
    pub xdg_data_home: Option<PathBuf>,
}

impl InstallEnv {
    /// Read the process environment.
    pub fn from_process() -> Self {
        let var = |name: &str| {
            std::env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        Self {
            home: dirs::home_dir().unwrap_or_default(),
            cargo_home: var("CARGO_HOME"),
            uv_tool_dir: var("UV_TOOL_DIR"),
            pipx_home: var("PIPX_HOME"),
            xdg_data_home: var("XDG_DATA_HOME"),
        }
    }

    /// Where `cargo install` puts programs: `$CARGO_HOME/bin`, else `~/.cargo/bin`.
    fn cargo_bin(&self) -> PathBuf {
        self.cargo_home
            .clone()
            .unwrap_or_else(|| self.home.join(".cargo"))
            .join("bin")
    }

    /// uv's tool environments: `$UV_TOOL_DIR`, else `$XDG_DATA_HOME/uv/tools`, else
    /// `~/.local/share/uv/tools`.
    fn uv_tools(&self) -> PathBuf {
        self.uv_tool_dir
            .clone()
            .unwrap_or_else(|| self.data_home().join("uv/tools"))
    }

    /// pipx's virtual environments: `$PIPX_HOME/venvs`, else the data-home
    /// location (`~/.local/share/pipx/venvs`) and the older `~/.local/pipx/venvs`.
    fn pipx_venvs(&self) -> Vec<PathBuf> {
        match &self.pipx_home {
            Some(h) => vec![h.join("venvs")],
            None => vec![
                self.data_home().join("pipx/venvs"),
                self.home.join(".local/pipx/venvs"),
            ],
        }
    }

    fn data_home(&self) -> PathBuf {
        self.xdg_data_home
            .clone()
            .unwrap_or_else(|| self.home.join(".local/share"))
    }
}

/// Whether `path` is inside `root`, comparing as written and with symlinks resolved.
fn is_under(path: &Path, root: &Path) -> bool {
    if path.starts_with(root) {
        return true;
    }
    match (path.canonicalize(), root.canonicalize()) {
        (Ok(p), Ok(r)) => p.starts_with(r),
        _ => false,
    }
}

/// The first executable called `name` on `path_var`, as an absolute path.
fn which_in(name: &str, path_var: Option<&OsStr>) -> Option<PathBuf> {
    let file = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    };
    std::env::split_paths(path_var?)
        .map(|dir| dir.join(&file))
        .find(|c| is_executable(c))
        .map(|c| std::path::absolute(&c).unwrap_or(c))
}

/// Whether `path` is a file the user can run.
fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        path.metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// The virtual environment root holding `exe`: the directory one or two
/// levels above the program's directory that has a `pyvenv.cfg`.
fn venv_root(exe: &Path) -> Option<PathBuf> {
    let dir = exe.parent()?;
    dir.ancestors()
        .skip(1)
        .take(2)
        .find(|d| d.join("pyvenv.cfg").is_file())
        .map(Path::to_path_buf)
}

/// Quote `word` for display in a shell command line when it needs it.
fn shell_word(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_./:=@%+-".contains(c));
    if plain {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

/// `argv` as one copyable command line.
fn command_line(argv: &[String]) -> String {
    argv.iter()
        .map(|w| shell_word(w))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Work out how `exe` was installed and the command that removes it.
/// An install.sh journal wins; then cargo's bin directory, uv's tool
/// directory, pipx's venvs and any other virtual environment, in that order.
/// `path_var` is the PATH the installing tool is looked up on.
pub fn plan_removal(
    exe: &Path,
    env: &InstallEnv,
    path_var: Option<&OsStr>,
    journal_exists: bool,
) -> Removal {
    if journal_exists {
        return Removal::Journal;
    }
    let tool = |how: &'static str, program: &str, args: &[&str]| {
        let tail: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        if let Some(found) = which_in(program, path_var) {
            let mut argv = vec![found.display().to_string()];
            argv.extend(tail);
            Removal::Command { how, argv }
        } else {
            let mut argv = vec![program.to_owned()];
            argv.extend(tail);
            Removal::ToolMissing {
                how,
                command: command_line(&argv),
            }
        }
    };
    if is_under(exe, &env.cargo_bin()) {
        return tool("cargo install", "cargo", &["uninstall", "goway"]);
    }
    if is_under(exe, &env.uv_tools()) {
        return tool("uv tool install", "uv", &["tool", "uninstall", "goway"]);
    }
    if env.pipx_venvs().iter().any(|v| is_under(exe, v)) {
        return tool("pipx install", "pipx", &["uninstall", "goway"]);
    }
    if let (Some(root), Some(dir)) = (venv_root(exe), exe.parent()) {
        let python = dir.join(if cfg!(windows) {
            "python.exe"
        } else {
            "python"
        });
        tracing::debug!(venv = %root.display(), "goway lives in a virtual environment");
        return Removal::Command {
            how: "pip install in a virtual environment",
            argv: vec![
                python.display().to_string(),
                "-m".to_owned(),
                "pip".to_owned(),
                "uninstall".to_owned(),
                "--yes".to_owned(),
                "goway".to_owned(),
            ],
        };
    }
    Removal::Unknown
}

/// The removal for the running program, from the real environment.
fn current_removal() -> Removal {
    let exe = std::env::current_exe().unwrap_or_default();
    let removal = plan_removal(
        &exe,
        &InstallEnv::from_process(),
        std::env::var_os("PATH").as_deref(),
        install_journal().exists(),
    );
    tracing::info!(exe = %exe.display(), ?removal, "how goway was installed");
    removal
}

/// Run the removal command and report the result: `Ok(true)` when it succeeded.
/// On Windows the command is printed instead (a running program cannot delete itself).
fn run_removal(renderer: Renderer, how: &str, argv: &[String]) -> bool {
    let line = command_line(argv);
    if cfg!(windows) {
        renderer.next(format_args!(
            "goway cannot remove its own running program on Windows; run `{line}` in a new terminal"
        ));
        return true;
    }
    tracing::info!(%line, "running the removal command");
    let status = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .status();
    match status {
        Ok(s) if s.success() => {
            renderer.ok(format_args!("removed the goway program ({how}): {line}"));
            true
        }
        Ok(s) => {
            tracing::warn!(%line, code = ?s.code(), "removal command failed");
            renderer.failed(format_args!("`{line}` failed ({s}); run it yourself"));
            false
        }
        Err(e) => {
            tracing::warn!(%line, error = %e, "removal command did not start");
            renderer.failed(format_args!("could not run `{line}`: {e}; run it yourself"));
            false
        }
    }
}

/// What uninstall will do on one helper, for the plan.
fn host_plan(paths: &Paths, config: &Config, host: &HostConfig) -> Vec<String> {
    let mut out = vec![format!(
        "remove goway's state there (~/{})",
        config.defaults.remote_root
    )];
    for item in doctor::load_installed(paths, &host.name) {
        match item.undo() {
            Undo::Run { root: false, .. } => {
                out.push(format!("remove {} (goway installed it)", item.check));
            }
            Undo::Run { command, root: true } => out.push(format!(
                "undo the {} change (needs --rsudo: administrator rights there; runs `{command}`)",
                item.check
            )),
            Undo::Windows {
                admin: false,
                command,
            } => out.push(format!(
                "remove {} (goway installed it; runs `{command}` in PowerShell there)",
                item.check.trim_start_matches(doctor::windows::RECORD_PREFIX)
            )),
            Undo::Windows {
                admin: true,
                command,
            } => out.push(format!(
                "undo {} (administrator rights: goway prints `{command}` to run in an administrator PowerShell there)",
                item.check.trim_start_matches(doctor::windows::RECORD_PREFIX)
            )),
            Undo::KeepPackage => out.push(format!(
                "keep the system package for {} (other software may use it; listed with its removal command)",
                item.check
            )),
            Undo::KeepCargo => out.push(
                "keep rustup and ~/.cargo (they may have existed before goway; remove them yourself if you want)"
                    .to_owned(),
            ),
            Undo::Unknown => out.push(format!(
                "ignore the unknown record `{}` (goway does not run commands from records)",
                item.check
            )),
        }
    }
    if sshsetup::record_path(paths, &host.name).exists() {
        out.push(
            "remove goway's key line from ~/.ssh/authorized_keys and restore its modes".to_owned(),
        );
    }
    out.push("forget the host: config entry, pinned key, cached address".to_owned());
    out
}

fn show_plan(renderer: Renderer, paths: &Paths, config: &Config, removal: &Removal) {
    renderer.headline("goway uninstall will:");
    for host in &config.hosts {
        renderer.line(format_args!("  on {}:", host.name));
        for step in host_plan(paths, config, host) {
            renderer.line(format_args!("    - {step}"));
        }
    }
    renderer.line("  on this laptop:");
    for f in local_files(paths) {
        renderer.line(format_args!("    - delete {}", f.display()));
    }
    match removal {
        Removal::Journal => {
            renderer.line("    - remove the goway program and its PATH line (install journal)");
        }
        Removal::Command { how, argv } => {
            let when = if cfg!(windows) { "print" } else { "run, last," };
            renderer.line(format_args!(
                "    - {when} `{}` to remove the goway program ({how})",
                command_line(argv)
            ));
        }
        Removal::ToolMissing { how, command } => renderer.line(format_args!(
            "    - (goway was installed with {how}, but that tool is not on PATH; run `{command}` yourself)"
        )),
        Removal::Unknown if cfg!(windows) => renderer.line(
            "    - (the goway program: remove it in Settings > Apps, or run goway-setup uninstall)",
        ),
        Removal::Unknown => renderer.line(
            "    - (goway could not tell how its program was installed; remove it yourself)",
        ),
    }
}

/// Run the PowerShell undo `command` on a Windows helper as its user (the
/// transport it was found by); whether it succeeded.
fn windows_undo(found: &resolve::Found, settings: &ssh::Settings, command: &str) -> bool {
    let cmd = crate::transport::command(
        found.kind,
        &found.target,
        settings,
        KeyPolicy::Strict,
        crate::transport::Script::Ps(command),
    );
    match cmd {
        Ok(mut cmd) => cmd
            .stdin(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success()),
        Err(e) => {
            tracing::warn!(error = %e, "cannot start the Windows undo");
            false
        }
    }
}

/// Clean one helper. Errors leave the local record intact so a rerun can
/// finish the job.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)] // the verb's environment, as for doctor
fn clean_host(
    paths: &Paths,
    renderer: Renderer,
    config: &Config,
    host: &HostConfig,
    args: &UninstallArgs,
    lookup: &(dyn Lookup + Sync),
    prober: &(dyn Prober + Sync),
    settings: &ssh::Settings,
) -> Result<()> {
    let mut state = State::load(&paths.state_file())?;
    let found = resolve::resolve_call(
        config,
        host,
        &mut state,
        lookup,
        prober,
        KeyPolicy::Strict,
        &Call::new("ping", &[] as &[&str]),
    )?;
    let transport = crate::sync::SshTransport::of(&found, settings);
    let out = transport.output(&Call::new("purge", &[&config.defaults.remote_root]))?;
    renderer.ok(format_args!(
        "{}: goway's state {}",
        host.name,
        if String::from_utf8_lossy(&out).contains("absent") {
            "was already gone"
        } else {
            "removed"
        }
    ));
    let runner = doctor::SshFixRunner {
        found: &found,
        settings,
    };
    let items = doctor::load_installed(paths, &host.name);
    let mut root_undos = Vec::new();
    for item in &items {
        match item.undo() {
            Undo::Run {
                command,
                root: false,
            } => {
                if runner.run(&command, false) {
                    renderer.ok(format_args!("{}: removed {}", host.name, item.check));
                } else {
                    renderer.warn(format_args!(
                        "{}: could not remove {}",
                        host.name, item.check
                    ));
                }
            }
            Undo::Run {
                command,
                root: true,
            } => root_undos.push(doctor::Fix {
                command,
                root: true,
                why: format!("undoes goway's {} change", item.check),
            }),
            Undo::Windows {
                command,
                admin: false,
            } => {
                if windows_undo(&found, settings, &command) {
                    renderer.ok(format_args!("{}: removed {}", host.name, item.check));
                } else {
                    renderer.warn(format_args!(
                        "{}: could not remove {}",
                        host.name, item.check
                    ));
                }
            }
            Undo::Windows {
                command,
                admin: true,
            } => renderer.next(format_args!(
                "on {}, in an administrator PowerShell: {command}  (undoes goway's {})",
                host.name, item.check
            )),
            Undo::KeepPackage => renderer.note(format_args!(
                "{}: kept the system package goway installed for {}",
                host.name, item.check
            )),
            Undo::KeepCargo => renderer.note(format_args!(
                "{}: kept rustup and ~/.cargo (they may have existed before goway)",
                host.name
            )),
            Undo::Unknown => {
                tracing::warn!(host = %host.name, check = %item.check, "unknown install record ignored");
                renderer.warn(format_args!(
                    "{}: ignored the unknown record `{}`; goway only runs undo actions it knows",
                    host.name, item.check
                ));
            }
        }
    }
    if !root_undos.is_empty() {
        if args.rsudo && doctor::confirm_root(renderer, &host.name, &root_undos, args.yes) {
            let script = format!(
                "set -e\n{}",
                root_undos
                    .iter()
                    .map(|f| f.command.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            if !runner.run(&script, true) {
                renderer.warn(format_args!("{}: the administrator undo failed", host.name));
            }
        } else {
            for f in &root_undos {
                renderer.next(format_args!(
                    "on {}: {}  ({})",
                    host.name,
                    f.display(),
                    f.why
                ));
            }
        }
    }
    let _ = std::fs::remove_file(doctor::installed_path(paths, &host.name));
    if sshsetup::record_path(paths, &host.name).exists() {
        sshsetup::undo(paths, renderer, &host.name)?;
    }
    if Config::load(&paths.config_file())?.host(&host.name).is_ok() {
        config::remove_host(&paths.config_file(), &host.name)?;
        let mut state = State::load(&paths.state_file())?;
        state.forget(&host.name);
        state.save(&paths.state_file())?;
        hosts::forget_key(&paths.known_hosts(), &config::key_alias(&host.name));
    }
    renderer.ok(format_args!("{}: goway is gone from it", host.name));
    Ok(())
}

/// `goway uninstall`.
pub fn uninstall(
    paths: &Paths,
    renderer: Renderer,
    args: &UninstallArgs,
    lookup: &(dyn Lookup + Sync),
    prober: &(dyn Prober + Sync),
    settings: &ssh::Settings,
) -> Result<u8> {
    let config = Config::load(&paths.config_file())?;
    let removal = current_removal();
    show_plan(renderer, paths, &config, &removal);
    if args.dry_run {
        return Ok(0);
    }
    if !args.everywhere && !args.yes {
        let go = crate::render::ask("Remove all of that now? [y/N]: ")
            .is_some_and(|a| matches!(a.trim(), "y" | "Y" | "yes" | "Yes" | "YES"));
        if !go {
            renderer.note("nothing was removed");
            renderer.next("run `goway uninstall --everywhere` to remove it without the question");
            return Ok(0);
        }
    }
    let mut failed = Vec::new();
    for host in &config.hosts {
        if let Err(e) = clean_host(
            paths, renderer, &config, host, args, lookup, prober, settings,
        ) {
            renderer.warn(format_args!("{}: {e}", host.name));
            failed.push(host.name.clone());
        }
    }
    if !failed.is_empty() {
        renderer.failed(format_args!(
            "could not finish on {}; this laptop's goway setup is kept so you can rerun `goway uninstall` when they are on",
            failed.join(", ")
        ));
        return Ok(1);
    }
    for f in local_files(paths) {
        std::fs::remove_file(&f).map_err(|e| Error::io("remove", &f, e))?;
    }
    for dir in [
        paths.control_dir(),
        paths.state_dir.clone(),
        paths.config_dir.clone(),
    ] {
        let _ = std::fs::remove_dir(dir);
    }
    renderer.ok("removed goway's config, keys and state from this laptop");
    match &removal {
        Removal::Journal => {
            let home = dirs::home_dir().unwrap_or_default();
            let exe = std::env::current_exe().ok();
            for line in revert_install_journal(&install_journal(), &home, exe.as_deref())? {
                renderer.ok(line);
            }
            renderer.ok("goway is uninstalled; open a new terminal to refresh PATH");
        }
        Removal::Command { how, argv } => {
            if !run_removal(renderer, how, argv) {
                return Ok(1);
            }
        }
        Removal::ToolMissing { how, command } => renderer.next(format_args!(
            "goway was installed with {how}, but that tool is not on PATH; run `{command}` to remove the program"
        )),
        Removal::Unknown if cfg!(windows) => renderer
            .next("remove the goway program in Settings > Apps (or run goway-setup uninstall)"),
        Removal::Unknown => renderer.next(format_args!(
            "goway could not tell how its program was installed (not cargo, uv, pipx, a virtual environment or install.sh); remove {} yourself",
            std::env::current_exe()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        )),
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::ffi::OsString;

    use super::*;

    fn journal(dir: &Path, lines: &[String]) -> PathBuf {
        let path = dir.join("install-journal");
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        path
    }

    #[test]
    fn journal_entries_outside_the_install_scope_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(home.join("sub")).unwrap();
        let precious = home.join("precious.txt");
        std::fs::write(&precious, "keep me").unwrap();
        let sum = crate::sync::file_sha256(&precious).unwrap();
        let outside = tmp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        let other_profile = home.join("sub/.bashrc");
        std::fs::write(&other_profile, "x\nEVIL\n").unwrap();
        let j = journal(
            tmp.path(),
            &[
                format!("file {} {sum}", precious.display()),
                format!("line 1 {} EVIL", other_profile.display()),
                format!("dir {}", outside.display()),
            ],
        );
        let done = revert_install_journal(&j, &home, None).unwrap();
        assert_eq!(
            done.iter().filter(|d| d.contains("skipped")).count(),
            3,
            "{done:?}"
        );
        assert_eq!(std::fs::read_to_string(&precious).unwrap(), "keep me");
        assert_eq!(
            std::fs::read_to_string(&other_profile).unwrap(),
            "x\nEVIL\n"
        );
        assert!(outside.exists());
        assert!(!j.exists());
    }

    #[test]
    fn a_crlf_profile_keeps_its_bytes_when_the_path_line_goes() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_owned();
        let profile = home.join(".profile");
        let before = b"export A=1\r\nexport B=2\r\n";
        let line = "export PATH=\"/h/.local/bin:$PATH\" # added by goway install";
        let mut after = before.to_vec();
        after.extend_from_slice(format!("{line}\n").as_bytes());
        std::fs::write(&profile, &after).unwrap();
        let j = journal(
            tmp.path(),
            &[format!("line 1 {} {line}", profile.display())],
        );
        revert_install_journal(&j, &home, None).unwrap();
        assert_eq!(std::fs::read(&profile).unwrap(), before);
    }

    fn env(home: &Path) -> InstallEnv {
        InstallEnv {
            home: home.to_owned(),
            ..InstallEnv::default()
        }
    }

    /// A PATH directory holding an executable stub for each of `names`.
    #[cfg(unix)]
    fn tools(dir: &Path, names: &[&str]) -> OsString {
        std::fs::create_dir_all(dir).unwrap();
        for n in names {
            let f = dir.join(n);
            std::fs::write(&f, "#!/bin/sh\n").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        dir.as_os_str().to_owned()
    }

    #[cfg(unix)]
    fn argv(r: &Removal) -> Vec<&str> {
        match r {
            Removal::Command { argv, .. } => argv.iter().map(String::as_str).collect(),
            other => panic!("expected a command, got {other:?}"),
        }
    }

    // frob:tests crates/goway/src/uninstall.rs::plan_removal
    #[cfg(unix)]
    #[test]
    fn each_install_location_names_its_own_removal_command() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let path = tools(&tmp.path().join("bin"), &["cargo", "uv", "pipx"]);
        let bin = tmp.path().join("bin");
        let e = env(&home);
        let cases = [
            (
                home.join(".cargo/bin/goway"),
                "cargo",
                vec!["uninstall", "goway"],
            ),
            (
                home.join(".local/share/uv/tools/goway/bin/goway"),
                "uv",
                vec!["tool", "uninstall", "goway"],
            ),
            (
                home.join(".local/share/pipx/venvs/goway/bin/goway"),
                "pipx",
                vec!["uninstall", "goway"],
            ),
            (
                home.join(".local/pipx/venvs/goway/bin/goway"),
                "pipx",
                vec!["uninstall", "goway"],
            ),
        ];
        for (exe, tool, rest) in cases {
            let r = plan_removal(&exe, &e, Some(&path), false);
            let got = argv(&r);
            assert_eq!(got[0], bin.join(tool).display().to_string(), "{exe:?}");
            assert_eq!(&got[1..], rest.as_slice(), "{exe:?}");
        }
    }

    // frob:tests crates/goway/src/uninstall.rs::plan_removal
    #[cfg(unix)]
    #[test]
    fn environment_variables_move_the_install_locations() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tools(&tmp.path().join("bin"), &["cargo", "uv", "pipx"]);
        let e = InstallEnv {
            home: tmp.path().join("home"),
            cargo_home: Some(tmp.path().join("ch")),
            uv_tool_dir: Some(tmp.path().join("uvt")),
            pipx_home: Some(tmp.path().join("px")),
            xdg_data_home: None,
        };
        let r = plan_removal(&tmp.path().join("ch/bin/goway"), &e, Some(&path), false);
        assert!(
            matches!(
                r,
                Removal::Command {
                    how: "cargo install",
                    ..
                }
            ),
            "{r:?}"
        );
        let r = plan_removal(
            &tmp.path().join("uvt/goway/bin/goway"),
            &e,
            Some(&path),
            false,
        );
        assert!(
            matches!(
                r,
                Removal::Command {
                    how: "uv tool install",
                    ..
                }
            ),
            "{r:?}"
        );
        let r = plan_removal(
            &tmp.path().join("px/venvs/goway/bin/goway"),
            &e,
            Some(&path),
            false,
        );
        assert!(
            matches!(
                r,
                Removal::Command {
                    how: "pipx install",
                    ..
                }
            ),
            "{r:?}"
        );
        // the default locations no longer apply once the variables are set
        let r = plan_removal(
            &tmp.path().join("home/.cargo/bin/goway"),
            &e,
            Some(&path),
            false,
        );
        assert_eq!(r, Removal::Unknown);
    }

    // frob:tests crates/goway/src/uninstall.rs::plan_removal
    #[cfg(unix)]
    #[test]
    fn a_virtual_environment_removes_itself_with_its_own_pip() {
        let tmp = tempfile::tempdir().unwrap();
        let venv = tmp.path().join("work/.venv");
        std::fs::create_dir_all(venv.join("bin")).unwrap();
        std::fs::write(venv.join("pyvenv.cfg"), "home = /usr/bin\n").unwrap();
        let r = plan_removal(&venv.join("bin/goway"), &env(tmp.path()), None, false);
        let python = venv.join("bin/python").display().to_string();
        assert_eq!(
            argv(&r),
            [python.as_str(), "-m", "pip", "uninstall", "--yes", "goway"]
        );
    }

    // frob:tests crates/goway/src/uninstall.rs::plan_removal
    #[test]
    fn journal_wins_and_unknown_locations_are_unknown() {
        let tmp = tempfile::tempdir().unwrap();
        let e = env(tmp.path());
        let cargo_exe = tmp.path().join(".cargo/bin/goway");
        assert_eq!(plan_removal(&cargo_exe, &e, None, true), Removal::Journal);
        let elsewhere = tmp.path().join("downloads/goway");
        assert_eq!(plan_removal(&elsewhere, &e, None, false), Removal::Unknown);
    }

    // frob:tests crates/goway/src/uninstall.rs::plan_removal
    #[test]
    fn a_missing_tool_is_reported_not_guessed() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join(".local/share/uv/tools/goway/bin/goway");
        let r = plan_removal(&exe, &env(tmp.path()), Some(OsStr::new("")), false);
        assert_eq!(
            r,
            Removal::ToolMissing {
                how: "uv tool install",
                command: "uv tool uninstall goway".to_owned()
            }
        );
    }

    #[test]
    fn command_lines_quote_what_a_shell_would_split() {
        let line = command_line(&["/a b/uv".to_owned(), "tool".to_owned()]);
        assert_eq!(line, "'/a b/uv' tool");
    }
}
