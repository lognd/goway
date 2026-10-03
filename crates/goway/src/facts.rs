//! Host facts: what a host has (GPUs, RAM, CPU features, KVM, Docker).
//!
//! Live facts (RAM) come with every probe. Static facts (the rest) cost
//! extra work on the host (nvidia-smi, docker info), so they are cached in
//! state per host with a timestamp and refreshed daily or on request.
//! Everything a host reports about itself is parsed defensively with
//! bounds, like the scheduling probe.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

/// Static facts older than this are probed again.
pub const STATIC_MAX_AGE: u64 = 24 * 60 * 60;

/// Most GPUs accepted from one host.
const MAX_GPUS: usize = 64;
/// Most bytes of RAM accepted (1 PiB) and MiB of GPU memory (16 TiB).
const MAX_BYTES: u64 = 1 << 50;
const MAX_GPU_MIB: u64 = 16 * 1024 * 1024;
/// CPU features goway cares about, as the host reports them.
pub const CPU_FEATURES: [&str; 3] = ["avx2", "avx512f", "neon"];

/// One GPU as the host's driver tools report it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Gpu {
    /// `nvidia` or `amd`.
    pub vendor: String,
    /// Model name.
    pub name: String,
    /// Memory in MiB, when the tool says.
    pub mem_mib: Option<u64>,
    /// Driver version.
    pub driver: Option<String>,
    /// CUDA version (NVIDIA only).
    pub cuda: Option<String>,
}

impl Gpu {
    /// One short line: model, memory, driver and CUDA.
    pub fn summary(&self) -> String {
        let mut s = self.name.clone();
        if let Some(m) = self.mem_mib {
            let _ = write!(s, " {} GiB", m.div_ceil(1024));
        }
        if let Some(d) = &self.driver {
            let _ = write!(s, " drv {d}");
        }
        if let Some(c) = &self.cuda {
            let _ = write!(s, " cuda {c}");
        }
        s
    }
}

/// The facts that change rarely; cached per host.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct StaticFacts {
    /// GPUs the host's driver tools list.
    pub gpus: Vec<Gpu>,
    /// Notable CPU features present (see [`CPU_FEATURES`]).
    pub cpu_features: Vec<String>,
    /// `/dev/kvm` is readable and writable.
    pub kvm: bool,
    /// `docker info` works.
    pub docker: bool,
    /// The host is WSL.
    pub wsl: bool,
    /// Video adapters Windows lists (WSL hosts only).
    pub windows_gpus: Vec<String>,
}

/// A cached [`StaticFacts`] and when it was probed (Unix seconds).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Cached {
    /// When the facts were probed.
    pub at: u64,
    /// The facts.
    pub facts: StaticFacts,
}

/// Everything known about a host for scheduling and display.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Facts {
    /// Total RAM in bytes (live).
    pub mem_total: Option<u64>,
    /// Available RAM in bytes (live).
    pub mem_avail: Option<u64>,
    /// The cached static facts, if any have been probed.
    pub hw: Option<StaticFacts>,
    /// Seconds since `hw` was probed.
    pub hw_age: Option<u64>,
}

impl Facts {
    /// The host's GPUs (none when static facts are unknown).
    pub fn gpus(&self) -> &[Gpu] {
        self.hw.as_ref().map_or(&[], |h| &h.gpus)
    }
}

/// Split `key=value` lines into a map.
pub fn kv(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect()
}

fn bytes(map: &BTreeMap<String, String>, key: &str) -> Option<u64> {
    map.get(key)
        .and_then(|v| v.parse().ok())
        .filter(|b| (1..=MAX_BYTES).contains(b))
}

/// Parse the live facts (RAM) from a probe's key/value map.
pub fn parse_live(map: &BTreeMap<String, String>) -> Facts {
    let mem_total = bytes(map, "mem_total");
    // Available can never exceed the total.
    let mem_avail = bytes(map, "mem_avail").filter(|a| mem_total.is_none_or(|t| *a <= t));
    Facts {
        mem_total,
        mem_avail,
        hw: None,
        hw_age: None,
    }
}

