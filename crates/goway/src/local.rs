//! This machine as a place to run: `goway run --host local`, the optional
//! `[local]` pool membership, and the offline fallback.
//!
//! A local run needs no sync: the command runs in the current work tree
//! with its stdio inherited (live output, the command's own exit code).
//! Priority works like on a helper (`nice`, `ionice` for `low`). The
//! machine is probed with the same remote script as a helper (run through
//! `sh`), so its facts (RAM, GPUs, CPU features) match what `--needs`
//! checks everywhere else. Running local jobs are counted with `flock`ed
//! files in goway's state dir, so `[local] max_jobs` works across
//! processes and a crashed run frees its slot by itself.
//!
//! The name `local` means this machine unless the config has a host
//! called `local`, which wins (and keeps this machine out of the pool).

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use crate::cli::RunArgs;
use crate::config::{Config, HostConfig, Local, Priority};
use crate::error::{Error, Result};
use crate::needs::{self, Selection};
use crate::paths::Paths;
use crate::pool::{self, Probe};
use crate::render::Renderer;
use crate::repo::Repo;
use crate::resolve::{Found, Source};
use crate::run::{self, Report};
use crate::spawn::CommandExt as _;
use crate::ssh::Target;
use crate::state::State;

/// The name that selects this machine.
pub const NAME: &str = "local";

/// Whether `name` selects this machine (and no configured host shadows it).
pub fn names_this_machine(config: &Config, name: &str) -> bool {
    name.eq_ignore_ascii_case(NAME) && config.host(NAME).is_err()
}

/// The `[local]` settings (defaults when there is no section).
pub fn settings(config: &Config) -> Local {
    config.local.clone().unwrap_or_default()
}

/// This machine as a pool host: capped at `[local] max_jobs`, labelled `local`.
pub fn host(config: &Config) -> HostConfig {
    let l = settings(config);
    HostConfig {
        name: NAME.to_owned(),
        max_jobs: Some(l.max_jobs),
        priority: l.priority,
        labels: vec![NAME.to_owned()],
        ..HostConfig::default()
    }
}

/// The "found" record of this machine (`source` says why it was chosen).
pub fn found(source: Source) -> Found {
    Found {
        kind: crate::transport::Kind::Unix,
        target: Target {
            name: NAME.to_owned(),
            address: "this machine".to_owned(),
            port: 0,
            user: None,
            identity: None,
        },
        source,
        output: String::new(),
    }
}

/// Where running local jobs leave their lock files.
pub fn jobs_dir(paths: &Paths) -> PathBuf {
    paths.state_dir.join("local-jobs")
}

/// A running local job's claim: a `flock`ed file that exists while it runs.
#[derive(Debug)]
pub struct Slot {
    _file: File,
    path: PathBuf,
}

impl Slot {
    /// Claim a slot for `run_id` under `dir`.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] when the file cannot be created or locked.
    pub fn acquire(dir: &Path, run_id: &str) -> Result<Self> {
        std::fs::create_dir_all(dir).map_err(|e| Error::io("create", dir, e))?;
        let path = dir.join(format!("{run_id}.lock"));
        let file = File::create(&path).map_err(|e| Error::io("create", &path, e))?;
        file.lock().map_err(|e| Error::io("lock", &path, e))?;
        tracing::debug!(path = %path.display(), "local job slot taken");
        Ok(Self { _file: file, path })
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// How many local jobs run now: lock files somebody holds. Files nobody
/// holds (a crashed run) are removed.
pub fn running_jobs(dir: &Path) -> u32 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut running = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "lock") {
            continue;
        }
        let Ok(file) = File::open(&path) else {
            continue;
        };
        if file.try_lock().is_ok() {
            let _ = std::fs::remove_file(&path);
        } else {
            running += 1;
        }
    }
    running
}

