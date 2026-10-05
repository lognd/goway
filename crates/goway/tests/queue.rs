//! Waves of runs queue instead of failing: a local first-come-first-served
//! queue, bounded by free memory and job slots, against fake hosts.
#![cfg(unix)]

use std::net::IpAddr;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use goway::config::{Config, HostConfig};
use goway::error::Error;
use goway::needs::Selection;
use goway::pool::{self, Wait};
use goway::queue::Queue;
use goway::resolve::{self, Lookup, Prober};
use goway::ssh::{Failure, KeyPolicy, Target};
use goway::state::State;

const GIB: u64 = 1 << 30;

struct NoLookup;
impl Lookup for NoLookup {
    fn system(&self, _: &str) -> Vec<IpAddr> {
        Vec::new()
    }
    fn windows(&self, _: &str) -> Vec<IpAddr> {
        Vec::new()
    }
}

/// Three fake hosts of 4 cores and `mem_free` free. Their probe shows the
/// jobs in `running` only when `visible` is set; a wave's runs that have
/// claimed a host but are not in its probe yet are what the queue must count.
struct Cluster {
    running: [AtomicU32; 3],
    probes: AtomicU32,
    mem_free: u64,
    visible: bool,
}

impl Cluster {
    fn new(mem_free: u64, visible: bool) -> Self {
        Self {
            visible,
            running: [AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)],
            probes: AtomicU32::new(0),
            mem_free,
        }
    }

    fn index(name: &str) -> usize {
        match name {
            "h0" => 0,
            "h1" => 1,
            _ => 2,
        }
    }
}

impl Prober for Cluster {
    fn probe(&self, target: &Target, _: KeyPolicy, _: &str) -> resolve::ProbeResult {
        self.probes.fetch_add(1, Ordering::SeqCst);
        let i = match target.address.as_str() {
            "10.0.0.1" => 0,
            "10.0.0.2" => 1,
            "10.0.0.3" => 2,
            _ => return Err((Failure::Unreachable, String::new())),
        };
        let jobs = if self.visible {
            self.running[i].load(Ordering::SeqCst)
        } else {
            0
        };
        let avail = self.mem_free.saturating_sub(u64::from(jobs) * GIB * 3 / 2);
        Ok(format!(
            "arch=x86_64\nhostname=h{i}\ncores=4\nload1=0\nload5=0\nload15=0\njobs={jobs}\nos=linux\nmem_total={}\nmem_avail={avail}\n",
            self.mem_free
        ))
    }
}

fn config() -> Config {
    let mut config = Config::default();
    for (i, name) in ["h0", "h1", "h2"].iter().enumerate() {
        config.hosts.push(HostConfig {
            name: (*name).to_owned(),
            address: Some(format!("10.0.0.{}", i + 1)),
            ..HostConfig::default()
        });
    }
    config
}

fn wait<'a>(queue: &'a Queue, limit: Duration, note: &'a (dyn Fn(&str) + Sync)) -> Wait<'a> {
    Wait {
        queue,
        limit,
        poll: Duration::from_millis(40),
        note,
    }
}

