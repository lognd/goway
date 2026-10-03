//! Hardware requirements and preferences: `goway run --needs` (hard) and
//! `--prefers` (soft).
//!
//! A term is `key`, `key=value` or `key>=N[K|M|G|T]` (sizes are binary:
//! `16G` is 16 GiB). The valid keys are listed in [`KEYS`]; an unknown key
//! is an error that names them. A host is judged against facts it reported
//! about itself ([`crate::pool::Probe`], [`crate::facts`]) plus the labels
//! of its config entry. Needs exclude hosts; preferences only improve the
//! score of the hosts that meet them. goway never infers needs from a
//! project's dependencies.

use std::fmt;

use serde::Serialize;

use crate::config::HostConfig;
use crate::error::{Error, Result};
use crate::facts::{Facts, Gpu};
use crate::pool::Probe;

/// Every key a term can use, with its form, for error messages and docs.
pub const KEYS: [&str; 12] = [
    "gpu",
    "gpu=cuda",
    "gpu=rocm",
    "gpu-mem>=SIZE",
    "cuda>=VERSION",
    "mem>=SIZE",
    "cores>=N",
    "arch=NAME",
    "os=NAME",
    "cpu=FEATURE",
    "kvm",
    "docker",
];
/// The keys that are not in [`KEYS`]'s first twelve entries (`disk`, `label`).
const MORE_KEYS: [&str; 2] = ["disk>=SIZE", "label=NAME"];

/// The largest size a term may name: 2^60 bytes.
const MAX_BYTES: f64 = 1_152_921_504_606_846_976.0;

/// CPU features a term may ask for (what the probe reports).
const CPU_FEATURES: [&str; 3] = ["avx2", "avx512f", "neon"];

/// Which GPUs a `gpu` term accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuKind {
    /// Any GPU.
    Any,
    /// NVIDIA (CUDA).
    Cuda,
    /// AMD (`ROCm`).
    Rocm,
}

/// One requirement or preference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Term {
    /// `gpu`, `gpu=cuda`, `gpu=rocm`.
    Gpu(GpuKind),
    /// `gpu-mem>=SIZE`: some single GPU has at least this much memory (bytes).
    GpuMem(u64),
    /// `cuda>=V`: the CUDA version, as numeric components.
    Cuda(Vec<u32>),
    /// `mem>=SIZE`: total RAM (bytes).
    Mem(u64),
    /// `cores>=N`.
    Cores(u64),
    /// `arch=NAME`, normalised (`x86_64` or `aarch64`).
    Arch(String),
    /// `os=NAME`, lower case.
    Os(String),
    /// `cpu=FEATURE`.
    Cpu(String),
    /// `kvm`: `/dev/kvm` is usable.
    Kvm,
    /// `docker`: `docker info` works.
    Docker,
    /// `disk>=SIZE`: free disk under goway's root (bytes).
    Disk(u64),
    /// `label=NAME`: the host's config lists this label.
    Label(String),
}

fn all_keys() -> String {
    KEYS.iter()
        .chain(MORE_KEYS.iter())
        .copied()
        .collect::<Vec<_>>()
        .join(", ")
}

fn bad(term: &str, why: impl fmt::Display) -> Error {
    Error::Usage(format!(
        "cannot use requirement `{term}`: {why}\n  valid terms: {}",
        all_keys()
    ))
}

/// Parse `N[K|M|G|T]` (binary multiples, optional `i`/`B` suffix) into bytes.
pub fn parse_size(text: &str) -> Option<u64> {
    let t = text.trim();
    let upper = t.to_ascii_uppercase();
    let body = upper
        .strip_suffix("IB")
        .or_else(|| upper.strip_suffix('B'))
        .unwrap_or(&upper);
    let (digits, shift) = match body.chars().last()? {
        'K' => (&body[..body.len() - 1], 10),
        'M' => (&body[..body.len() - 1], 20),
        'G' => (&body[..body.len() - 1], 30),
        'T' => (&body[..body.len() - 1], 40),
        c if c.is_ascii_digit() => (body, 0),
        _ => return None,
    };
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return None;
    }
    let n: f64 = digits.parse().ok().filter(|n: &f64| n.is_finite())?;
    #[allow(clippy::cast_precision_loss)] // bounds only
    let bytes = n * (1u64 << shift) as f64;
    // Bound at 1 EiB so nothing overflows later.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    (0.0..=MAX_BYTES)
        .contains(&bytes)
        .then(|| bytes.round() as u64)
}