/// One `gpu.N=vendor|name|mem_mib|driver|cuda` value.
fn parse_gpu(value: &str) -> Option<Gpu> {
    let mut f = value.split('|');
    let vendor = f.next()?;
    if !matches!(vendor, "nvidia" | "amd") {
        return None;
    }
    let name = crate::render::clean(f.next()?.trim());
    if name.is_empty() || name.len() > 120 {
        return None;
    }
    let mem_mib = f
        .next()
        .and_then(|m| m.trim().parse::<u64>().ok())
        .filter(|m| (1..=MAX_GPU_MIB).contains(m));
    let version = |s: Option<&str>| {
        s.map(str::trim)
            .filter(|v| {
                !v.is_empty()
                    && v.len() <= 32
                    && v.chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
            })
            .map(str::to_owned)
    };
    Some(Gpu {
        vendor: vendor.to_owned(),
        name,
        mem_mib,
        driver: version(f.next()),
        cuda: version(f.next()),
    })
}

/// Parse the static facts, or `None` when the host did not send them
/// (`static=1` marks that it did).
pub fn parse_static(map: &BTreeMap<String, String>) -> Option<StaticFacts> {
    if map.get("static").map(String::as_str) != Some("1") {
        return None;
    }
    let gpus = (0..MAX_GPUS)
        .map_while(|i| map.get(&format!("gpu.{i}")))
        .filter_map(|v| parse_gpu(v))
        .collect();
    let cpu_features = map
        .get("cpu_flags")
        .map(|v| {
            v.split(',')
                .filter(|f| CPU_FEATURES.contains(f))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let flag = |k: &str| map.get(k).map(String::as_str) == Some("1");
    let windows_gpus = map
        .get("winvideo")
        .map(|v| {
            v.split(';')
                .map(|n| crate::render::clean(n.trim()))
                .filter(|n| !n.is_empty() && n.len() <= 120)
                .take(8)
                .collect()
        })
        .unwrap_or_default();
    Some(StaticFacts {
        gpus,
        cpu_features,
        kvm: flag("kvm"),
        docker: flag("docker"),
        wsl: flag("wsl"),
        windows_gpus,
    })
}

/// Whether the cached facts (probed at `at`) must be probed again at `now`.
pub fn stale(cached: Option<&Cached>, now: u64) -> bool {
    cached.is_none_or(|c| now.saturating_sub(c.at) >= STATIC_MAX_AGE)
}

/// RAM as `avail/total GiB`, or `-`.
pub fn ram_summary(f: &Facts) -> String {
    #[allow(clippy::cast_precision_loss)] // display only
    let gib = |b: u64| b as f64 / (1024.0 * 1024.0 * 1024.0);
    match (f.mem_avail, f.mem_total) {
        (Some(a), Some(t)) => format!("{:.1}/{:.1} GiB", gib(a), gib(t)),
        (None, Some(t)) => format!("{:.1} GiB", gib(t)),
        _ => "-".to_owned(),
    }
}

/// The notable features of a host, space separated, or `-`.
pub fn feature_summary(hw: Option<&StaticFacts>) -> String {
    let Some(h) = hw else {
        return "-".to_owned();
    };
    let mut v: Vec<&str> = h.cpu_features.iter().map(String::as_str).collect();
    if h.kvm {
        v.push("kvm");
    }
    if h.docker {
        v.push("docker");
    }
    if v.is_empty() {
        "-".to_owned()
    } else {
        v.join(" ")
    }
}

/// How long ago facts were probed, for display.
pub fn age_summary(age: Option<u64>) -> String {
    match age {
        None => "-".to_owned(),
        Some(s) if s < 90 => "live".to_owned(),
        Some(s) if s < 5400 => format!("{}m ago", s / 60),
        Some(s) => format!("{}h ago", s / 3600),
    }
}

/// Whether Windows lists a discrete GPU that WSL does not see.
pub fn gpu_invisible_to_wsl(hw: &StaticFacts) -> Option<&str> {
    if !hw.wsl || !hw.gpus.is_empty() {
        return None;
    }
    hw.windows_gpus.iter().map(String::as_str).find(|n| {
        let l = n.to_ascii_lowercase();
        l.contains("nvidia") || l.contains("radeon") || l.contains("amd")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // frob:tests crates/goway/src/facts.rs::parse_static
    #[test]
    fn parses_gpus_features_and_flags() {
        let m = kv(
            "static=1\ngpu.0=nvidia|RTX 4090|24564|555.42|12.5\ngpu.1=amd|Radeon|0||\ncpu_flags=avx2,sse,neon\nkvm=1\ndocker=0\nwsl=1\nwinvideo=NVIDIA RTX;Intel UHD\n",
        );
        let s = parse_static(&m).unwrap();
        assert_eq!(s.gpus.len(), 2);
        assert_eq!(s.gpus[0].mem_mib, Some(24564));
        assert_eq!(s.gpus[0].cuda.as_deref(), Some("12.5"));
        assert_eq!(s.gpus[1].mem_mib, None);
        assert_eq!(s.cpu_features, ["avx2", "neon"]);
        assert!(s.kvm && !s.docker && s.wsl);
        assert_eq!(s.windows_gpus.len(), 2);
        assert_eq!(s.gpus[0].summary(), "RTX 4090 24 GiB drv 555.42 cuda 12.5");
    }

    #[test]
    fn absent_static_marker_means_unknown() {
        assert!(parse_static(&kv("kvm=1\n")).is_none());
    }

    #[test]
    fn hostile_values_are_bounded() {
        let m = kv(
            "static=1\ngpu.0=intel|X|1||\ngpu.1=nvidia|A\u{1b}[2J|99999999999999|bad ver;rm|1\nmem_total=0\nmem_avail=5\n",
        );
        let s = parse_static(&m).unwrap();
        // gpu.0 is rejected (unknown vendor) so gpu.1 is the only entry.
        assert_eq!(s.gpus.len(), 1);
        assert_eq!(s.gpus[0].name, "A?[2J");
        assert_eq!(s.gpus[0].mem_mib, None);
        assert_eq!(s.gpus[0].driver, None);
        let l = parse_live(&m);
        assert_eq!((l.mem_total, l.mem_avail), (None, Some(5)));
        let l = parse_live(&kv("mem_total=10\nmem_avail=20\n"));
        assert_eq!(l.mem_avail, None, "available above total is nonsense");
    }

    #[test]
    fn staleness_is_daily() {
        let c = Cached {
            at: 1000,
            facts: StaticFacts::default(),
        };
        assert!(stale(None, 1000));
        assert!(!stale(Some(&c), 1000 + STATIC_MAX_AGE - 1));
        assert!(stale(Some(&c), 1000 + STATIC_MAX_AGE));
    }

    // frob:tests crates/goway/src/facts.rs::gpu_invisible_to_wsl
    #[test]
    fn a_windows_gpu_wsl_cannot_see_is_flagged() {
        let mut h = StaticFacts {
            wsl: true,
            windows_gpus: vec!["NVIDIA GeForce RTX 3060".into(), "Intel UHD".into()],
            ..StaticFacts::default()
        };
        assert_eq!(gpu_invisible_to_wsl(&h), Some("NVIDIA GeForce RTX 3060"));
        h.windows_gpus = vec!["Intel UHD".into()];
        assert_eq!(gpu_invisible_to_wsl(&h), None);
        h.windows_gpus = vec!["NVIDIA".into()];
        h.gpus.push(Gpu::default());
        assert_eq!(gpu_invisible_to_wsl(&h), None);
    }

    #[test]
    fn summaries() {
        let f = parse_live(&kv("mem_total=3221225472\nmem_avail=1073741824\n"));
        assert_eq!(ram_summary(&f), "1.0/3.0 GiB");
        assert_eq!(ram_summary(&Facts::default()), "-");
        assert_eq!(feature_summary(None), "-");
        assert_eq!(age_summary(Some(7200)), "2h ago");
        assert_eq!(age_summary(Some(10)), "live");
    }
}
