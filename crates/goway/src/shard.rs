//! `goway run --shard N`: one test run split across N hosts.
//!
//! The split itself is the [`crate::runners`] adapters' job (native
//! sharding flags, or a deterministic split of files, packages and
//! classes); every command also gets `GOWAY_SHARD=i` and
//! `GOWAY_SHARD_COUNT=N`. Each host syncs and runs in parallel; output lines are
//! prefixed with the host; goway exits with the first failing shard's code
//! (in shard order), or 0 when every shard passed.

use std::io::BufRead as _;
use std::process::Stdio;
use std::time::Instant;

use serde::Serialize;

use crate::cli::RunArgs;
use crate::config::Config;
use crate::detect::{self, Detection, ResultSplitter};
use crate::error::{Error, Result};
use crate::needs::Matched;
use crate::pool;
use crate::project::{self, Applied};
use crate::render::{self, Renderer};
use crate::repo::Repo;
use crate::run::{self, Env};
use crate::runners;
use crate::ssh::{self, KeyPolicy};
use crate::state::State;
use crate::termfilter;

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
    /// The host facts that met the run's `--needs` and `--prefers`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub matched: Vec<Matched>,
    /// What the helper's test-binary detection did, when it was asked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detection: Option<Detection>,
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
    /// The `goway.toml` rule that applied, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<Applied>,
    /// Every shard.
    pub shards: Vec<ShardReport>,
}

/// Copy `reader` to our stdout or stderr line by line with `prefix`. With
/// `filter` (the target is a terminal and the mode is not raw) the bytes go
/// through one stateful terminal filter per stream, so a sequence cannot be
/// hidden across lines; a line the filter empties entirely prints nothing.
fn pump(reader: impl std::io::Read, to_stderr: bool, prefix: &str, filter: bool) {
    pump_with(reader, prefix, filter, |bytes| {
        render::prefixed_batch(to_stderr, bytes);
    });
}

/// [`pump`] with the destination as a closure, so tests can capture each batch.
fn pump_with(reader: impl std::io::Read, prefix: &str, filter: bool, mut write: impl FnMut(&[u8])) {
    let mut reader = std::io::BufReader::with_capacity(render::MAX_LINE, reader);
    let mut lines = render::LineFramer::new(prefix);
    let mut state = termfilter::Filter::new();
    let mut filtered = Vec::new();
    let mut framed = Vec::new();
    loop {
        // Everything already buffered is framed and written as one batch;
        // while the write blocks (slow terminal) nothing more is read, so
        // the remote command is held back instead of memory growing.
        let n = match reader.fill_buf() {
            Ok([]) | Err(_) => break,
            Ok(data) => {
                framed.clear();
                if filter {
                    filtered.clear();
                    state.push(data, &mut filtered);
                    lines.push(&filtered, &mut framed);
                } else {
                    lines.push(data, &mut framed);
                }
                data.len()
            }
        };
        reader.consume(n);
        write(&framed);
    }
    framed.clear();
    lines.finish(&mut framed);
    write(&framed);
}

