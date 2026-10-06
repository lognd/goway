//! Fitting a cargo or nextest run to a host's memory.
//!
//! Cargo's peak memory scales with its job count (parallel rustc and linker
//! processes), so a host that is too small for the repository at full
//! parallelism often fits it at fewer jobs. Instead of refusing the host or
//! letting the kernel kill the linker, goway estimates the peak at each job
//! count from the peaks every helper recorded per level (`mempeakj.*` in the
//! probe), picks the largest count whose peak plus margin fits what is
//! available, and sends it to the helper as `jobs:N`, which exports
//! `CARGO_BUILD_JOBS` and `NEXTEST_TEST_THREADS` unless the user set them.
//! Only when even one job does not fit is the host refused.
//!
//! The host's CPU is shared the same way: goway's runs already on the host (running or claimed,
//! `probe.jobs`) each hold a share of the cores, so a new adaptive run gets an equal share
//! (`cores / (runs + 1)`, at least one) as its job count, and the linker is bounded with it
//! (`MOLD_JOBS` concurrent mold processes, [`LINK_THREADS_VAR`] threads each), which keeps
//! several concurrent builds from oversubscribing the cores while one alone still gets them all.

use crate::footprint::{self, MemRoom};
use crate::pool::Probe;
use crate::status::human_bytes;

/// The share of a recorded peak assumed fixed (independent of the job count) when only one
/// level has been measured: a quarter.
const FIXED_PERCENT: u64 = 25;

/// The environment variable carrying the thread budget of one link (mold has no variable of its
/// own for its thread count; a linker wrapper or `-Wl,--threads=$GOWAY_LINK_THREADS` uses it).
pub const LINK_THREADS_VAR: &str = "GOWAY_LINK_THREADS";

/// How many cores one concurrent mold process is budgeted (so a share of 12 allows 3 at once).
const CORES_PER_LINK: u32 = 4;

/// How a run's parallelism is handled.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mode {
    /// Not a cargo run: nothing is lowered and no level is recorded.
    #[default]
    Off,
    /// A cargo run without an explicit job count: goway may lower it to fit.
    Adaptive,
    /// The user fixed the job count (`CARGO_BUILD_JOBS`, `-j`): never changed, only judged.
    Explicit(u32),
}

/// The value of `name` in `KEY=VALUE` environment words, if set.
fn env_value<'a>(env: &'a [String], name: &str) -> Option<&'a str> {
    env.iter()
        .rev()
        .find_map(|kv| kv.strip_prefix(name)?.strip_prefix('='))
}

/// A job count the user fixed.
enum Fixed {
    /// A readable count.
    Count(u32),
    /// Something that is not a number (`default`, a shell expansion).
    Unreadable,
}

impl Fixed {
    /// A parsed count, or the unreadable marker.
    fn of(text: &str) -> Self {
        text.parse().map_or(Self::Unreadable, Self::Count)
    }
}

/// The job count a command line fixes with `-j N`, `-jN`, `--jobs N` or `--jobs=N` (before `--`).
fn jobs_flag(command: &[String]) -> Option<Fixed> {
    let end = command
        .iter()
        .position(|a| a == "--")
        .unwrap_or(command.len());
    let args = &command[..end];
    args.iter().enumerate().find_map(|(i, a)| {
        if a == "-j" || a == "--jobs" {
            Some(args.get(i + 1).map_or(Fixed::Unreadable, |v| Fixed::of(v)))
        } else if let Some(v) = a.strip_prefix("--jobs=") {
            Some(Fixed::of(v))
        } else {
            // `-jN` only; other `-j...` words are not counts and are ignored.
            let v = a.strip_prefix("-j").filter(|v| !v.is_empty())?;
            v.parse().ok().map(Fixed::Count)
        }
    })
}