/// Probe this machine like a helper: the remote script's `probe` verb run
/// through `sh`, with RAM, GPUs and the like cached in `state`.
///
/// # Errors
///
/// [`Error::Usage`] when the probe cannot run or its output is unusable.
pub fn probe(config: &Config, jobs: &Path, state: &mut State, disk: bool) -> Result<Probe> {
    let key = NAME;
    let now = crate::state::now_secs();
    let statics = state.refresh_facts || crate::facts::stale(state.facts.get(key), now);
    let out = Command::new("sh")
        .arg("-c")
        .arg(pool::probe_command(config, disk, statics))
        .stdin(Stdio::null())
        .output_locked()
        .map_err(|e| Error::Usage(format!("cannot probe this machine: {e}")))?;
    if !out.status.success() {
        return Err(Error::Usage(format!(
            "cannot probe this machine: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    let (_noise, stdout) = crate::remote::split_frame(out.stdout);
    let mut probe = pool::complete_probe(NAME, &String::from_utf8_lossy(&stdout), state, now)?;
    probe.jobs = running_jobs(jobs);
    Ok(probe)
}

/// This machine as a candidate: host, found record and live probe.
///
/// # Errors
///
/// As [`probe`].
pub fn candidate(
    config: &Config,
    jobs: &Path,
    state: &mut State,
    disk: bool,
    source: Source,
) -> Result<(HostConfig, Found, Probe)> {
    let probe = probe(config, jobs, state, disk)?;
    Ok((host(config), found(source), probe))
}

/// What a local run needs to start.
pub struct Job<'a> {
    /// The command line.
    pub command: &'a [String],
    /// `KEY=VALUE` environment on top of ours.
    pub env: &'a [String],
    /// `low` runs under `nice` and `ionice` where they exist.
    pub priority: Priority,
    /// The directory to run in.
    pub cwd: &'a Path,
    /// The run id (exported as `GOWAY_RUN_ID`).
    pub run_id: &'a str,
    /// This machine's name (exported as `GOWAY_HOST`).
    pub hostname: &'a str,
}

fn on_path(tool: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(tool).is_file()))
}

/// The command line with the priority wrappers for `priority`.
pub fn wrapped(command: &[String], priority: Priority) -> Vec<String> {
    let mut out = Vec::new();
    if priority == Priority::Low {
        if on_path("nice") {
            out.extend(["nice".to_owned(), "-n".to_owned(), "10".to_owned()]);
        }
        if on_path("ionice") {
            out.extend(["ionice".to_owned(), "-c".to_owned(), "3".to_owned()]);
        }
    }
    out.extend_from_slice(command);
    out
}

/// Build the process for `job` (stdio is the caller's to set).
pub fn process(job: &Job<'_>) -> Command {
    let argv = wrapped(job.command, job.priority);
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]).current_dir(job.cwd);
    for pair in job.env {
        if let Some((k, v)) = pair.split_once('=') {
            cmd.env(k, v);
        }
    }
    cmd.env("GOWAY", "1")
        .env("GOWAY_RUN_ID", job.run_id)
        .env("GOWAY_HOST", job.hostname);
    cmd
}

/// The exit code for a command that could not be started, as a shell reports it.
pub fn spawn_failure_code(e: &std::io::Error) -> u8 {
    if e.kind() == std::io::ErrorKind::NotFound {
        127
    } else {
        126
    }
}

