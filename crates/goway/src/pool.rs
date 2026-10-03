//! The host pool: probe every host in parallel (one ssh call each, which
//! also resolves its address) and pick the least loaded.
//!
//! Score = (1-minute load + goway jobs running) / cores. goway's own jobs
//! are counted on top of the load average because a job that just started
//! has not shown up in the load yet. Hosts at `max_jobs` are skipped.

use std::collections::BTreeMap;

use crate::config::{Config, HostConfig};
use crate::error::{Error, Result};
use crate::remote;
use crate::resolve::{self, Found, Lookup, Prober};
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
}

/// Parse the `probe` verb's `key=value` lines.
pub fn parse_probe(text: &str) -> Option<Probe> {
    let kv: BTreeMap<&str, &str> = text
        .lines()
        .filter_map(|l| l.trim().split_once('='))
        .collect();
    let num = |k: &str| kv.get(k).and_then(|v| v.parse::<f64>().ok());
    Some(Probe {
        arch: (*kv.get("arch")?).to_owned(),
        hostname: (*kv.get("hostname")?).to_owned(),
        cores: kv.get("cores")?.parse().ok()?,
        load: [num("load1")?, num("load5")?, num("load15")?],
        jobs: kv.get("jobs")?.parse().ok()?,
        disk_used: kv.get("disk_used").and_then(|v| v.parse().ok()),
        disk_free: kv.get("disk_free").and_then(|v| v.parse().ok()),
    })
}

/// Lower is better.
pub fn score(p: &Probe) -> f64 {
    (p.load[0] + f64::from(p.jobs)) / f64::from(p.cores.max(1))
}

/// A probed host: where it answered and what it said, or why it did not.
#[derive(Debug)]
pub struct Probed<'a> {
    /// The host's config.
    pub host: &'a HostConfig,
    /// The working address and probe, or the failure.
    pub result: Result<(Found, Probe)>,
}

/// Index of the best host among `probed`, if any is usable. Hosts at
/// `max_jobs` or above their `max_load` (load per core) are skipped.
pub fn pick(config: &Config, probed: &[Probed<'_>]) -> Option<usize> {
    probed
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
            Some((i, score(probe), probe.jobs))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1).then(a.2.cmp(&b.2)).then(a.0.cmp(&b.0)))
        .map(|(i, s, _)| {
            tracing::info!(host = %probed[i].host.name, score = s, "picked");
            i
        })
}

/// The remote command that probes a host.
pub fn probe_command(config: &Config, disk: bool) -> String {
    let root = config.defaults.remote_root.as_str();
    if disk {
        remote::invocation("probe", &[root, "disk"])
    } else {
        remote::invocation("probe", &[root])
    }
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
    let found = resolve::resolve(
        config,
        host,
        state,
        lookup,
        prober,
        KeyPolicy::Strict,
        &probe_command(config, disk),
    )?;
    let probe = parse_probe(&found.output).ok_or_else(|| Error::Ssh {
        host: host.name.clone(),
        message: format!("unexpected probe output: {}", found.output.trim()),
    })?;
    tracing::debug!(host = %host.name, ?probe, "probed");
    Ok((found, probe))
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
    state: &mut State,
    lookup: &(dyn Lookup + Sync),
    prober: &(dyn Prober + Sync),
    wanted: Option<&str>,
) -> Result<(HostConfig, Found, Probe)> {
    if let Some(name) = wanted {
        let host = config.host(name)?;
        let (found, probe) = probe_one(config, host, state, lookup, prober, false)?;
        return Ok((host.clone(), found, probe));
    }
    if config.hosts.is_empty() {
        return Err(Error::Usage(
            "no hosts configured; add one with `goway host add NAME`".to_owned(),
        ));
    }
    let mut results = probe_all(config, state, lookup, prober, false);
    match pick(config, &results) {
        Some(i) => {
            let chosen = results.swap_remove(i);
            let (found, probe) = chosen.result?;
            Ok((chosen.host.clone(), found, probe))
        }
        None => Err(Error::NoHost(
            results
                .iter()
                .map(|p| match &p.result {
                    Ok((_, probe)) => format!(
                        "{}: busy (load {:.2} on {} cores, {} goway jobs)",
                        p.host.name, probe.load[0], probe.cores, probe.jobs
                    ),
                    Err(e) => format!("{}: {e}", p.host.name),
                })
                .collect(),
        )),
    }
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
        }
    }

    fn host(name: &str, max_jobs: Option<u32>) -> HostConfig {
        HostConfig {
            name: name.to_owned(),
            address: None,
            port: None,
            user: None,
            max_jobs,
            priority: None,
            max_load: None,
            identity: None,
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
        assert!((score(&p) - 2.5 / 12.0).abs() < 1e-9);
        assert!(parse_probe("arch=x\n").is_none());
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
        assert!(choose(&config, &mut state, &NoLookup, &ByAddress, None).is_err());
        assert!(choose(&config, &mut state, &NoLookup, &ByAddress, Some("a")).is_ok());
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
        let (h, found, probe) = choose(&config, &mut state, &NoLookup, &ByAddress, None).unwrap();
        assert_eq!(h.name, "b", "2/16 beats 3/4");
        assert_eq!(found.target.address, "10.0.0.2");
        assert_eq!(probe.hostname, "b");
        assert_eq!(state.get("a").unwrap().address, "10.0.0.1");
        assert!(state.get("c").is_none());

        let (h, ..) = choose(&config, &mut state, &NoLookup, &ByAddress, Some("a")).unwrap();
        assert_eq!(h.name, "a", "--host pins");
        assert!(choose(&config, &mut state, &NoLookup, &ByAddress, Some("c")).is_err());
        assert!(choose(&Config::default(), &mut state, &NoLookup, &ByAddress, None).is_err());
    }
}