impl Mode {
    /// The mode for `command` run with `--env` words `env`: cargo runs adapt unless the user
    /// fixed the job count (any set `CARGO_BUILD_JOBS`, `-j`, `--jobs`), which is then honoured.
    pub fn for_run(command: &[String], env: &[String]) -> Self {
        if crate::runners::portable_runner(command) != Some("cargo") {
            return Self::Off;
        }
        match env_value(env, "CARGO_BUILD_JOBS")
            .map(Fixed::of)
            .or_else(|| jobs_flag(command))
        {
            // A fixed but unreadable count (`-j $(nproc)`, `default`) leaves the run alone.
            Some(Fixed::Unreadable) => Self::Off,
            Some(Fixed::Count(n)) => Self::Explicit(n.max(1)),
            None => Self::Adaptive,
        }
    }
}

/// The estimated peak in bytes of a run of `repo` at `jobs` parallel jobs on `probe`'s host,
/// from the per-level records (else the overall record, taken as a run at the host's core
/// count), or `None` when nothing is recorded.
pub fn estimate(probe: &Probe, repo: &str, jobs: u32) -> Option<u64> {
    let natural = probe.cores.max(1);
    let mut points: Vec<(u32, u64)> = probe
        .mem_levels
        .get(repo)
        .map(|m| m.iter().map(|(&j, &p)| (j, p)).collect())
        .unwrap_or_default();
    if points.is_empty() {
        points.push((natural, *probe.mem_peaks.get(repo)?));
    }
    points.sort_unstable();
    if let Some(&(_, exact)) = points.iter().find(|(j, _)| *j == jobs) {
        return Some(exact);
    }
    let seen_below = points
        .iter()
        .filter(|(j, _)| *j < jobs)
        .map(|(_, p)| *p)
        .max()
        .unwrap_or(0);
    let lower = points.iter().rev().find(|(j, _)| *j < jobs).copied();
    let upper = points.iter().find(|(j, _)| *j > jobs).copied();
    let model = match (lower, upper) {
        (Some((j0, p0)), Some((j1, p1))) => {
            // Interpolate between the neighbours.
            let span = u64::from(j1 - j0);
            let at = u64::from(jobs - j0);
            p0 + p1.saturating_sub(p0) * at / span
        }
        (Some((j0, p0)), None) => {
            // Above every record: the slope of the top two, else a quarter fixed, rest per job.
            let slope = points.len().checked_sub(2).map_or_else(
                || p0 * (100 - FIXED_PERCENT) / 100 / u64::from(j0),
                |i| {
                    let (a, pa) = points[i];
                    p0.saturating_sub(pa) / u64::from(j0 - a)
                },
            );
            p0 + slope * u64::from(jobs - j0)
        }
        (None, Some((j1, p1))) => {
            // Below every record: the slope of the bottom two, else a quarter fixed.
            let fixed = p1 * FIXED_PERCENT / 100;
            let slope = match points.get(1) {
                Some(&(b, pb)) if points[0].0 == j1 => pb.saturating_sub(p1) / u64::from(b - j1),
                _ => p1.saturating_sub(fixed) / u64::from(j1),
            };
            // Never above the record, never below its fixed part.
            p1.saturating_sub(slope * u64::from(j1 - jobs)).max(fixed)
        }
        (None, None) => return None,
    };
    Some(model.max(seen_below))
}

/// What goway decided for one run on one host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    /// The job count the run will use.
    pub jobs: u32,
    /// The job count it would use unmodified (the host's cores).
    pub natural: u32,
    /// Whether the plan changes the job count (the helper must export it).
    pub lowered: bool,
    /// The estimated peak at `jobs`, when anything is recorded.
    pub peak: Option<u64>,
    /// The memory to set aside for the run (peak plus margin), 0 when unknown.
    pub reserve: u64,
    /// Whether the plan fits the host's available memory (false: it is the one-job last resort).
    pub fits: bool,
    /// The cores this run may use: an equal share of the host with the runs already there.
    pub cpu_share: u32,
    /// Concurrent mold processes allowed (`MOLD_JOBS`).
    pub mold_jobs: u32,
    /// Threads each link may use ([`LINK_THREADS_VAR`]).
    pub link_threads: u32,
}

