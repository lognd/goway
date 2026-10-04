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
use crate::local;
use crate::needs::Matched;
use crate::pool;
use crate::project::{self, Applied};
use crate::render::{self, Renderer};
use crate::repo::Repo;
use crate::run::{self, Env};
use crate::runners;
use crate::spawn::CommandExt as _;
use crate::ssh;
use crate::state::State;
use crate::termfilter;

/// How big a shard was, relative to the others.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Share {
    /// Free cores the host had when it was chosen (cores minus load).
    pub capacity: f64,
    /// This shard's weight; shares are `weight` out of `total_weight`.
    pub weight: u32,
    /// The sum of all shards' weights.
    pub total_weight: u32,
    /// `weight / total_weight`.
    pub fraction: f64,
    /// Whether the split followed capacity (false: equal shares, because
    /// the framework or command cannot be split by weight).
    pub weighted: bool,
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
    /// The host's operating system (`linux` or `windows`).
    pub os: String,
    /// `uname -m`, or the Windows processor architecture.
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
    /// This shard's share of the tests.
    pub share: Share,
    /// Every attempt (copy verification), when the shard ran on a helper.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attempts: Vec<run::AttemptRecord>,
    /// What the helper's test-binary detection did, when it was asked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detection: Option<Detection>,
    /// What the host ran in place of argv[0], when the program was translated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub translation: Option<crate::translate::Translation>,
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

/// The shares note body: each host with its share, largest share first and
/// then by name, so the line is the same whatever order the pool chose.
fn shares_note<'a>(names: impl Iterator<Item = &'a str>, weights: &runners::Weights) -> String {
    let mut shares: Vec<(&str, u32)> = names
        .enumerate()
        .map(|(i, name)| (name, weights.weight(i + 1)))
        .collect();
    shares.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    shares
        .iter()
        .map(|(name, w)| format!("{name} {w}/{}", weights.total()))
        .collect::<Vec<_>>()
        .join(", ")
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
pub fn run_sharded(env: &Env<'_>, renderer: Renderer, args: &RunArgs, count: u16) -> Result<u8> {
    fan_out(env, renderer, args, Fan::Shards(usize::from(count)))
}

/// `goway run --each-os`: the whole command once on the best host of each OS in the pool,
/// in parallel, with `[host os]` prefixes. Exits with the first failing OS's code in OS-name
/// order (125 when goway itself failed there), or 0 when every OS passed.
///
/// # Errors
///
/// As [`run_sharded`].
pub fn run_each_os(env: &Env<'_>, renderer: Renderer, args: &RunArgs) -> Result<u8> {
    fan_out(env, renderer, args, Fan::EachOs)
}

/// The OS a host reported (`linux`, `darwin`, `windows`), else the one its config says.
fn os_of(found: &crate::resolve::Found, probe: &pool::Probe) -> String {
    reported_os(found.kind.os().as_str(), probe)
}

fn reported_os(configured: &str, probe: &pool::Probe) -> String {
    probe
        .facts
        .os
        .as_deref()
        .map_or_else(|| configured.to_owned(), str::to_ascii_lowercase)
}

/// How a run is spread over hosts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fan {
    /// One part of the tests per host, N hosts.
    Shards(usize),
    /// The whole command on one host of each OS.
    EachOs,
}

