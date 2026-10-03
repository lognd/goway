//! `goway uninstall`: remove everything goway added, on every helper and
//! on this laptop, in an order that keeps undo possible:
//!
//! 1. each helper: goway's state directory (refused while a run is in
//!    progress), the tools `doctor --fix` installed (as recorded; root
//!    ones with `--rsudo`, system packages are listed, not removed), the
//!    key setup, and the host's entry and pinned key;
//! 2. this laptop: goway's config, keys and state;
//! 3. the goway binary, by replaying the install journal of
//!    scripts/install.sh (on Windows: the uninstall entry does it).
//!
//! Without `--everywhere` it lists exactly that and asks first.

use std::path::{Path, PathBuf};

use crate::cli::UninstallArgs;
use crate::config::{self, Config, HostConfig};
use crate::doctor::{self, FixRunner as _};
use crate::error::{Error, Result};
use crate::paths::Paths;
use crate::remote;
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

/// Undo scripts/install.sh by replaying its journal backwards (the same
/// rules as scripts/uninstall.sh: the binary only while it is still the
/// installed build, the exact PATH line, the added newline, created
/// directories only when empty). Returns what it did, for the user.
pub fn revert_install_journal(journal: &Path) -> Result<Vec<String>> {
    let text = std::fs::read_to_string(journal).map_err(|e| Error::io("read", journal, e))?;
    let mut done = Vec::new();
    let mut dirs = Vec::new();
    for entry in text.lines().rev() {
        let (kind, rest) = entry.split_once(' ').unwrap_or((entry, ""));
        match kind {
            "file" => {
                let Some((path, sum)) = rest.rsplit_once(' ') else {
                    continue;
                };
                let path = Path::new(path);
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
                let Ok(content) = std::fs::read_to_string(profile) else {
                    continue;
                };
                if !content.lines().any(|l| l == line) {
                    continue;
                }
                let kept: String =
                    content
                        .lines()
                        .filter(|l| *l != line)
                        .fold(String::new(), |mut acc, l| {
                            acc.push_str(l);
                            acc.push('\n');
                            acc
                        });
                if existed == "0" && kept.is_empty() {
                    std::fs::remove_file(profile).map_err(|e| Error::io("remove", profile, e))?;
                    done.push(format!(
                        "removed {} (created by install)",
                        profile.display()
                    ));
                } else {
                    std::fs::write(profile, kept).map_err(|e| Error::io("write", profile, e))?;
                    done.push(format!("removed the PATH line from {}", profile.display()));
                }
            }
            "newline" => {
                let profile = Path::new(rest);
                if let Ok(mut content) = std::fs::read_to_string(profile)
                    && content.ends_with('\n')
                {
                    content.pop();
                    std::fs::write(profile, content).map_err(|e| Error::io("write", profile, e))?;
                }
            }
            "dir" => dirs.push(PathBuf::from(rest)),
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

/// What uninstall will do on one helper, for the plan.
fn host_plan(paths: &Paths, config: &Config, host: &HostConfig) -> Vec<String> {
    let mut out = vec![format!(
        "remove goway's state there (~/{})",
        config.defaults.remote_root
    )];
    for item in doctor::load_installed(paths, &host.name) {
        match (&item.undo, item.root) {
            (Some(_), false) => out.push(format!("remove {} (goway installed it)", item.check)),
            (Some(_), true) => out.push(format!(
                "undo the {} change (needs --rsudo: administrator rights there)",
                item.check
            )),
            (None, _) => out.push(format!(
                "keep the system package for {} (other software may use it; listed with its removal command)",
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

fn show_plan(renderer: Renderer, paths: &Paths, config: &Config) {
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
    if install_journal().exists() {
        renderer.line("    - remove the goway program and its PATH line (install journal)");
    } else if cfg!(windows) {
        renderer.line(
            "    - (the goway program: remove it in Settings > Apps, or run goway-setup uninstall)",
        );
    } else {
        renderer.line(
            "    - (the goway program was not installed by scripts/install.sh; remove it yourself)",
        );
    }
}

/// Clean one helper. Errors leave the local record intact so a rerun can
/// finish the job.
#[allow(clippy::too_many_arguments)] // the verb's environment, as for doctor
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
    let found = resolve::resolve(
        config,
        host,
        &mut state,
        lookup,
        prober,
        KeyPolicy::Strict,
        "true",
    )?;
    let transport = crate::sync::SshTransport {
        target: &found.target,
        settings,
    };
    let out = transport.output(&remote::invocation(
        "purge",
        &[&config.defaults.remote_root],
    ))?;
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
        match (&item.undo, item.root) {
            (Some(undo), false) => {
                if runner.run(undo, false) {
                    renderer.ok(format_args!("{}: removed {}", host.name, item.check));
                } else {
                    renderer.warn(format_args!(
                        "{}: could not remove {}",
                        host.name, item.check
                    ));
                }
            }
            (Some(undo), true) => root_undos.push(doctor::Fix {
                command: undo.clone(),
                root: true,
                why: format!("undoes goway's {} change", item.check),
            }),
            (None, _) => renderer.note(format_args!(
                "{}: kept the system package goway installed for {}",
                host.name, item.check
            )),
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
    show_plan(renderer, paths, &config);
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
    let journal = install_journal();
    if journal.exists() {
        for line in revert_install_journal(&journal)? {
            renderer.ok(line);
        }
        renderer.ok("goway is uninstalled; open a new terminal to refresh PATH");
    } else if cfg!(windows) {
        renderer.next("remove the goway program in Settings > Apps (or run goway-setup uninstall)");
    } else {
        renderer.next(format_args!(
            "the goway program was not installed by scripts/install.sh; remove {} yourself",
            std::env::current_exe()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        ));
    }
    Ok(0)
}
