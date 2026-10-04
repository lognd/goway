//! What a repository needs on a helper's disk, and whether a helper has room.
//!
//! Each helper records the peak footprint of every repository it has built
//! (slot tree plus target and build dirs, measured with `du` after a run)
//! and reports them in its probe as `footprint.<repo id>=BYTES`. Before a
//! run is placed, a helper whose free space plus what the disk budget could
//! evict is below the footprint plus a margin is held back like one short
//! of memory: the queue waits for it instead of the build filling the disk
//! (mold's "Disk full?", a bus error in clang). The margin constants are
//! sent to the helper in the `room:` word so both sides agree.

use std::collections::BTreeMap;

use crate::pool::Probe;
use crate::status::human_bytes;

/// The smallest margin kept free on top of a footprint (1 GiB).
pub const MIN_MARGIN: u64 = 1 << 30;

/// The margin as a percentage of the footprint, when larger than [`MIN_MARGIN`].
pub const MARGIN_PERCENT: u64 = 10;

/// The prefix of a footprint line in the probe's output.
pub const KEY_PREFIX: &str = "footprint.";

/// The margin kept on top of a footprint of `footprint` bytes.
pub fn margin(footprint: u64) -> u64 {
    MIN_MARGIN.max(footprint.saturating_mul(MARGIN_PERCENT) / 100)
}

/// The space a run of a repository with `footprint` bytes needs: footprint plus margin.
pub fn required(footprint: u64) -> u64 {
    footprint.saturating_add(margin(footprint))
}

/// The option word that gives a helper the margin rule (`room:MIN:PERCENT`).
pub fn room_word() -> String {
    format!("room:{MIN_MARGIN}:{MARGIN_PERCENT}")
}

/// The footprints in a probe's `key=value` lines, by repository id.
pub fn parse(kv: &BTreeMap<&str, &str>) -> BTreeMap<String, u64> {
    parse_prefixed(kv, KEY_PREFIX)
}

/// The `PREFIX<repo id>=BYTES` lines of a probe, by well-formed repository id.
fn parse_prefixed(kv: &BTreeMap<&str, &str>, prefix: &str) -> BTreeMap<String, u64> {
    kv.iter()
        .filter_map(|(k, v)| {
            let id = k.strip_prefix(prefix)?;
            let ok = !id.is_empty()
                && id.len() <= 128
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
            ok.then(|| Some((id.to_owned(), v.parse().ok()?)))?
        })
        .collect()
}

/// The prefix of a memory peak line in the probe's output.
pub const MEM_KEY_PREFIX: &str = "mempeak.";

/// The smallest memory margin kept on top of a peak (256 MiB).
pub const MEM_MIN_MARGIN: u64 = 256 << 20;

/// The memory a run of a repository whose job tree peaked at `peak` bytes needs:
/// the peak plus the larger of [`MEM_MIN_MARGIN`] and a tenth of it.
pub fn mem_required(peak: u64) -> u64 {
    peak.saturating_add(MEM_MIN_MARGIN.max(peak / 10))
}

/// The memory peaks in a probe's `key=value` lines, by repository id.
pub fn parse_mem_peaks(kv: &BTreeMap<&str, &str>) -> BTreeMap<String, u64> {
    parse_prefixed(kv, MEM_KEY_PREFIX)
}

/// How a helper's memory stands against one repository's peak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemRoom {
    /// No peak is known, or the helper did not report its memory: nothing to judge.
    Unknown,
    /// Available memory covers the peak and margin.
    Fits,
    /// The helper's total memory is below the peak and margin: it can never run this.
    TooSmall {
        /// The helper's total memory.
        total: u64,
        /// Peak plus margin.
        need: u64,
    },
    /// Enough in total, but too little is available now: wait.
    Short {
        /// Bytes available now.
        avail: u64,
        /// Peak plus margin.
        need: u64,
    },
}

/// Judge `probe` for repository `repo_id`: total memory, then available memory, against
/// the repository's recorded peak plus margin.
pub fn assess_mem(probe: &Probe, repo_id: &str) -> MemRoom {
    let Some(&peak) = probe.mem_peaks.get(repo_id) else {
        return MemRoom::Unknown;
    };
    let need = mem_required(peak);
    if probe.facts.mem_total.is_some_and(|total| total < need) {
        return MemRoom::TooSmall {
            total: probe.facts.mem_total.unwrap_or(0),
            need,
        };
    }
    match probe.facts.mem_avail {
        Some(avail) if avail < need => MemRoom::Short { avail, need },
        Some(_) => MemRoom::Fits,
        None => MemRoom::Unknown,
    }
}

