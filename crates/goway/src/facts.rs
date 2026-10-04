//! Host facts: what a host has (GPUs, RAM, CPU features, KVM, Docker).
//!
//! Live facts (RAM) come with every probe. Static facts (the rest) cost
//! extra work on the host (nvidia-smi, docker info), so they are cached in
//! state per host with a timestamp and refreshed daily or on request.
//! Everything a host reports about itself is parsed defensively with
//! bounds, like the scheduling probe.

pub mod clock;

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
#[allow(clippy::struct_excessive_bools)] // one bool per probed fact
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
    /// RAM of the whole laptop in bytes (WSL hosts with interop only).
    #[serde(default)]
    pub windows_ram: Option<u64>,
    /// Logical processors of the whole laptop (WSL hosts with interop only).
    #[serde(default)]
    pub windows_cores: Option<u32>,
    /// Swap WSL has, in bytes.
    #[serde(default)]
    pub swap_total: Option<u64>,
    /// `nvcc` (the CUDA toolkit) is installed.
    #[serde(default)]
    pub nvcc: bool,
    /// How WSL interop behaves on this host (WSL hosts only; `None` when not probed).
    #[serde(default)]
    pub interop: Option<Interop>,
}

/// Whether and with which Windows token WSL interop runs Windows programs from a WSL host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Interop {
    /// Windows programs cannot run from the distro (interop disabled): safe.
    Off,
    /// Windows programs run with a filtered, non-administrator token: safe.
    Limited,
    /// Windows programs run as a Windows administrator: every WSL user is one.
    Elevated,
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
    /// The operating system (`uname -s`, lower case): `linux`, `windows`.
    pub os: Option<String>,
    /// Total RAM in bytes (live).
    pub mem_total: Option<u64>,
    /// Available RAM in bytes (live).
    pub mem_avail: Option<u64>,
    /// The cached static facts, if any have been probed.
    pub hw: Option<StaticFacts>,
    /// Seconds since `hw` was probed.
    pub hw_age: Option<u64>,
    /// The host runs on battery (`None`: it cannot be told).
    pub on_battery: Option<bool>,
    /// Seconds since its user last used the keyboard or mouse (`None`: unknown).
    pub idle_secs: Option<u64>,
    /// Milliseconds the host's clock is ahead of this machine's (negative: behind); see [`clock`].
    pub clock_offset_ms: Option<i64>,
    /// Tool name to version line, for the tools a probe was asked about (`want.TOOL`).
    pub tools: BTreeMap<String, String>,
}