/// `goway run` on this machine: no sync, stdio inherited, the command's own
/// exit code. `fallback` says why we are here (no helper was reachable).
///
/// # Errors
///
/// Only goway's own failures (slot, report); a command that fails is a code.
#[allow(clippy::too_many_arguments)] // the pieces `run` already holds
pub fn run_here(
    env: &run::Env<'_>,
    renderer: Renderer,
    args: &RunArgs,
    config: &Config,
    repo: &Repo,
    selection: &Selection,
    rule: Option<crate::project::Applied>,
    chosen: (HostConfig, Found, Probe),
    started: Instant,
) -> Result<u8> {
    let (host, found, probe) = chosen;
    let run_id = run::new_run_id();
    let matched = selection.assess(&host, &probe).matched;
    if found.source == Source::Fallback {
        renderer.warn(
            "no helper is reachable; running on this machine because [local] fallback = true",
        );
    }
    let slot = Slot::acquire(&jobs_dir(env.paths), &run_id)?;
    renderer.headline(format_args!(
        "running on {} ({}, {}){}: {}",
        host.name,
        probe.arch,
        probe.hostname,
        if matched.is_empty() {
            String::new()
        } else {
            format!(" [{}]", needs::summary(&matched))
        },
        crate::ssh::shell_join(&args.command)
    ));
    let job = Job {
        command: &args.command,
        env: &args.env,
        priority: config.priority_of(&host),
        cwd: env.cwd,
        run_id: &run_id,
        hostname: &probe.hostname,
    };
    let _interrupted = run::interrupt_flag();
    let code = match process(&job).stdin(Stdio::inherit()).status() {
        Ok(status) => run::exit_code_of(status),
        Err(e) => {
            renderer.warn(format_args!("cannot run `{}`: {e}", args.command[0]));
            spawn_failure_code(&e)
        }
    };
    drop(slot);
    let elapsed = started.elapsed();
    tracing::info!(code, ?elapsed, run_id, "local run finished");
    let summary = format!(
        "exit {code} on {} in {:.1}s",
        host.name,
        elapsed.as_secs_f64()
    );
    if code == 0 {
        renderer.ok(summary);
    } else {
        renderer.failed(summary);
    }
    if let Some(path) = &args.report {
        run::write_report(
            path,
            &Report {
                host: host.name.clone(),
                address: found.target.address.clone(),
                os: found.kind.os().as_str().to_owned(),
                arch: probe.arch.clone(),
                hostname: probe.hostname.clone(),
                command: args.command.clone(),
                exit_code: code,
                duration_secs: elapsed.as_secs_f64(),
                run_id,
                repo: repo.name.clone(),
                matched,
                rule,
            },
        )?;
    }
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    // frob:tests crates/goway/src/local.rs::running_jobs
    #[test]
    fn running_jobs_counts_held_locks_and_forgets_dead_ones() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(running_jobs(dir.path()), 0);
        let a = Slot::acquire(dir.path(), "a").unwrap();
        let b = Slot::acquire(dir.path(), "b").unwrap();
        assert_eq!(running_jobs(dir.path()), 2);
        drop(a);
        assert_eq!(running_jobs(dir.path()), 1);
        // A crashed run leaves its file but not its lock.
        std::fs::write(dir.path().join("crashed.lock"), "").unwrap();
        assert_eq!(running_jobs(dir.path()), 1);
        assert!(!dir.path().join("crashed.lock").exists());
        drop(b);
        assert_eq!(running_jobs(dir.path()), 0);
    }

    // frob:tests crates/goway/src/local.rs::wrapped
    #[test]
    fn low_priority_wraps_in_nice_and_normal_does_not() {
        let cmd = vec!["echo".to_owned(), "hi".to_owned()];
        assert_eq!(wrapped(&cmd, Priority::Normal), cmd);
        let low = wrapped(&cmd, Priority::Low);
        assert!(low.ends_with(&cmd));
        if on_path("nice") {
            assert_eq!(&low[..3], ["nice", "-n", "10"]);
        }
    }

    // frob:tests crates/goway/src/local.rs::names_this_machine
    #[test]
    fn a_configured_host_called_local_shadows_this_machine() {
        let mut config = Config::default();
        assert!(names_this_machine(&config, "local"));
        assert!(names_this_machine(&config, "LOCAL"));
        assert!(!names_this_machine(&config, "helios"));
        let mut h = host(&config);
        h.name = "local".to_owned();
        config.hosts.push(h);
        assert!(!names_this_machine(&config, "local"));
        config.local = Some(Local {
            pool: true,
            fallback: true,
            ..Local::default()
        });
        assert!(!config.local_in_pool() && !config.local_fallback());
    }
}
