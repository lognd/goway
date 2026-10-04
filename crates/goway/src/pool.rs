//! The host pool: probe every host in parallel (one ssh call each, which
//! also resolves its address) and pick the least loaded.
//!
//! Score = (1-minute load + goway jobs running) / cores. goway's own jobs
//! are counted on top of the load average because a job that just started
//! has not shown up in the load yet. Hosts at their job limit (`max_jobs`, default every core) are skipped.
//!
//! A host short of memory scores worse: when its available RAM per core is
//! below `mem_per_core` GiB (default 0.5), up to 1.0 is added in proportion
//! to the shortfall, so a 16-core host with 3 GiB loses to a roomier one but
//! is still used when nothing else is.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::config::{Config, HostConfig};
use crate::error::{Error, Result};
use crate::facts::{self, Facts};
use crate::local;
use crate::needs::Selection;
use crate::queue::{Claim, Queue, Snapshot, Ticket};
use crate::remote::Call;
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
    /// The host's disk budget for goway in bytes (status only).
    pub disk_max: Option<u64>,
    /// Peak disk footprint of each repository built there, by repository id (see [`crate::footprint`]).
    pub footprints: BTreeMap<String, u64>,
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
        disk_max: kv.get("disk_max").and_then(|v| v.parse().ok()),
        footprints: crate::footprint::parse(&kv),
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

/// What a helper's owner is doing there, as far as the probe could tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerUse {
    /// Nothing is known (no interop, no tool, a probe that timed out): never blocks, never penalises.
    Unknown,
    /// On mains power and nobody touched it within the idle window.
    Idle,
    /// Running on battery.
    Battery,
    /// Somebody used the keyboard or mouse this many seconds ago.
    Active(u64),
}

impl OwnerUse {
    /// Whether the owner is using the host, so goway should be extra polite.
    pub fn in_use(self) -> bool {
        matches!(self, Self::Battery | Self::Active(_))
    }

    /// One short phrase for notes and status: `-` when unknown.
    pub fn summary(self) -> String {
        match self {
            Self::Unknown => "-".to_owned(),
            Self::Idle => "idle".to_owned(),
            Self::Battery => "on battery".to_owned(),
            Self::Active(s) if s < 90 => "in use".to_owned(),
            Self::Active(s) => format!("in use ({}m ago)", s / 60),
        }
    }
}

/// How much a host in use scores worse (a quarter of a core of load per core), so an
/// idle host on mains wins when the choice is otherwise close; it never excludes.
pub const OWNER_PENALTY: f64 = 0.25;

/// What the owner of `p` is doing, judged against the idle `window` (zero
/// switches owner awareness off: everything is unknown). Battery wins over idle.
pub fn owner_use(p: &Probe, window: std::time::Duration) -> OwnerUse {
    if window.is_zero() {
        return OwnerUse::Unknown;
    }
    if p.facts.on_battery == Some(true) {
        return OwnerUse::Battery;
    }
    match p.facts.idle_secs {
        Some(s) if s < window.as_secs() => OwnerUse::Active(s),
        Some(_) => OwnerUse::Idle,
        None if p.facts.on_battery == Some(false) => OwnerUse::Idle,
        None => OwnerUse::Unknown,
    }
}

/// The priority word a run on `host` sends: `owner` (nice 19, half the cores
/// for builds) while its owner uses a helper, else the configured one. This
/// machine and Windows hosts keep the configured priority.
pub fn priority_word(
    config: &Config,
    host: &HostConfig,
    found: &Found,
    probe: &Probe,
) -> &'static str {
    let configured = config.priority_of(host).as_str();
    if found.is_local() || found.kind != crate::transport::Kind::Unix {
        return configured;
    }
    if owner_use(probe, config.defaults.owner_idle).in_use() {
        "owner"
    } else {
        configured
    }
}

