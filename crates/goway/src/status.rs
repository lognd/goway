//! `goway status`: every host's address, load, goway jobs and disk.

use crate::config::Config;
use crate::error::Result;
use crate::local;
use crate::paths::Paths;
use crate::pool::{self, Probed};
use crate::render::Renderer;
use crate::resolve::{Lookup, Prober};
use crate::state::State;

/// Human-readable bytes (binary units, one decimal).
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    #[allow(clippy::cast_precision_loss)] // display only
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// The GPUs of a host, comma separated, or `none` (`-` when not yet probed).
fn gpu_summary(f: &crate::facts::Facts) -> String {
    match &f.hw {
        None => "-".to_owned(),
        Some(h) if h.gpus.is_empty() => "none".to_owned(),
        Some(h) => h
            .gpus
            .iter()
            .map(crate::facts::Gpu::summary)
            .collect::<Vec<_>>()
            .join(", "),
    }
}

/// goway's disk use against its budget: `1.2 GiB / 20.0 GiB`, `(over)` past it.
fn disk_cell(used: Option<u64>, max: Option<u64>) -> String {
    match (used, max) {
        (Some(u), Some(m)) if u > m => format!("{} / {} (over)", human_bytes(u), human_bytes(m)),
        (Some(u), Some(m)) => format!("{} / {}", human_bytes(u), human_bytes(m)),
        (Some(u), None) => human_bytes(u),
        _ => "-".to_owned(),
    }
}