fn parse_version(text: &str) -> Option<Vec<u32>> {
    let parts: Option<Vec<u32>> = text.trim().split('.').map(|p| p.parse().ok()).collect();
    parts.filter(|p| (1..=3).contains(&p.len()))
}

fn norm_arch(s: &str) -> String {
    match s.to_ascii_lowercase().as_str() {
        "amd64" | "x86-64" | "x86_64" => "x86_64".to_owned(),
        "arm64" | "aarch64" => "aarch64".to_owned(),
        other => other.to_owned(),
    }
}

impl Term {
    /// Parse one term.
    ///
    /// # Errors
    ///
    /// [`Error::Usage`] naming the valid terms when `text` is not one.
    pub fn parse(text: &str) -> Result<Self> {
        let t = text.trim();
        if t.is_empty() {
            return Err(bad(text, "empty"));
        }
        let (key, op, value) = if let Some((k, v)) = t.split_once(">=") {
            (k.trim(), Some(">="), v.trim())
        } else if let Some((k, v)) = t.split_once('=') {
            (k.trim(), Some("="), v.trim())
        } else {
            (t, None, "")
        };
        let key = key.to_ascii_lowercase();
        let need_value = |what: &str| {
            if value.is_empty() {
                Err(bad(text, format!("`{key}` needs a value, like {what}")))
            } else {
                Ok(())
            }
        };
        match (key.as_str(), op) {
            ("gpu", None) => Ok(Self::Gpu(GpuKind::Any)),
            ("gpu", Some("=")) => match value.to_ascii_lowercase().as_str() {
                "cuda" | "nvidia" => Ok(Self::Gpu(GpuKind::Cuda)),
                "rocm" | "amd" => Ok(Self::Gpu(GpuKind::Rocm)),
                _ => Err(bad(text, "`gpu=` takes cuda or rocm")),
            },
            ("kvm", None) => Ok(Self::Kvm),
            ("docker", None) => Ok(Self::Docker),
            ("gpu-mem" | "mem" | "disk", Some(">=")) => {
                need_value("16G")?;
                let bytes = parse_size(value)
                    .filter(|b| *b > 0)
                    .ok_or_else(|| bad(text, format!("`{value}` is not a size like 8G")))?;
                Ok(match key.as_str() {
                    "gpu-mem" => Self::GpuMem(bytes),
                    "mem" => Self::Mem(bytes),
                    _ => Self::Disk(bytes),
                })
            }
            ("cuda", Some(">=")) => parse_version(value)
                .map(Self::Cuda)
                .ok_or_else(|| bad(text, format!("`{value}` is not a version like 12.1"))),
            ("cores", Some(">=")) => value
                .parse::<u64>()
                .ok()
                .filter(|n| (1..=1_000_000).contains(n))
                .map(Self::Cores)
                .ok_or_else(|| bad(text, format!("`{value}` is not a core count like 8"))),
            ("arch", Some("=")) => {
                need_value("x86_64")?;
                Ok(Self::Arch(norm_arch(value)))
            }
            ("os", Some("=")) => {
                need_value("linux")?;
                Ok(Self::Os(value.to_ascii_lowercase()))
            }
            ("cpu", Some("=")) => {
                let f = value.to_ascii_lowercase();
                let f = if f == "asimd" { "neon".to_owned() } else { f };
                if CPU_FEATURES.contains(&f.as_str()) {
                    Ok(Self::Cpu(f))
                } else {
                    Err(bad(
                        text,
                        format!("cpu features goway can check: {}", CPU_FEATURES.join(", ")),
                    ))
                }
            }
            ("label", Some("=")) => {
                need_value("gpu-box")?;
                if crate::config::valid_name(value) {
                    Ok(Self::Label(value.to_owned()))
                } else {
                    Err(bad(text, "labels are 1-63 letters, digits, `-` or `_`"))
                }
            }
            ("gpu" | "kvm" | "docker", Some(_)) => Err(bad(
                text,
                format!("`{key}` takes no `>=` value (only `gpu=cuda` and `gpu=rocm` exist)"),
            )),
            ("gpu-mem" | "mem" | "disk" | "cuda" | "cores", _) => {
                Err(bad(text, format!("`{key}` is a minimum: write {key}>=N")))
            }
            ("arch" | "os" | "cpu" | "label", _) => {
                Err(bad(text, format!("`{key}` needs `=`, like {key}=NAME")))
            }
            _ => Err(bad(text, format!("unknown key `{key}`"))),
        }
    }
}

