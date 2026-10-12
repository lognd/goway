//! What most likely happened when a run lost its connection.
//!
//! ssh exits 255 when its transport dies and a helper stops a job with
//! SIGTERM (exit 143) when its lifeline goes quiet, but neither says *why*.
//! This module gathers cheap evidence at failure time ([`Watch`] for the
//! local clock and route, [`parse_stderr`] and [`parse_probe`] for ssh's own
//! words) and ranks the likely causes with the pure [`classify`]. Nothing
//! here changes an exit code; the caller decides what to do with the
//! [`Verdict`] and prints [`Verdict::lines`].

use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

/// A wall-clock minus monotonic gap at least this long means the machine slept.
pub const SUSPEND_MIN: Duration = Duration::from_secs(20);

/// How much of the end of a stream is kept as evidence.
const TAIL_BYTES: usize = 4096;

/// The likely reason a run lost its connection, most specific first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cause {
    /// This machine suspended (lid closed, sleep) while the run streamed.
    LocalSuspended,
    /// This machine's default route or source address changed or vanished.
    LocalNetworkChanged,
    /// The helper no longer answers: asleep, off, off the network, sshd down.
    HelperUnreachable,
    /// The helper's watchdog stopped the job because the client looked gone.
    HelperStoppedJob,
    /// The ssh transport was reset while both ends are still up.
    TransportReset,
    /// ssh could not authenticate or the host key no longer matches.
    AuthOrHostKey,
    /// The command itself exited 255 or 143; nothing says the link failed.
    CommandExit,
}

impl Cause {
    /// A one-line human explanation of the cause.
    pub fn explanation(self) -> &'static str {
        match self {
            Self::LocalSuspended => {
                "this machine went to sleep while the job ran (closed lid or suspend), so the connection timed out"
            }
            Self::LocalNetworkChanged => {
                "this machine's network changed or went away while the job ran (new Wi-Fi, VPN, cable or no route)"
            }
            Self::HelperUnreachable => {
                "the helper stopped answering: it is asleep, off, rebooting, or off the network"
            }
            Self::HelperStoppedJob => {
                "the helper stopped the job because it considered this client gone (its lifeline went quiet or closed)"
            }
            Self::TransportReset => {
                "the ssh connection was reset or timed out in transit while both machines are still up"
            }
            Self::AuthOrHostKey => {
                "ssh could not log in: the key was refused or the helper's host key changed"
            }
            Self::CommandExit => {
                "the command itself most likely exited with this code; nothing shows the link failing"
            }
        }
    }

    /// A one-line suggested next step.
    pub fn next_step(self) -> &'static str {
        match self {
            Self::LocalSuspended => {
                "keep this machine awake for long runs, then run again; nothing keeps running on the helper"
            }
            Self::LocalNetworkChanged => {
                "check this machine's network, then run again; `goway status` shows when the helper answers"
            }
            Self::HelperUnreachable => {
                "wake the helper or check its power and network, then run again; `goway status` shows when it answers"
            }
            Self::HelperStoppedJob => {
                "run again; if this repeats, check the network between the machines (`goway doctor HOST`)"
            }
            Self::TransportReset => {
                "run again; if it repeats, check Wi-Fi quality, VPN or a firewall that drops idle connections"
            }
            Self::AuthOrHostKey => {
                "run `goway doctor HOST`; re-add the helper with `goway add HOST` if its key changed"
            }
            Self::CommandExit => {
                "read the command's own output above; goway passes its exit code through"
            }
        }
    }
}

/// How sure the classifier is of its best guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    /// Little or conflicting evidence; treat the guess as a hint.
    Low,
    /// Some evidence points here and little points elsewhere.
    Medium,
    /// Strong evidence and a clear margin.
    High,
}