/// The cores a new run gets on `probe`'s host: an equal share with the goway runs already
/// running or claimed there, at least one.
pub fn cpu_share(probe: &Probe) -> u32 {
    let cores = probe.cores.max(1);
    cores.div_ceil(probe.jobs.saturating_add(1)).clamp(1, cores)
}

/// The linker budget for `jobs` build jobs: concurrent mold processes and threads per link,
/// whose product stays within `jobs`.
fn link_budget(jobs: u32) -> (u32, u32) {
    let procs = (jobs / CORES_PER_LINK).max(1);
    (procs, (jobs / procs).max(1))
}

/// The job counts to try for `mode` on `probe`'s host, best first (an adaptive run starts at
/// its CPU share).
fn candidates(mode: Mode, probe: &Probe) -> Vec<u32> {
    match mode {
        Mode::Off => Vec::new(),
        Mode::Explicit(n) => vec![n],
        Mode::Adaptive => (1..=cpu_share(probe)).rev().collect(),
    }
}

/// Decide the job count of a run of `repo` in `mode` on `probe`'s host: the largest count
/// whose estimated peak plus margin fits the available (and total) memory, else one job.
/// `None` for a run that is not adaptive or explicit.
pub fn plan(mode: Mode, probe: &Probe, repo: &str) -> Option<Plan> {
    let natural = probe.cores.max(1);
    let tries = candidates(mode, probe);
    let first = *tries.first()?;
    let fitting = |peak: u64| {
        let need = footprint::mem_required(peak);
        probe.facts.mem_total.is_none_or(|t| need <= t)
            && probe.facts.mem_avail.is_none_or(|a| need <= a)
    };
    let chosen = tries
        .iter()
        .copied()
        .find(|&j| estimate(probe, repo, j).is_none_or(&fitting))
        .map(|j| (j, true));
    let (jobs, fits) = chosen.unwrap_or((tries.last().copied().unwrap_or(first), false));
    let peak = estimate(probe, repo, jobs);
    let (mold_jobs, link_threads) = link_budget(jobs);
    Some(Plan {
        cpu_share: cpu_share(probe),
        mold_jobs,
        link_threads,
        jobs,
        natural,
        lowered: matches!(mode, Mode::Adaptive) && jobs < natural,
        peak,
        reserve: peak.map_or(0, footprint::mem_required),
        fits,
    })
}

/// How `probe`'s memory stands for a run of `repo` in `mode`: the plan's estimate judged like
/// a recorded peak (so a run that fits at some job count is [`MemRoom::Fits`]), or the
/// overall record when the run is not adaptive.
pub fn assess(mode: Mode, probe: &Probe, repo: &str) -> MemRoom {
    match plan(mode, probe, repo) {
        Some(Plan {
            peak: Some(peak), ..
        }) => footprint::judge_mem(probe, peak),
        _ => footprint::assess_mem(probe, repo),
    }
}

/// The words of an environment that sets the job count the plan chose, for a run on this
/// machine (a helper gets them as the `jobs:` word instead).
pub fn local_env(plan: &Plan, env: &[String]) -> Vec<String> {
    local_env_with(plan, env, |name| std::env::var_os(name).is_some())
}

/// [`local_env`] with `inherited` saying whether a variable is already set in the process.
fn local_env_with(plan: &Plan, env: &[String], inherited: impl Fn(&str) -> bool) -> Vec<String> {
    if !plan.lowered {
        return Vec::new();
    }
    [
        ("CARGO_BUILD_JOBS", plan.jobs),
        ("NEXTEST_TEST_THREADS", plan.jobs),
        ("MOLD_JOBS", plan.mold_jobs),
        (LINK_THREADS_VAR, plan.link_threads),
    ]
    .iter()
    .filter(|(name, _)| env_value(env, name).is_none() && !inherited(name))
    .map(|(name, n)| format!("{name}={n}"))
    .collect()
}