/// Tell what the helper's detection did for one shard; true when it failed
/// in a way worth remembering (the next run then skips detection).
fn note_detection(renderer: Renderer, report: &ShardReport, d: &Detection, program: &str) -> bool {
    let at = format!("shard {} on {}", report.shard, report.host);
    if d.duplicated {
        renderer.warn(format_args!(
            "{at}: {program} is a GoogleTest binary but did not apply sharding, so this shard ran the whole suite; \
             results stand, the work was duplicated; later runs use GOWAY_SHARD only"
        ));
    }
    if d.rerun {
        renderer.warn(format_args!(
            "{at}: {program} rejected the Catch2 shard flags; the shard was rerun once without them \
             (attempts exited {:?}); later runs use GOWAY_SHARD only",
            d.attempts
        ));
    }
    if d.rejected_again {
        renderer.warn(format_args!(
            "{at}: {program} was rejected again after the rerun; goway stopped there (exit {})",
            report.exit_code
        ));
    }
    if d.flagged {
        renderer.warn(format_args!(
            "{at}: {program} failed with a command-line error that may be about the shard flags; \
             not certain, so it was not rerun"
        ));
    }
    d.failed()
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
    let (selection, rule) =
        project::selection_for(&repo.root, &args.command, &args.needs, &args.prefers)?;
    if let Some(r) = &rule {
        renderer.note(r.describe());
    }
    let project = runners::project_for(&args.command, &args.env, &repo.root)?;
    let mut state = State::load(&env.paths.state_file())?;
    // Plan every shard first, so a command that cannot be split is refused before any host is touched.
    let mut plans = (1..=count)
        .map(|index| runners::plan(&args.command, &args.env, &project, index, count))
        .collect::<Result<Vec<_>>>()?;
    if let Some(framework) = plans.first().and_then(|p| p.framework) {
        renderer.note(format_args!("sharding as {}", framework.name()));
    }
    let program = args.command.first().cloned().unwrap_or_default();
    if plans.first().is_some_and(|p| p.detect) {
        if detect::is_marked(&state, &repo.id, &program) {
            renderer.note(format_args!(
                "not detecting the test framework of {program}: it failed before in this repository; \
                 shards only see GOWAY_SHARD and GOWAY_SHARD_COUNT (GOWAY_RUNNER=gtest or catch2 overrides)"
            ));
            for plan in &mut plans {
                plan.detect = false;
            }
        } else {
            renderer.note(format_args!(
                "each helper checks whether {program} is a GoogleTest or Catch2 v3 binary (reading it, never running it)"
            ));
        }
    }
    let hosts = pool::choose_many(
        &config, &selection, &mut state, env.lookup, env.prober, count,
    )?;
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
                let selection = &selection;
                let repo = &repo;
                let interrupted = &interrupted;
                let plan = &plans[i];
                scope.spawn(move || -> Result<ShardReport> {
                    let shard_started = Instant::now();
                    let index = i + 1;
                    let prefix = format!("[{:<width$}] ", host.name);
                    let run_id = run::new_run_id() + &format!("-s{index}");
                    run::sync_snapshot(env, config, repo, found, &run_id, args.keep)?;
                    let mut pairs = args.env.clone();
                    pairs.push(format!("GOWAY_SHARD={index}"));
                    pairs.push(format!("GOWAY_SHARD_COUNT={count}"));
                    pairs.extend(plan.env.iter().cloned());
                    let command = plan.command.clone();
                    run::send_env(env, config, found, &run_id, &run::encode_env(&pairs)?)?;
                    let nonce = detect::nonce();
                    let extra: Vec<String> = if plan.detect {
                        vec![detect::request_word(index, count, &nonce)]
                    } else {
                        Vec::new()
                    };
                    let cmd = run::run_invocation_with(
                        config,
                        config.priority_of(host).as_str(),
                        repo,
                        &run_id,
                        args.keep,
                        &extra,
                        &command,
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
                    let filter_out = termfilter::should_filter(
                        args.output,
                        std::io::IsTerminal::is_terminal(&std::io::stdout()),
                    );
                    let filter_err = termfilter::should_filter(
                        args.output,
                        std::io::IsTerminal::is_terminal(&std::io::stderr()),
                    );
                    let mut result_line = None;
                    std::thread::scope(|s| {
                        if let Some(out) = out {
                            let p = prefix.clone();
                            s.spawn(move || pump(out, false, &p, filter_out));
                        }
                        if let Some(err) = err {
                            let p = prefix.clone();
                            if plan.detect {
                                let (split, line) = ResultSplitter::new(err, &nonce);
                                result_line = Some(line);
                                s.spawn(move || pump(split, true, &p, filter_err));
                            } else {
                                s.spawn(move || pump(err, true, &p, filter_err));
                            }
                        }
                    });
                    let detection = result_line
                        .and_then(|f| f.lock().ok().and_then(|l| l.clone()))
                        .and_then(|text| Detection::parse(&text));
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
                        matched: selection.assess(host, probe).matched,
                        shard: index,
                        host: host.name.clone(),
                        address: found.target.address.clone(),
                        arch: probe.arch.clone(),
                        hostname: probe.hostname.clone(),
                        command,
                        exit_code: code,
                        detection,
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
    let mut remember = false;
    for (i, result) in results.into_iter().enumerate() {
        match result {
            Ok(report) => {
                if let Some(d) = &report.detection {
                    remember |= note_detection(renderer, &report, d, &program);
                }
                let line = format_args!(
                    "shard {}/{count} on {}: exit {} in {:.1}s",
                    report.shard, report.host, report.exit_code, report.duration_secs
                );
                if report.exit_code == 0 {
                    renderer.ok(line);
                } else {
                    renderer.failed(line);
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
    if remember {
        detect::mark(&mut state, &repo.name, &repo.id, &program);
        if let Err(e) = state.save(&env.paths.state_file()) {
            tracing::warn!(error = %e, "cannot remember the failed detection");
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
            rule,
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

    fn pumped(input: &[u8], filter: bool) -> Vec<Vec<u8>> {
        let mut batches = Vec::new();
        pump_with(input, "[h1] ", filter, |b| {
            if !b.is_empty() {
                batches.push(b.to_vec());
            }
        });
        batches
    }

    // frob:tests crates/goway/src/shard.rs::pump
    #[test]
    fn complete_buffered_lines_go_out_in_one_batch() {
        let batches = pumped(b"a\nb\nc\n", false);
        assert_eq!(batches, vec![b"[h1] a\n[h1] b\n[h1] c\n".to_vec()]);
    }

    // frob:tests crates/goway/src/shard.rs::pump
    #[test]
    fn a_partial_last_line_gets_a_newline_at_eof() {
        let batches = pumped(b"one\ntwo", false);
        let all: Vec<u8> = batches.concat();
        assert_eq!(all, b"[h1] one\n[h1] two\n");
    }

    // frob:tests crates/goway/src/shard.rs::pump
    #[test]
    fn a_long_line_is_split_with_a_continuation_prefix() {
        let mut input = vec![b'x'; render::MAX_LINE * 2 + 10];
        input.push(b'\n');
        input.extend_from_slice(b"next\n");
        let all: Vec<u8> = pumped(&input, false).concat();
        let lines: Vec<&[u8]> = all
            .split(|&b| b == b'\n')
            .filter(|l| !l.is_empty())
            .collect();
        assert_eq!(lines.len(), 4);
        assert!(lines[0].starts_with(b"[h1] x"));
        assert!(lines[1].starts_with(b"[h1]+ x"));
        assert!(lines[2].starts_with(b"[h1]+ x"));
        assert_eq!(lines[3], b"[h1] next");
        assert!(lines.iter().all(|l| l.len() <= render::MAX_LINE + 6));
        let x: usize = lines[..3]
            .iter()
            .map(|l| l.len() - l.iter().position(|&b| b == b'x').unwrap())
            .sum();
        assert_eq!(x, render::MAX_LINE * 2 + 10);
    }

    // frob:tests crates/goway/src/shard.rs::pump
    #[test]
    fn lines_split_across_reads_stay_whole_and_filtered_output_is_framed() {
        struct Dribble<'a>(&'a [u8]);
        impl std::io::Read for Dribble<'_> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let n = self.0.len().min(3).min(buf.len());
                buf[..n].copy_from_slice(&self.0[..n]);
                self.0 = &self.0[n..];
                Ok(n)
            }
        }
        let mut out = Vec::new();
        pump_with(Dribble(b"hello\nwor\x1b]0;t\x07ld\n"), "[h1] ", true, |b| {
            out.extend_from_slice(b);
        });
        assert_eq!(out, b"[h1] hello\n[h1] world\n");
    }
}