// frob:tests crates/goway/src/pool.rs::choose_queued
#[test]
fn twenty_runs_spread_over_three_hosts_in_arrival_order_within_the_memory_bound() {
    let dir = tempfile::tempdir().unwrap();
    let queue = Queue::at(dir.path().join("queue"));
    let cluster = Cluster::new(3 * GIB, false);
    let config = config();
    let order = Mutex::new(Vec::new());
    let peak = [AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)];
    let noted = AtomicU32::new(0);
    let note = |_: &str| {
        noted.fetch_add(1, Ordering::SeqCst);
    };
    // The arrival number each run takes just before it asks for a host: a loaded runner wakes
    // a sleeping thread late, so "arrival order" is what actually happened, not the sleep plan.
    let arrivals = AtomicU32::new(0);
    std::thread::scope(|s| {
        for n in 0..20u64 {
            let (queue, cluster, config, order, peak, note, arrivals) =
                (&queue, &cluster, &config, &order, &peak, &note, &arrivals);
            s.spawn(move || {
                std::thread::sleep(Duration::from_millis(60 * n));
                let arrival = u64::from(arrivals.fetch_add(1, Ordering::SeqCst));
                let mut state = State::default();
                let w = wait(queue, Duration::from_secs(60), note);
                let (host, _, _, claim) = pool::choose_queued(
                    config,
                    &Selection::default(),
                    &mut state,
                    Path::new(""),
                    &NoLookup,
                    cluster,
                    None,
                    &w,
                )
                .unwrap();
                let claim = claim.unwrap();
                order.lock().unwrap().push(arrival);
                let i = Cluster::index(&host.name);
                let now = cluster.running[i].fetch_add(1, Ordering::SeqCst) + 1;
                claim.started();
                peak[i].fetch_max(now, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(500));
                cluster.running[i].fetch_sub(1, Ordering::SeqCst);
                drop(claim);
            });
        }
    });
    let order = order.into_inner().unwrap();
    // Up to three hosts free a slot together, so their claimants may record
    // themselves in either order; nobody may be served more than two places
    // away from their turn.
    assert_eq!(order.len(), 20);
    for (at, n) in order.iter().enumerate() {
        assert!(
            at.abs_diff(usize::try_from(*n).unwrap()) <= 2,
            "first come, first served: {order:?}"
        );
    }
    for (i, p) in peak.iter().enumerate() {
        let p = p.load(Ordering::SeqCst);
        assert!(p <= 2, "host {i} ran {p} jobs at once with memory for 2");
    }
    let used = peak.iter().filter(|p| p.load(Ordering::SeqCst) > 0).count();
    assert_eq!(used, 3, "the wave spreads over every host");
    assert!(
        noted.load(Ordering::SeqCst) > 0,
        "waiters say why they wait"
    );
    // Only the first few waiters probe, so the probe rate does not grow with the wave.
    let probes = cluster.probes.load(Ordering::SeqCst);
    assert!(probes < 20 * 3 * 12, "{probes} probes");
}

// frob:tests crates/goway/src/pool.rs::choose_queued
#[test]
fn a_full_pool_fails_at_once_with_zero_wait_and_after_the_wait_otherwise() {
    let dir = tempfile::tempdir().unwrap();
    let queue = Queue::at(dir.path().join("queue"));
    let cluster = Cluster::new(3 * GIB, true);
    for r in &cluster.running {
        r.store(4, Ordering::SeqCst);
    }
    let config = config();
    let lines = Mutex::new(Vec::new());
    let note = |l: &str| lines.lock().unwrap().push(l.to_owned());
    let run = |limit: Duration| {
        let mut state = State::default();
        let w = wait(&queue, limit, &note);
        pool::choose_queued(
            &config,
            &Selection::default(),
            &mut state,
            Path::new(""),
            &NoLookup,
            &cluster,
            None,
            &w,
        )
    };
    let started = std::time::Instant::now();
    let err = run(Duration::ZERO).unwrap_err();
    assert!(matches!(err, Error::NoHost(_)), "{err}");
    assert_eq!(err.exit_code(), 125);
    assert!(started.elapsed() < Duration::from_secs(2));
    let err = run(Duration::from_secs(1)).unwrap_err();
    assert_eq!(err.exit_code(), 125);
    let text = err.to_string();
    assert!(text.contains("waited"), "{text}");
    assert!(text.contains("4 of 4 job slots in use"), "{text}");
    let lines = lines.into_inner().unwrap();
    assert!(
        lines.iter().any(|l| l.contains("queued at position 1")),
        "{lines:?}"
    );
}

