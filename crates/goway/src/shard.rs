//! `goway run --shard N`: one test run split across N hosts.
//!
//! goway does not invent its own partitioning: nextest already has
//! `--partition count:i/N`, so for `cargo nextest run` goway adds it, and
//! any other command gets `GOWAY_SHARD=i` and `GOWAY_SHARD_COUNT=N` to split
//! its own work. Each host syncs and runs in parallel; output lines are
//! prefixed with the host; goway exits with the first failing shard's code
//! (in shard order), or 0 when every shard passed.

use std::io::BufRead as _;
use std::process::Stdio;
use std::time::Instant;

use serde::Serialize;

use crate::cli::RunArgs;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::pool;
use crate::render::{self, Renderer};
use crate::repo::Repo;
use crate::run::{self, Env};
use crate::ssh::{self, KeyPolicy};
use crate::state::State;

/// `command` for shard `index` (1-based) of `count`: nextest runs get
/// `--partition count:index/count` (before any `--`), others are unchanged.
pub fn shard_command(command: &[String], index: usize, count: usize) -> Vec<String> {
    let nextest_run = command
        .windows(2)
        .position(|w| w[0].ends_with("nextest") && w[1] == "run");
    let Some(at) = nextest_run else {
        return command.to_vec();
    };
    let insert = command[at + 2..]
        .iter()
        .position(|a| a == "--")
        .map_or(command.len(), |p| at + 2 + p);
    let mut out = command[..insert].to_vec();
    out.push("--partition".to_owned());
    out.push(format!("count:{index}/{count}"));
    out.extend_from_slice(&command[insert..]);
    out
}

/// One shard's outcome, in the `--report` file.
#[derive(Debug, Clone, Serialize)]
pub struct ShardReport {
    /// 1-based shard index.
    pub shard: usize,
    /// goway host name.
    pub host: String,
    /// Address used.
    pub address: String,
    /// `uname -m`.
    pub arch: String,
    /// `uname -n`.
    pub hostname: String,
    /// The command as run on this host.
    pub command: Vec<String>,
    /// This shard's exit code.
    pub exit_code: u8,
    /// Seconds including sync.
    pub duration_secs: f64,
}

/// The `--report` file of a sharded run.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    /// The exit code goway exits with.
    pub exit_code: u8,
    /// Wall-clock seconds.
    pub duration_secs: f64,
    /// Repository name.
    pub repo: String,
    /// Every shard.
    pub shards: Vec<ShardReport>,
}

/// Copy `reader` to our stdout or stderr line by line with `prefix`.
fn pump(reader: impl std::io::Read, to_stderr: bool, prefix: &str) {
    let mut reader = std::io::BufReader::new(reader);
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => render::prefixed_line(to_stderr, prefix, &line),
        }
    }
}