impl Confidence {
    /// The word printed for this level.
    pub fn word(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

/// What changed on this machine's route to the network since the run began.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetChange {
    /// The same route and source address as at the start.
    Same,
    /// A different route or source address.
    Changed {
        /// The route at the start.
        from: String,
        /// The route now.
        to: String,
    },
    /// There is no route now.
    Lost,
    /// Could not be measured on this platform.
    Unknown,
}

/// What a trivial ssh call to the helper said after the failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    /// The helper answered.
    Answers,
    /// The connection attempt timed out.
    TimedOut,
    /// The helper refused the connection (host up, sshd not listening).
    Refused,
    /// No route to the host, the network is unreachable, or the name is gone.
    NoRoute,
    /// The helper answered but ssh authentication or the host key failed.
    Auth,
    /// ssh failed for a reason not recognised.
    Failed,
}

impl Probe {
    /// Whether the helper could not be reached at all.
    pub fn unreachable(self) -> bool {
        matches!(
            self,
            Self::TimedOut | Self::Refused | Self::NoRoute | Self::Failed
        )
    }

    fn describe(self) -> &'static str {
        match self {
            Self::Answers => "the helper answers a new ssh call",
            Self::TimedOut => "re-probe of the helper timed out",
            Self::Refused => "re-probe of the helper was refused (host up, sshd not listening?)",
            Self::NoRoute => "re-probe of the helper found no route to it",
            Self::Auth => "re-probe reached the helper but login or the host key failed",
            Self::Failed => "re-probe of the helper failed",
        }
    }
}

#[allow(clippy::struct_excessive_bools)]
/// Phrases found in the tail of ssh's (and the helper's) stderr.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StderrSignals {
    /// "Connection timed out" or "Timeout, server not responding".
    pub timed_out: bool,
    /// "Connection reset by peer", "Broken pipe", "closed by remote host".
    pub reset: bool,
    /// "No route to host" or "Network is unreachable".
    pub no_route: bool,
    /// "Connection refused".
    pub refused: bool,
    /// "Permission denied" or "Host key verification failed".
    pub auth: bool,
    /// The helper printed that it stopped this run ("stopped by the helper").
    pub helper_stop_note: bool,
}

impl StderrSignals {
    /// Whether any transport-level phrase was seen.
    pub fn transport(&self) -> bool {
        self.timed_out || self.reset || self.no_route || self.refused
    }
}

/// Find the known phrases in `text` (case-insensitive).
pub fn parse_stderr(text: &str) -> StderrSignals {
    let t = text.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| t.contains(n));
    StderrSignals {
        timed_out: has(&[
            "connection timed out",
            "timeout, server",
            "operation timed out",
        ]),
        reset: has(&[
            "connection reset by peer",
            "broken pipe",
            "closed by remote host",
            "connection closed by",
        ]),
        no_route: has(&["no route to host", "network is unreachable"]),
        refused: has(&["connection refused"]),
        auth: has(&[
            "permission denied",
            "host key verification failed",
            "host key for",
        ]),
        helper_stop_note: has(&["stopped by the helper"]),
    }
}

/// Turn the outcome of the re-probe (`ok`, its stderr) into a [`Probe`].
pub fn parse_probe(ok: bool, stderr: &str) -> Probe {
    if ok {
        return Probe::Answers;
    }
    let s = parse_stderr(stderr);
    let t = stderr.to_ascii_lowercase();
    if s.auth {
        Probe::Auth
    } else if s.refused {
        Probe::Refused
    } else if s.no_route || t.contains("could not resolve hostname") {
        Probe::NoRoute
    } else if s.timed_out {
        Probe::TimedOut
    } else {
        Probe::Failed
    }
}

/// Everything known when the connection was judged lost.
#[derive(Debug, Clone)]
pub struct Evidence {
    /// The exit code ssh or the helper produced (255 or 143).
    pub exit: u8,
    /// Wall-clock time that passed beyond monotonic time during the run.
    pub suspend_gap: Option<Duration>,
    /// The longest silence between two lifeline beats (expected: a few seconds).
    pub beat_gap: Option<Duration>,
    /// What this machine's route did since the start.
    pub net: NetChange,
    /// Phrases seen in the stderr tail.
    pub stderr: StderrSignals,
    /// The re-probe of the helper.
    pub probe: Probe,
    /// Whether the lifeline ssh ended while the job still streamed.
    pub lifeline_broken: bool,
    /// Time from the start of the run to the failure.
    pub elapsed: Duration,
    /// Time from the last output byte to the failure, when any byte arrived.
    pub since_output: Option<Duration>,
    /// Whether this client runs under WSL (the Windows side may be the real cause).
    pub wsl: bool,
}