/// The status table rows (header first); pure so it can be tested.
pub fn rows(probed: &[Probed<'_>], local_in_pool: bool) -> Vec<Vec<String>> {
    let mut rows = vec![
        [
            "host",
            "address",
            "arch",
            "cores",
            "ram avail/total",
            "gpu",
            "features",
            "load 1/5/15",
            "jobs",
            "goway disk",
            "free",
            "facts",
        ]
        .map(str::to_owned)
        .to_vec(),
    ];
    for p in probed {
        match &p.result {
            Ok((found, probe)) => rows.push(vec![
                p.host.name.clone(),
                if found.is_local() {
                    format!(
                        "{} ({})",
                        found.target.address,
                        if local_in_pool {
                            "in the pool"
                        } else {
                            "not in the pool"
                        }
                    )
                } else {
                    format!("{} ({})", found.target.address, found.source)
                },
                probe.arch.clone(),
                probe.cores.to_string(),
                crate::facts::ram_summary(&probe.facts),
                gpu_summary(&probe.facts),
                crate::facts::feature_summary(probe.facts.hw.as_ref()),
                format!(
                    "{:.2} {:.2} {:.2}",
                    probe.load[0], probe.load[1], probe.load[2]
                ),
                format!("{}/{}", probe.jobs, p.host.job_limit(probe.cores)),
                disk_cell(probe.disk_used, probe.disk_max),
                probe.disk_free.map_or_else(|| "-".to_owned(), human_bytes),
                crate::facts::age_summary(probe.facts.hw_age),
            ]),
            Err(_) => rows.push(vec![
                p.host.name.clone(),
                "unreachable".to_owned(),
                "-".to_owned(),
                "-".to_owned(),
                "-".to_owned(),
                "-".to_owned(),
                "-".to_owned(),
                "-".to_owned(),
                "-".to_owned(),
                "-".to_owned(),
                "-".to_owned(),
                "-".to_owned(),
            ]),
        }
    }
    rows
}

/// `goway status`. Exits 0 even when hosts are unreachable (it reports).
pub fn status(
    paths: &Paths,
    renderer: Renderer,
    lookup: &(dyn Lookup + Sync),
    prober: &(dyn Prober + Sync),
    refresh: bool,
) -> Result<u8> {
    let config = Config::load(&paths.config_file())?;
    if config.hosts.is_empty() && config.local.is_none() {
        renderer.note(format_args!(
            "no hosts configured in {}; add one with `goway host add NAME`",
            paths.config_file().display()
        ));
        return Ok(0);
    }
    let mut state = State::load(&paths.state_file())?;
    state.refresh_facts = refresh;
    let local_host = local::host(&config);
    let mut results = pool::probe_all(&config, &mut state, lookup, prober, true);
    // `[local]` configured (and not shadowed by a host called local): a row for this machine.
    if config.local.is_some() && config.host(local::NAME).is_err() {
        let result = local::candidate(
            &config,
            &local::jobs_dir(paths),
            &mut state,
            true,
            crate::resolve::Source::Local,
        )
        .map(|(_, found, probe)| (found, probe));
        results.push(Probed {
            host: &local_host,
            result,
        });
    }
    if let Err(e) = state.save(&paths.state_file()) {
        tracing::warn!(error = %e, "cannot cache host addresses");
    }
    renderer.table(&rows(&results, config.local_in_pool()));
    for p in &results {
        if let Err(e) = &p.result {
            renderer.warn(format_args!("{}: {e}", p.host.name));
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // frob:tests crates/goway/src/status.rs::rows
    #[test]
    fn rows_show_gpu_ram_and_features_and_stay_aligned() {
        use crate::config::HostConfig;
        use crate::facts::{Facts, Gpu, StaticFacts};
        use crate::resolve::{Found, Source};
        use crate::ssh::Target;
        let host = HostConfig {
            name: "helios".to_owned(),
            ..HostConfig::default()
        };
        let found = Found {
            target: Target {
                name: "helios".to_owned(),
                address: "192.0.2.1".to_owned(),
                port: 22,
                user: None,
                identity: None,
            },
            source: Source::Cached,
            output: String::new(),
        };
        let gib = 1024u64 * 1024 * 1024;
        let probe = crate::pool::Probe {
            arch: "x86_64".to_owned(),
            hostname: "h".to_owned(),
            cores: 8,
            load: [0.0; 3],
            jobs: 0,
            disk_used: None,
            disk_free: None,
            disk_max: None,
            facts: Facts {
                os: Some("linux".to_owned()),
                mem_total: Some(16 * gib),
                mem_avail: Some(8 * gib),
                hw: Some(StaticFacts {
                    gpus: vec![Gpu {
                        vendor: "nvidia".to_owned(),
                        name: "RTX 4090".to_owned(),
                        mem_mib: Some(24576),
                        driver: None,
                        cuda: Some("12.5".to_owned()),
                    }],
                    cpu_features: vec!["avx2".to_owned()],
                    kvm: true,
                    ..StaticFacts::default()
                }),
                hw_age: Some(7200),
            },
        };
        let probed = vec![
            Probed {
                host: &host,
                result: Ok((found, probe)),
            },
            Probed {
                host: &host,
                result: Err(crate::error::Error::Usage("x".to_owned())),
            },
        ];
        let rows = rows(&probed, false);
        assert!(rows.iter().all(|r| r.len() == rows[0].len()));
        let line = rows[1].join(" | ");
        assert!(line.contains("8.0/16.0 GiB"), "{line}");
        assert!(line.contains("RTX 4090 24 GiB cuda 12.5"), "{line}");
        assert!(line.contains("avx2 kvm"), "{line}");
        assert!(line.contains("2h ago"), "{line}");
    }

    #[test]
    fn disk_cell_shows_use_against_budget() {
        let gib = 1u64 << 30;
        assert_eq!(disk_cell(Some(gib), Some(20 * gib)), "1.0 GiB / 20.0 GiB");
        assert_eq!(
            disk_cell(Some(21 * gib), Some(20 * gib)),
            "21.0 GiB / 20.0 GiB (over)"
        );
        assert_eq!(disk_cell(Some(gib), None), "1.0 GiB");
        assert_eq!(disk_cell(None, None), "-");
    }

    #[test]
    fn bytes_are_human() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.5 KiB");
        assert_eq!(human_bytes(3 * 1024 * 1024 * 1024), "3.0 GiB");
    }
}