/// `goway run --shard N`.
///
/// # Panics
///
/// Only if a shard thread panics, which is a bug.
#[allow(clippy::too_many_lines)] // the shard thread is one sequence: sync, run, report
pub fn run_sharded(env: &Env<'_>, renderer: Renderer, args: &RunArgs, count: u16) -> Result<u8> {
    let started = Instant::now();
    let count = usize::from(count);
    let config = Config::load(&env.paths.config_file())?;
    let repo = Repo::discover(env.cwd)?;
    let mut state = State::load(&env.paths.state_file())?;
    let hosts = pool::choose_many(&config, &mut state, env.lookup, env.prober, count)?;
    if let Err(e) = state.save(&env.paths.state_file()) {
        tracing::warn!(error = %e, "cannot cache host addresses");
    }
    renderer.headline(format_args!(
        "sharding {} across {count} hosts: {}",
        ssh::shell_join(&args.command),
        hosts
            .iter()
            .map(|(h, ..)| h.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    ));
    let interrupted = run::interrupt_flag();
    let width = hosts.iter().map(|(h, ..)| h.name.len()).max().unwrap_or(0);
    let results: Vec<Result<ShardReport>> = std::thread::scope(|scope| {
        let handles: Vec<_> = hosts
            .iter()
            .enumerate()
            .map(|(i, (host, found, probe))| {
                let config = &config;
                let repo = &repo;
                let interrupted = &interrupted;
                scope.spawn(move || -> Result<ShardReport> {
                    let shard_started = Instant::now();
                    let index = i + 1;
                    let prefix = format!("[{:<width$}] ", host.name);
                    let run_id = run::new_run_id() + &format!("-s{index}");
                    run::sync_snapshot(env, config, repo, found, &run_id, args.keep)?;
                    let mut pairs = args.env.clone();
                    pairs.push(format!("GOWAY_SHARD={index}"));
                    pairs.push(format!("GOWAY_SHARD_COUNT={count}"));
                    let command = shard_command(&args.command, index, count);
                    let cmd = run::run_invocation(
                        config,
                        config.priority_of(host).as_str(),
                        repo,
                        &run_id,
                        args.keep,
                        &command,
                        &run::encode_env(&pairs)?,
                    );
                    let mut child =
                        ssh::command(&found.target, env.settings, KeyPolicy::Strict, &cmd)
                            .stdin(Stdio::null())
                            .stdout(Stdio::piped())
                            .stderr(Stdio::piped())
                            .spawn()
                            .map_err(|e| Error::Ssh {
                                host: host.name.clone(),
                                message: format!("cannot run ssh: {e}"),
                            })?;
                    let (out, err) = (child.stdout.take(), child.stderr.take());
                    std::thread::scope(|s| {
                        if let Some(out) = out {
                            let p = prefix.clone();
                            s.spawn(move || pump(out, false, &p));
                        }
                        if let Some(err) = err {
                            let p = prefix.clone();
                            s.spawn(move || pump(err, true, &p));
                        }
                    });
                    let status = child.wait().map_err(|e| Error::Ssh {
                        host: host.name.clone(),
                        message: format!("ssh failed: {e}"),
                    })?;
                    let code = if interrupted.load(std::sync::atomic::Ordering::SeqCst)
                        && status.code() == Some(255)
                    {
                        130
                    } else {
                        run::exit_code_of(status)
                    };
                    tracing::info!(host = %host.name, index, code, "shard finished");
                    Ok(ShardReport {
                        shard: index,
                        host: host.name.clone(),
                        address: found.target.address.clone(),
                        arch: probe.arch.clone(),
                        hostname: probe.hostname.clone(),
                        command,
                        exit_code: code,
                        duration_secs: shard_started.elapsed().as_secs_f64(),
                    })
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("shard thread panicked"))
            .collect()
    });
    let mut shards = Vec::new();
    let mut code = 0;
    for (i, result) in results.into_iter().enumerate() {
        match result {
            Ok(report) => {
                let line = format_args!(
                    "shard {}/{count} on {}: exit {} in {:.1}s",
                    report.shard, report.host, report.exit_code, report.duration_secs
                );
                if report.exit_code == 0 {
                    renderer.ok(line);
                } else {
                    renderer.note(line);
                    if code == 0 {
                        code = report.exit_code;
                    }
                }
                shards.push(report);
            }
            Err(e) => {
                renderer.error(&e);
                if code == 0 {
                    code = e.exit_code();
                }
                tracing::warn!(shard = i + 1, error = %e, "shard failed to run");
            }
        }
    }
    let elapsed = started.elapsed();
    renderer.note(format_args!(
        "all shards done in {:.1}s: exit {code}",
        elapsed.as_secs_f64()
    ));
    if let Some(path) = &args.report {
        let report = Report {
            exit_code: code,
            duration_secs: elapsed.as_secs_f64(),
            repo: repo.name.clone(),
            shards,
        };
        let text =
            serde_json::to_string_pretty(&report).map_err(|e| Error::Usage(e.to_string()))?;
        crate::config::write_atomic(path, text.as_bytes())?;
    }
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn nextest_gets_its_own_partition_flag() {
        assert_eq!(
            shard_command(&words("cargo nextest run --workspace"), 2, 3),
            words("cargo nextest run --workspace --partition count:2/3")
        );
        assert_eq!(
            shard_command(&words("cargo nextest run -E all() -- --nocapture"), 1, 2),
            words("cargo nextest run -E all() --partition count:1/2 -- --nocapture")
        );
        assert_eq!(
            shard_command(&words("/x/cargo-nextest nextest run"), 1, 2),
            words("/x/cargo-nextest nextest run --partition count:1/2")
        );
        assert_eq!(shard_command(&words("make test"), 1, 2), words("make test"));
    }
}