// frob:tests crates/goway/src/pool.rs::choose_queued
#[test]
fn a_host_short_of_memory_for_one_more_job_is_waited_for_not_used() {
    let dir = tempfile::tempdir().unwrap();
    let queue = Queue::at(dir.path().join("queue"));
    // 1 GiB free is below the 1.5 GiB per-job reserve, so no host takes a job.
    let cluster = Cluster::new(GIB, true);
    let config = config();
    let note = |_: &str| {};
    let mut state = State::default();
    let w = wait(&queue, Duration::from_millis(300), &note);
    let err = pool::choose_queued(
        &config,
        &Selection::default(),
        &mut state,
        Path::new(""),
        &NoLookup,
        &cluster,
        None,
        &w,
    )
    .unwrap_err();
    assert_eq!(err.exit_code(), 125);
    assert!(err.to_string().contains("GiB free"), "{err}");
}

/// One fake 16-core host for the memory-peak tests.
struct Spec {
    total: u64,
    avail: u64,
    jobs: AtomicU32,
    peak: Option<u64>,
}

fn spec(total: u64, avail: u64, jobs: u32, peak: Option<u64>) -> Spec {
    Spec {
        total,
        avail,
        jobs: AtomicU32::new(jobs),
        peak,
    }
}

/// Hosts `h0`, `h1`... (addresses 10.0.0.1...) that report `specs`, with repository `r`'s peak when set.
struct Peaky(Vec<Spec>);

impl Prober for Peaky {
    fn probe(&self, target: &Target, _: KeyPolicy, _: &str) -> resolve::ProbeResult {
        let i = target
            .address
            .strip_prefix("10.0.0.")
            .and_then(|n| n.parse::<usize>().ok())
            .and_then(|n| n.checked_sub(1))
            .filter(|i| *i < self.0.len())
            .ok_or((Failure::Unreachable, String::new()))?;
        let s = &self.0[i];
        let peak = s.peak.map_or(String::new(), |p| format!("mempeak.r={p}\n"));
        Ok(format!(
            "arch=x86_64\nhostname=h{i}\ncores=16\nload1=0\nload5=0\nload15=0\njobs={}\nos=linux\nmem_total={}\nmem_avail={}\n{peak}",
            s.jobs.load(Ordering::SeqCst),
            s.total,
            s.avail
        ))
    }
}

/// `n` hosts and no per-job memory reserve, so only the repository's peak limits admission.
fn peak_config(n: usize) -> Config {
    let mut config = config();
    config.hosts.truncate(n);
    "0".clone_into(&mut config.defaults.job_mem);
    config
}

fn repo() -> Selection {
    Selection {
        repo_id: Some("r".to_owned()),
        ..Selection::default()
    }
}

fn ignoring() -> Selection {
    Selection {
        ignore_footprint: true,
        ..repo()
    }
}

type Chosen = goway::error::Result<(
    HostConfig,
    resolve::Found,
    goway::pool::Probe,
    Option<goway::queue::Claim>,
)>;

fn choose(
    queue: &Queue,
    config: &Config,
    prober: &Peaky,
    selection: &Selection,
    limit: Duration,
    notes: &Mutex<Vec<String>>,
) -> Chosen {
    let note = |l: &str| notes.lock().unwrap().push(l.to_owned());
    let mut state = State::default();
    let w = wait(queue, limit, &note);
    pool::choose_queued(
        config,
        selection,
        &mut state,
        Path::new(""),
        &NoLookup,
        prober,
        None,
        &w,
    )
}