fn gib(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)] // display only
    let g = bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    format!("{g:.1} GiB")
}

/// `bytes` with the largest unit that divides it exactly (so it parses back to the same value).
fn size_text(bytes: u64) -> String {
    for (unit, shift) in [("T", 40), ("G", 30), ("M", 20), ("K", 10)] {
        if bytes != 0 && bytes.is_multiple_of(1 << shift) {
            return format!("{}{unit}", bytes >> shift);
        }
    }
    bytes.to_string()
}

fn version_text(v: &[u32]) -> String {
    v.iter().map(u32::to_string).collect::<Vec<_>>().join(".")
}

impl fmt::Display for Term {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Gpu(GpuKind::Any) => write!(f, "gpu"),
            Self::Gpu(GpuKind::Cuda) => write!(f, "gpu=cuda"),
            Self::Gpu(GpuKind::Rocm) => write!(f, "gpu=rocm"),
            Self::GpuMem(b) => write!(f, "gpu-mem>={}", size_text(*b)),
            Self::Cuda(v) => write!(f, "cuda>={}", version_text(v)),
            Self::Mem(b) => write!(f, "mem>={}", size_text(*b)),
            Self::Cores(n) => write!(f, "cores>={n}"),
            Self::Arch(a) => write!(f, "arch={a}"),
            Self::Os(o) => write!(f, "os={o}"),
            Self::Cpu(c) => write!(f, "cpu={c}"),
            Self::Kvm => write!(f, "kvm"),
            Self::Docker => write!(f, "docker"),
            Self::Disk(b) => write!(f, "disk>={}", size_text(*b)),
            Self::Label(l) => write!(f, "label={l}"),
        }
    }
}

/// The verdict of one term for one host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Met; the fact that met it, for the report.
    Met(String),
    /// Not met; what the host has instead.
    Unmet(String),
}

fn unmet(s: impl Into<String>) -> Verdict {
    Verdict::Unmet(s.into())
}

fn gpu_matches(kind: GpuKind, g: &Gpu) -> bool {
    match kind {
        GpuKind::Any => true,
        GpuKind::Cuda => g.vendor == "nvidia",
        GpuKind::Rocm => g.vendor == "amd",
    }
}

fn cuda_version(g: &Gpu) -> Option<Vec<u32>> {
    g.cuda.as_deref().and_then(parse_version)
}

fn no_gpu(facts: &Facts) -> String {
    match &facts.hw {
        None => "GPU facts not probed yet".to_owned(),
        Some(h) => match crate::facts::gpu_invisible_to_wsl(h) {
            Some(name) => format!("no GPU visible (Windows has {name} but WSL cannot see it)"),
            None => "no GPU visible".to_owned(),
        },
    }
}

