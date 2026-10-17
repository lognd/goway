//! `goway circle revoke --now`: tell every reachable helper to refresh its `cert-authority`
//! lines so a revoked key stops working before its certificate expires, and remember when each
//! helper last answered (shown as "last seen" in `goway circle list`).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use goway_circle::{Role, State};

use crate::error::{Error, Result};
use crate::paths::Paths;

/// Largest roster (base64 text) sent on a remote command line; bigger rosters wait for stdin transport.
const MAX_OFFER_B64: usize = 100_000;

/// The roster this machine offers a helper: its operations, merged on the helper before it refreshes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterOffer {
    /// Short id of the circle.
    pub circle: String,
    /// The `ops.jsonl` text, url-safe base64 without padding (shell-safe).
    pub ops_b64: String,
}

impl RosterOffer {
    /// Encode the operations file `ops_text` of circle `circle`.
    pub fn new(circle: &str, ops_text: &str) -> Self {
        Self {
            circle: circle.to_owned(),
            ops_b64: URL_SAFE_NO_PAD.encode(ops_text.as_bytes()),
        }
    }

    /// The command a helper runs: adopt the offered roster, then refresh its access lines.
    pub fn remote_command(&self) -> std::result::Result<String, String> {
        if self.ops_b64.len() > MAX_OFFER_B64 {
            return Err(format!(
                "roster is too large to push ({} bytes of text)",
                self.ops_b64.len()
            ));
        }
        Ok(format!(
            "goway circle sync --circle {c} {o} && goway circle access --circle {c}",
            c = self.circle,
            o = self.ops_b64
        ))
    }
}

/// Reaches one helper, hands it the roster and makes it refresh its access lines.
pub trait Pusher {
    /// Update `host`; the error text says why it could not be reached or updated.
    fn push(&self, host: &str, offer: &RosterOffer) -> std::result::Result<(), String>;
}

/// Who answered and who did not.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PushReport {
    /// Helpers that refreshed.
    pub reached: Vec<String>,
    /// Helpers that stay exposed until they hear, with the reason.
    pub unreachable: Vec<(String, String)>,
}

/// Names of the helpers in `state` other than `me`, in roster order.
pub fn helper_names(state: &State, me: Option<&str>) -> Vec<String> {
    state
        .members
        .iter()
        .filter(|(key, m)| Some(key.as_str()) != me && m.roles.contains(&Role::Helper))
        .map(|(_, m)| m.name.clone())
        .collect()
}

/// Push to every host in `hosts`, collecting who answered.
pub fn push_all(hosts: &[String], offer: &RosterOffer, pusher: &dyn Pusher) -> PushReport {
    let mut report = PushReport::default();
    for host in hosts {
        match pusher.push(host, offer) {
            Ok(()) => {
                tracing::info!(host = %host, "helper refreshed its access lines");
                report.reached.push(host.clone());
            }
            Err(why) => {
                tracing::warn!(host = %host, reason = %why, "helper not reached");
                report.unreachable.push((host.clone(), why));
            }
        }
    }
    report
}

/// The production [`Pusher`]: finds the helper like any pool host and sends the roster and runs `goway circle access` there.
pub struct SshPusher {
    /// Local paths (config and state).
    pub paths: Paths,
}

impl Pusher for SshPusher {
    fn push(&self, host: &str, offer: &RosterOffer) -> std::result::Result<(), String> {
        let remote = offer.remote_command()?;
        let config =
            crate::config::Config::load(&self.paths.config_file()).map_err(|e| e.to_string())?;
        let host = config.host(host).map_err(|e| e.to_string())?;
        let settings = crate::ssh::Settings::from_paths(&self.paths);
        let mut state =
            crate::state::State::load(&self.paths.state_file()).map_err(|e| e.to_string())?;
        let prober = crate::resolve::SshProber {
            settings: settings.clone(),
        };
        let found = crate::resolve::resolve_call(
            &config,
            host,
            &mut state,
            &crate::resolve::SystemLookup,
            &prober,
            crate::ssh::KeyPolicy::Strict,
            &crate::remote::Call::new("ping", &[] as &[&str]),
        )
        .map_err(|e| e.to_string())?;
        crate::resolve::Prober::probe(
            &prober,
            &found.target,
            crate::ssh::KeyPolicy::Strict,
            &remote,
        )
        .map(|_| ())
        .map_err(|(_, e)| e)
    }
}

/// Where last-seen times of circle `id` are kept.
fn seen_path(paths: &Paths, id: &str) -> PathBuf {
    paths.state_dir.join(format!("circle-seen-{id}.txt"))
}

/// Last-seen unix times by member name (empty when none were recorded).
pub fn last_seen(paths: &Paths, id: &str) -> BTreeMap<String, u64> {
    std::fs::read_to_string(seen_path(paths, id))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let (t, name) = l.split_once(' ')?;
            Some((name.to_owned(), t.parse().ok()?))
        })
        .collect()
}

/// Record that `names` answered at unix time `now`.
pub fn record_seen(paths: &Paths, id: &str, names: &[String], now: u64) -> Result<()> {
    let mut seen = last_seen(paths, id);
    for n in names {
        seen.insert(n.clone(), now);
    }
    let mut text = String::new();
    for (n, t) in &seen {
        let _ = writeln!(text, "{t} {n}");
    }
    let file = seen_path(paths, id);
    std::fs::create_dir_all(&paths.state_dir)
        .map_err(|e| Error::io("create", &paths.state_dir, e))?;
    std::fs::write(&file, text).map_err(|e| Error::io("write", &file, e))?;
    tracing::debug!(circle = id, count = names.len(), "recorded last seen");
    Ok(())
}

/// "5m ago", "3h ago", "2d ago" for a unix time `then` seen at `now`; "never" when unknown.
pub fn ago(then: Option<u64>, now: u64) -> String {
    let Some(then) = then else {
        return "never".to_owned();
    };
    let d = now.saturating_sub(then);
    match d {
        0..=59 => "just now".to_owned(),
        60..=3599 => format!("{}m ago", d / 60),
        3600..=86399 => format!("{}h ago", d / 3600),
        _ => format!("{}d ago", d / 86400),
    }
}
