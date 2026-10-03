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
use crate::config::{Config, HostConfig};
use crate::error::{Error, Result};
use crate::needs::{self, Matched, Selection};
use crate::paths::Paths;
use crate::pool;
use crate::project::{self, Applied};
use crate::remote;
use crate::render::Renderer;
use crate::repo::Repo;
use crate::resolve::{Found, Lookup, Prober};
use crate::ssh::{self, KeyPolicy};
use crate::state::State;
use crate::sync::{self, Label, SshTransport};
use crate::termfilter::{self, OutputMode};

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
    /// The host facts that met the run's `--needs` and `--prefers`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub matched: Vec<Matched>,
    /// The `goway.toml` rule that applied, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<Applied>,
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

/// Validate `KEY=VALUE` pairs and encode them NUL-separated.
pub fn encode_env(pairs: &[String]) -> Result<Vec<u8>> {
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
    Ok(bytes)
}

/// Hand the run's `--env` values to its work dir over ssh's stdin, never
/// as a command-line argument other users on the host could read.
pub(crate) fn send_env(
    env: &Env<'_>,
    config: &Config,
    found: &Found,
    run_id: &str,
    bytes: &[u8],
) -> Result<()> {
    if bytes.is_empty() {
        return Ok(());
    }
    let transport = SshTransport {
        target: &found.target,
        settings: env.settings,
    };
    sync::Transport::exchange(
        &transport,
        &remote::invocation("envfile", &[&config.defaults.remote_root, run_id]),
        bytes,
    )
    .map(drop)
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
#[allow(clippy::too_many_lines)] // one sequence: choose, sync, run, report
pub fn run(env: &Env<'_>, renderer: Renderer, args: &RunArgs) -> Result<u8> {
    if let Some(count) = args.shard {
        return crate::shard::run_sharded(env, renderer, args, count);
    }
    let started = Instant::now();
    let env_bytes = encode_env(&args.env)?;
    let config = Config::load(&env.paths.config_file())?;
    let repo = Repo::discover(env.cwd)?;
    let (selection, rule) =
        project::selection_for(&repo.root, &args.command, &args.needs, &args.prefers)?;
    if let Some(r) = &rule {
        renderer.note(r.describe());
    }
    let mut state = State::load(&env.paths.state_file())?;
    let (host, found, probe) = pool::choose(
        &config,
        &selection,
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
    let matched = selection.assess(&host, &probe).matched;
    if args.host.is_none() && config.hosts.len() > 1 {
        renderer.note(format_args!(
            "picked {} (load {:.2} on {} cores, {} goway jobs)",
            host.name, probe.load[0], probe.cores, probe.jobs
        ));
    }

    let remote_root = config.defaults.remote_root.as_str();
    let run_id = new_run_id();
    let synced = sync_snapshot(env, &config, &repo, &found, &run_id, args.keep)?;
    renderer.note(format_args!(
        "synced {} files ({} sent, {} bytes, {} deleted)",
        synced.files, synced.sent, synced.bytes, synced.deleted
    ));
    report_withheld(renderer, &synced);
    send_env(env, &config, &found, &run_id, &env_bytes)?;

    let cmd = run_invocation_with(
        &config,
        config.priority_of(&host).as_str(),
        &repo,
        &run_id,
        args.keep,
        &gpu_words(&selection, &config, &host),
        &args.command,
    );

    renderer.headline(format_args!(
        "running on {} ({arch}, {hostname}) at {}{}: {}",
        host.name,
        found.target.address,
        if matched.is_empty() {
            String::new()
        } else {
            format!(" [{}]", needs::summary(&matched))
        },
        ssh::shell_join(&args.command)
    ));
    let (code, interrupted) = stream(&found, env.settings, &cmd, args.output)?;
    let elapsed = started.elapsed();
    tracing::info!(host = %host.name, code, ?elapsed, run_id, "run finished");
    if interrupted {
        renderer.warn(format_args!(
            "interrupted; the remote job is stopped by its watchdog (run {run_id})"
        ));
    }
    let kept = if args.keep {
        format!(" (kept {remote_root}/work/{run_id})")
    } else {
        String::new()
    };
    let summary = format!(
        "exit {code} on {} in {:.1}s{kept}",
        host.name,
        elapsed.as_secs_f64()
    );
    if code == 0 {
        renderer.ok(summary);
    } else {
        renderer.failed(summary);
    }
    if let Some(path) = &args.report {
        write_report(
            path,
            &Report {
                host: host.name.clone(),
                address: found.target.address.clone(),
                arch,
                hostname,
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

/// Sync the work tree to `found` and snapshot it as work dir `run_id`.
pub(crate) fn sync_snapshot(
    env: &Env<'_>,
    config: &Config,
    repo: &Repo,
    found: &Found,
    run_id: &str,
    keep: bool,
) -> Result<sync::Stats> {
    let transport = SshTransport {
        target: &found.target,
        settings: env.settings,
    };
    let snapshot = sync::Snapshot {
        run_id: run_id.to_owned(),
        meta_b64: label_b64(repo, "work"),
        keep,
    };
    sync::sync(
        &transport,
        &config.defaults.remote_root,
        repo,
        &sync::Secrets::from_config(&config.defaults),
        Some(&snapshot),
    )
}

/// Tell the user which files stayed on this machine and why.
pub(crate) fn report_withheld(renderer: Renderer, synced: &sync::Stats) {
    let list = |paths: &[String]| {
        let mut shown: Vec<&str> = paths.iter().take(5).map(String::as_str).collect();
        if paths.len() > 5 {
            shown.push("...");
        }
        shown.join(", ")
    };
    if !synced.kept_local.is_empty() {
        renderer.note(format_args!(
            "kept {} secret-looking file(s) on this machine: {} (allow with secret_allow in the config)",
            synced.kept_local.len(),
            list(&synced.kept_local)
        ));
    }
    if !synced.behind_links.is_empty() {
        renderer.warn(format_args!(
            "did not send {} file(s) under symlinked directories: {}",
            synced.behind_links.len(),
            list(&synced.behind_links)
        ));
    }
}

/// Write a `--report` file as pretty JSON.
pub(crate) fn write_report(path: &std::path::Path, report: &impl Serialize) -> Result<()> {
    let text = serde_json::to_string_pretty(report).map_err(|e| Error::Usage(e.to_string()))?;
    crate::config::write_atomic(path, text.as_bytes())
}

/// A base64 `meta.json` label of `kind` for this repository.
pub(crate) fn label_b64(repo: &Repo, kind: &str) -> String {
    let label = Label {
        kind,
        repo: &repo.name,
        repo_id: &repo.id,
        worktree: repo.root.to_string_lossy().into_owned(),
        client: &repo.client,
        updated: crate::state::now_secs(),
    };
    base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&label).unwrap_or_default())
}

/// The `run` option words that make a run hold a GPU slot on `host`: only
/// when the run needs a GPU, with the host's `gpu_jobs` runs per GPU.
pub(crate) fn gpu_words(selection: &Selection, config: &Config, host: &HostConfig) -> Vec<String> {
    if selection.needs_gpu() {
        vec![format!("gpu-slots:{}", config.gpu_jobs_of(host))]
    } else {
        Vec::new()
    }
}

/// The remote `run` invocation for this run (its work dir already exists),
/// with extra option words for the remote `run` verb (the shard detection
/// request, the GPU slot) placed before the command.
pub(crate) fn run_invocation_with(
    config: &Config,
    priority: &str,
    repo: &Repo,
    run_id: &str,
    keep: bool,
    extra: &[String],
    command: &[String],
) -> String {
    let cache_meta = label_b64(repo, "cache");
    let slots = config.defaults.target_slots.max(1).to_string();
    let keep = if keep { "1" } else { "0" };
    let d = &config.defaults;
    let ttls = format!(
        "{}:{}:{}",
        d.cache_ttl.as_secs(),
        d.orphan_ttl.as_secs(),
        d.kept_ttl.as_secs()
    );
    let (keep_ignored, keep_list) = config.keep_words();
    let mut words: Vec<&str> = vec![
        &config.defaults.remote_root,
        run_id,
        &repo.id,
        keep,
        &slots,
        &cache_meta,
        &ttls,
        priority,
        keep_ignored,
        &keep_list,
    ];
    words.extend(extra.iter().map(String::as_str));
    words.push("--");
    words.extend(command.iter().map(String::as_str));
    remote::invocation("run", &words)
}

/// A flag set when the user presses Ctrl-C. ssh receives the terminal's
/// Ctrl-C itself; goway only waits for it to exit so the exit code stays
/// faithful.
pub(crate) fn interrupt_flag() -> std::sync::Arc<std::sync::atomic::AtomicBool> {
    let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let setter = flag.clone();
    if let Err(e) = ctrlc::set_handler(move || {
        setter.store(true, std::sync::atomic::Ordering::SeqCst);
    }) {
        tracing::debug!(error = %e, "ctrl-c handler not installed");
    }
    flag
}

/// Run `cmd` on the found host with stdio passed through; returns the exit
/// code and whether the user interrupted. A stream that is a terminal goes
/// through the control-sequence filter unless `mode` is raw; any other
/// stream is inherited and so stays byte-exact.
fn stream(
    found: &Found,
    settings: &ssh::Settings,
    cmd: &str,
    mode: OutputMode,
) -> Result<(u8, bool)> {
    let interrupted = interrupt_flag();
    let mut command = ssh::command(&found.target, settings, KeyPolicy::Strict, cmd);
    if std::io::IsTerminal::is_terminal(&std::io::stderr()) {
        command.env("CARGO_TERM_COLOR", "always");
    }
    let filter_out =
        termfilter::should_filter(mode, std::io::IsTerminal::is_terminal(&std::io::stdout()));
    let filter_err =
        termfilter::should_filter(mode, std::io::IsTerminal::is_terminal(&std::io::stderr()));
    let piped = |filtered: bool| {
        if filtered {
            Stdio::piped()
        } else {
            Stdio::inherit()
        }
    };
    tracing::debug!(filter_out, filter_err, "remote output handling");
    let ssh_err = |e: std::io::Error| Error::Ssh {
        host: found.target.name.clone(),
        message: format!("cannot run ssh: {e}"),
    };
    let mut child = command
        .stdin(Stdio::inherit())
        .stdout(piped(filter_out))
        .stderr(piped(filter_err))
        .spawn()
        .map_err(ssh_err)?;
    let (out, err) = (child.stdout.take(), child.stderr.take());
    let status = std::thread::scope(|s| {
        if let Some(out) = out {
            s.spawn(move || termfilter::relay(out, false));
        }
        if let Some(err) = err {
            s.spawn(move || termfilter::relay(err, true));
        }
        child.wait()
    })
    .map_err(ssh_err)?;
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
        let raw = encode_env(&["A=1".to_owned(), "B_2=x y".to_owned()]).unwrap();
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