impl Term {
    /// Judge `host` (which reported `probe`) against this term.
    pub fn check(&self, host: &HostConfig, probe: &Probe) -> Verdict {
        let facts = &probe.facts;
        let gpus = facts.gpus();
        match self {
            Self::Gpu(kind) => match gpus.iter().find(|g| gpu_matches(*kind, g)) {
                Some(g) => Verdict::Met(g.summary()),
                None if gpus.is_empty() => unmet(no_gpu(facts)),
                None => unmet(format!("has {}, not {self}", gpus[0].summary())),
            },
            Self::GpuMem(min) => {
                let best = gpus
                    .iter()
                    .filter_map(|g| g.mem_mib.map(|m| (m, g)))
                    .max_by_key(|(m, _)| *m);
                match best {
                    Some((m, g)) if m << 20 >= *min => Verdict::Met(g.summary()),
                    Some((m, _)) => unmet(format!("largest GPU has {}", gib(m << 20))),
                    None if gpus.is_empty() => unmet(no_gpu(facts)),
                    None => unmet("GPU memory unknown"),
                }
            }
            Self::Cuda(min) => {
                let best = gpus
                    .iter()
                    .filter_map(|g| cuda_version(g).map(|v| (v, g)))
                    .max_by(|a, b| a.0.cmp(&b.0));
                match best {
                    Some((v, g)) if v >= *min => {
                        Verdict::Met(format!("CUDA {} ({})", version_text(&v), g.name))
                    }
                    Some((v, _)) => unmet(format!("has CUDA {}", version_text(&v))),
                    None if gpus.is_empty() => unmet(no_gpu(facts)),
                    None => unmet("no CUDA version reported"),
                }
            }
            Self::Mem(min) => match facts.mem_total {
                Some(t) if t >= *min => Verdict::Met(format!("{} RAM", gib(t))),
                Some(t) => unmet(format!("has {} RAM", gib(t))),
                None => unmet("RAM unknown"),
            },
            Self::Cores(min) => {
                if u64::from(probe.cores) >= *min {
                    Verdict::Met(format!("{} cores", probe.cores))
                } else {
                    unmet(format!("has {} cores", probe.cores))
                }
            }
            Self::Arch(a) => {
                if norm_arch(&probe.arch) == *a {
                    Verdict::Met(probe.arch.clone())
                } else {
                    unmet(format!("is {}", probe.arch))
                }
            }
            Self::Os(o) => match facts.os.as_deref() {
                Some(have) if have.eq_ignore_ascii_case(o) => Verdict::Met(have.to_owned()),
                Some(have) => unmet(format!("is {have}")),
                None => unmet("OS unknown"),
            },
            Self::Cpu(c) => match &facts.hw {
                Some(h) if h.cpu_features.contains(c) => Verdict::Met(c.clone()),
                Some(_) => unmet(format!("no {c}")),
                None => unmet("CPU features not probed yet"),
            },
            Self::Kvm => match &facts.hw {
                Some(h) if h.kvm => Verdict::Met("/dev/kvm usable".to_owned()),
                Some(_) => unmet("no usable /dev/kvm"),
                None => unmet("KVM not probed yet"),
            },
            Self::Docker => match &facts.hw {
                Some(h) if h.docker => Verdict::Met("docker works".to_owned()),
                Some(_) => unmet("docker does not work"),
                None => unmet("docker not probed yet"),
            },
            Self::Disk(min) => match probe.disk_free {
                Some(d) if d >= *min => Verdict::Met(format!("{} free", gib(d))),
                Some(d) => unmet(format!("has {} free", gib(d))),
                None => unmet("free disk unknown"),
            },
            Self::Label(l) => {
                if host.labels.iter().any(|h| h.eq_ignore_ascii_case(l)) {
                    Verdict::Met(format!("labelled {l}"))
                } else {
                    unmet("not labelled so")
                }
            }
        }
    }

