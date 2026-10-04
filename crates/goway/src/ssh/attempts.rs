//! A ledger of failed logins goway itself caused, so it never trips fail2ban
//! or sshguard (default: 5 failures in 10 minutes bans the client).
//!
//! Only failed *authentications* count. Automatic probing (resolution and
//! status probes) may cause at most [`AUTO_MAX`] per host per [`WINDOW_SECS`];
//! user-initiated steps (the one password attempt of `goway add`, the key
//! re-check after the user pressed Enter) are allowed on top, up to
//! [`TOTAL_MAX`] overall, which stays under fail2ban's default of 5.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::config::write_atomic;

/// The look-back window, in seconds (fail2ban's default `findtime`).
pub const WINDOW_SECS: u64 = 600;
/// Most failures automatic probing may cause per host per window.
pub const AUTO_MAX: usize = 2;
/// Most failures of any origin per host per window (fail2ban bans at 5).
pub const TOTAL_MAX: usize = 4;

/// Who started a login attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Resolution and status probes goway makes on its own.
    Automatic,
    /// A step the user is driving and just asked for.
    User,
}

/// One recorded failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    at: u64,
    user: bool,
}

/// Why an attempt was held back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Blocked {
    /// Seconds until the oldest counted failure leaves the window.
    pub retry_in_secs: u64,
}

impl std::fmt::Display for Blocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "goway is holding back: too many failed logins to this host in the last {} minutes \
             (more could get this machine banned by fail2ban); retry in about {} minute(s)",
            WINDOW_SECS / 60,
            self.retry_in_secs.div_ceil(60).max(1)
        )
    }
}

/// Failed logins per host (lower-case name), newest last.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ledger {
    hosts: BTreeMap<String, Vec<Entry>>,
}

impl Ledger {
    fn live(&self, host: &str, now: u64) -> impl Iterator<Item = &Entry> {
        self.hosts
            .get(&host.to_ascii_lowercase())
            .into_iter()
            .flatten()
            .filter(move |e| now.saturating_sub(e.at) < WINDOW_SECS)
    }

    /// Failures of `host` within the window.
    pub fn recent(&self, host: &str, now: u64) -> usize {
        self.live(host, now).count()
    }

    /// Whether an attempt of `origin` may start now.
    ///
    /// # Errors
    ///
    /// [`Blocked`] when it would exceed the per-origin or the overall cap.
    pub fn check(&self, host: &str, origin: Origin, now: u64) -> Result<(), Blocked> {
        let total = self.live(host, now).count();
        let auto = self.live(host, now).filter(|e| !e.user).count();
        let over = total >= TOTAL_MAX || (origin == Origin::Automatic && auto >= AUTO_MAX);
        if !over {
            return Ok(());
        }
        let oldest = self.live(host, now).map(|e| e.at).min().unwrap_or(now);
        Err(Blocked {
            retry_in_secs: (oldest + WINDOW_SECS).saturating_sub(now),
        })
    }

    /// Record a failed login and forget entries past the window.
    pub fn record(&mut self, host: &str, origin: Origin, now: u64) {
        let list = self.hosts.entry(host.to_ascii_lowercase()).or_default();
        list.retain(|e| now.saturating_sub(e.at) < WINDOW_SECS);
        list.push(Entry {
            at: now,
            user: origin == Origin::User,
        });
    }

    fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                tracing::warn!(path = %path.display(), error = %e, "login ledger unreadable; starting empty");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }
}

static LEDGER_FILE: OnceLock<PathBuf> = OnceLock::new();

/// Keep the ledger at `<state_dir>/auth-failures.json` for this process.
pub fn init(state_dir: &Path) {
    let _ = LEDGER_FILE.set(state_dir.join("auth-failures.json"));
}

thread_local! {
    static USER_STEPS: Cell<u32> = const { Cell::new(0) };
}

/// While alive, attempts on this thread are user-initiated.
#[derive(Debug)]
pub struct UserStep(());