#[allow(clippy::too_many_lines)] // the shard thread is one sequence: sync, run, report
fn fan_out(env: &Env<'_>, renderer: Renderer, args: &RunArgs, fan: Fan) -> Result<u8> {
    let started = Instant::now();
    let each_os = fan == Fan::EachOs;
    let mut count = match fan {
        Fan::Shards(n) => n,
        Fan::EachOs => 0,
    };
    let config = Config::load(&env.paths.config_file())?;
    let repo = Repo::discover(env.cwd)?;
    // --each-os covers every OS by definition; the hint only applies to plain sharding.
    let (selection, rule) = project::selection_for(
        &repo.root,
        &args.command,
        &args.needs,
        &args.prefers,
        args.any_os || each_os,
    )?;
    if let Some(r) = &rule {
        renderer.note(r.describe());
    }
    if !each_os {
        project::warn_cross_os(
            renderer,
            &config,
            &args.command,
            &selection,
            project::cross_os_setting(&repo.root)?,
            &project::translate_overrides(&repo.root)?,
        );
    }
    let with_git = args.with_git || project::wants_git(&repo.root)?;
    let overrides = project::translate_overrides(&repo.root)?;
    let project = runners::project_for(&args.command, &args.env, &repo.root)?;
    let mut state = State::load(&env.paths.state_file())?;
    // Plan every shard first, so a command that cannot be split is refused before any host is touched.
    let mut plans = if each_os {
        Vec::new()
    } else {
        (1..=count)
            .map(|index| runners::plan(&args.command, &args.env, &project, index, count))
            .collect::<Result<Vec<_>>>()?
    };
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
    let hosts = if each_os {
        let hosts = pool::choose_each_os(
            &config,
            &selection,
            &mut state,
            &local::jobs_dir(env.paths),
            env.lookup,
            env.prober,
        )?;
        count = hosts.len();
        // The same command everywhere: nothing is split.
        plans = (0..count)
            .map(|_| runners::Plan {
                framework: None,
                command: args.command.clone(),
                env: Vec::new(),
                detect: false,
            })
            .collect();
        hosts
    } else {
        pool::choose_many(
            &config,
            &selection,
            &mut state,
            &local::jobs_dir(env.paths),
            env.lookup,
            env.prober,
            count,
        )?
    };
    if let Err(e) = state.save(&env.paths.state_file()) {
        tracing::warn!(error = %e, "cannot cache host addresses");
    }
    // Capacity-weighted shares (for the adapters that can split by weight).
    let capacities: Vec<f64> = hosts.iter().map(|(_, _, p)| pool::capacity(p)).collect();
    let can_weigh = plans
        .first()
        .and_then(|p| p.framework)
        .is_some_and(runners::Framework::weighted);
    let weights = if can_weigh {
        runners::Weights::from_capacities(&capacities)
    } else {
        runners::Weights::equal(count)
    };
    if weights.is_unequal() {
        let detect_flags: Vec<bool> = plans.iter().map(|p| p.detect).collect();
        plans = (1..=count)
            .map(|index| {
                runners::plan_weighted(&args.command, &args.env, &project, index, &weights)
            })
            .collect::<Result<Vec<_>>>()?;
        for (p, d) in plans.iter_mut().zip(detect_flags) {
            p.detect = d;
        }
        renderer.note(format_args!(
            "shares by free cores: {}",
            shares_note(hosts.iter().map(|(h, ..)| h.name.as_str()), &weights)
        ));
    } else if plans.first().and_then(|p| p.framework).is_some()
        && !can_weigh
        && capacities.len() > 1
    {
        let unequal = runners::Weights::from_capacities(&capacities).is_unequal();
        if unequal {
            renderer.note(
                "hosts differ in free capacity, but this framework splits its shards equally",
            );
        }
    }
    let label_of = |h: &crate::config::HostConfig, f: &crate::resolve::Found, p: &pool::Probe| {
        if each_os {
            format!("{} {}", h.name, os_of(f, p))
        } else {
            h.name.clone()
        }
    };
    let labels: Vec<String> = hosts.iter().map(|(h, f, p)| label_of(h, f, p)).collect();
    renderer.headline(format_args!(
        "{} {} {}: {}",
        if each_os { "running" } else { "sharding" },
        ssh::shell_join(&args.command),
        if each_os {
            format!("once per OS on {count} hosts")
        } else {
            format!("across {count} hosts")
        },
        labels.join(", ")
    ));
    let interrupted = run::interrupt_flag();
    let now = crate::state::now_secs();
    let distrusted: Vec<bool> = hosts
        .iter()
        .map(|(h, ..)| !args.trust_copy && state.distrusted(&h.name, &repo.id, now))
        .collect();
    let width = labels.iter().map(String::len).max().unwrap_or(0);
    let results: Vec<Result<ShardReport>> = std::thread::scope(|scope| {
        let handles: Vec<_> = hosts
            .iter()
            .enumerate()
            .map(|(i, (host, found, probe))| {
                let config = &config;
                let labels = &labels;
                let selection = &selection;
                let capacities = &capacities;
                let weights = &weights;
                let repo = &repo;
                let overrides = &overrides;
                let interrupted = &interrupted;
                let plan = &plans[i];
                let distrusted = distrusted[i];
                scope.spawn(move || -> Result<ShardReport> {
                    let shard_started = Instant::now();
                    let index = i + 1;
                    let prefix = format!("[{:<width$}] ", labels[i]);
                    let first_run_id = run::new_run_id() + &format!("-s{index}");
                    let mut pairs = args.env.clone();
                    if !each_os {
                        pairs.push(format!("GOWAY_SHARD={index}"));
                        pairs.push(format!("GOWAY_SHARD_COUNT={count}"));
                    }
                    pairs.extend(plan.env.iter().cloned());
                    let command = plan.command.clone();
                    let nonce = detect::nonce();
                    // Held until this shard ends (only a local shard takes one).
                    let mut _slot = None;
                    let mut verify = run::Verify::first(distrusted);
                    let mut translation: Option<crate::translate::Translation> = None;
                    let mut attempts: Vec<run::AttemptRecord> = Vec::new();
                    let (code, detection) = loop {
                        let run_id = if verify.attempt == 1 {
                            first_run_id.clone()
                        } else {
                            format!("{first_run_id}-a{}", verify.attempt)
                        };
                        let mut manifest = None;
                        let mut git_overlay = None;
                        let mut child = if found.is_local() {
                            // This machine: nothing to sync; the command runs in the current directory.
                            _slot =
                                Some(local::Slot::acquire(&local::jobs_dir(env.paths), &run_id)?);
                            let job = local::Job {
                                command: &command,
                                env: &pairs,
                                priority: config.priority_of(host),
                                cwd: env.cwd,
                                run_id: &run_id,
                                hostname: &probe.hostname,
                            };
                            local::process(&job)
                                .stdin(Stdio::null())
                                .stdout(Stdio::piped())
                                .stderr(Stdio::piped())
                                .spawn_locked()
                                .map_err(|e| {
                                    Error::Usage(format!("cannot run `{}`: {e}", command[0]))
                                })?
                        } else {
                            let synced = run::sync_snapshot(
                                env,
                                config,
                                repo,
                                found,
                                &run_id,
                                args.keep,
                                with_git.then_some(host.os),
                            )?;
                            git_overlay = synced.git_overlay;
                            manifest = Some(synced.manifest);
                            run::send_env(env, config, found, &run_id, &run::encode_env(&pairs)?)?;
                            let (run_command, translated) = run::translated_command(
                                env,
                                found,
                                probe,
                                &config.defaults.remote_root,
                                &run_id,
                                &command,
                                overrides,
                            )?;
                            if let Some(t) = &translated
                                && translation.is_none()
                            {
                                renderer.note(format_args!("{}: {}", host.name, t.describe()));
                            }
                            translation = translated;
                            let mut extra = run::gpu_words(selection, config, host);
                            if plan.detect {
                                extra.push(detect::request_word(index, count, &nonce));
                            }
                            extra.push(verify.word());
                            let priority = pool::priority_word(config, host, found, probe);
                            if verify.attempt == 1
                                && let Some(note) = pool::owner_note(&host.name, probe, priority)
                            {
                                renderer.note(format_args!("{note}"));
                            }
                            let cmd = run::run_invocation_with(
                                &config.for_host(host),
                                priority,
                                repo,
                                &run_id,
                                args.keep,
                                &extra,
                                &run_command,
                            );
                            crate::sync::SshTransport::of(found, env.settings)
                                .command(&cmd)?
                                .stdin(Stdio::null())
                                .stdout(Stdio::piped())
                                .stderr(Stdio::piped())
                                .spawn_locked()
                                .map_err(|e| Error::Ssh {
                                    host: host.name.clone(),
                                    message: format!("cannot run ssh: {e}"),
                                })?
                        };
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
                        let gate = manifest.as_ref().map(|manifest| run::Gate {
                            transport: crate::sync::SshTransport::of(found, env.settings),
                            remote_root: config.defaults.remote_root.as_str(),
                            run_id: &run_id,
                            repo_root: &repo.root,
                            git_overlay: git_overlay.as_deref(),
                            manifest,
                        });
                        let done = std::sync::atomic::AtomicBool::new(false);
                        let gate_report = std::thread::scope(|s| {
                            let watcher = gate.as_ref().map(|g| {
                                let done = &done;
                                s.spawn(move || g.drive(done))
                            });
                            let mut pumps = Vec::new();
                            if let Some(out) = out {
                                let p = prefix.clone();
                                let framed =
                                    !found.is_local() && found.kind == crate::transport::Kind::Unix;
                                pumps.push(s.spawn(move || {
                                    if framed {
                                        pump(
                                            crate::remote::Framed::new(out),
                                            false,
                                            &p,
                                            filter_out,
                                        );
                                    } else {
                                        pump(out, false, &p, filter_out);
                                    }
                                }));
                            }
                            if let Some(err) = err {
                                let p = prefix.clone();
                                if plan.detect && !found.is_local() {
                                    let (split, line) = ResultSplitter::new(err, &nonce);
                                    result_line = Some(line);
                                    pumps.push(s.spawn(move || pump(split, true, &p, filter_err)));
                                } else {
                                    pumps.push(s.spawn(move || pump(err, true, &p, filter_err)));
                                }
                            }
                            for p in pumps {
                                let _ = p.join();
                            }
                            // The output ended with the ssh session; stop the watcher.
                            done.store(true, std::sync::atomic::Ordering::SeqCst);
                            watcher.map(|w| w.join().expect("the verification thread panicked"))
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
                        let Some(gate_report) = gate_report else {
                            break (code, detection);
                        };
                        attempts.push(run::AttemptRecord::new(
                            verify.attempt,
                            &run_id,
                            code,
                            &gate_report,
                        ));
                        if let Some(why) = &gate_report.error {
                            return Err(Error::Ssh {
                                host: host.name.clone(),
                                message: format!("copy verification failed: {why}"),
                            });
                        }
                        if gate_report.mismatches.is_empty() {
                            break (code, detection);
                        }
                        run::warn_mismatch(
                            renderer,
                            &host.name,
                            repo,
                            &gate_report,
                            verify.attempt < run::MAX_ATTEMPTS,
                        );
                        if verify.attempt >= run::MAX_ATTEMPTS {
                            // The rebuilt copy failed too: stop with the evidence.
                            break (crate::error::EXIT_GOWAY_FAILURE, detection);
                        }
                        verify = run::Verify::rerun();
                    };
                    tracing::info!(host = %host.name, index, code, "shard finished");
                    Ok(ShardReport {
                        matched: selection.assess(host, probe).matched,
                        share: Share {
                            capacity: capacities[i],
                            weight: weights.weight(index),
                            total_weight: u32::try_from(weights.total()).unwrap_or(u32::MAX),
                            fraction: f64::from(weights.weight(index))
                                / f64::from(u32::try_from(weights.total()).unwrap_or(u32::MAX)),
                            weighted: can_weigh && weights.is_unequal(),
                        },
                        shard: index,
                        host: host.name.clone(),
                        address: found.target.address.clone(),
                        os: os_of(found, probe),
                        arch: probe.arch.clone(),
                        hostname: probe.hostname.clone(),
                        command,
                        exit_code: code,
                        attempts,
                        detection,
                        translation,
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
                let share = if report.share.weighted {
                    format!(", {:.0}% share", report.share.fraction * 100.0)
                } else {
                    String::new()
                };
                let line = if each_os {
                    format!(
                        "{} on {}: exit {} in {:.1}s",
                        report.os, report.host, report.exit_code, report.duration_secs
                    )
                } else {
                    format!(
                        "shard {}/{count} on {} ({}): exit {} in {:.1}s{share}",
                        report.shard,
                        report.host,
                        report.os,
                        report.exit_code,
                        report.duration_secs
                    )
                };
                if report.exit_code == 0 {
                    renderer.ok(&line);
                } else {
                    renderer.failed(&line);
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
    let mut mismatched = false;
    for report in &shards {
        if report.attempts.iter().any(|a| !a.valid) {
            state.mark_mismatch(&report.host, &repo.name, &repo.id, crate::state::now_secs());
            mismatched = true;
        }
    }
    if mismatched && let Err(e) = state.save(&env.paths.state_file()) {
        tracing::warn!(error = %e, "cannot remember the copy mismatch");
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

    #[test]
    fn shares_note_lists_the_largest_share_first_whatever_the_pool_order() {
        let w = runners::Weights::from_capacities(&[1.0, 4.0]);
        let note = shares_note(["small", "big"].into_iter(), &w);
        assert_eq!(
            note,
            format!(
                "big {}/{t}, small {}/{t}",
                w.weight(2),
                w.weight(1),
                t = w.total()
            )
        );
        let swapped = runners::Weights::from_capacities(&[4.0, 1.0]);
        assert_eq!(shares_note(["big", "small"].into_iter(), &swapped), note);
    }

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

    // frob:ticket 01M439YZ808PY91MXTGS4SAKKH
    // frob:tests crates/goway/src/shard.rs::os_of
    #[test]
    fn the_os_a_host_reported_beats_the_configured_one() {
        let probe = |os: &str| {
            pool::parse_probe(&format!(
                "arch=x86_64\nhostname=h\ncores=1\nload1=0\nload5=0\nload15=0\njobs=0\n{os}"
            ))
            .unwrap()
        };
        assert_eq!(reported_os("linux", &probe("os=Darwin\n")), "darwin");
        assert_eq!(reported_os("linux", &probe("")), "linux");
    }
}
