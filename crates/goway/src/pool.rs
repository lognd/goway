//! The host pool: probe every host in parallel (one ssh call each, which
//! also resolves its address) and pick the least loaded.
//!
//! Score = (1-minute load + goway jobs running) / cores. goway's own jobs
//! are counted on top of the load average because a job that just started
//! has not shown up in the load yet. Hosts at `max_jobs` are skipped.
//!
//! A host short of memory scores worse: when its available RAM per core is
//! below `mem_per_core` GiB (default 0.5), up to 1.0 is added in proportion
//! to the shortfall, so a 16-core host with 3 GiB loses to a roomier one but
//! is still used when nothing else is.

use std::collections::BTreeMap;
use std::path::Path;

use crate::config::{Config, HostConfig};
use crate::error::{Error, Result};
use crate::facts::{self, Facts};
use crate::local;
use crate::needs::Selection;
use crate::remote;
use crate::resolve::{self, Found, Lookup, Prober, Source};
use crate::ssh::KeyPolicy;
use crate::state::State;

/// What a probe reports about a host.
#[derive(Debug, Clone, PartialEq)]
pub struct Probe {
    /// `uname -m`.
    pub arch: String,
    /// `uname -n`.
    pub hostname: String,
    /// Logical CPUs.
    pub cores: u32,
    /// Load averages over 1, 5 and 15 minutes.
    pub load: [f64; 3],
    /// goway jobs running now.
    pub jobs: u32,
    /// Bytes under goway's remote root (status only).
    pub disk_used: Option<u64>,
    /// Bytes free in the remote home (status only).
    pub disk_free: Option<u64>,
    /// RAM, GPUs and other facts (see [`crate::facts`]).
    pub facts: Facts,
}

/// The default `defaults.mem_per_core` in GiB.
pub const DEFAULT_MEM_PER_CORE: f64 = 0.5;

/// Parse the `probe` verb's `key=value` lines.
pub fn parse_probe(text: &str) -> Option<Probe> {
    let kv: BTreeMap<&str, &str> = text
        .lines()
        .filter_map(|l| l.trim().split_once('='))
        .collect();
    // A host reports these about itself: accept only sane values, so a
    // hostile or broken host cannot win scheduling with -inf or 0 cores.
    let num = |k: &str| {
        kv.get(k)
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|x| x.is_finite() && (0.0..1.0e6).contains(x))
    };
    let cores: u32 = kv
        .get("cores")?
        .parse()
        .ok()
        .filter(|c| (1..=65_536).contains(c))?;
    let jobs: u32 = kv.get("jobs")?.parse().ok().filter(|j| *j <= 1_000_000)?;
    Some(Probe {
        arch: crate::render::clean(kv.get("arch")?),
        hostname: crate::render::clean(kv.get("hostname")?),
        cores,
        load: [num("load1")?, num("load5")?, num("load15")?],
        jobs,
        disk_used: kv.get("disk_used").and_then(|v| v.parse().ok()),
        disk_free: kv.get("disk_free").and_then(|v| v.parse().ok()),
        facts: facts::parse_live(
            &kv.iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        ),
    })
}

/// Penalty in 0..=1 for a host with less than `mem_per_core` GiB of
/// available RAM per core; 0 when unknown or switched off.
pub fn mem_penalty(p: &Probe, mem_per_core: f64) -> f64 {
    let Some(ram) = p.facts.mem_avail.or(p.facts.mem_total) else {
        return 0.0;
    };
    if mem_per_core.is_nan() || mem_per_core <= 0.0 {
        return 0.0;
    }
    #[allow(clippy::cast_precision_loss)] // scoring only
    let per_core = ram as f64 / (1024.0 * 1024.0 * 1024.0) / f64::from(p.cores.max(1));
    (1.0 - per_core / mem_per_core).clamp(0.0, 1.0)
}

/// Lower is better.
pub fn score(p: &Probe, mem_per_core: f64) -> f64 {
    (p.load[0] + f64::from(p.jobs)) / f64::from(p.cores.max(1)) + mem_penalty(p, mem_per_core)
}

/// Free capacity in cores: cores minus 1-minute load minus goway's own
/// jobs (not yet in the load), never below half a core. Sharding gives
/// each host a share proportional to this.
pub fn capacity(p: &Probe) -> f64 {
    (f64::from(p.cores) - p.load[0] - f64::from(p.jobs)).max(0.5)
}