impl Evidence {
    /// Evidence with nothing notable, for tests and as a starting point.
    pub fn quiet(exit: u8) -> Self {
        Self {
            exit,
            suspend_gap: None,
            beat_gap: None,
            net: NetChange::Same,
            stderr: StderrSignals::default(),
            probe: Probe::Answers,
            lifeline_broken: false,
            elapsed: Duration::from_secs(120),
            since_output: Some(Duration::from_secs(1)),
            wsl: false,
        }
    }

    fn slept(&self) -> Option<Duration> {
        [self.suspend_gap, self.beat_gap]
            .into_iter()
            .flatten()
            .filter(|g| *g >= SUSPEND_MIN)
            .max()
    }
}

/// The ranked outcome of [`classify`].
#[derive(Debug, Clone)]
pub struct Verdict {
    /// The most likely cause.
    pub cause: Cause,
    /// How sure the classifier is.
    pub confidence: Confidence,
    /// Every cause with evidence, best first, with its score.
    pub ranked: Vec<(Cause, u32)>,
    /// Short bullets of the evidence that was used.
    pub evidence: Vec<String>,
}

impl Verdict {
    /// The lines to show: best guess, next step, confidence, evidence bullets.
    pub fn lines(&self) -> Vec<String> {
        let mut out = vec![
            format!("most likely: {}", self.cause.explanation()),
            format!("next: {}", self.cause.next_step()),
        ];
        out.push(match self.confidence {
            Confidence::Low => "confidence: low (little evidence, treat this as a hint)".to_owned(),
            c => format!("confidence: {}", c.word()),
        });
        if let Some((second, _)) = self.ranked.get(1) {
            out.push(format!("also possible: {}", second.explanation()));
        }
        out.push("evidence:".to_owned());
        out.extend(self.evidence.iter().map(|e| format!("  - {e}")));
        out
    }

    /// The lines joined for an error message.
    pub fn text(&self) -> String {
        self.lines().join("\n")
    }
}

fn secs(d: Duration) -> String {
    format!("{}s", d.as_secs())
}

/// Every cause in tie-break order (the order of the [`Cause`] variants).
const ORDER: [Cause; 7] = [
    Cause::LocalSuspended,
    Cause::LocalNetworkChanged,
    Cause::HelperUnreachable,
    Cause::HelperStoppedJob,
    Cause::TransportReset,
    Cause::AuthOrHostKey,
    Cause::CommandExit,
];

/// Scores per cause plus the evidence bullets that produced them.
#[derive(Default)]
struct Scorer {
    score: [u32; 7],
    bullets: Vec<String>,
}

impl Scorer {
    fn add(&mut self, c: Cause, n: u32) {
        self.score[c as usize] += n;
    }

    fn note(&mut self, text: impl Into<String>) {
        self.bullets.push(text.into());
    }
}

/// Score what happened on this machine: suspend and network change.
fn score_local(ev: &Evidence, sc: &mut Scorer) {
    if let Some(gap) = ev.slept() {
        sc.add(Cause::LocalSuspended, 80);
        sc.note(format!(
            "this machine's clock jumped {} beyond elapsed run time (sleep/suspend)",
            secs(gap)
        ));
    } else if ev.suspend_gap.is_some() || ev.beat_gap.is_some() {
        sc.note("no local suspend seen (clock and heartbeat steady)");
    }
    match &ev.net {
        NetChange::Changed { from, to } => {
            sc.add(Cause::LocalNetworkChanged, 60);
            sc.note(format!("local route changed: {from} -> {to}"));
        }
        NetChange::Lost => {
            sc.add(Cause::LocalNetworkChanged, 70);
            sc.note("this machine has no route to the network now");
        }
        NetChange::Same => sc.note("local route unchanged since the start"),
        NetChange::Unknown => {}
    }
    if ev.wsl && matches!(ev.net, NetChange::Same | NetChange::Unknown) && ev.slept().is_none() {
        sc.note(
            "client is under WSL: Windows may have changed network or slept without WSL noticing",
        );
    }
}