/// The `jobs:` option word: `jobs:N` (export it) or `jobs:N:rec` (only record the level).
pub fn word(mode: Mode, plan: &Plan) -> String {
    match mode {
        Mode::Adaptive => format!("jobs:{}", plan.jobs),
        _ => format!("jobs:{}:rec", plan.jobs),
    }
}

/// The `mold:PROCS:THREADS` option word of a lowered adaptive run (the helper exports
/// `MOLD_JOBS` and [`LINK_THREADS_VAR`] from it unless the user set them), if the plan has one.
pub fn mold_word(plan: &Plan) -> Option<String> {
    plan.lowered
        .then(|| format!("mold:{}:{}", plan.mold_jobs, plan.link_threads))
}

/// The note on stderr saying what was chosen and why, when the run was lowered or is the
/// one-job last resort.
pub fn note(host: &str, plan: &Plan, avail: Option<u64>) -> Option<String> {
    if !plan.lowered {
        return None;
    }
    let why = if plan.jobs < plan.cpu_share {
        avail.map_or_else(
            || "limited memory".to_owned(),
            |a| format!("{} free", human_bytes(a)),
        )
    } else {
        format!("{} cores shared with other runs", plan.natural)
    };
    let peak = plan.peak.map_or_else(String::new, |p| {
        format!(", estimated peak {}", human_bytes(p))
    });
    Some(format!(
        "{host} has {why}: running with {} of {} jobs (CARGO_BUILD_JOBS={n}, NEXTEST_TEST_THREADS={n}, MOLD_JOBS={}, {LINK_THREADS_VAR}={}{peak}){}",
        plan.jobs,
        plan.natural,
        plan.mold_jobs,
        plan.link_threads,
        if plan.fits {
            ""
        } else {
            "; even this may not fit, it is the lowest there is"
        },
        n = plan.jobs,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1 << 30;

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    fn probe(cores: u32, total: u64, avail: u64) -> Probe {
        let mut p = crate::pool::parse_probe(&format!(
            "arch=x86_64\nhostname=h\ncores={cores}\nload1=0\nload5=0\nload15=0\njobs=0\n"
        ))
        .unwrap();
        p.facts.mem_total = Some(total * GIB);
        p.facts.mem_avail = Some(avail * GIB);
        p
    }

    // frob:ticket 01M47SYH8F5YD6P5G2741CG5RF
    // frob:tests crates/goway/src/fit.rs::Mode.for_run
    #[test]
    fn only_cargo_runs_adapt_and_an_explicit_job_count_is_honoured() {
        let none: Vec<String> = Vec::new();
        assert_eq!(Mode::for_run(&words("cargo build"), &none), Mode::Adaptive);
        assert_eq!(
            Mode::for_run(&words("cargo nextest run --workspace"), &none),
            Mode::Adaptive
        );
        assert_eq!(Mode::for_run(&words("make -j4"), &none), Mode::Off);
        assert_eq!(
            Mode::for_run(&words("cargo build -j 3"), &none),
            Mode::Explicit(3)
        );
        assert_eq!(
            Mode::for_run(&words("cargo build --jobs=5"), &none),
            Mode::Explicit(5)
        );
        assert_eq!(
            Mode::for_run(&words("cargo build -j2"), &none),
            Mode::Explicit(2)
        );
        assert_eq!(
            Mode::for_run(&words("cargo build"), &["CARGO_BUILD_JOBS=6".to_owned()]),
            Mode::Explicit(6)
        );
        assert_eq!(
            Mode::for_run(
                &words("cargo build"),
                &["CARGO_BUILD_JOBS=default".to_owned()]
            ),
            Mode::Off,
            "an unreadable fixed count is left alone"
        );
        assert_eq!(
            Mode::for_run(&words("cargo nextest run -- -j9"), &none),
            Mode::Adaptive,
            "words after -- belong to the test binary"
        );
    }

    // frob:ticket 01M47SYH8F5YD6P5G2741CG5RF
    // frob:tests crates/goway/src/fit.rs::estimate
    #[test]
    fn the_estimate_uses_the_levels_measured_and_scales_a_single_record() {
        let mut p = probe(8, 16, 16);
        assert_eq!(estimate(&p, "r", 4), None, "nothing recorded");
        // Only the overall record: a run at the host's 8 cores.
        p.mem_peaks.insert("r".to_owned(), 8 * GIB);
        assert_eq!(estimate(&p, "r", 8), Some(8 * GIB));
        let one = estimate(&p, "r", 1).unwrap();
        assert!((2 * GIB..4 * GIB).contains(&one), "{one}");
        // Levels measured win over the overall record.
        p.mem_levels
            .entry("r".to_owned())
            .or_default()
            .extend([(2, 3 * GIB), (8, 7 * GIB)]);
        assert_eq!(estimate(&p, "r", 2), Some(3 * GIB));
        assert_eq!(estimate(&p, "r", 5), Some(5 * GIB), "interpolated");
        assert!(estimate(&p, "r", 1).unwrap() <= 3 * GIB);
        assert!(estimate(&p, "r", 16).unwrap() > 7 * GIB, "extrapolated up");
    }

    // frob:ticket 01M47SYH8F5YD6P5G2741CG5RF
    // frob:tests crates/goway/src/fit.rs::plan
    #[test]
    fn a_host_too_small_for_full_parallelism_gets_the_largest_level_that_fits() {
        // The measured case: 7.6 GiB host, peak 7.0 GiB at full parallelism.
        let mut p = probe(8, 8, 7);
        p.mem_peaks.insert("r".to_owned(), 7 * GIB);
        assert!(matches!(
            footprint::assess_mem(&p, "r"),
            MemRoom::TooSmall { .. } | MemRoom::Alone { .. }
        ));
        let plan = plan_of(Mode::Adaptive, &p);
        assert!(plan.lowered && plan.fits, "{plan:?}");
        assert!((1..8).contains(&plan.jobs), "{plan:?}");
        assert!(plan.reserve <= 7 * GIB);
        assert_eq!(assess(Mode::Adaptive, &p, "r"), MemRoom::Fits);
        // One more job would not have fit.
        let next = estimate(&p, "r", plan.jobs + 1).unwrap();
        assert!(footprint::mem_required(next) > 7 * GIB);
        assert!(
            note("quasar", &plan, p.facts.mem_avail)
                .unwrap()
                .contains("CARGO_BUILD_JOBS")
        );
        assert_eq!(word(Mode::Adaptive, &plan), format!("jobs:{}", plan.jobs));
    }

    // frob:ticket 01M47SYH8F5YD6P5G2741CG5RF
    // frob:tests crates/goway/src/fit.rs::assess
    #[test]
    fn it_refuses_only_when_one_job_cannot_fit_and_never_lowers_an_explicit_count() {
        let mut p = probe(8, 3, 3);
        p.mem_levels
            .entry("r".to_owned())
            .or_default()
            .extend([(1, 4 * GIB), (8, 9 * GIB)]);
        let one = plan_of(Mode::Adaptive, &p);
        assert_eq!((one.jobs, one.fits), (1, false));
        assert!(matches!(
            assess(Mode::Adaptive, &p, "r"),
            MemRoom::TooSmall { .. }
        ));
        // An explicit count is judged as it is, never lowered.
        let mut roomy = probe(8, 16, 5);
        roomy.mem_peaks.insert("r".to_owned(), 8 * GIB);
        let fixed = plan_of(Mode::Explicit(8), &roomy);
        assert_eq!((fixed.jobs, fixed.lowered, fixed.fits), (8, false, false));
        assert_eq!(word(Mode::Explicit(8), &fixed), "jobs:8:rec");
        assert!(local_env(&fixed, &[]).is_empty());
        assert!(plan_of(Mode::Adaptive, &roomy).lowered);
        assert!(plan(Mode::Off, &roomy, "r").is_none());
    }

    fn plan_of(mode: Mode, p: &Probe) -> Plan {
        plan(mode, p, "r").unwrap()
    }

    // frob:ticket 01M47SYH8F5YD6P5G2741CG5RF
    // frob:tests crates/goway/src/fit.rs::local_env
    #[test]
    fn a_local_run_gets_the_lowered_variables_unless_the_user_set_them() {
        let lowered = Plan {
            jobs: 3,
            natural: 8,
            lowered: true,
            peak: None,
            reserve: 0,
            fits: true,
            cpu_share: 3,
            mold_jobs: 1,
            link_threads: 3,
        };
        let got = local_env_with(&lowered, &[], |_| false);
        assert!(got.contains(&"CARGO_BUILD_JOBS=3".to_owned()), "{got:?}");
        assert!(
            got.contains(&"NEXTEST_TEST_THREADS=3".to_owned()),
            "{got:?}"
        );
        let user = ["CARGO_BUILD_JOBS=2".to_owned()];
        let got = local_env_with(&lowered, &user, |_| false);
        assert!(
            !got.iter().any(|w| w.starts_with("CARGO_BUILD_JOBS")),
            "{got:?}"
        );
        assert!(
            got.contains(&"NEXTEST_TEST_THREADS=3".to_owned()),
            "{got:?}"
        );
        assert!(local_env_with(&lowered, &[], |_| true).is_empty());
    }

    // frob:ticket 01M48MPBRZBVGBP1JF5EW8738V
    // frob:tests crates/goway/src/fit.rs::plan
    #[test]
    fn concurrent_runs_share_the_cores_and_bound_the_linker() {
        let jobs_with = |others: u32| {
            let mut p = probe(12, 64, 64);
            p.jobs = others;
            plan_of(Mode::Adaptive, &p)
        };
        let (a, b, c) = (jobs_with(0), jobs_with(1), jobs_with(2));
        assert_eq!((a.jobs, b.jobs, c.jobs), (12, 6, 4));
        assert!(!a.lowered && b.lowered && c.lowered);
        assert_eq!(mold_word(&a), None, "a lone run is left alone");
        assert_eq!(mold_word(&b).as_deref(), Some("mold:1:6"));
        // Each run's own link budget stays within its share of the cores.
        for p in [&a, &b, &c] {
            assert!(p.mold_jobs * p.link_threads <= p.jobs, "{p:?}");
        }
        assert_eq!(jobs_with(40).jobs, 1, "never below one job");
        let text = note("helios", &b, None).unwrap();
        assert!(
            text.contains("shared with other runs") && text.contains("MOLD_JOBS=1"),
            "{text}"
        );
        // An explicit count is never lowered by the CPU share.
        let mut busy = probe(12, 64, 64);
        busy.jobs = 3;
        assert_eq!(plan_of(Mode::Explicit(12), &busy).jobs, 12);
    }

    // frob:ticket 01M48MPBRZBVGBP1JF5EW8738V
    // frob:tests crates/goway/src/fit.rs::local_env
    #[test]
    fn mold_variables_are_exported_unless_the_user_set_them() {
        let (mold_jobs, link_threads) = link_budget(12);
        assert_eq!((mold_jobs, link_threads), (3, 4));
        let plan = Plan {
            jobs: 12,
            natural: 24,
            lowered: true,
            peak: None,
            reserve: 0,
            fits: true,
            cpu_share: 12,
            mold_jobs,
            link_threads,
        };
        let got = local_env_with(&plan, &[], |_| false);
        assert!(got.contains(&"MOLD_JOBS=3".to_owned()), "{got:?}");
        assert!(got.contains(&format!("{LINK_THREADS_VAR}=4")), "{got:?}");
        let user = ["MOLD_JOBS=1".to_owned()];
        let got = local_env_with(&plan, &user, |_| false);
        assert!(!got.iter().any(|w| w.starts_with("MOLD_JOBS")), "{got:?}");
        let got = local_env_with(&plan, &[], |n| n == LINK_THREADS_VAR);
        assert!(
            !got.iter().any(|w| w.starts_with(LINK_THREADS_VAR)),
            "{got:?}"
        );
    }
}