    /// What makes two terms conflict when a command-line term overrides a
    /// rule's: the key (`mem`), plus the value for the keys that may repeat
    /// (`cpu=avx2`, `label=gpu-box`).
    pub fn key(&self) -> String {
        match self {
            Self::Gpu(_) => "gpu".to_owned(),
            Self::GpuMem(_) => "gpu-mem".to_owned(),
            Self::Cuda(_) => "cuda".to_owned(),
            Self::Mem(_) => "mem".to_owned(),
            Self::Cores(_) => "cores".to_owned(),
            Self::Arch(_) => "arch".to_owned(),
            Self::Os(_) => "os".to_owned(),
            Self::Cpu(c) => format!("cpu={c}"),
            Self::Kvm => "kvm".to_owned(),
            Self::Docker => "docker".to_owned(),
            Self::Disk(_) => "disk".to_owned(),
            Self::Label(l) => format!("label={l}"),
        }
    }

    /// Whether judging this term needs the free-disk probe.
    pub fn needs_disk(&self) -> bool {
        matches!(self, Self::Disk(_))
    }
}

/// One term that applied to a host and the fact that met it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Matched {
    /// `need` or `prefer`.
    pub kind: &'static str,
    /// The term as written (normalised).
    pub term: String,
    /// The host fact that met it.
    pub fact: String,
}

/// What a host looks like against a [`Selection`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Assessment {
    /// Needs the host fails: `term: what it has`.
    pub lacks: Vec<String>,
    /// Needs and preferences it meets.
    pub matched: Vec<Matched>,
    /// How many preferences it meets.
    pub preferences_met: usize,
}

impl Assessment {
    /// Whether every need is met.
    pub fn qualifies(&self) -> bool {
        self.lacks.is_empty()
    }
}

/// The requirements and preferences of one run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    /// Hard requirements.
    pub needs: Vec<Term>,
    /// Soft preferences.
    pub prefers: Vec<Term>,
}

impl Selection {
    /// Parse the `--needs` and `--prefers` values (each may hold comma-separated terms).
    ///
    /// # Errors
    ///
    /// [`Error::Usage`] for the first bad term.
    pub fn parse(needs: &[String], prefers: &[String]) -> Result<Self> {
        let terms = |list: &[String]| -> Result<Vec<Term>> {
            list.iter()
                .flat_map(|s| s.split(','))
                .filter(|s| !s.trim().is_empty())
                .map(Term::parse)
                .collect()
        };
        Ok(Self {
            needs: terms(needs)?,
            prefers: terms(prefers)?,
        })
    }

    /// Whether nothing is asked for.
    pub fn is_empty(&self) -> bool {
        self.needs.is_empty() && self.prefers.is_empty()
    }

    /// Whether the run needs a GPU (so it holds a GPU slot while it runs).
    pub fn needs_gpu(&self) -> bool {
        self.needs
            .iter()
            .any(|t| matches!(t, Term::Gpu(_) | Term::GpuMem(_) | Term::Cuda(_)))
    }

    /// Whether the hosts must be probed for free disk.
    pub fn wants_disk(&self) -> bool {
        self.needs.iter().chain(&self.prefers).any(Term::needs_disk)
    }

    /// Judge one host.
    pub fn assess(&self, host: &HostConfig, probe: &Probe) -> Assessment {
        let mut a = Assessment::default();
        for t in &self.needs {
            match t.check(host, probe) {
                Verdict::Met(fact) => a.matched.push(Matched {
                    kind: "need",
                    term: t.to_string(),
                    fact,
                }),
                Verdict::Unmet(why) => a.lacks.push(format!("{t}: {why}")),
            }
        }
        for t in &self.prefers {
            if let Verdict::Met(fact) = t.check(host, probe) {
                a.preferences_met += 1;
                a.matched.push(Matched {
                    kind: "prefer",
                    term: t.to_string(),
                    fact,
                });
            }
        }
        a
    }
}

