//! Helper clock offset: how far a helper's wall clock is from this machine's.
//!
//! The probe reports `epoch=SECONDS` (whole seconds, read on the helper).
//! goway times the round trip and assumes the helper read its clock halfway
//! through it, so the offset is the helper's time minus the laptop's time at
//! the midpoint. A WSL helper whose clock stopped during sleep, or a Windows
//! host that never resynced, shows up here before it breaks make or cargo
//! freshness checks.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Offsets beyond this many milliseconds are reported (2 s).
pub const WARN_MS: i64 = 2_000;

/// The doctor fact carrying the measured offset in milliseconds.
pub const FACT: &str = "clock_offset_ms";

/// Offsets beyond ten years are a broken answer, not a clock.
const MAX_MS: i64 = 10 * 365 * 24 * 3600 * 1000;

/// The offset in ms of a helper that read `epoch` (whole seconds) during a
/// call sent at `sent` that took `rtt`; positive means the helper is ahead.
pub fn offset_ms(epoch: u64, sent: SystemTime, rtt: Duration) -> Option<i64> {
    let to_ms = |d: Duration| i64::try_from(d.as_millis()).ok();
    let sent_ms = to_ms(sent.duration_since(UNIX_EPOCH).ok()?)?;
    // Whole seconds are floored, so the true reading is half a second later on average.
    let helper_ms = i64::try_from(epoch)
        .ok()?
        .checked_mul(1000)?
        .checked_add(500)?;
    let laptop_ms = sent_ms.checked_add(to_ms(rtt)? / 2)?;
    let off = helper_ms.checked_sub(laptop_ms)?;
    (off.abs() <= MAX_MS).then_some(off)
}

/// The offset from a probe's or doctor's `output` (its `epoch=` line), if it has one.
pub fn measure(output: &str, sent: SystemTime, rtt: Duration) -> Option<i64> {
    let epoch = super::kv(output).get("epoch")?.parse().ok()?;
    let off = offset_ms(epoch, sent, rtt);
    tracing::debug!(epoch, ?rtt, ?off, "helper clock measured");
    off
}

/// Time `f`, returning its result, when it started and how long it took.
pub fn timed<T>(f: impl FnOnce() -> T) -> (T, SystemTime, Duration) {
    let sent = SystemTime::now();
    let start = std::time::Instant::now();
    let out = f();
    (out, sent, start.elapsed())
}

/// Whether `ms` is far enough off to report.
pub fn significant(ms: i64) -> bool {
    ms.abs() > WARN_MS
}

/// One phrase for an offset, e.g. `clock is 3.2 s ahead of this machine`.
pub fn describe(ms: i64) -> String {
    #[allow(clippy::cast_precision_loss)] // display only; offsets are capped at ten years
    let secs = ms.unsigned_abs() as f64 / 1000.0;
    let way = if ms >= 0 { "ahead of" } else { "behind" };
    if secs >= 120.0 {
        format!("clock is {:.0} min {way} this machine", secs / 60.0)
    } else {
        format!("clock is {secs:.1} s {way} this machine")
    }
}

/// How to fix a drifting clock on a Unix (`windows == false`) or Windows host.
pub fn fix_text(windows: bool) -> &'static str {
    if windows {
        "resync the time: Settings > Time & language > Date & time > Sync now (or `w32tm /resync` as administrator)"
    } else {
        "WSL after sleep: run `wsl --shutdown` from Windows, or `sudo hwclock -s` on the host"
    }
}

/// What is wrong and how to fix it, or `None` when `ms` is within tolerance.
pub fn detail(ms: i64, windows: bool) -> Option<String> {
    significant(ms).then(|| format!("{}; {}", describe(ms), fix_text(windows)))
}

/// [`detail`] prefixed with the `host` name, for status.
pub fn warning(host: &str, ms: i64, windows: bool) -> Option<String> {
    detail(ms, windows).map(|d| format!("{host}: {d}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    // frob:tests crates/goway/src/facts/clock.rs::offset_ms
    #[test]
    fn the_offset_is_corrected_by_half_the_round_trip() {
        // Sent at 1000 s, 2 s round trip: the helper read at 1001 s on the laptop.
        // A whole-second reading of 1001 stands for 1001.5 on average: +500 ms.
        let rtt = Duration::from_secs(2);
        assert_eq!(offset_ms(1001, at(1000), rtt), Some(500));
        assert_eq!(offset_ms(1011, at(1000), rtt), Some(10_500));
        assert_eq!(offset_ms(990, at(1000), rtt), Some(-10_500));
    }

    // frob:tests crates/goway/src/facts/clock.rs::measure
    #[test]
    fn a_missing_or_absurd_epoch_measures_nothing() {
        let rtt = Duration::from_millis(40);
        assert_eq!(measure("cores=4\n", at(1000), rtt), None);
        assert_eq!(measure("epoch=soon\n", at(1000), rtt), None);
        assert_eq!(measure("epoch=99999999999999\n", at(1000), rtt), None);
        assert!(measure("epoch=1000\n", at(1000), rtt).is_some());
    }

    // frob:tests crates/goway/src/facts/clock.rs::warning
    #[test]
    fn only_offsets_over_two_seconds_warn_and_the_fix_matches_the_os() {
        assert_eq!(warning("helios", 1_999, false), None);
        assert_eq!(warning("helios", -2_000, false), None);
        let w = warning("helios", -3_200, false).unwrap();
        assert!(
            w.contains("3.2 s behind") && w.contains("hwclock -s") && w.contains("wsl --shutdown")
        );
        let w = warning("orion", 600_000, true).unwrap();
        assert!(w.contains("10 min ahead") && w.contains("resync"));
    }
}