// frob:ticket 01M44Q40P0JX72QMP799SKK7DR
// frob:tests crates/goway/src/queue.rs::Waiter
#[test]
fn a_head_that_cannot_fit_is_passed_a_bounded_number_of_times_then_everyone_waits_for_it() {
    let dir = tempfile::tempdir().unwrap();
    let queue = Queue::at(dir.path().join("queue"));
    // One job keeps the host busy, so memory for the head's peak (7.1 GiB needed, 7 free) never frees.
    let prober = Peaky(vec![spec(
        8 * GIB,
        7 * GIB,
        1,
        Some(6 * GIB + GIB * 8 / 10),
    )]);
    let config = peak_config(1);
    let head = queue.enter().unwrap();
    head.set_eligible(&["h0".to_owned()]).unwrap();
    head.set_ready(&[]).unwrap();
    let notes = Mutex::new(Vec::new());
    let mut claims = Vec::new();
    for n in 0..goway::queue::MAX_OVERTAKEN {
        let started = std::time::Instant::now();
        let (host, _, _, claim) = choose(
            &queue,
            &config,
            &prober,
            &ignoring(),
            Duration::from_secs(5),
            &notes,
        )
        .unwrap_or_else(|e| panic!("entry {n} behind the unfit head: {e}"));
        assert_eq!(host.name, "h0");
        assert!(started.elapsed() < Duration::from_secs(3));
        claims.push(claim.unwrap());
    }
    // The head has been passed enough: the next one waits for it, even with the footprint check off.
    let err = choose(
        &queue,
        &config,
        &prober,
        &ignoring(),
        Duration::from_millis(600),
        &notes,
    )
    .unwrap_err();
    assert_eq!(err.exit_code(), 125);
    assert!(err.to_string().contains("reserved for runs ahead"), "{err}");
    // Once the head leaves, the line moves again.
    drop(head);
    choose(
        &queue,
        &config,
        &prober,
        &ignoring(),
        Duration::from_secs(5),
        &notes,
    )
    .unwrap();
}

// frob:ticket 01M44Q40P0JX72QMP799SKK7DR
// frob:tests crates/goway/src/pool.rs::decide_round
#[test]
fn an_idle_host_does_not_stay_blocked_behind_a_head_whose_peak_never_fits() {
    let dir = tempfile::tempdir().unwrap();
    let queue = Queue::at(dir.path().join("queue"));
    // Today's starvation: an idle host, a recorded peak just above what is free, and later
    // entries that skip the footprint check.
    let prober = Peaky(vec![spec(
        8 * GIB,
        6 * GIB + GIB * 9 / 10,
        0,
        Some(6 * GIB + GIB * 8 / 10),
    )]);
    let config = peak_config(1);
    let notes = Mutex::new(Vec::new());
    let started = std::time::Instant::now();
    let got = std::thread::scope(|s| {
        let head = s.spawn(|| {
            choose(
                &queue,
                &config,
                &prober,
                &repo(),
                Duration::from_secs(30),
                &notes,
            )
        });
        std::thread::sleep(Duration::from_millis(200));
        let later: Vec<_> = (0..2)
            .map(|_| {
                s.spawn(|| {
                    choose(
                        &queue,
                        &config,
                        &prober,
                        &ignoring(),
                        Duration::from_secs(30),
                        &notes,
                    )
                })
            })
            .collect();
        let mut got = vec![head.join().unwrap()];
        got.extend(later.into_iter().map(|h| h.join().unwrap()));
        got
    });
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "{:?}",
        started.elapsed()
    );
    assert!(
        got.iter().all(Result::is_ok),
        "{:?}",
        got.iter()
            .map(|g| g.as_ref().err().map(ToString::to_string))
            .collect::<Vec<_>>()
    );
}

// frob:ticket 01M44Q40P0JX72QMP799SKK7DR
// frob:tests crates/goway/src/footprint.rs::assess_mem
#[test]
fn a_repository_that_does_not_fit_in_free_memory_runs_alone_on_an_idle_host_with_a_warning() {
    let dir = tempfile::tempdir().unwrap();
    let queue = Queue::at(dir.path().join("queue"));
    let prober = Peaky(vec![spec(
        8 * GIB,
        6 * GIB + GIB * 9 / 10,
        0,
        Some(6 * GIB + GIB * 8 / 10),
    )]);
    let config = peak_config(1);
    let notes = Mutex::new(Vec::new());
    let (host, _, _, claim) = choose(
        &queue,
        &config,
        &prober,
        &repo(),
        Duration::from_secs(5),
        &notes,
    )
    .unwrap();
    assert_eq!(host.name, "h0");
    let notes_now = notes.lock().unwrap().clone();
    assert!(
        notes_now.iter().any(|l| l.contains("running alone")),
        "{notes_now:?}"
    );
    // While it runs, a second run of the repository waits for the memory.
    let err = choose(
        &queue,
        &config,
        &prober,
        &repo(),
        Duration::from_millis(600),
        &notes,
    )
    .unwrap_err();
    assert_eq!(err.exit_code(), 125);
    assert!(err.to_string().contains("memory free"), "{err}");
    drop(claim);
}