/// Score the re-probe and ssh's own words.
fn score_link(ev: &Evidence, local_bad: bool, sc: &mut Scorer) {
    sc.note(ev.probe.describe());
    match ev.probe {
        Probe::Answers => {}
        Probe::Auth => sc.add(Cause::AuthOrHostKey, 60),
        p => {
            // A probe that fails right after this machine slept or moved
            // networks says little about the helper.
            let base = if p == Probe::Refused { 60 } else { 50 };
            sc.add(
                Cause::HelperUnreachable,
                if local_bad { base - 30 } else { base },
            );
        }
    }
    let s = ev.stderr;
    if s.auth {
        sc.add(Cause::AuthOrHostKey, 80);
        sc.note("ssh said permission denied or the host key failed");
    }
    if s.transport() {
        let seen: Vec<&str> = [
            (s.timed_out, "timed out"),
            (s.reset, "reset/broken pipe/closed"),
            (s.no_route, "no route"),
            (s.refused, "refused"),
        ]
        .into_iter()
        .filter_map(|(on, w)| on.then_some(w))
        .collect();
        sc.note(format!("ssh stderr mentions: {}", seen.join(", ")));
        if ev.exit == 255 {
            let clean = ev.probe == Probe::Answers && !local_bad;
            sc.add(Cause::TransportReset, if clean { 60 } else { 15 });
        }
    } else if ev.exit == 255
        && ev.probe == Probe::Answers
        && !local_bad
        && ev
            .since_output
            .is_some_and(|d| d >= Duration::from_secs(60))
    {
        // A silent drop with the helper up and this machine steady is a hint only.
        sc.note("no output for a minute or more before the drop");
    }
}

/// Score whether the helper's watchdog stopped the job.
fn score_watchdog(ev: &Evidence, sc: &mut Scorer) {
    if ev.stderr.helper_stop_note {
        sc.add(Cause::HelperStoppedJob, 80);
        sc.note("the helper printed that it stopped this run");
    }
    if ev.lifeline_broken {
        sc.add(
            Cause::HelperStoppedJob,
            if ev.exit == 143 { 75 } else { 30 },
        );
        sc.note("the lifeline connection broke while the job ran");
    }
    if ev.exit == 143 {
        sc.add(Cause::HelperStoppedJob, 15);
        sc.note("exit 143 is SIGTERM, which the helper's watchdog sends");
    }
}

/// Rank the likely causes of a lost connection from `ev`; pure.
pub fn classify(ev: &Evidence) -> Verdict {
    let mut sc = Scorer::default();
    sc.note(format!(
        "exit {} after {}{}",
        ev.exit,
        secs(ev.elapsed),
        ev.since_output.map_or(String::new(), |s| format!(
            ", last output {} before",
            secs(s)
        ))
    ));
    // Baseline: a bare 255 or 143 may be the command's own.
    sc.add(Cause::CommandExit, 20);
    score_local(ev, &mut sc);
    let local_bad =
        matches!(ev.net, NetChange::Changed { .. } | NetChange::Lost) || ev.slept().is_some();
    score_link(ev, local_bad, &mut sc);
    score_watchdog(ev, &mut sc);

    // Other evidence makes "the command's own exit" less credible.
    let others: u32 = ORDER[..6].iter().map(|c| sc.score[*c as usize]).sum();
    if others >= 40 {
        sc.score[Cause::CommandExit as usize] = 0;
    }
    let mut ranked: Vec<(Cause, u32)> = ORDER
        .iter()
        .map(|c| (*c, sc.score[*c as usize]))
        .filter(|(_, n)| *n > 0)
        .collect();
    ranked.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    let (cause, top) = ranked.first().copied().unwrap_or((Cause::CommandExit, 0));
    let margin = top - ranked.get(1).map_or(0, |r| r.1);
    let confidence = if top >= 70 && margin >= 20 {
        Confidence::High
    } else if top >= 45 && margin >= 10 {
        Confidence::Medium
    } else {
        Confidence::Low
    };
    tracing::debug!(?cause, ?confidence, ?ranked, "classified a lost connection");
    Verdict {
        cause,
        confidence,
        ranked,
        evidence: sc.bullets,
    }
}