/// One phrase saying why a helper lacks memory for the repository (queue wait lines).
pub fn mem_short_text(room: MemRoom) -> Option<String> {
    match room {
        MemRoom::TooSmall { total, need } => Some(format!(
            "{} of memory in total, this repository peaked at about {} and needs it all",
            human_bytes(total),
            human_bytes(need)
        )),
        MemRoom::Short { avail, need } => Some(format!(
            "{} of memory free, this repository needs about {}",
            human_bytes(avail),
            human_bytes(need)
        )),
        MemRoom::Unknown | MemRoom::Fits => None,
    }
}

/// How a helper's disk stands against one repository's footprint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Room {
    /// No footprint is known, or the helper did not report its disk: nothing to judge.
    Unknown,
    /// Free space (plus evictable space) covers the footprint and margin.
    Fits,
    /// It does not.
    Short {
        /// Bytes free now.
        free: u64,
        /// Bytes goway holds there that the disk budget could evict.
        evictable: u64,
        /// Footprint plus margin.
        need: u64,
    },
}

/// Judge `probe` for repository `repo_id`: free space plus goway's own data (all evictable
/// when nothing runs; the run evicts first) against the footprint plus margin.
pub fn assess(probe: &Probe, repo_id: &str) -> Room {
    let Some(&fp) = probe.footprints.get(repo_id) else {
        return Room::Unknown;
    };
    let Some(free) = probe.disk_free else {
        return Room::Unknown;
    };
    let evictable = probe.disk_used.unwrap_or(0);
    let need = required(fp);
    if free.saturating_add(evictable) >= need {
        Room::Fits
    } else {
        Room::Short {
            free,
            evictable,
            need,
        }
    }
}

/// One phrase saying why a helper lacks room (for the queue's wait lines and notes).
pub fn short_text(free: u64, evictable: u64, need: u64) -> String {
    format!(
        "{} free ({} of it evictable), this repository needs about {}",
        human_bytes(free),
        human_bytes(evictable),
        human_bytes(need)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1 << 30;

    #[test]
    fn the_margin_is_a_gibibyte_or_a_tenth() {
        assert_eq!(margin(2 * GIB), GIB);
        assert_eq!(margin(50 * GIB), 5 * GIB);
        assert_eq!(required(20 * GIB), 22 * GIB);
        assert_eq!(room_word(), format!("room:{GIB}:10"));
    }

    // frob:tests crates/goway/src/footprint.rs::parse
    #[test]
    fn only_well_formed_footprint_lines_are_read() {
        let kv: BTreeMap<&str, &str> = [
            ("footprint.abc-1", "42"),
            ("footprint.", "1"),
            ("footprint.a/b", "1"),
            ("footprint.bad", "x"),
            ("cores", "4"),
        ]
        .into();
        let got = parse(&kv);
        assert_eq!(got.len(), 1);
        assert_eq!(got["abc-1"], 42);
    }

    // frob:ticket 01M43CWNW1JNQZMCJBQ2NH4FTC
    // frob:tests crates/goway/src/footprint.rs::assess_mem
    #[test]
    fn memory_is_judged_by_total_first_then_by_what_is_available() {
        let mut p = crate::pool::parse_probe(
            "arch=x86_64\nhostname=h\ncores=4\nload1=0\nload5=0\nload15=0\njobs=0\nmempeak.r=3221225472\n",
        )
        .unwrap();
        assert_eq!(p.mem_peaks["r"], 3 * GIB);
        p.facts.mem_total = Some(3 * GIB);
        p.facts.mem_avail = Some(3 * GIB);
        assert!(matches!(assess_mem(&p, "r"), MemRoom::TooSmall { .. }));
        p.facts.mem_total = Some(16 * GIB);
        assert!(matches!(assess_mem(&p, "r"), MemRoom::Short { .. }));
        p.facts.mem_avail = Some(8 * GIB);
        assert_eq!(assess_mem(&p, "r"), MemRoom::Fits);
        assert_eq!(assess_mem(&p, "other"), MemRoom::Unknown);
        assert_eq!(mem_required(GIB), GIB + 256 * (1 << 20));
        assert_eq!(mem_required(20 * GIB), 22 * GIB);
    }
}