/// The one line saying a run goes extra nicely, when it does.
pub fn owner_note(host: &str, probe: &Probe, word: &str) -> Option<String> {
    (word == "owner").then(|| {
        format!(
            "{host} is being used ({}): running extra nicely (nice 19, idle I/O, at most {} build jobs)",
            owner_use(probe, std::time::Duration::MAX).summary(),
            (probe.cores / 2).max(1)
        )
    })
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
            let limit = p.host.job_limit(probe.cores);
            if probe.jobs >= limit {
                tracing::info!(host = %p.host.name, jobs = probe.jobs, limit, "host at its job limit; skipped");
                return None;
            }
            let reserve = config.job_mem_of(p.host);
            if reserve > 0 && probe.facts.mem_avail.is_some_and(|a| a < reserve) {
                tracing::info!(host = %p.host.name, avail = ?probe.facts.mem_avail, reserve, "host has less free memory than one job reserves; skipped");
                return None;
            }
            if let Some(why) = room_shortage(selection, probe) {
                tracing::info!(host = %p.host.name, %why, "host has no disk room for this repository; skipped");
                return None;
            }
            let per_core = probe.load[0] / f64::from(probe.cores.max(1));
            if config.max_load_of(p.host).is_some_and(|m| per_core > m) {
                tracing::info!(host = %p.host.name, per_core, "host above max_load; skipped");
                return None;
            }
            if let Some(os) = selection.outside_pool(probe) {
                tracing::info!(host = %p.host.name, os, "host is outside the default pool OS; skipped");
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
            // A helper its owner is using is never skipped for it, only less attractive.
            let in_use = !matches!(&p.result, Ok((found, _)) if found.is_local())
                && owner_use(probe, config.defaults.owner_idle).in_use();
            let owner = if in_use { OWNER_PENALTY } else { 0.0 };
            Some((
                i,
                score(probe, config.defaults.mem_per_core) - bonus + margin + owner,
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
                if let Some(os) = selection.outside_pool(probe) {
                    format!(
                        "{}: {os} host, outside the default {} pool (pin it with --host, or ask with --needs os={os})",
                        p.host.name,
                        selection.pool_os.as_deref().unwrap_or("?")
                    )
                } else if a.qualifies() {
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

/// One usable host per OS family the reachable hosts report (`--each-os`), in OS-name order,
/// each the best of its family for `selection`.
///
/// # Errors
///
/// [`Error::NoHost`] / [`Error::NeedsUnmet`] when no host is usable.
///
/// # Panics
///
/// Never: each ranked index is taken once.
pub fn choose_each_os(
    config: &Config,
    selection: &Selection,
    state: &mut State,
    jobs: &Path,
    lookup: &(dyn Lookup + Sync),
    prober: &(dyn Prober + Sync),
) -> Result<Vec<(HostConfig, Found, Probe)>> {
    let local_host = local::host(config);
    let mut results = probe_all(config, state, lookup, prober, selection.wants_disk());
    if config.local_in_pool() {
        push_local(config, &local_host, jobs, state, selection, &mut results);
    }
    let order = each_os_order(config, selection, &results);
    if order.is_empty() {
        return Err(none_usable(selection, &results));
    }
    let mut slots: Vec<Option<Probed<'_>>> = results.into_iter().map(Some).collect();
    order
        .into_iter()
        .map(|i| {
            let p = slots[i].take().expect("each index once");
            let (found, probe) = p.result?;
            Ok((p.host.clone(), found, probe))
        })
        .collect()
}

/// The index of the best usable host of each reported OS, in OS-name order.
fn each_os_order(config: &Config, selection: &Selection, probed: &[Probed<'_>]) -> Vec<usize> {
    let oses: std::collections::BTreeSet<String> = probed
        .iter()
        .filter_map(|p| p.result.as_ref().ok()?.1.facts.os.clone())
        .map(|o| o.to_ascii_lowercase())
        .collect();
    oses.iter()
        .filter_map(|os| {
            let mut one = selection.clone();
            one.pool_os = Some(os.clone());
            let best = ranked_for(config, &one, probed).first().copied();
            tracing::info!(os, ?best, "best host for this OS");
            best
        })
        .collect()
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

/// The remote command line that probes a Unix host (also this machine).
pub fn probe_command(config: &Config, disk: bool, statics: bool) -> String {
    probe_call(config, disk, statics, &[]).bash()
}

/// The call that probes a host, whatever it speaks.
///
/// A non-empty `tools` adds the word `tools:A,B`: the host also reports the
/// versions of those tools (names with other characters are left out).
pub fn probe_call(config: &Config, disk: bool, statics: bool, tools: &[String]) -> Call {
    let root = config.defaults.remote_root.as_str();
    let mut args = vec![root];
    let budget;
    if disk {
        args.push("disk");
        let (max_disk, min_free, _) = config.defaults.budget_bytes();
        budget = format!("budget:{max_disk}:{min_free}");
        args.push(&budget);
    }
    if statics {
        args.push("static");
    }
    if !config.defaults.owner_idle.is_zero() {
        args.push("owner");
    }
    let wanted = tools
        .iter()
        .filter(|t| {
            !t.is_empty()
                && t.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._+-".contains(c))
        })
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(",");
    let wanted = format!("tools:{wanted}");
    if wanted.len() > "tools:".len() {
        args.push(&wanted);
    }
    Call::new("probe", &args)
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
    let call = probe_call(config, disk, statics, state.tools_to_probe(&host.name, now));
    let (found, sent, rtt) = crate::facts::clock::timed(|| {
        resolve::resolve_call(
            config,
            host,
            state,
            lookup,
            prober,
            KeyPolicy::Strict,
            &call,
        )
    });
    let found = found?;
    let mut probe = complete_probe(&host.name, &found.output, state, now)?;
    // frob:ticket 01M42TD5V6H043JYBGK591BBA2
    probe.facts.clock_offset_ms = crate::facts::clock::measure(&found.output, sent, rtt);
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
    let mut results = probe_pool(config, &local_host, selection, state, jobs, lookup, prober);
    match pick_for(config, selection, &results) {
        Some(i) => {
            let chosen = results.swap_remove(i);
            let (found, probe) = chosen.result?;
            Ok((chosen.host.clone(), found, probe))
        }
        None => no_pick(config, selection, state, jobs, &results),
    }
}

/// Probe every helper (and this machine when it is pooled).
fn probe_pool<'a>(
    config: &'a Config,
    local_host: &'a HostConfig,
    selection: &Selection,
    state: &mut State,
    jobs: &Path,
    lookup: &(dyn Lookup + Sync),
    prober: &(dyn Prober + Sync),
) -> Vec<Probed<'a>> {
    let mut results = probe_all(config, state, lookup, prober, selection.wants_disk());
    if config.local_in_pool() {
        push_local(config, local_host, jobs, state, selection, &mut results);
    }
    results
}

/// What to do when no host can take the run: fall back to this machine when
/// no helper answers at all and `[local] fallback` is on, else the error.
fn no_pick(
    config: &Config,
    selection: &Selection,
    state: &mut State,
    jobs: &Path,
    results: &[Probed<'_>],
) -> Result<(HostConfig, Found, Probe)> {
    if config.local_fallback() && results.iter().all(|p| p.result.is_err()) {
        tracing::warn!("no helper is reachable; falling back to this machine");
        let chosen = local::candidate(
            config,
            jobs,
            state,
            selection.wants_disk(),
            Source::Fallback,
        )?;
        return needs_met(selection, chosen);
    }
    Err(none_usable(selection, results))
}

/// How a run waits for a host that has no room yet (see [`crate::queue`]).
pub struct Wait<'a> {
    /// The local queue of runs.
    pub queue: &'a Queue,
    /// The longest to wait for a host to qualify; zero fails at once.
    pub limit: Duration,
    /// How often a waiter near the front probes the hosts again.
    pub poll: Duration,
    /// Where progress notes go (one line each).
    pub note: &'a dyn Fn(&str),
}

/// The default `--wait`: five minutes.
pub const DEFAULT_WAIT: Duration = Duration::from_mins(5);

/// The default time between probe rounds of a waiting run.
pub const DEFAULT_POLL: Duration = Duration::from_secs(5);

/// Only this many waiters at the front of the queue probe the hosts, so the
/// probe rate stays bounded however large the wave is.
pub const PROBING_WAITERS: usize = 3;

/// How often a waiter looks at the local queue (no ssh involved).
const LOCAL_POLL: Duration = Duration::from_millis(250);

/// Count the claims not yet visible in a probe as jobs, memory and disk spoken for.
fn apply_pending(config: &Config, snap: &Snapshot, results: &mut [Probed<'_>]) {
    for p in results {
        let reserve = config.job_mem_of(p.host);
        let key = p.host.name.to_ascii_lowercase();
        let Some(n) = snap.pending.get(&key).copied() else {
            continue;
        };
        if let Ok((_, probe)) = &mut p.result {
            probe.jobs += n;
            if let Some(avail) = probe.facts.mem_avail.as_mut() {
                *avail = avail.saturating_sub(u64::from(n) * reserve);
            }
            // Disk the same way: each pending run of a repository will grow by its footprint.
            let claimed: u64 = snap
                .pending_repos
                .get(&key)
                .into_iter()
                .flatten()
                .filter_map(|id| probe.footprints.get(id))
                .sum();
            if let Some(free) = probe.disk_free.as_mut() {
                *free = free.saturating_sub(claimed);
            }
        }
    }
}

/// Why `probe`'s host has no disk room for the run's repository, when it lacks it
/// (free plus evictable space below the repository's footprint plus margin).
fn room_shortage(selection: &Selection, probe: &Probe) -> Option<String> {
    let id = selection.repo_id.as_deref()?;
    match crate::footprint::assess(probe, id) {
        crate::footprint::Room::Short {
            free,
            evictable,
            need,
        } => Some(crate::footprint::short_text(free, evictable, need)),
        _ => None,
    }
}

/// The hosts that could ever take the run: reachable, in the pool, meeting every need.
fn eligible_hosts(selection: &Selection, results: &[Probed<'_>]) -> Vec<String> {
    results
        .iter()
        .filter(|p| {
            p.result.as_ref().is_ok_and(|(_, probe)| {
                selection.outside_pool(probe).is_none()
                    && selection.assess(p.host, probe).qualifies()
            })
        })
        .map(|p| p.host.name.clone())
        .collect()
}

/// One line per eligible host saying what it lacks room for.
fn busy_lines(
    config: &Config,
    selection: &Selection,
    results: &[Probed<'_>],
    snap: &Snapshot,
) -> Vec<String> {
    let eligible = eligible_hosts(selection, results);
    results
        .iter()
        .filter(|p| eligible.contains(&p.host.name))
        .filter_map(|p| {
            let (_, probe) = p.result.as_ref().ok()?;
            let limit = p.host.job_limit(probe.cores);
            let name = &p.host.name;
            let reserve = config.job_mem_of(p.host);
            Some(if probe.jobs >= limit {
                format!("{name}: {} of {limit} job slots in use", probe.jobs)
            } else if reserve > 0 && probe.facts.mem_avail.is_some_and(|a| a < reserve) {
                format!(
                    "{name}: {:.1} GiB free, {:.1} GiB needed per job",
                    gib(probe.facts.mem_avail.unwrap_or(0)),
                    gib(reserve)
                )
            } else if let Some(why) = room_shortage(selection, probe) {
                format!("{name}: {why}")
            } else if snap.held_for_earlier(name) {
                format!("{name}: reserved for runs ahead in the queue")
            } else {
                format!("{name}: busy")
            })
        })
        .collect()
}

#[allow(clippy::cast_precision_loss)] // display only
fn gib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0 * 1024.0)
}

/// One `host: why` per eligible host that has no disk room for the run's repository.
fn skipped_for_room(selection: &Selection, results: &[Probed<'_>]) -> Vec<String> {
    results
        .iter()
        .filter_map(|p| {
            let (_, probe) = p.result.as_ref().ok()?;
            let why = room_shortage(selection, probe)?;
            Some(format!("{}: {why}", p.host.name))
        })
        .collect()
}

/// What one probe round decided.
enum Decision {
    /// Run on this index of the results, holding this claim.
    Taken(usize, Claim),
    /// Nothing qualifies, ever (unreachable or failing a need).
    Hopeless,
    /// Some host qualifies but has no room now.
    NoRoom(Vec<String>),
}

/// Judge one probe round under the queue's decision lock, so that reading
/// the queue and claiming a host are one step across processes.
fn decide_round(
    config: &Config,
    selection: &Selection,
    wait: &Wait<'_>,
    ticket: Option<&Ticket>,
    results: &mut [Probed<'_>],
) -> Result<Decision> {
    wait.queue.decide(|| {
        let snap = wait.queue.snapshot(ticket);
        apply_pending(config, &snap, results);
        let eligible = eligible_hosts(selection, results);
        if eligible.is_empty() {
            return Ok(Decision::Hopeless);
        }
        if let Some(t) = ticket {
            t.set_eligible(&eligible)?;
        }
        let pick = ranked_for(config, selection, results)
            .into_iter()
            .find(|&i| !snap.held_for_earlier(&results[i].host.name));
        match pick {
            Some(i) => {
                let short = skipped_for_room(selection, results);
                if !short.is_empty() {
                    (wait.note)(&format!(
                        "skipped for disk room: {}; using {}",
                        short.join("; "),
                        results[i].host.name
                    ));
                }
                let claim = wait
                    .queue
                    .claim(&results[i].host.name, selection.repo_id.as_deref())?;
                tracing::info!(host = %results[i].host.name, position = snap.position(), "picked");
                Ok(Decision::Taken(i, claim))
            }
            None => Ok(Decision::NoRoom(busy_lines(
                config, selection, results, &snap,
            ))),
        }
    })?
}

/// [`choose`] for a run that may have to wait: with a free host it claims it
/// at once; otherwise it joins the local first-come-first-served queue and
/// re-probes (only while among the first [`PROBING_WAITERS`]) until a host
/// has a job slot and memory for one more job, or `wait.limit` runs out.
///
/// # Errors
///
/// [`Error::NoHost`] when the wait ran out (naming how long and what for),
/// the errors of [`choose`], and [`Error::Io`] for queue files.
#[allow(clippy::too_many_arguments)] // the context of `choose`, plus how to wait
pub fn choose_queued(
    config: &Config,
    selection: &Selection,
    state: &mut State,
    jobs: &Path,
    lookup: &(dyn Lookup + Sync),
    prober: &(dyn Prober + Sync),
    wanted: Option<&str>,
    wait: &Wait<'_>,
) -> Result<(HostConfig, Found, Probe, Option<Claim>)> {
    if wanted.is_some() {
        let (host, found, probe) = choose(config, selection, state, jobs, lookup, prober, wanted)?;
        let claim = wait.queue.claim(&host.name, selection.repo_id.as_deref())?;
        return Ok((host, found, probe, Some(claim)));
    }
    if config.hosts.is_empty() && !config.local_fallback() && !config.local_in_pool() {
        // The one error `choose` words for this.
        return choose(config, selection, state, jobs, lookup, prober, None)
            .map(|(h, f, p)| (h, f, p, None));
    }
    let local_host = local::host(config);
    let ticket = (!wait.limit.is_zero())
        .then(|| wait.queue.enter())
        .transpose()?;
    let began = Instant::now();
    let mut last_probe: Option<Instant> = None;
    let mut last_pos = usize::MAX;
    let mut noted_pos = usize::MAX;
    let mut lines: Vec<String> = Vec::new();
    loop {
        let pos = wait.queue.snapshot(ticket.as_ref()).position();
        let due = last_probe.is_none_or(|t| t.elapsed() >= wait.poll) || pos < last_pos;
        last_pos = pos;
        if ticket.is_none() || pos < PROBING_WAITERS {
            if due {
                let mut results =
                    probe_pool(config, &local_host, selection, state, jobs, lookup, prober);
                last_probe = Some(Instant::now());
                match decide_round(config, selection, wait, ticket.as_ref(), &mut results)? {
                    Decision::Taken(i, claim) => {
                        let chosen = results.swap_remove(i);
                        let (found, probe) = chosen.result?;
                        return Ok((chosen.host.clone(), found, probe, Some(claim)));
                    }
                    Decision::Hopeless => {
                        let (h, f, p) = no_pick(config, selection, state, jobs, &results)?;
                        return Ok((h, f, p, None));
                    }
                    Decision::NoRoom(why) => {
                        if wait.limit.is_zero() {
                            return Err(none_usable(selection, &results));
                        }
                        let changed = why != lines;
                        lines.clone_from(&why);
                        if pos != noted_pos || changed {
                            noted_pos = pos;
                            (wait.note)(&format!(
                                "no host has room yet ({}); queued at position {}, waiting up to {}",
                                why.join("; "),
                                pos + 1,
                                humantime::format_duration(Duration::from_secs(
                                    wait.limit.as_secs()
                                ))
                            ));
                        }
                    }
                }
            }
        } else if pos != noted_pos {
            noted_pos = pos;
            (wait.note)(&format!(
                "queued at position {} behind other runs, waiting up to {}",
                pos + 1,
                humantime::format_duration(Duration::from_secs(wait.limit.as_secs()))
            ));
        }
        let elapsed = began.elapsed();
        if elapsed >= wait.limit {
            tracing::warn!(?elapsed, "gave up waiting for a host");
            let mut out = vec![format!(
                "waited {} in the queue (position {}) for a host with a free job slot, memory and disk for one more job",
                humantime::format_duration(Duration::from_secs(elapsed.as_secs())),
                pos + 1
            )];
            out.append(&mut lines);
            return Err(Error::NoHost(out));
        }
        std::thread::sleep(LOCAL_POLL.min(wait.limit.saturating_sub(elapsed)));
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
            disk_max: None,
            footprints: std::collections::BTreeMap::new(),
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
            kind: crate::transport::Kind::Unix,
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

    fn owned(mut p: Probe, power: Option<bool>, idle: Option<u64>) -> Probe {
        p.facts.on_battery = power;
        p.facts.idle_secs = idle;
        p
    }

    const WINDOW: std::time::Duration = std::time::Duration::from_secs(300);

    // frob:tests crates/goway/src/pool.rs::owner_use
    #[test]
    fn the_owner_is_in_use_on_battery_or_within_the_idle_window_and_unknown_never_counts() {
        let p = probe(4, 0.0, 0);
        assert_eq!(owner_use(&p, WINDOW), OwnerUse::Unknown);
        assert_eq!(
            owner_use(&owned(p.clone(), Some(true), None), WINDOW),
            OwnerUse::Battery
        );
        assert_eq!(
            owner_use(&owned(p.clone(), Some(true), Some(9999)), WINDOW),
            OwnerUse::Battery
        );
        assert_eq!(
            owner_use(&owned(p.clone(), Some(false), Some(10)), WINDOW),
            OwnerUse::Active(10)
        );
        assert_eq!(
            owner_use(&owned(p.clone(), Some(false), Some(300)), WINDOW),
            OwnerUse::Idle
        );
        assert_eq!(
            owner_use(&owned(p.clone(), Some(false), None), WINDOW),
            OwnerUse::Idle
        );
        assert_eq!(
            owner_use(&owned(p.clone(), None, Some(5)), WINDOW),
            OwnerUse::Active(5)
        );
        assert_eq!(
            owner_use(&owned(p, Some(true), Some(1)), std::time::Duration::ZERO),
            OwnerUse::Unknown,
            "a zero window switches the awareness off"
        );
        assert!(!OwnerUse::Unknown.in_use() && !OwnerUse::Idle.in_use());
    }

    // frob:tests crates/goway/src/pool.rs::ranked
    #[test]
    fn a_host_in_use_is_never_skipped_only_less_attractive_when_the_choice_is_close() {
        let hosts = [host("busy", None), host("quiet", None)];
        let rank = |busy: Probe, quiet: Probe| {
            let probed = vec![
                Probed {
                    host: &hosts[0],
                    result: Ok((found("busy"), busy)),
                },
                Probed {
                    host: &hosts[1],
                    result: Ok((found("quiet"), quiet)),
                },
            ];
            ranked(&Config::default(), &probed)
        };
        // Equal load: the idle one first, but both stay.
        let tie = rank(
            owned(probe(4, 1.0, 0), Some(true), None),
            owned(probe(4, 1.0, 0), Some(false), Some(9000)),
        );
        assert_eq!(tie, [1, 0]);
        // Far less loaded: the busy one still wins.
        let far = rank(
            owned(probe(4, 0.0, 0), None, Some(1)),
            owned(probe(4, 3.0, 0), Some(false), Some(9000)),
        );
        assert_eq!(far, [0, 1]);
        // Unknown is not penalised: equal load keeps the config order.
        let unknown = rank(
            probe(4, 1.0, 0),
            owned(probe(4, 1.0, 0), Some(false), Some(9000)),
        );
        assert_eq!(unknown, [0, 1]);
    }

    // frob:tests crates/goway/src/pool.rs::priority_word
    #[test]
    fn a_run_on_a_host_in_use_gets_the_owner_priority_but_this_machine_and_idle_hosts_do_not() {
        let config = Config::default();
        let h = host("h", None);
        let busy = owned(probe(8, 0.0, 0), Some(true), None);
        assert_eq!(priority_word(&config, &h, &found("h"), &busy), "owner");
        let idle = owned(probe(8, 0.0, 0), Some(false), Some(9000));
        assert_eq!(priority_word(&config, &h, &found("h"), &idle), "low");
        let mut local = found("h");
        local.source = resolve::Source::Local;
        assert_eq!(priority_word(&config, &h, &local, &busy), "low");
        let mut windows = found("h");
        windows.kind = crate::transport::Kind::WindowsSsh;
        assert_eq!(priority_word(&config, &h, &windows, &busy), "low");
        let mut off = Config::default();
        off.defaults.owner_idle = std::time::Duration::ZERO;
        assert_eq!(priority_word(&off, &h, &found("h"), &busy), "low");
        let note = owner_note("h", &busy, "owner").unwrap();
        assert!(
            note.contains("extra nicely") && note.contains("4 build jobs"),
            "{note}"
        );
        assert!(owner_note("h", &busy, "low").is_none());
    }

    #[test]
    fn the_probe_asks_for_the_owner_state_unless_it_is_switched_off() {
        let mut config = Config::default();
        assert!(
            probe_call(&config, false, false, &[])
                .args
                .contains(&"owner".to_owned())
        );
        config.defaults.owner_idle = std::time::Duration::ZERO;
        assert!(
            !probe_call(&config, false, false, &[])
                .args
                .contains(&"owner".to_owned())
        );
    }

    // frob:tests crates/goway/src/pool.rs::ranked
    #[test]
    fn an_unset_max_jobs_means_every_core() {
        let hosts = [host("h", None)];
        let at = |jobs| {
            let probed = vec![Probed {
                host: &hosts[0],
                result: Ok((found("h"), probe(8, 0.0, jobs))),
            }];
            ranked(&Config::default(), &probed)
        };
        assert_eq!(at(7), [0]);
        assert!(at(8).is_empty(), "8 jobs on 8 cores is the default limit");
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

    // frob:ticket 01M43CFMDFG8YSM3HNRD0GB213
    // frob:tests crates/goway/src/pool.rs::apply_pending
    #[test]
    fn a_pending_claim_of_the_repository_takes_its_footprint_from_free_disk() {
        let gib = 1024u64 * 1024 * 1024;
        let mut roomy = probe(16, 0.0, 0);
        roomy.disk_free = Some(30 * gib);
        roomy.disk_used = Some(0);
        roomy.footprints.insert("repo1".to_owned(), 20 * gib);
        let hosts = [host("helios", None)];
        let mut probed = vec![Probed {
            host: &hosts[0],
            result: Ok((found("helios"), roomy)),
        }];
        let sel = Selection {
            repo_id: Some("repo1".to_owned()),
            ..Selection::default()
        };
        let config = Config::default();
        assert_eq!(ranked_for(&config, &sel, &probed), [0], "room for one copy");
        let mut snap = Snapshot::default();
        snap.pending.insert("helios".to_owned(), 1);
        snap.pending_repos
            .insert("helios".to_owned(), vec!["repo1".to_owned()]);
        apply_pending(&config, &snap, &mut probed);
        assert!(
            ranked_for(&config, &sel, &probed).is_empty(),
            "the first run's pending claim leaves no room for a second copy"
        );
        assert_eq!(
            probed[0].result.as_ref().unwrap().1.disk_free,
            Some(10 * gib)
        );
    }

    // frob:tests crates/goway/src/pool.rs::ranked_for
    #[test]
    fn a_host_without_disk_room_for_the_repositorys_footprint_is_held_back() {
        let gib = 1024u64 * 1024 * 1024;
        let mut small = probe(16, 0.0, 0);
        small.disk_free = Some(3 * gib);
        small.disk_used = Some(2 * gib);
        small.footprints.insert("repo1".to_owned(), 20 * gib);
        let mut evictable = probe(16, 4.0, 0);
        evictable.disk_free = Some(3 * gib);
        evictable.disk_used = Some(30 * gib);
        evictable.footprints.insert("repo1".to_owned(), 20 * gib);
        let hosts = [host("small", None), host("evictable", None)];
        let probed = vec![
            Probed {
                host: &hosts[0],
                result: Ok((found("small"), small)),
            },
            Probed {
                host: &hosts[1],
                result: Ok((found("evictable"), evictable)),
            },
        ];
        let config = Config::default();
        let mut sel = Selection {
            repo_id: Some("repo1".to_owned()),
            ..Selection::default()
        };
        assert_eq!(
            ranked_for(&config, &sel, &probed),
            [1],
            "5 GiB cannot hold 20 GiB plus its margin; 33 GiB with eviction can"
        );
        let why = room_shortage(&sel, &probed[0].result.as_ref().unwrap().1);
        assert!(why.unwrap().contains("needs about 22.0 GiB"));
        assert_eq!(skipped_for_room(&sel, &probed).len(), 1);
        sel.repo_id = Some("other".to_owned());
        assert_eq!(
            ranked_for(&config, &sel, &probed).len(),
            2,
            "unknown repository"
        );
    }

    // frob:tests crates/goway/src/pool.rs::ranked_for
    #[test]
    fn a_host_with_less_free_memory_than_one_jobs_reserve_gets_no_further_job() {
        let gib = 1024u64 * 1024 * 1024;
        let mut tight = probe(16, 0.0, 6);
        tight.facts.mem_avail = Some(gib * 7 / 10);
        let mut roomy = probe(16, 4.0, 0);
        roomy.facts.mem_avail = Some(8 * gib);
        let hosts = [host("tight", Some(16)), host("roomy", None)];
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
        let config = Config::default();
        assert_eq!(config.defaults.job_mem_bytes(), gib * 3 / 2);
        assert_eq!(
            ranked(&config, &probed),
            [1],
            "0.7 GiB free is below the 1.5 GiB reserve"
        );
        // The reserve is configurable, per host too (0 turns it off).
        let mut off = Config::default();
        off.defaults.job_mem = "0".to_owned();
        assert_eq!(ranked(&off, &probed).len(), 2);
        let mut small = hosts.clone();
        small[0].job_mem = Some("512M".to_owned());
        let probed: Vec<Probed<'_>> = probed
            .into_iter()
            .zip(small.iter())
            .map(|(p, h)| Probed {
                host: h,
                result: p.result,
            })
            .collect();
        assert_eq!(
            ranked(&config, &probed).len(),
            2,
            "512M reserve fits in 0.7 GiB"
        );
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

    /// A host whose clock runs `ahead` seconds fast (and `None`: it reports no epoch).
    struct Clocked(Option<i64>);
    impl Prober for Clocked {
        fn probe(&self, _: &Target, _: KeyPolicy, _: &str) -> resolve::ProbeResult {
            let epoch = self.0.map_or_else(String::new, |a| {
                let now = i64::try_from(crate::state::now_secs()).unwrap();
                format!("epoch={}\n", now + a)
            });
            Ok(format!(
                "arch=x86_64\nhostname=h\ncores=4\nload1=0\nload5=0\nload15=0\njobs=0\n{epoch}"
            ))
        }
    }

    // frob:tests crates/goway/src/pool.rs::probe_one
    #[test]
    fn probe_one_measures_the_helper_clock_offset() {
        let mut h = host("h", None);
        h.address = Some("10.0.0.9".to_owned());
        let mut config = Config::default();
        config.hosts.push(h.clone());
        let offset = |ahead| {
            let mut state = State::default();
            probe_one(&config, &h, &mut state, &NoLookup, &Clocked(ahead), false)
                .unwrap()
                .1
                .facts
                .clock_offset_ms
        };
        let ms = offset(Some(3600)).unwrap();
        assert!((3_595_000..=3_605_000).contains(&ms), "{ms}");
        let ms = offset(Some(-90)).unwrap();
        assert!((-95_000..=-85_000).contains(&ms), "{ms}");
        assert!(offset(Some(0)).unwrap().abs() < 2_000);
        assert_eq!(offset(None), None);
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

    /// `10.0.0.1` (the idlest) is a Windows host; the other two are Linux.
    struct MixedOsProber;
    impl Prober for MixedOsProber {
        fn probe(&self, target: &Target, _: KeyPolicy, _: &str) -> resolve::ProbeResult {
            let (os, load) = match target.address.as_str() {
                "10.0.0.1" => ("windows", 0.0),
                "10.0.0.2" => ("linux", 2.0),
                "10.0.0.3" => ("linux", 1.0),
                _ => return Err((Failure::Unreachable, String::new())),
            };
            Ok(format!(
                "arch=x86_64\nhostname=h\ncores=8\nload1={load}\nload5=0\nload15=0\njobs=0\nos={os}\n"
            ))
        }
    }

    fn pool_choice(selection: &Selection, wanted: Option<&str>) -> Result<String> {
        let config = three_hosts();
        let mut state = State::default();
        choose(
            &config,
            selection,
            &mut state,
            Path::new(""),
            &NoLookup,
            &MixedOsProber,
            wanted,
        )
        .map(|(h, ..)| h.name)
    }

    // frob:ticket 01M42EZ3TAWHTCJ4MWVYKNEA39
    // frob:tests crates/goway/src/pool.rs::ranked_for
    // frob:tests crates/goway/src/pool.rs::choose
    #[test]
    fn the_default_pool_only_holds_hosts_of_the_laptops_os() {
        let linux = Selection::default().with_default_os("linux");
        // The idlest host is Windows; a plain run never lands there.
        assert_eq!(pool_choice(&linux, None).unwrap(), "small");
        // A macOS laptop's pool has neither of these.
        let err = pool_choice(&Selection::default().with_default_os("darwin"), None).unwrap_err();
        assert!(matches!(err, Error::NoHost(_)), "{err}");
        assert!(
            err.to_string()
                .contains("idle: windows host, outside the default darwin pool"),
            "{err}"
        );
    }

    // frob:ticket 01M42EZ3TAWHTCJ4MWVYKNEA39
    // frob:tests crates/goway/src/pool.rs::choose
    #[test]
    fn asking_for_another_os_or_pinning_a_host_reaches_it() {
        let win = selection("os=windows", "").with_default_os("linux");
        assert_eq!(win.pool_os, None, "an os need replaces the default");
        assert_eq!(pool_choice(&win, None).unwrap(), "idle");
        let linux = Selection::default().with_default_os("linux");
        assert_eq!(pool_choice(&linux, Some("idle")).unwrap(), "idle");
    }

    // frob:ticket 01M42EZ3TAWHTCJ4MWVYKNEA39
    // frob:tests crates/goway/src/pool.rs::choose_many
    #[test]
    fn shards_never_mix_operating_systems() {
        let config = three_hosts();
        let mut state = State::default();
        let mut shards = |sel: &Selection, n| {
            choose_many(
                &config,
                sel,
                &mut state,
                Path::new(""),
                &NoLookup,
                &MixedOsProber,
                n,
            )
        };
        let linux = Selection::default().with_default_os("linux");
        let names: Vec<String> = shards(&linux, 2)
            .unwrap()
            .into_iter()
            .map(|h| h.0.name)
            .collect();
        assert_eq!(names, ["small", "gpu"]);
        assert!(
            shards(&linux, 3).is_err(),
            "the Windows host is not a third shard"
        );
        let win = selection("os=windows", "").with_default_os("linux");
        assert_eq!(shards(&win, 1).unwrap()[0].0.name, "idle");
    }

    // frob:ticket 01M42FJVGY91ND091THEDPP8DN
    // frob:tests crates/goway/src/pool.rs::choose_each_os
    #[test]
    fn each_os_takes_the_best_host_of_every_os() {
        let config = three_hosts();
        let mut state = State::default();
        let hosts = choose_each_os(
            &config,
            &Selection::default(),
            &mut state,
            Path::new(""),
            &NoLookup,
            &MixedOsProber,
        )
        .unwrap();
        // Linux: the idler of the two (small); Windows: its only host (idle). OS-name order.
        let names: Vec<&str> = hosts.iter().map(|h| h.0.name.as_str()).collect();
        assert_eq!(names, ["small", "idle"]);
    }
}
