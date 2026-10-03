//! `goway status`: every host's address, load, goway jobs and disk.

use crate::config::Config;
use crate::error::Result;
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

/// The status table rows (header first); pure so it can be tested.
pub fn rows(probed: &[Probed<'_>]) -> Vec<Vec<String>> {
    let mut rows = vec![
        [
            "host",
            "address",
            "arch",
            "cores",
            "load 1/5/15",
            "jobs",
            "goway disk",
            "free",
        ]
        .map(str::to_owned)
        .to_vec(),
    ];
    for p in probed {
        match &p.result {
            Ok((found, probe)) => rows.push(vec![
                p.host.name.clone(),
                format!("{} ({})", found.target.address, found.source),
                probe.arch.clone(),
                probe.cores.to_string(),
                format!(
                    "{:.2} {:.2} {:.2}",
                    probe.load[0], probe.load[1], probe.load[2]
                ),
                match p.host.max_jobs {
                    Some(max) => format!("{}/{max}", probe.jobs),
                    None => probe.jobs.to_string(),
                },
                probe.disk_used.map_or_else(|| "-".to_owned(), human_bytes),
                probe.disk_free.map_or_else(|| "-".to_owned(), human_bytes),
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
) -> Result<u8> {
    let config = Config::load(&paths.config_file())?;
    if config.hosts.is_empty() {
        renderer.note(format_args!(
            "no hosts configured in {}; add one with `goway host add NAME`",
            paths.config_file().display()
        ));
        return Ok(0);
    }
    let mut state = State::load(&paths.state_file())?;
    let results = pool::probe_all(&config, &mut state, lookup, prober, true);
    if let Err(e) = state.save(&paths.state_file()) {
        tracing::warn!(error = %e, "cannot cache host addresses");
    }
    renderer.table(&rows(&results));
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

    #[test]
    fn bytes_are_human() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.5 KiB");
        assert_eq!(human_bytes(3 * 1024 * 1024 * 1024), "3.0 GiB");
    }
}