/// Mark the attempts made until the guard drops as user-initiated.
pub fn user_step() -> UserStep {
    USER_STEPS.with(|c| c.set(c.get() + 1));
    UserStep(())
}

impl Drop for UserStep {
    fn drop(&mut self) {
        USER_STEPS.with(|c| c.set(c.get().saturating_sub(1)));
    }
}

/// The origin of attempts made on this thread right now.
pub fn current_origin() -> Origin {
    if USER_STEPS.with(Cell::get) > 0 {
        Origin::User
    } else {
        Origin::Automatic
    }
}

/// Whether an attempt on `host` may start now (always, with no ledger set).
///
/// # Errors
///
/// [`Blocked`] when the caps are reached.
pub fn permit(host: &str, origin: Origin) -> Result<(), Blocked> {
    let Some(path) = LEDGER_FILE.get() else {
        return Ok(());
    };
    let verdict = Ledger::load(path).check(host, origin, crate::state::now_secs());
    if let Err(b) = &verdict {
        tracing::warn!(
            host,
            ?origin,
            retry_in_secs = b.retry_in_secs,
            "login attempt held back"
        );
    }
    verdict
}

/// Record a failed login to `host` (no-op with no ledger set).
pub fn record_failure(host: &str, origin: Origin) {
    let Some(path) = LEDGER_FILE.get() else {
        return;
    };
    let mut ledger = Ledger::load(path);
    ledger.record(host, origin, crate::state::now_secs());
    tracing::info!(
        host,
        ?origin,
        recent = ledger.recent(host, crate::state::now_secs()),
        "failed login recorded"
    );
    match serde_json::to_string(&ledger) {
        Ok(text) => {
            if let Err(e) = write_atomic(path, text.as_bytes()) {
                tracing::warn!(error = %e, "cannot save the login ledger");
            }
        }
        Err(e) => tracing::warn!(error = %e, "cannot encode the login ledger"),
    }
}

/// Failures of `host` in the window (0 with no ledger set).
pub fn recent_failures(host: &str) -> usize {
    LEDGER_FILE.get().map_or(0, |p| {
        Ledger::load(p).recent(host, crate::state::now_secs())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // frob:tests crates/goway/src/ssh/attempts.rs::Ledger
    #[test]
    fn automatic_probing_stops_at_two_but_user_steps_continue_to_four() {
        let mut l = Ledger::default();
        assert!(l.check("h", Origin::Automatic, 0).is_ok());
        l.record("h", Origin::Automatic, 0);
        l.record("h", Origin::Automatic, 1);
        assert!(l.check("h", Origin::Automatic, 2).is_err());
        // The paste flow: one password attempt, then the key re-check.
        assert!(l.check("h", Origin::User, 2).is_ok());
        l.record("h", Origin::User, 2);
        assert!(l.check("h", Origin::User, 3).is_ok());
        l.record("h", Origin::User, 3);
        // Four in all: nothing more, so fail2ban's 5 is never reached.
        assert!(l.check("h", Origin::User, 4).is_err());
        assert_eq!(l.recent("H", 4), TOTAL_MAX);
    }

    #[test]
    fn failures_leave_the_window_and_hosts_are_independent() {
        let mut l = Ledger::default();
        l.record("a", Origin::Automatic, 0);
        l.record("a", Origin::Automatic, 1);
        assert!(l.check("b", Origin::Automatic, 2).is_ok());
        let blocked = l.check("a", Origin::Automatic, 100).unwrap_err();
        assert_eq!(blocked.retry_in_secs, WINDOW_SECS - 100);
        assert!(l.check("a", Origin::Automatic, WINDOW_SECS + 1).is_ok());
    }

    #[test]
    fn user_step_guards_nest_and_restore_the_origin() {
        assert_eq!(current_origin(), Origin::Automatic);
        let outer = user_step();
        let inner = user_step();
        drop(inner);
        assert_eq!(current_origin(), Origin::User);
        drop(outer);
        assert_eq!(current_origin(), Origin::Automatic);
    }
}
