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
    std::thread::scope(|s| {
        for n in 0..20u64 {
            let (queue, cluster, config, order, peak, note) =
                (&queue, &cluster, &config, &order, &peak, &note);
            s.spawn(move || {
                std::thread::sleep(Duration::from_millis(20 * n));
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
                order.lock().unwrap().push(n);
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