/// A probed host: where it answered and what it said, or why it did not.
#[derive(Debug)]
pub struct Probed<'a> {
    /// The host's config.
    pub host: &'a HostConfig,
    /// The working address and probe, or the failure.
    pub result: Result<(Found, Probe)>,
}

/// How much one met `--prefers` term lowers a host's score (a host with
/// that much more load per core still wins on the preference).
pub const PREFER_BONUS: f64 = 0.5;

/// Indices of the usable hosts among `probed`, best first. Hosts at
/// `max_jobs` or above their `max_load` (load per core) are left out.
pub fn ranked(config: &Config, probed: &[Probed<'_>]) -> Vec<usize> {
    ranked_for(config, &Selection::default(), probed)
}

/// [`ranked`] for a run with `selection`: hosts failing a `--needs` term
/// are left out, and each met `--prefers` term lowers the score by
/// [`PREFER_BONUS`] (preferences never exclude).
pub fn ranked_for(config: &Config, selection: &Selection, probed: &[Probed<'_>]) -> Vec<usize> {
    let mut usable: Vec<(usize, f64, u32)> = probed
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            let (_, probe) = p.result.as_ref().ok()?;
            if p.host.max_jobs.is_some_and(|m| probe.jobs >= m) {
                tracing::info!(host = %p.host.name, jobs = probe.jobs, "host at max_jobs; skipped");
                return None;
            }
            let per_core = probe.load[0] / f64::from(probe.cores.max(1));
            if config.max_load_of(p.host).is_some_and(|m| per_core > m) {
                tracing::info!(host = %p.host.name, per_core, "host above max_load; skipped");
                return None;
            }
            let a = selection.assess(p.host, probe);
            if !a.qualifies() {
                tracing::info!(host = %p.host.name, lacks = ?a.lacks, "host fails --needs; skipped");
                return None;
            }
            #[allow(clippy::cast_precision_loss)] // a handful of terms
            let bonus = PREFER_BONUS * a.preferences_met as f64;
            // This machine competes with a margin: it is somebody's laptop too.
            let margin = match &p.result {
                Ok((found, _)) if found.is_local() => crate::local::settings(config).margin,
                _ => 0.0,
            };
            Some((
                i,
                score(probe, config.defaults.mem_per_core) - bonus + margin,
                probe.jobs,
            ))
        })
        .collect();
    usable.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.2.cmp(&b.2)).then(a.0.cmp(&b.0)));
    usable.into_iter().map(|(i, _, _)| i).collect()
}