/// A short `term (fact), ...` line for the "running on" note.
pub fn summary(matched: &[Matched]) -> String {
    matched
        .iter()
        .map(|m| format!("{} ({})", m.term, m.fact))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::StaticFacts;
    use proptest::prelude::*;

    fn host(labels: &[&str]) -> HostConfig {
        HostConfig {
            name: "helios".to_owned(),
            labels: labels.iter().map(|s| (*s).to_owned()).collect(),
            ..HostConfig::default()
        }
    }

    fn probe() -> Probe {
        let gib = 1u64 << 30;
        Probe {
            arch: "x86_64".to_owned(),
            hostname: "h".to_owned(),
            cores: 16,
            load: [0.0; 3],
            jobs: 0,
            disk_used: None,
            disk_free: Some(100 * gib),
            facts: Facts {
                mem_total: Some(32 * gib),
                mem_avail: Some(30 * gib),
                os: Some("linux".to_owned()),
                hw: Some(StaticFacts {
                    gpus: vec![Gpu {
                        vendor: "nvidia".to_owned(),
                        name: "RTX 4090".to_owned(),
                        mem_mib: Some(24576),
                        driver: Some("555.1".to_owned()),
                        cuda: Some("12.5".to_owned()),
                    }],
                    cpu_features: vec!["avx2".to_owned()],
                    kvm: true,
                    docker: false,
                    ..StaticFacts::default()
                }),
                hw_age: Some(0),
            },
        }
    }

    fn sel(needs: &str, prefers: &str) -> Selection {
        Selection::parse(&[needs.to_owned()], &[prefers.to_owned()]).unwrap()
    }

    // frob:tests crates/goway/src/needs.rs::Term
    #[test]
    fn every_documented_term_parses_and_round_trips() {
        for t in [
            "gpu",
            "gpu=cuda",
            "gpu=rocm",
            "gpu-mem>=8G",
            "cuda>=12.1",
            "mem>=16G",
            "cores>=8",
            "arch=x86_64",
            "os=linux",
            "cpu=avx512f",
            "kvm",
            "docker",
            "disk>=50G",
            "label=gpu-box",
        ] {
            let term = Term::parse(t).unwrap();
            assert_eq!(term.to_string(), t, "{t}");
            assert_eq!(Term::parse(&term.to_string()).unwrap(), term);
        }
        assert_eq!(Term::parse("mem>=16GiB").unwrap(), Term::Mem(16 << 30));
        assert_eq!(Term::parse("MEM>=512m").unwrap(), Term::Mem(512 << 20));
        assert_eq!(
            Term::parse("arch=amd64").unwrap(),
            Term::Arch("x86_64".to_owned())
        );
        assert_eq!(
            Term::parse("cpu=asimd").unwrap(),
            Term::Cpu("neon".to_owned())
        );
    }

    // frob:tests crates/goway/src/needs.rs::Term
    #[test]
    fn bad_terms_are_errors_that_name_the_valid_ones() {
        for t in [
            "",
            "gpus",
            "ram>=4G",
            "mem",
            "mem=16G",
            "mem>=",
            "mem>=lots",
            "mem>=0",
            "gpu>=1",
            "gpu=intel",
            "cuda>=twelve",
            "cuda>=1.2.3.4",
            "cores>=0",
            "cores>=-1",
            "kvm=1",
            "cpu=sse2",
            "label=a b",
            "arch=",
            "os>=linux",
            "docker>=1",
            "disk>=1X",
        ] {
            let err = Term::parse(t).unwrap_err().to_string();
            assert!(err.contains("valid terms"), "{t}: {err}");
            assert!(
                err.contains("gpu-mem>=SIZE") && err.contains("label=NAME"),
                "{t}: {err}"
            );
        }
        let err = Term::parse("gpus").unwrap_err().to_string();
        assert!(err.contains("unknown key `gpus`"), "{err}");
    }

    // frob:tests crates/goway/src/needs.rs::Selection
    #[test]
    fn needs_exclude_and_say_what_a_host_lacks() {
        let h = host(&["gpu-box"]);
        let p = probe();
        let ok = sel("gpu=cuda,gpu-mem>=8G,cuda>=12.1,mem>=16G,cores>=8,arch=x86_64,os=linux,cpu=avx2,kvm,disk>=50G,label=gpu-box", "")
            .assess(&h, &p);
        assert!(ok.qualifies(), "{:?}", ok.lacks);
        assert!(
            ok.matched
                .iter()
                .any(|m| m.term == "gpu-mem>=8G" && m.fact.contains("RTX 4090"))
        );
        let bad = sel("gpu=rocm,gpu-mem>=32G,cuda>=13,mem>=64G,cores>=32,arch=aarch64,os=windows,cpu=avx512f,docker,disk>=500G,label=other", "")
            .assess(&h, &p);
        assert_eq!(bad.lacks.len(), 11, "{:?}", bad.lacks);
        assert!(
            bad.lacks
                .iter()
                .any(|l| l.starts_with("gpu-mem>=32G: largest GPU has 24.0 GiB")),
            "{:?}",
            bad.lacks
        );
        assert!(
            bad.lacks
                .iter()
                .any(|l| l.starts_with("mem>=64G: has 32.0 GiB"))
        );
        // No GPU, or facts not probed.
        let mut none = probe();
        none.facts.hw.as_mut().unwrap().gpus.clear();
        assert!(sel("gpu", "").assess(&h, &none).lacks[0].contains("no GPU visible"));
        none.facts.hw = None;
        assert!(sel("kvm", "").assess(&h, &none).lacks[0].contains("not probed"));
    }

    // frob:tests crates/goway/src/needs.rs::Selection
    #[test]
    fn preferences_never_exclude() {
        let h = host(&[]);
        let a = sel("", "gpu=rocm,docker,kvm,mem>=8G").assess(&h, &probe());
        assert!(a.qualifies());
        assert_eq!(a.preferences_met, 2);
        assert!(a.matched.iter().all(|m| m.kind == "prefer"));
        assert!(sel("", "").is_empty());
        assert!(sel("disk>=1G", "").wants_disk());
        assert!(!sel("kvm", "mem>=1G").wants_disk());
    }

    // frob:tests crates/goway/src/needs.rs::parse_size
    #[test]
    fn sizes_are_binary_and_bounded() {
        assert_eq!(parse_size("8G"), Some(8 << 30));
        assert_eq!(parse_size("1.5g"), Some(3 << 29));
        assert_eq!(parse_size("2T"), Some(2 << 40));
        assert_eq!(parse_size("100"), Some(100));
        assert_eq!(parse_size("4KiB"), Some(4096));
        for bad in [
            "",
            "G",
            "-1G",
            "1e3G",
            "NaN",
            "inf",
            "99999999999999999T",
            "1.2.3G",
            "8GG",
        ] {
            assert_eq!(parse_size(bad), None, "{bad}");
        }
    }

    proptest! {
        // frob:tests crates/goway/src/needs.rs::Term
        #[test]
        fn parsing_never_panics_and_valid_terms_round_trip(s in "\\PC{0,40}") {
            if let Ok(term) = Term::parse(&s) {
                prop_assert_eq!(Term::parse(&term.to_string()).unwrap(), term);
            }
        }

        // frob:tests crates/goway/src/needs.rs::parse_size
        #[test]
        fn whole_sizes_scale_by_their_unit(n in 1u64..100_000, unit in 0usize..4) {
            let (suffix, shift) = [("K", 10), ("M", 20), ("G", 30), ("T", 40)][unit];
            prop_assert_eq!(parse_size(&format!("{n}{suffix}")), Some(n << shift));
        }

        // frob:tests crates/goway/src/needs.rs::Term
        #[test]
        fn a_host_with_more_always_meets_a_minimum(have in 1u64..1000, extra in 0u64..1000) {
            let mut p = probe();
            p.cores = u32::try_from(have + extra).unwrap();
            let term = Term::Cores(have);
            prop_assert!(matches!(term.check(&host(&[]), &p), Verdict::Met(_)));
            let term = Term::Cores(have + extra + 1);
            prop_assert!(matches!(term.check(&host(&[]), &p), Verdict::Unmet(_)));
        }
    }
}