/// A route as `ip route get` reports it: device, source address and gateway.
fn parse_route(text: &str) -> Option<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let after = |key: &str| {
        words
            .iter()
            .position(|w| *w == key)
            .and_then(|i| words.get(i + 1))
    };
    let dev = after("dev")?;
    let src = after("src").map_or("-", |s| *s);
    let via = after("via").map_or("-", |s| *s);
    Some(format!("dev {dev} src {src} via {via}"))
}

/// Compare the route at the start with the route now; pure.
pub fn compare_routes(before: Option<&str>, now: Option<&str>) -> NetChange {
    match (before, now) {
        (_, None) if before.is_some() => NetChange::Lost,
        (Some(a), Some(b)) if a == b => NetChange::Same,
        (Some(a), Some(b)) => NetChange::Changed {
            from: a.to_owned(),
            to: b.to_owned(),
        },
        _ => NetChange::Unknown,
    }
}

/// The current route to the outside, best effort; `None` when there is none or
/// it cannot be read (non-Linux, no `ip`).
#[cfg(target_os = "linux")]
fn current_route() -> Option<String> {
    use crate::spawn::CommandExt as _;
    let out = std::process::Command::new("ip")
        .args(["-4", "route", "get", "1.1.1.1"])
        .stdin(std::process::Stdio::null())
        .output_locked()
        .map_err(|e| tracing::debug!(error = %e, "cannot read the local route"))
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_route(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(not(target_os = "linux"))]
fn current_route() -> Option<String> {
    None
}

/// Whether this client runs under WSL.
pub fn under_wsl() -> bool {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .is_ok_and(|v| v.to_ascii_lowercase().contains("microsoft"))
}

/// Local facts captured when a run starts, compared with the failure moment.
#[derive(Debug)]
pub struct Watch {
    wall: SystemTime,
    mono: Instant,
    route: Option<String>,
    can_route: bool,
}

impl Watch {
    /// Capture the clocks and the route now.
    pub fn start() -> Self {
        let route = current_route();
        tracing::debug!(?route, "local route at the start of the run");
        Self {
            wall: SystemTime::now(),
            mono: Instant::now(),
            can_route: cfg!(target_os = "linux"),
            route,
        }
    }

    /// Time since the start by the monotonic clock.
    pub fn elapsed(&self) -> Duration {
        self.mono.elapsed()
    }

    /// Wall-clock time beyond monotonic time since the start (sleep shows here).
    pub fn suspend_gap(&self) -> Option<Duration> {
        let wall = self.wall.elapsed().ok()?;
        wall.checked_sub(self.mono.elapsed())
    }

    /// How the route differs from the start.
    pub fn net_change(&self) -> NetChange {
        if !self.can_route {
            return NetChange::Unknown;
        }
        compare_routes(self.route.as_deref(), current_route().as_deref())
    }
}

/// The last bytes of a stream and when the last byte arrived.
#[derive(Debug, Default)]
pub struct Tail {
    inner: Mutex<TailState>,
}

#[derive(Debug, Default)]
struct TailState {
    bytes: Vec<u8>,
    last: Option<Instant>,
}

impl Tail {
    /// Record `chunk` as the newest output.
    pub fn push(&self, chunk: &[u8]) {
        let mut s = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        s.bytes.extend_from_slice(chunk);
        if s.bytes.len() > TAIL_BYTES {
            let cut = s.bytes.len() - TAIL_BYTES;
            s.bytes.drain(..cut);
        }
        s.last = Some(Instant::now());
    }

    /// The kept bytes as lossy text.
    pub fn text(&self) -> String {
        let s = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        String::from_utf8_lossy(&s.bytes).into_owned()
    }

    /// When the most recent byte arrived.
    pub fn last(&self) -> Option<Instant> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .last
    }
}