// frob:ticket 01M44Q40P0JX72QMP799SKK7DR
// frob:tests crates/goway/src/pool.rs::none_usable
#[test]
fn a_peak_above_the_hosts_total_memory_fails_at_once_naming_both_sizes() {
    let dir = tempfile::tempdir().unwrap();
    let queue = Queue::at(dir.path().join("queue"));
    let prober = Peaky(vec![spec(4 * GIB, 4 * GIB, 0, Some(6 * GIB))]);
    let config = peak_config(1);
    let notes = Mutex::new(Vec::new());
    let started = std::time::Instant::now();
    let err = choose(
        &queue,
        &config,
        &prober,
        &repo(),
        Duration::from_secs(60),
        &notes,
    )
    .unwrap_err();
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "waited out --wait"
    );
    assert_eq!(err.exit_code(), 125);
    let text = err.to_string();
    assert!(text.contains("4.0 GiB of memory in total"), "{text}");
    assert!(text.contains("peak is 6.0 GiB"), "{text}");
    assert!(text.contains("--ignore-footprint"), "{text}");
    // Skipping the check still works.
    choose(
        &queue,
        &config,
        &prober,
        &ignoring(),
        Duration::from_secs(5),
        &notes,
    )
    .unwrap();
}

// frob:ticket 01M44Q40P0JX72QMP799SKK7DR
// frob:tests crates/goway/src/footprint.rs::share_mem_peaks
#[test]
fn a_host_without_a_peak_is_judged_by_the_one_another_host_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let queue = Queue::at(dir.path().join("queue"));
    // h0 is first in line and has no record; h1 recorded 6 GiB, which h0 (4 GiB) cannot hold.
    let prober = Peaky(vec![
        spec(4 * GIB, 4 * GIB, 0, None),
        spec(16 * GIB, 16 * GIB, 0, Some(6 * GIB)),
    ]);
    let config = peak_config(2);
    let notes = Mutex::new(Vec::new());
    let (host, _, _, _claim) = choose(
        &queue,
        &config,
        &prober,
        &repo(),
        Duration::from_secs(5),
        &notes,
    )
    .unwrap();
    assert_eq!(host.name, "h1");
}

// frob:ticket 01M44Q40P0JX72QMP799SKK7DR
// frob:tests crates/goway/src/footprint.rs::assess_mem
#[test]
fn with_no_peak_anywhere_one_run_of_the_repository_is_admitted_per_host() {
    let dir = tempfile::tempdir().unwrap();
    let queue = Queue::at(dir.path().join("queue"));
    let prober = Peaky(vec![spec(8 * GIB, 8 * GIB, 0, None)]);
    let config = peak_config(1);
    let notes = Mutex::new(Vec::new());
    let (_, _, _, first) = choose(
        &queue,
        &config,
        &prober,
        &repo(),
        Duration::from_secs(5),
        &notes,
    )
    .unwrap();
    let err = choose(
        &queue,
        &config,
        &prober,
        &repo(),
        Duration::from_millis(600),
        &notes,
    )
    .unwrap_err();
    assert_eq!(err.exit_code(), 125);
    assert!(err.to_string().contains("measured alone"), "{err}");
    // A run that skips the checks is not held; once the first leaves and shows no job, nor are others.
    choose(
        &queue,
        &config,
        &prober,
        &ignoring(),
        Duration::from_secs(5),
        &notes,
    )
    .unwrap();
    drop(first);
}
