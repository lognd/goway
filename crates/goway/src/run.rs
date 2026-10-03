//! `goway run`: sync the work tree and run a command on a host, streaming
//! its output untouched and exiting with its exit code.
//!
//! Exit code contract: the command's own code; 128+N when it died of signal
//! N (also when the local ssh was interrupted); 125 when goway failed before
//! the command could run. A provenance line naming host, arch and address
//! goes to stderr, and `--report FILE` writes the same as JSON (for frob).

use std::path::Path;
use std::process::Stdio;
use std::time::Instant;

use base64::Engine as _;
use serde::Serialize;

use crate::cli::RunArgs;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::paths::Paths;
use crate::pool;
use crate::remote;
use crate::render::Renderer;
use crate::repo::Repo;
use crate::resolve::{Found, Lookup, Prober};
use crate::ssh::{self, KeyPolicy};
use crate::state::State;
use crate::sync::{self, Label, SshTransport};

/// Provenance of one run, written by `--report`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Report {
    /// goway host name.
    pub host: String,
    /// Address the run used.
    pub address: String,
    /// Remote machine architecture (`uname -m`).
    pub arch: String,
    /// Remote machine hostname (`uname -n`).
    pub hostname: String,
    /// The command as given.
    pub command: Vec<String>,
    /// The exit code goway exits with.
    pub exit_code: u8,
    /// Wall-clock seconds including sync.
    pub duration_secs: f64,
    /// Unique run id (remote work dir name).
    pub run_id: String,
    /// Repository name.
    pub repo: String,
}

/// A new run id: time plus process id, unique per client.
pub fn new_run_id() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!(
        "{}-{:08x}-{}",
        now.as_secs(),
        now.subsec_nanos(),
        std::process::id()
    )
}

/// Validate `KEY=VALUE` pairs and encode them NUL-separated in base64.
pub fn encode_env(pairs: &[String]) -> Result<String> {
    let mut bytes = Vec::new();
    for pair in pairs {
        let valid = pair.split_once('=').is_some_and(|(k, _)| {
            !k.is_empty()
                && !k.starts_with(|c: char| c.is_ascii_digit())
                && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        });
        if !valid || pair.contains('\0') {
            return Err(Error::Usage(format!(
                "--env expects KEY=VALUE with a shell-safe KEY, got `{pair}`"
            )));
        }
        bytes.extend_from_slice(pair.as_bytes());
        bytes.push(0);
    }
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// Map a local ssh exit status to goway's exit code.
pub fn exit_code_of(status: std::process::ExitStatus) -> u8 {
    if let Some(code) = status.code() {
        return u8::try_from(code).unwrap_or(crate::error::EXIT_GOWAY_FAILURE);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        if let Some(signal) = status.signal() {
            return u8::try_from(128 + signal).unwrap_or(crate::error::EXIT_GOWAY_FAILURE);
        }
    }
    crate::error::EXIT_GOWAY_FAILURE
}

/// Everything `run` needs from the environment, injectable for tests.
pub struct Env<'a> {
    /// Local paths.
    pub paths: &'a Paths,
    /// Name lookups.
    pub lookup: &'a (dyn Lookup + Sync),
    /// Host probing.
    pub prober: &'a (dyn Prober + Sync),
    /// ssh settings.
    pub settings: &'a ssh::Settings,
    /// Directory the run starts from.
    pub cwd: &'a Path,
}

/// `goway run`.
pub fn run(env: &Env<'_>, renderer: Renderer, args: &RunArgs) -> Result<u8> {
    let started = Instant::now();
    let env_b64 = encode_env(&args.env)?;
    let config = Config::load(&env.paths.config_file())?;
    let repo = Repo::discover(env.cwd)?;
    let mut state = State::load(&env.paths.state_file())?;
    let (host, found, probe) = pool::choose(
        &config,
        &mut state,
        env.lookup,
        env.prober,
        args.host.as_deref(),
    )?;
    if let Err(e) = state.save(&env.paths.state_file()) {
        tracing::warn!(error = %e, "cannot cache host address");
    }
    let arch = probe.arch.clone();
    let hostname = probe.hostname.clone();
    if args.host.is_none() && config.hosts.len() > 1 {
        renderer.note(format_args!(
            "picked {} (load {:.2} on {} cores, {} goway jobs)",
            host.name, probe.load[0], probe.cores, probe.jobs
        ));
    }

    let transport = SshTransport {
        target: &found.target,
        settings: env.settings,
    };
    let remote_root = config.defaults.remote_root.as_str();
    let synced = sync::sync(
        &transport,
        remote_root,
        &repo,
        config.defaults.send_env_files,
    )?;
    renderer.note(format_args!(
        "synced {} files ({} sent, {} bytes, {} deleted)",
        synced.files, synced.sent, synced.bytes, synced.deleted
    ));

    let run_id = new_run_id();
    let cmd = run_invocation(
        &config,
        config.priority_of(&host).as_str(),
        &repo,
        &run_id,
        args,
        &env_b64,
    );

    renderer.headline(format_args!(
        "running on {} ({arch}, {hostname}) at {}: {}",
        host.name,
        found.target.address,
        ssh::shell_join(&args.command)
    ));
    let (code, interrupted) = stream(&found, env.settings, &cmd)?;
    let elapsed = started.elapsed();
    tracing::info!(host = %host.name, code, ?elapsed, run_id, "run finished");
    if interrupted {
        renderer.warn(format_args!(
            "interrupted; the remote job is stopped by its watchdog (run {run_id})"
        ));
    }
    let summary = format_args!(
        "exit {code} on {} in {:.1}s{}",
        host.name,
        elapsed.as_secs_f64(),
        if args.keep {
            format!(" (kept {remote_root}/work/{run_id})")
        } else {
            String::new()
        }
    );
    if code == 0 {
        renderer.ok(summary);
    } else {
        renderer.note(summary);
    }
    if let Some(path) = &args.report {
        let report = Report {
            host: host.name.clone(),
            address: found.target.address.clone(),
            arch,
            hostname,
            command: args.command.clone(),
            exit_code: code,
            duration_secs: elapsed.as_secs_f64(),
            run_id,
            repo: repo.name.clone(),
        };
        let text =
            serde_json::to_string_pretty(&report).map_err(|e| Error::Usage(e.to_string()))?;
        crate::config::write_atomic(path, text.as_bytes())?;
    }
    Ok(code)
}