/// Index of the best host among `probed`, if any is usable.
pub fn pick(config: &Config, probed: &[Probed<'_>]) -> Option<usize> {
    pick_for(config, &Selection::default(), probed)
}

/// [`pick`] for a run with `selection`.
pub fn pick_for(config: &Config, selection: &Selection, probed: &[Probed<'_>]) -> Option<usize> {
    let best = ranked_for(config, selection, probed).first().copied();
    if let Some(i) = best {
        tracing::info!(host = %probed[i].host.name, "picked");
    }
    best
}

/// Why no (or not enough) hosts are usable, one line per host.
fn unusable(selection: &Selection, results: &[Probed<'_>]) -> Vec<String> {
    results
        .iter()
        .map(|p| match &p.result {
            Ok((_, probe)) => {
                let a = selection.assess(p.host, probe);
                if a.qualifies() {
                    format!(
                        "{}: load {:.2} on {} cores, {} goway jobs",
                        p.host.name, probe.load[0], probe.cores, probe.jobs
                    )
                } else {
                    format!("{}: lacks {}", p.host.name, a.lacks.join("; lacks "))
                }
            }
            Err(e) => format!("{}: {e}", p.host.name),
        })
        .collect()
}

/// The error for "nothing usable": a needs error when every reachable host
/// fails a need (and at least one is reachable), else the general one.
fn none_usable(selection: &Selection, results: &[Probed<'_>]) -> Error {
    let reachable: Vec<&Probed<'_>> = results.iter().filter(|p| p.result.is_ok()).collect();
    let all_fail_needs = !selection.needs.is_empty()
        && !reachable.is_empty()
        && reachable.iter().all(|p| {
            p.result
                .as_ref()
                .is_ok_and(|(_, probe)| !selection.assess(p.host, probe).qualifies())
        });
    let lines = unusable(selection, results);
    if all_fail_needs {
        Error::NeedsUnmet(lines)
    } else {
        Error::NoHost(lines)
    }
}

/// The `n` least-loaded usable hosts, best first (for sharding).
///
/// # Panics
///
/// Never: each ranked index is taken once.
pub fn choose_many(
    config: &Config,
    selection: &Selection,
    state: &mut State,
    jobs: &Path,
    lookup: &(dyn Lookup + Sync),
    prober: &(dyn Prober + Sync),
    n: usize,
) -> Result<Vec<(HostConfig, Found, Probe)>> {
    let local_host = local::host(config);
    let mut results = probe_all(config, state, lookup, prober, selection.wants_disk());
    if config.local_in_pool() {
        push_local(config, &local_host, jobs, state, selection, &mut results);
    }
    let order = ranked_for(config, selection, &results);
    if order.len() < n {
        let mut why = vec![format!(
            "{n} shards need {n} usable hosts{}, {} are usable",
            if selection.needs.is_empty() {
                ""
            } else {
                " that each meet --needs"
            },
            order.len()
        )];
        why.extend(unusable(selection, &results));
        return Err(if selection.needs.is_empty() {
            Error::NoHost(why)
        } else {
            Error::NeedsUnmet(why)
        });
    }
    let mut slots: Vec<Option<Probed<'_>>> = results.into_iter().map(Some).collect();
    order
        .into_iter()
        .take(n)
        .map(|i| {
            let p = slots[i].take().expect("each index once");
            let (found, probe) = p.result?;
            Ok((p.host.clone(), found, probe))
        })
        .collect()
}

/// The remote command that probes a host.
pub fn probe_command(config: &Config, disk: bool, statics: bool) -> String {
    let root = config.defaults.remote_root.as_str();
    let mut args = vec![root];
    if disk {
        args.push("disk");
    }
    if statics {
        args.push("static");
    }
    remote::invocation("probe", &args)
}

/// Resolve and probe one host.
pub fn probe_one(
    config: &Config,
    host: &HostConfig,
    state: &mut State,
    lookup: &dyn Lookup,
    prober: &dyn Prober,
    disk: bool,
) -> Result<(Found, Probe)> {
    let key = host.name.to_ascii_lowercase();
    let now = crate::state::now_secs();
    let statics = state.refresh_facts || facts::stale(state.facts.get(&key), now);
    let found = resolve::resolve(
        config,
        host,
        state,
        lookup,
        prober,
        KeyPolicy::Strict,
        &probe_command(config, disk, statics),
    )?;
    let probe = complete_probe(&host.name, &found.output, state, now)?;
    Ok((found, probe))
}

/// Parse a probe's output for host `name`, caching any static facts in
/// `state` and attaching the cached ones to the result.
///
/// # Errors
///
/// [`Error::Ssh`] when the output is not a probe's.
pub fn complete_probe(name: &str, output: &str, state: &mut State, now: u64) -> Result<Probe> {
    let key = name.to_ascii_lowercase();
    let mut probe = parse_probe(output).ok_or_else(|| Error::Ssh {
        host: name.to_owned(),
        message: format!("unexpected probe output: {}", output.trim()),
    })?;
    if let Some(hw) = facts::parse_static(&facts::kv(output)) {
        tracing::info!(host = name, gpus = hw.gpus.len(), "host facts refreshed");
        state
            .facts
            .insert(key.clone(), facts::Cached { at: now, facts: hw });
    }
    if let Some(c) = state.facts.get(&key) {
        probe.facts.hw = Some(c.facts.clone());
        probe.facts.hw_age = Some(now.saturating_sub(c.at));
    }
    tracing::debug!(host = name, ?probe, "probed");
    Ok(probe)
}

/// Run `f` for each of `hosts` in parallel, each with its own copy of
/// `state`; working addresses found on the way are merged back.
///
/// # Panics
///
/// Only if `f` panics, which is a bug.
pub fn on_hosts<'a, T, F>(
    hosts: &[&'a HostConfig],
    state: &mut State,
    f: F,
) -> Vec<(&'a HostConfig, Result<T>)>
where
    T: Send,
    F: Fn(&HostConfig, &mut State) -> Result<T> + Sync,
{
    let snapshot = state.clone();
    let f = &f;
    let results: Vec<(&'a HostConfig, Result<T>, State)> = std::thread::scope(|scope| {
        let handles: Vec<_> = hosts
            .iter()
            .map(|&host| {
                let mut local = snapshot.clone();
                scope.spawn(move || {
                    let result = f(host, &mut local);
                    (host, result, local)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("host thread panicked"))
            .collect()
    });
    results
        .into_iter()
        .map(|(host, result, local)| {
            if let Some(s) = local.get(&host.name) {
                state
                    .hosts
                    .insert(host.name.to_ascii_lowercase(), s.clone());
            }
            if let Some(f) = local.facts.get(&host.name.to_ascii_lowercase()) {
                state
                    .facts
                    .insert(host.name.to_ascii_lowercase(), f.clone());
            }
            (host, result)
        })
        .collect()
}

/// Probe every host in parallel; working addresses are merged into `state`.
pub fn probe_all<'a>(
    config: &'a Config,
    state: &mut State,
    lookup: &(dyn Lookup + Sync),
    prober: &(dyn Prober + Sync),
    disk: bool,
) -> Vec<Probed<'a>> {
    let hosts: Vec<&HostConfig> = config.hosts.iter().collect();
    on_hosts(&hosts, state, |host, local| {
        probe_one(config, host, local, lookup, prober, disk)
    })
    .into_iter()
    .map(|(host, result)| Probed { host, result })
    .collect()
}

/// Choose where to run: `wanted` if given, else the least-loaded host.
pub fn choose(
    config: &Config,
    selection: &Selection,
    state: &mut State,
    jobs: &Path,
    lookup: &(dyn Lookup + Sync),
    prober: &(dyn Prober + Sync),
    wanted: Option<&str>,
) -> Result<(HostConfig, Found, Probe)> {
    if let Some(name) = wanted {
        if local::names_this_machine(config, name) {
            let chosen =
                local::candidate(config, jobs, state, selection.wants_disk(), Source::Local)?;
            return needs_met(selection, chosen);
        }
        let host = config.host(name)?;
        let (found, probe) =
            probe_one(config, host, state, lookup, prober, selection.wants_disk())?;
        // A pinned host is used as is, but not when it cannot meet the needs.
        let a = selection.assess(host, &probe);
        if !a.qualifies() {
            return Err(Error::NeedsUnmet(vec![format!(
                "{}: lacks {}",
                host.name,
                a.lacks.join("; lacks ")
            )]));
        }
        return Ok((host.clone(), found, probe));
    }
    if config.hosts.is_empty() && !config.local_fallback() && !config.local_in_pool() {
        return Err(Error::Usage(
            "no hosts configured; add one with `goway host add NAME` (or run here with `goway run --host local`)".to_owned(),
        ));
    }
    let local_host = local::host(config);
    let mut results = probe_all(config, state, lookup, prober, selection.wants_disk());
    if config.local_in_pool() {
        push_local(config, &local_host, jobs, state, selection, &mut results);
    }
    match pick_for(config, selection, &results) {
        Some(i) => {
            let chosen = results.swap_remove(i);
            let (found, probe) = chosen.result?;
            Ok((chosen.host.clone(), found, probe))
        }
        None if config.local_fallback() && results.iter().all(|p| p.result.is_err()) => {
            tracing::warn!("no helper is reachable; falling back to this machine");
            let chosen = local::candidate(
                config,
                jobs,
                state,
                selection.wants_disk(),
                Source::Fallback,
            )?;
            needs_met(selection, chosen)
        }
        None => Err(none_usable(selection, &results)),
    }
}

/// `chosen` unless it lacks a need (a pinned or fallback host is still held to the needs).
fn needs_met(
    selection: &Selection,
    chosen: (HostConfig, Found, Probe),
) -> Result<(HostConfig, Found, Probe)> {
    let a = selection.assess(&chosen.0, &chosen.2);
    if a.qualifies() {
        Ok(chosen)
    } else {
        Err(Error::NeedsUnmet(vec![format!(
            "{}: lacks {}",
            chosen.0.name,
            a.lacks.join("; lacks ")
        )]))
    }
}

/// Add this machine (`[local] pool = true`) to the candidates.
fn push_local<'a>(
    config: &Config,
    host: &'a HostConfig,
    jobs: &Path,
    state: &mut State,
    selection: &Selection,
    results: &mut Vec<Probed<'a>>,
) {
    let result = local::candidate(config, jobs, state, selection.wants_disk(), Source::Local)
        .map(|(_, found, probe)| (found, probe));
    if let Err(e) = &result {
        tracing::warn!(error = %e, "this machine cannot be probed; not in the pool");
    }
    results.push(Probed { host, result });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh::{Failure, Target};

    fn probe(cores: u32, load1: f64, jobs: u32) -> Probe {
        Probe {
            arch: "x86_64".to_owned(),
            hostname: "h".to_owned(),
            cores,
            load: [load1, 0.0, 0.0],
            jobs,
            disk_used: None,
            disk_free: None,
            facts: Facts::default(),
        }
    }

    fn host(name: &str, max_jobs: Option<u32>) -> HostConfig {
        HostConfig {
            name: name.to_owned(),
            max_jobs,
            ..HostConfig::default()
        }
    }

    fn found(name: &str) -> Found {
        Found {
            target: Target {
                name: name.to_owned(),
                address: "10.0.0.1".to_owned(),
                port: 2222,
                user: None,
                identity: None,
            },
            source: resolve::Source::Cached,
            output: String::new(),
        }
    }

    #[test]
    fn parses_probe_output() {
        let p = parse_probe(
            "arch=x86_64\nhostname=Helios\ncores=12\nload1=0.50\nload5=0.2\nload15=0.1\njobs=2\ndisk_used=100\n",
        )
        .unwrap();
        assert_eq!(
            (p.cores, p.jobs, p.disk_used, p.disk_free),
            (12, 2, Some(100), None)
        );
        assert!((score(&p, 0.5) - 2.5 / 12.0).abs() < 1e-9);
        assert!(parse_probe("arch=x\n").is_none());
        for bad in ["load1=-inf", "load1=NaN", "load1=-1", "cores=0", "cores=-3"] {
            let text = format!(
                "arch=x86_64\nhostname=h\ncores=4\nload1=1\nload5=0\nload15=0\njobs=0\n{bad}\n"
            );
            assert!(parse_probe(&text).is_none(), "{bad}");
        }
        let p = parse_probe(
            "arch=x86_64\nhostname=ev\u{1b}[2Jil\ncores=4\nload1=1\nload5=0\nload15=0\njobs=0\n",
        )
        .unwrap();
        assert_eq!(p.hostname, "ev?[2Jil");
    }

    #[test]
    fn picks_lowest_relative_load_among_usable_hosts() {
        let hosts = [
            host("busy", None),
            host("big", None),
            host("full", Some(1)),
            host("down", None),
            host("small", None),
        ];
        let probed = vec![
            Probed {
                host: &hosts[0],
                result: Ok((found("busy"), probe(12, 11.0, 0))),
            },
            Probed {
                host: &hosts[1],
                result: Ok((found("big"), probe(16, 2.0, 2))),
            },
            Probed {
                host: &hosts[2],
                result: Ok((found("full"), probe(64, 0.0, 1))),
            },
            Probed {
                host: &hosts[3],
                result: Err(Error::Usage("down".to_owned())),
            },
            Probed {
                host: &hosts[4],
                result: Ok((found("small"), probe(4, 1.0, 0))),
            },
        ];
        // big: 4/16 = 0.25, small: 1/4 = 0.25 -> fewer jobs wins (small).
        assert_eq!(pick(&Config::default(), &probed), Some(4));
        assert_eq!(pick(&Config::default(), &probed[..2]), Some(1));
        assert_eq!(
            pick(&Config::default(), &probed[2..4]),
            None,
            "full and down hosts are never picked"
        );
    }

    // frob:tests crates/goway/src/pool.rs::mem_penalty
    #[test]
    fn a_host_short_of_memory_scores_worse_but_stays_usable() {
        let gib = 1024u64 * 1024 * 1024;
        let mut tight = probe(16, 0.0, 0);
        tight.facts.mem_avail = Some(3 * gib);
        let mut roomy = probe(12, 0.0, 0);
        roomy.facts.mem_avail = Some(7 * gib);
        // 3 GiB over 16 cores is 0.19 GiB/core against a 0.5 target.
        assert!(mem_penalty(&tight, 0.5) > 0.6);
        assert!(mem_penalty(&roomy, 0.5).abs() < 1e-12);
        assert!(score(&tight, 0.5) > score(&roomy, 0.5));
        // Switched off, the bigger idle host ties and wins nothing extra.
        assert!((score(&tight, 0.0) - score(&roomy, 0.0)).abs() < 1e-12);
        // Unknown RAM is never penalised.
        assert!(mem_penalty(&probe(16, 0.0, 0), 0.5).abs() < 1e-12);
        let hosts = [host("tight", None), host("roomy", None)];
        let probed = vec![
            Probed {
                host: &hosts[0],
                result: Ok((found("tight"), tight)),
            },
            Probed {
                host: &hosts[1],
                result: Ok((found("roomy"), roomy)),
            },
        ];
        assert_eq!(pick(&Config::default(), &probed), Some(1));
        assert_eq!(pick(&Config::default(), &probed[..1]), Some(0));
    }

    #[test]
    fn hosts_above_max_load_are_skipped_unless_pinned() {
        let mut hosts = [host("hot", None), host("cool", None)];
        hosts[0].max_load = Some(0.5);
        let probed = vec![
            Probed {
                host: &hosts[0],
                result: Ok((found("hot"), probe(64, 40.0, 0))),
            },
            Probed {
                host: &hosts[1],
                result: Ok((found("cool"), probe(2, 1.8, 0))),
            },
        ];
        let config = Config::default();
        // hot: 40/64 = 0.625 per core > 0.5 -> skipped even though its score is lower.
        assert_eq!(pick(&config, &probed), Some(1));
        let mut ceiling = Config::default();
        ceiling.defaults.max_load = Some(0.5);
        let only_cool = [Probed {
            host: &hosts[1],
            result: Ok((found("cool"), probe(2, 1.8, 0))),
        }];
        assert_eq!(
            pick(&ceiling, &only_cool),
            None,
            "defaults.max_load applies too"
        );
    }

    #[test]
    fn pinned_host_ignores_max_load() {
        let mut config = Config::default();
        let mut h = host("a", None);
        h.address = Some("10.0.0.1".to_owned());
        h.max_load = Some(0.0);
        config.hosts.push(h);
        let mut state = State::default();
        assert!(
            choose(
                &config,
                &Selection::default(),
                &mut state,
                Path::new(""),
                &NoLookup,
                &ByAddress,
                None
            )
            .is_err()
        );
        assert!(
            choose(
                &config,
                &Selection::default(),
                &mut state,
                Path::new(""),
                &NoLookup,
                &ByAddress,
                Some("a")
            )
            .is_ok()
        );
    }

    struct ByAddress;
    impl Prober for ByAddress {
        fn probe(&self, target: &Target, _: KeyPolicy, _: &str) -> resolve::ProbeResult {
            match target.address.as_str() {
                "10.0.0.1" => Ok(
                    "arch=x86_64\nhostname=a\ncores=4\nload1=3\nload5=0\nload15=0\njobs=0\n"
                        .to_owned(),
                ),
                "10.0.0.2" => Ok(
                    "arch=x86_64\nhostname=b\ncores=16\nload1=1\nload5=0\nload15=0\njobs=1\n"
                        .to_owned(),
                ),
                _ => Err((Failure::Unreachable, String::new())),
            }
        }
    }
    struct NoLookup;
    impl Lookup for NoLookup {
        fn system(&self, _: &str) -> Vec<std::net::IpAddr> {
            Vec::new()
        }
        fn windows(&self, _: &str) -> Vec<std::net::IpAddr> {
            Vec::new()
        }
    }

    // frob:tests crates/goway/src/pool.rs::probe_all
    // frob:tests crates/goway/src/pool.rs::probe_one
    // frob:tests crates/goway/src/pool.rs::probe_command
    #[test]
    fn choose_probes_in_parallel_and_caches_addresses() {
        let mut config = Config::default();
        for (name, addr) in [("a", "10.0.0.1"), ("b", "10.0.0.2"), ("c", "10.0.0.3")] {
            let mut h = host(name, None);
            h.address = Some(addr.to_owned());
            config.hosts.push(h);
        }
        let mut state = State::default();
        let (h, found, probe) = choose(
            &config,
            &Selection::default(),
            &mut state,
            Path::new(""),
            &NoLookup,
            &ByAddress,
            None,
        )
        .unwrap();
        assert_eq!(h.name, "b", "2/16 beats 3/4");
        assert_eq!(found.target.address, "10.0.0.2");
        assert_eq!(probe.hostname, "b");
        assert_eq!(state.get("a").unwrap().address, "10.0.0.1");
        assert!(state.get("c").is_none());

        let (h, ..) = choose(
            &config,
            &Selection::default(),
            &mut state,
            Path::new(""),
            &NoLookup,
            &ByAddress,
            Some("a"),
        )
        .unwrap();
        assert_eq!(h.name, "a", "--host pins");
        assert!(
            choose(
                &config,
                &Selection::default(),
                &mut state,
                Path::new(""),
                &NoLookup,
                &ByAddress,
                Some("c")
            )
            .is_err()
        );
        assert!(
            choose(
                &Config::default(),
                &Selection::default(),
                &mut state,
                Path::new(""),
                &NoLookup,
                &ByAddress,
                None
            )
            .is_err()
        );
    }

    /// Answers every address like a different machine: only `10.0.0.2` has a GPU.
    struct GpuProber;
    impl Prober for GpuProber {
        fn probe(&self, target: &Target, _: KeyPolicy, _: &str) -> resolve::ProbeResult {
            let gpu = "static=1\ngpu.0=nvidia|RTX 4090|24576|555.1|12.5\ncpu_flags=avx2\nkvm=1\n";
            let plain = "static=1\ncpu_flags=\nkvm=0\n";
            let (cores, load, extra) = match target.address.as_str() {
                "10.0.0.1" => (16, 0.0, plain),
                "10.0.0.2" => (8, 4.0, gpu),
                "10.0.0.3" => (8, 0.0, plain),
                _ => return Err((Failure::Unreachable, String::new())),
            };
            Ok(format!(
                "arch=x86_64\nhostname={}\ncores={cores}\nload1={load}\nload5=0\nload15=0\njobs=0\nos=linux\nmem_total=34359738368\nmem_avail=34359738368\n{extra}",
                target.address
            ))
        }
    }

    fn three_hosts() -> Config {
        let mut config = Config::default();
        for (name, addr) in [
            ("idle", "10.0.0.1"),
            ("gpu", "10.0.0.2"),
            ("small", "10.0.0.3"),
        ] {
            let mut h = host(name, None);
            h.address = Some(addr.to_owned());
            if name == "gpu" {
                h.labels = vec!["gpu-box".to_owned()];
            }
            config.hosts.push(h);
        }
        config
    }

    fn selection(needs: &str, prefers: &str) -> Selection {
        let split = |s: &str| {
            if s.is_empty() {
                Vec::new()
            } else {
                vec![s.to_owned()]
            }
        };
        Selection::parse(&split(needs), &split(prefers)).unwrap()
    }

    // frob:tests crates/goway/src/pool.rs::choose
    // frob:tests crates/goway/src/pool.rs::ranked_for
    #[test]
    fn needs_pick_only_qualifying_hosts_even_when_busier() {
        let config = three_hosts();
        let mut state = State::default();
        // Without needs the idle host wins; with `gpu` only the busy gpu host qualifies.
        let (h, ..) = choose(
            &config,
            &Selection::default(),
            &mut state,
            Path::new(""),
            &NoLookup,
            &GpuProber,
            None,
        )
        .unwrap();
        assert_eq!(h.name, "idle");
        let (h, _, probe) = choose(
            &config,
            &selection("gpu,gpu-mem>=8G,label=gpu-box", ""),
            &mut state,
            Path::new(""),
            &NoLookup,
            &GpuProber,
            None,
        )
        .unwrap();
        assert_eq!(h.name, "gpu");
        assert_eq!(probe.facts.gpus()[0].name, "RTX 4090");
        // Nothing qualifies: exit 125 listing every host and what it lacks.
        let err = choose(
            &config,
            &selection("gpu-mem>=48G", ""),
            &mut state,
            Path::new(""),
            &NoLookup,
            &GpuProber,
            None,
        )
        .unwrap_err();
        assert_eq!(err.exit_code(), 125);
        let text = err.to_string();
        assert!(matches!(err, Error::NeedsUnmet(_)), "{text}");
        for name in ["idle", "gpu", "small"] {
            assert!(
                text.contains(&format!("{name}: lacks gpu-mem>=48G")),
                "{text}"
            );
        }
        assert!(text.contains("largest GPU has 24.0 GiB"), "{text}");
        // A pinned host that lacks a need is refused too.
        let err = choose(
            &config,
            &selection("kvm", ""),
            &mut state,
            Path::new(""),
            &NoLookup,
            &GpuProber,
            Some("idle"),
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("idle: lacks kvm: no usable /dev/kvm"),
            "{err}"
        );
        assert!(
            choose(
                &config,
                &selection("kvm", ""),
                &mut state,
                Path::new(""),
                &NoLookup,
                &GpuProber,
                Some("gpu")
            )
            .is_ok()
        );
    }

    // frob:tests crates/goway/src/pool.rs::ranked_for
    #[test]
    fn preferences_reorder_but_never_exclude() {
        let config = three_hosts();
        let mut state = State::default();
        // Load per core: idle 0.0, small 0.0, gpu 0.5. Preferring a gpu (bonus 0.5) ties it with the idle ones; kvm too beats them.
        let (h, ..) = choose(
            &config,
            &selection("", "gpu,kvm"),
            &mut state,
            Path::new(""),
            &NoLookup,
            &GpuProber,
            None,
        )
        .unwrap();
        assert_eq!(h.name, "gpu");
        // A preference nobody meets changes nothing and excludes nobody.
        let (h, ..) = choose(
            &config,
            &selection("", "gpu=rocm"),
            &mut state,
            Path::new(""),
            &NoLookup,
            &GpuProber,
            None,
        )
        .unwrap();
        assert_eq!(h.name, "idle");
        let hosts = choose_many(
            &config,
            &selection("", "gpu=rocm"),
            &mut state,
            Path::new(""),
            &NoLookup,
            &GpuProber,
            3,
        )
        .unwrap();
        assert_eq!(hosts.len(), 3);
    }

    // frob:tests crates/goway/src/pool.rs::choose_many
    #[test]
    fn every_shard_host_meets_the_needs() {
        let config = three_hosts();
        let mut state = State::default();
        let hosts = choose_many(
            &config,
            &selection("cores>=8,os=linux", ""),
            &mut state,
            Path::new(""),
            &NoLookup,
            &GpuProber,
            3,
        )
        .unwrap();
        assert_eq!(hosts.len(), 3);
        // Only one host has a GPU, so two shards cannot be placed.
        let err = choose_many(
            &config,
            &selection("gpu", ""),
            &mut state,
            Path::new(""),
            &NoLookup,
            &GpuProber,
            2,
        )
        .unwrap_err();
        assert!(matches!(err, Error::NeedsUnmet(_)));
        assert!(
            err.to_string()
                .contains("2 shards need 2 usable hosts that each meet --needs, 1 are usable"),
            "{err}"
        );
        let one = choose_many(
            &config,
            &selection("gpu", ""),
            &mut state,
            Path::new(""),
            &NoLookup,
            &GpuProber,
            1,
        )
        .unwrap();
        assert_eq!(one[0].0.name, "gpu");
    }
    // frob:tests crates/goway/src/pool.rs::ranked_for
    #[test]
    fn this_machine_competes_with_a_margin_and_its_max_jobs() {
        let mut config = Config::default();
        config.local = Some(crate::config::Local {
            pool: true,
            margin: 0.5,
            ..crate::config::Local::default()
        });
        let remote = host("helper", None);
        let local_host = local::host(&config);
        let probed = |remote_load: f64, local_jobs: u32| {
            vec![
                (remote.clone(), found("helper"), probe(4, remote_load, 0)),
                (
                    local_host.clone(),
                    local::found(Source::Local),
                    probe(4, 0.0, local_jobs),
                ),
            ]
        };
        let pick_of = |cands: &[(HostConfig, Found, Probe)]| {
            let p: Vec<Probed<'_>> = cands
                .iter()
                .map(|(h, f, pr)| Probed {
                    host: h,
                    result: Ok((f.clone(), pr.clone())),
                })
                .collect();
            pick(&config, &p).map(|i| p[i].host.name.clone())
        };
        // The helper is a little busier (0.25 per core): the margin keeps the run there.
        assert_eq!(pick_of(&probed(1.0, 0)).as_deref(), Some("helper"));
        // Much busier (1.5 per core): this machine wins.
        assert_eq!(pick_of(&probed(6.0, 0)).as_deref(), Some("local"));
        // At [local] max_jobs (default 1) it is skipped however idle it is.
        assert_eq!(pick_of(&probed(6.0, 1)).as_deref(), Some("helper"));
    }
}
