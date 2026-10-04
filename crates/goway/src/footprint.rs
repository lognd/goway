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
    MIN_MARGIN.max(footprint / 100 * MARGIN_PERCENT)
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
    kv.iter()
        .filter_map(|(k, v)| {
            let id = k.strip_prefix(KEY_PREFIX)?;
            let ok = !id.is_empty()
                && id.len() <= 128
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
            ok.then(|| Some((id.to_owned(), v.parse().ok()?)))?
        })
        .collect()
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
}