/// The remote `run` invocation for this run.
fn run_invocation(
    config: &Config,
    priority: &str,
    repo: &Repo,
    run_id: &str,
    args: &RunArgs,
    env_b64: &str,
) -> String {
    let b64 = base64::engine::general_purpose::STANDARD;
    let label = |kind| Label {
        kind,
        repo: &repo.name,
        repo_id: &repo.id,
        worktree: repo.root.to_string_lossy().into_owned(),
        client: &repo.client,
        updated: crate::state::now_secs(),
    };
    let meta = b64.encode(serde_json::to_vec(&label("work")).unwrap_or_default());
    let cache_meta = b64.encode(serde_json::to_vec(&label("cache")).unwrap_or_default());
    let slots = config.defaults.target_slots.max(1).to_string();
    let seed = repo.seed_key();
    let keep = if args.keep { "1" } else { "0" };
    let d = &config.defaults;
    let ttls = format!(
        "{}:{}:{}",
        d.cache_ttl.as_secs(),
        d.orphan_ttl.as_secs(),
        d.kept_ttl.as_secs()
    );
    let mut words: Vec<&str> = vec![
        &config.defaults.remote_root,
        &seed,
        run_id,
        &repo.id,
        keep,
        &slots,
        &meta,
        &cache_meta,
        env_b64,
        &ttls,
        priority,
        "--",
    ];
    words.extend(args.command.iter().map(String::as_str));
    remote::invocation("run", &words)
}

/// Run `cmd` on the found host with stdio passed through; returns the exit
/// code and whether the user interrupted.
fn stream(found: &Found, settings: &ssh::Settings, cmd: &str) -> Result<(u8, bool)> {
    let interrupted = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let flag = interrupted.clone();
        // ssh receives the terminal's Ctrl-C itself; goway only waits for it
        // to exit so the exit code stays faithful.
        if let Err(e) = ctrlc::set_handler(move || {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        }) {
            tracing::debug!(error = %e, "ctrl-c handler not installed");
        }
    }
    let mut command = ssh::command(&found.target, settings, KeyPolicy::Strict, cmd);
    if std::io::IsTerminal::is_terminal(&std::io::stderr()) {
        command.env("CARGO_TERM_COLOR", "always");
    }
    let status = command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| Error::Ssh {
            host: found.target.name.clone(),
            message: format!("cannot run ssh: {e}"),
        })?;
    let interrupted = interrupted.load(std::sync::atomic::Ordering::SeqCst);
    // ssh exits 255 when interrupted; report it like the shell would (128+SIGINT).
    let code = if interrupted && status.code() == Some(255) {
        130
    } else {
        exit_code_of(status)
    };
    Ok((code, interrupted))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_pairs_are_validated_and_encoded() {
        let b64 = encode_env(&["A=1".to_owned(), "B_2=x y".to_owned()]).unwrap();
        let raw = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap();
        assert_eq!(raw, b"A=1\0B_2=x y\0");
        for bad in ["=1", "1A=2", "A-B=1", "NOEQ"] {
            assert!(encode_env(&[bad.to_owned()]).is_err(), "{bad}");
        }
    }

    #[test]
    fn run_ids_are_unique() {
        assert_ne!(new_run_id(), new_run_id());
    }

    #[cfg(unix)]
    #[test]
    fn exit_codes_map_signals_to_128_plus_n() {
        let status = |script: &str| {
            std::process::Command::new("sh")
                .args(["-c", script])
                .status()
                .unwrap()
        };
        assert_eq!(exit_code_of(status("exit 7")), 7);
        assert_eq!(exit_code_of(status("kill -INT $$")), 130);
    }
}