/// Longest idle time accepted (ten years): anything above is a broken answer.
const MAX_IDLE_SECS: u64 = 10 * 365 * 24 * 3600;

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
    let os = map
        .get("os")
        .map(|v| crate::render::clean(v).to_ascii_lowercase())
        .filter(|v| !v.is_empty() && v.len() <= 32);
    let on_battery = match map.get("power").map(String::as_str) {
        Some("battery") => Some(true),
        Some("ac") => Some(false),
        _ => None,
    };
    let idle_secs = map
        .get("idle_secs")
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|s| *s <= MAX_IDLE_SECS);
    Facts {
        os,
        mem_total,
        mem_avail,
        hw: None,
        hw_age: None,
        on_battery,
        idle_secs,
        clock_offset_ms: None,
        tools: map
            .iter()
            .filter_map(|(k, v)| Some((k.strip_prefix("want.")?, v)))
            .filter(|(_, v)| !v.is_empty())
            .map(|(k, v)| (k.to_owned(), crate::render::clean(v)))
            .collect(),
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
    // "<RAM bytes>;<logical cores>" from Windows; both must be sane to count.
    let windows_hw = map.get("winhw").and_then(|v| {
        let (ram, cores) = v.split_once(';')?;
        let ram: u64 = ram.trim().parse().ok()?;
        let cores: u32 = cores.trim().parse().ok()?;
        ((1..=1 << 50).contains(&ram) && (1..=4096).contains(&cores)).then_some((ram, cores))
    });
    Some(StaticFacts {
        gpus,
        cpu_features,
        kvm: flag("kvm"),
        docker: flag("docker"),
        wsl: flag("wsl"),
        windows_gpus,
        windows_ram: windows_hw.map(|h| h.0),
        windows_cores: windows_hw.map(|h| h.1),
        swap_total: map.get("swap_total").and_then(|v| v.trim().parse().ok()),
        nvcc: flag("nvcc"),
        interop: match map.get("interop").map(String::as_str) {
            Some("off") => Some(Interop::Off),
            Some("limited") => Some(Interop::Limited),
            Some("elevated") => Some(Interop::Elevated),
            _ => None,
        },
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

/// What goway suggests WSL should get on a machine, leaving Windows enough.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Suggestion {
    /// Memory for WSL in whole GiB.
    pub memory_gib: u64,
    /// Swap for WSL in whole GiB.
    pub swap_gib: u64,
    /// Processors for WSL.
    pub processors: u32,
}

impl Suggestion {
    /// The `goway-setup tune` command line that applies it.
    pub fn command(self) -> String {
        format!(
            "goway-setup.exe tune --memory {}GB --swap {}GB --processors {}",
            self.memory_gib, self.swap_gib, self.processors
        )
    }
}

/// The suggested WSL size for a machine with `total_ram` bytes and `cores` logical processors.
///
/// Windows keeps at least 4 GiB or 25% of the RAM, whichever is more (on a machine of 4 GiB or
/// less WSL gets half). Swap is half the memory, at most 8 GiB; Windows keeps a core on small
/// machines and two on large ones.
pub fn suggest(total_ram: u64, cores: u32) -> Suggestion {
    let reserve = (4 * GIB).max(total_ram / 4);
    let memory = if total_ram > reserve + GIB {
        total_ram - reserve
    } else {
        total_ram / 2
    };
    let memory_gib = (memory / GIB).max(1);
    let processors = match cores {
        0..=2 => cores.max(1),
        3..=7 => cores - 1,
        _ => cores - 2,
    };
    Suggestion {
        memory_gib,
        swap_gib: (memory_gib / 2).min(8),
        processors,
    }
}

/// How far below the suggestion WSL is: the settings worth changing, as plain words, or none.
///
/// A value counts as "far below" when it is under 60% of the suggested memory, 70% of the suggested processors or half the suggested swap,
/// which leaves WSL's own default (half the RAM, all cores) quiet on small machines.
pub fn shortfalls(
    wsl_ram: Option<u64>,
    wsl_swap: Option<u64>,
    wsl_cores: Option<u32>,
    windows_ram: u64,
    windows_cores: u32,
) -> Vec<String> {
    let want = suggest(windows_ram, windows_cores);
    let gib = |b: u64| {
        #[allow(clippy::cast_precision_loss)] // display only
        let v = b as f64 / GIB as f64;
        format!("{v:.1} GiB")
    };
    let mut out = Vec::new();
    if let Some(r) = wsl_ram
        && r * 10 < want.memory_gib * GIB * 6
    {
        out.push(format!(
            "memory: WSL has {} of the laptop's {}; suggested {} GiB",
            gib(r),
            gib(windows_ram),
            want.memory_gib
        ));
    }
    if let Some(s) = wsl_swap
        && s * 2 < want.swap_gib * GIB
    {
        out.push(format!(
            "swap: WSL has {}; suggested {} GiB",
            gib(s),
            want.swap_gib
        ));
    }
    if let Some(c) = wsl_cores
        && u64::from(c) * 10 < u64::from(want.processors) * 7
    {
        out.push(format!(
            "processors: WSL has {c} of the laptop's {windows_cores}; suggested {}",
            want.processors
        ));
    }
    out
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

/// The security warning for a WSL host whose interop runs Windows programs as administrator.
pub fn elevated_interop_warning(hw: &StaticFacts) -> Option<&'static str> {
    (hw.wsl && hw.interop == Some(Interop::Elevated)).then_some(ELEVATED_INTEROP_WARNING)
}

/// What an elevated WSL interop means and how to fix it (shared by doctor and status).
pub const ELEVATED_INTEROP_WARNING: &str = "WSL interop on this host runs Windows programs as a Windows administrator \
     (WSL was started by an elevated process, or by a boot task of an administrator account): \
     anyone who can log in to this WSL can act as a Windows administrator. Fix on Windows: run \
     `wsl --shutdown` from a normal, non-admin terminal and let the logon keepalive task restart \
     it limited, or disable interop with `[interop] enabled=false` in the distro's /etc/wsl.conf";

const GIB: u64 = 1024 * 1024 * 1024;

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

    // frob:tests crates/goway/src/facts.rs::parse_live
    #[test]
    fn owner_state_is_parsed_strictly_and_absent_means_unknown() {
        let l = parse_live(&kv("power=battery\nidle_secs=42\n"));
        assert_eq!((l.on_battery, l.idle_secs), (Some(true), Some(42)));
        let l = parse_live(&kv("power=ac\nidle_secs=0\n"));
        assert_eq!((l.on_battery, l.idle_secs), (Some(false), Some(0)));
        let l = parse_live(&kv("mem_total=10\n"));
        assert_eq!((l.on_battery, l.idle_secs), (None, None));
        let l = parse_live(&kv("power=maybe\nidle_secs=-3\n"));
        assert_eq!((l.on_battery, l.idle_secs), (None, None));
        let l = parse_live(&kv("idle_secs=99999999999999\n"));
        assert_eq!(l.idle_secs, None, "an absurd idle time is dropped");
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

    // frob:tests crates/goway/src/facts.rs::elevated_interop_warning
    #[test]
    fn interop_is_parsed_and_only_elevated_warns() {
        let hw = |v: &str| parse_static(&kv(&format!("static=1\nwsl=1\ninterop={v}\n"))).unwrap();
        assert_eq!(hw("elevated").interop, Some(Interop::Elevated));
        assert!(elevated_interop_warning(&hw("elevated")).is_some());
        assert_eq!(hw("off").interop, Some(Interop::Off));
        assert!(elevated_interop_warning(&hw("off")).is_none());
        assert!(elevated_interop_warning(&hw("limited")).is_none());
        assert_eq!(hw("junk").interop, None);
        assert!(elevated_interop_warning(&StaticFacts::default()).is_none());
    }

    // frob:tests crates/goway/src/facts.rs::suggest
    #[test]
    fn windows_keeps_four_gib_or_a_quarter_of_the_ram() {
        let g = 1u64 << 30;
        let s = suggest(16 * g, 16);
        assert_eq!((s.memory_gib, s.swap_gib, s.processors), (12, 6, 14));
        assert_eq!(
            suggest(64 * g, 8).memory_gib,
            48,
            "25% beats 4 GiB on big machines"
        );
        assert_eq!(suggest(8 * g, 4).memory_gib, 4);
        assert_eq!(
            suggest(4 * g, 2).memory_gib,
            2,
            "small machines split in half"
        );
        assert_eq!(suggest(128 * g, 64).swap_gib, 8, "swap is capped");
        assert_eq!(
            s.command(),
            "goway-setup.exe tune --memory 12GB --swap 6GB --processors 14"
        );
    }

    // frob:tests crates/goway/src/facts.rs::shortfalls
    // frob:tests crates/goway/src/facts.rs::parse_static
    #[test]
    fn a_wsl_far_below_the_laptop_is_listed_and_a_fair_share_is_not() {
        let g = 1u64 << 30;
        // A 16-core helper with 3.6 GiB of RAM and no swap, on a 16 GiB laptop.
        let low = shortfalls(Some(g * 36 / 10), Some(0), Some(16), 16 * g, 16);
        assert_eq!(low.len(), 2, "{low:?}");
        assert!(low[0].starts_with("memory:") && low[1].starts_with("swap:"));
        // WSL's own default (half the RAM, a quarter swap, every core) is close enough.
        assert!(shortfalls(Some(8 * g), Some(4 * g), Some(16), 16 * g, 16).is_empty());
        let cores = shortfalls(Some(12 * g), Some(6 * g), Some(4), 16 * g, 16);
        assert!(cores.len() == 1 && cores[0].starts_with("processors:"));
        let hw = |v: &str| {
            parse_static(&kv(&format!(
                "static=1\nwsl=1\nwinhw={v}\nswap_total=1073741824\nnvcc=1\n"
            )))
            .unwrap()
        };
        let ok = hw("17179869184;16");
        assert_eq!((ok.windows_ram, ok.windows_cores), (Some(16 * g), Some(16)));
        assert_eq!(ok.swap_total, Some(g));
        assert!(ok.nvcc);
        assert_eq!(hw(";").windows_ram, None);
        assert_eq!(hw("1;99999").windows_cores, None);
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