/// A reader that copies what it reads into a [`Tail`].
pub struct Tap<'a, R> {
    inner: R,
    tails: Vec<&'a Tail>,
}

impl<'a, R> Tap<'a, R> {
    /// Wrap `inner`, recording into every tail in `tails`.
    pub fn new(inner: R, tails: Vec<&'a Tail>) -> Self {
        Self { inner, tails }
    }
}

impl<R: std::io::Read> std::io::Read for Tap<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            for t in &self.tails {
                t.push(&buf[..n]);
            }
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sec(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    // frob:tests crates/goway/src/lostlink.rs::classify
    #[test]
    fn table_of_evidence_picks_the_cause() {
        type Tweak = fn(&mut Evidence);
        let cases: Vec<(&str, Tweak, Cause, Option<Confidence>)> = vec![
            (
                "slept",
                |e| {
                    e.suspend_gap = Some(sec(600));
                    e.probe = Probe::TimedOut;
                },
                Cause::LocalSuspended,
                Some(Confidence::High),
            ),
            (
                "heartbeat gap only",
                |e| e.beat_gap = Some(sec(90)),
                Cause::LocalSuspended,
                None,
            ),
            (
                "route changed",
                |e| {
                    e.net = NetChange::Changed {
                        from: "dev eth0 src 10.0.0.2 via 10.0.0.1".into(),
                        to: "dev wlan0 src 192.168.1.9 via 192.168.1.1".into(),
                    };
                },
                Cause::LocalNetworkChanged,
                None,
            ),
            (
                "route lost, probe fails too",
                |e| {
                    e.net = NetChange::Lost;
                    e.probe = Probe::NoRoute;
                },
                Cause::LocalNetworkChanged,
                Some(Confidence::High),
            ),
            (
                "helper timed out",
                |e| e.probe = Probe::TimedOut,
                Cause::HelperUnreachable,
                Some(Confidence::Medium),
            ),
            (
                "helper refused",
                |e| e.probe = Probe::Refused,
                Cause::HelperUnreachable,
                None,
            ),
        ];
        check_table(cases);
    }

    // frob:tests crates/goway/src/lostlink.rs::classify
    #[test]
    fn table_of_watchdog_and_link_evidence() {
        type Tweak = fn(&mut Evidence);
        let cases: Vec<(&str, Tweak, Cause, Option<Confidence>)> = vec![
            (
                "watchdog stop",
                |e| {
                    e.exit = 143;
                    e.lifeline_broken = true;
                },
                Cause::HelperStoppedJob,
                Some(Confidence::High),
            ),
            (
                "helper note",
                |e| {
                    e.exit = 143;
                    e.stderr.helper_stop_note = true;
                },
                Cause::HelperStoppedJob,
                Some(Confidence::High),
            ),
            (
                "reset with both ends up",
                |e| e.stderr.reset = true,
                Cause::TransportReset,
                Some(Confidence::Medium),
            ),
            (
                "auth",
                |e| {
                    e.stderr.auth = true;
                    e.probe = Probe::Auth;
                },
                Cause::AuthOrHostKey,
                Some(Confidence::High),
            ),
            (
                "bare 255 after a long silence",
                |e| e.since_output = Some(sec(300)),
                Cause::CommandExit,
                Some(Confidence::Low),
            ),
            (
                "bare 143",
                |e| e.exit = 143,
                Cause::CommandExit,
                Some(Confidence::Low),
            ),
        ];
        check_table(cases);
    }

    type Case = (&'static str, fn(&mut Evidence), Cause, Option<Confidence>);

    fn check_table(cases: Vec<Case>) {
        for (name, tweak, want, conf) in cases {
            let mut ev = Evidence::quiet(255);
            tweak(&mut ev);
            let v = classify(&ev);
            assert_eq!(v.cause, want, "{name}: {:?}", v.ranked);
            if let Some(c) = conf {
                assert_eq!(v.confidence, c, "{name}: {:?}", v.ranked);
            }
            assert!(v.evidence.len() >= 2, "{name}");
        }
    }

    // frob:tests crates/goway/src/lostlink.rs::Verdict
    #[test]
    fn a_verdict_prints_its_evidence_and_says_when_it_is_unsure() {
        let v = classify(&Evidence::quiet(255));
        let text = v.text();
        assert!(text.contains("confidence: low"), "{text}");
        assert!(text.contains("evidence:\n  - exit 255"), "{text}");
        assert!(text.contains("next: "), "{text}");
        let sure = classify(&{
            let mut e = Evidence::quiet(255);
            e.suspend_gap = Some(sec(900));
            e
        });
        assert!(sure.text().contains("confidence: high"));
        assert!(sure.text().contains("clock jumped 900s"));
    }

    // frob:tests crates/goway/src/lostlink.rs::parse_stderr
    #[test]
    fn stderr_phrases_are_recognised() {
        let s = parse_stderr("client_loop: send disconnect: Broken pipe");
        assert!(s.reset && !s.timed_out);
        let s = parse_stderr("ssh: connect to host h port 22: Connection timed out");
        assert!(s.timed_out);
        assert!(parse_stderr("No route to host").no_route);
        assert!(parse_stderr("Network is unreachable").no_route);
        assert!(parse_stderr("Connection refused").refused);
        assert!(parse_stderr("Host key verification failed.").auth);
        assert!(parse_stderr("goway: this run was stopped by the helper: x").helper_stop_note);
        assert!(!parse_stderr("all fine").transport());
    }

    // frob:tests crates/goway/src/lostlink.rs::parse_probe
    #[test]
    fn probe_failures_are_told_apart() {
        assert_eq!(parse_probe(true, ""), Probe::Answers);
        assert_eq!(parse_probe(false, "Connection timed out"), Probe::TimedOut);
        assert_eq!(parse_probe(false, "Connection refused"), Probe::Refused);
        assert_eq!(parse_probe(false, "No route to host"), Probe::NoRoute);
        assert_eq!(
            parse_probe(false, "ssh: Could not resolve hostname h"),
            Probe::NoRoute
        );
        assert_eq!(
            parse_probe(false, "Permission denied (publickey)"),
            Probe::Auth
        );
        assert_eq!(parse_probe(false, ""), Probe::Failed);
    }

    // frob:tests crates/goway/src/lostlink.rs::compare_routes
    #[test]
    fn routes_are_parsed_and_compared() {
        let a = parse_route("1.1.1.1 via 192.168.1.1 dev eth0 src 192.168.1.5 uid 1000").unwrap();
        assert_eq!(a, "dev eth0 src 192.168.1.5 via 192.168.1.1");
        let b = parse_route("1.1.1.1 dev wlan0 src 10.1.1.4").unwrap();
        assert_eq!(compare_routes(Some(&a), Some(&a)), NetChange::Same);
        assert!(matches!(
            compare_routes(Some(&a), Some(&b)),
            NetChange::Changed { .. }
        ));
        assert_eq!(compare_routes(Some(&a), None), NetChange::Lost);
        assert_eq!(compare_routes(None, None), NetChange::Unknown);
        assert_eq!(
            parse_route("RTNETLINK answers: Network is unreachable"),
            None
        );
    }

    // frob:tests crates/goway/src/lostlink.rs::Tail
    #[test]
    fn the_tail_keeps_only_the_end() {
        let t = Tail::default();
        assert!(t.last().is_none());
        t.push(&vec![b'a'; TAIL_BYTES]);
        t.push(b"Broken pipe");
        assert_eq!(t.text().len(), TAIL_BYTES);
        assert!(t.text().ends_with("Broken pipe"));
        assert!(t.last().is_some());
        let mut r = Tap::new(&b"hello"[..], vec![&t]);
        let mut buf = [0u8; 8];
        assert_eq!(std::io::Read::read(&mut r, &mut buf).unwrap(), 5);
        assert!(t.text().ends_with("hello"));
    }
}
