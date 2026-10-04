//! `goway gc`: remove stale remote state on every host.
//!
//! Each remote entry (work dir, seed, per-repository cache) carries a
//! `meta.json` label and a lock. An entry is removed when its lock is free
//! and it has been idle longer than its TTL: orphaned work dirs after
//! `orphan_ttl`, `--keep` work dirs after `kept_ttl`, seeds and caches (with their slot trees) after
//! `cache_ttl`. `--older-than` replaces every TTL and `--all` sets them to 0.
//! A locked entry is reported busy and never touched. Every `goway run`
//! also triggers this with the default TTLs on its host.

use std::time::Duration;

use crate::cli::GcArgs;
use crate::config::{Config, HostConfig};
use crate::error::{Error, Result};
use crate::paths::Paths;
use crate::pool;
use crate::remote::Call;
use crate::render::Renderer;
use crate::resolve::{self, Lookup, Prober};
use crate::ssh::KeyPolicy;
use crate::state::State;
use crate::status::human_bytes;

/// What gc did or would do with an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Fresh: kept.
    Keep,
    /// Expired and unlocked: removed (or would be, under `--dry-run`).
    Remove,
    /// Unlocked and least recently used: removed (or would be) to fit the disk budget.
    Evict,
    /// Locked by a run: never touched.
    Busy,
}

/// One remote entry as gc saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The decision.
    pub action: Action,
    /// `work`, `seed`, `cache` or `slot`.
    pub kind: String,
    /// Seconds since last use.
    pub age_secs: u64,
    /// Size on disk.
    pub bytes: u64,
    /// Repository name from the label.
    pub repo: String,
    /// Repository id from the label.
    pub repo_id: String,
    /// Remote path.
    pub path: String,
}

/// Parse the remote `gc` verb's tab-separated lines.
pub fn parse(text: &str) -> Vec<Entry> {
    text.lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.splitn(7, '\t').collect();
            let [action, kind, age, bytes, repo, repo_id, path] = f.as_slice() else {
                tracing::warn!(line, "malformed gc line");
                return None;
            };
            let action = match *action {
                "keep" | "unlabelled" => Action::Keep,
                "remove" => Action::Remove,
                "evict" => Action::Evict,
                "busy" => Action::Busy,
                _ => return None,
            };
            Some(Entry {
                action,
                kind: (*kind).to_owned(),
                age_secs: age.parse().unwrap_or(0),
                bytes: bytes.parse().unwrap_or(0),
                repo: (*repo).to_owned(),
                repo_id: (*repo_id).to_owned(),
                path: (*path).to_owned(),
            })
        })
        .collect()
}

/// TTL override from the flags: `--all` is 0, `--older-than D` is D.
pub fn override_ttl(args: &GcArgs) -> Result<Option<Duration>> {
    if args.all {
        return Ok(Some(Duration::ZERO));
    }
    args.older_than
        .as_deref()
        .map(|s| {
            humantime::parse_duration(s)
                .map_err(|e| Error::Usage(format!("--older-than `{s}`: {e}")))
        })
        .transpose()
}

/// The remote gc call.
pub fn command(
    config: &Config,
    host: Option<&HostConfig>,
    args: &GcArgs,
    now: u64,
) -> Result<Call> {
    let d = &config.defaults;
    let (max_disk, min_free, _) = config.budget_of(host);
    let older = override_ttl(args)?.map(|t| t.as_secs().to_string());
    let words = [
        d.remote_root.clone(),
        now.to_string(),
        d.cache_ttl.as_secs().to_string(),
        d.orphan_ttl.as_secs().to_string(),
        d.kept_ttl.as_secs().to_string(),
        if args.dry_run { "dry" } else { "apply" }.to_owned(),
        args.repo.clone().unwrap_or_default(),
        older.unwrap_or_default(),
        max_disk.to_string(),
        min_free.to_string(),
    ];
    let refs: Vec<&str> = words.iter().map(String::as_str).collect();
    Ok(Call::new("gc", &refs))
}

fn human_age(secs: u64) -> String {
    match secs {
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}

/// Forget local memories of a repository (or all) after a real `--repo` or `--all` gc.
fn forget_local_state(state: &mut State, renderer: Renderer, args: &GcArgs) {
    // Remembered detection failures are local state: `--repo` clears that
    // repository's, `--all` everyone's.
    if args.repo.is_none() && !args.all {
        return;
    }
    let cleared = crate::detect::clear(state, args.repo.as_deref());
    if cleared > 0 {
        renderer.note(format_args!(
            "forgot {cleared} remembered failed test-binary detections"
        ));
    }
    // So is the distrust a copy mismatch left behind (see `goway run`).
    let cleared = state.clear_distrust(args.repo.as_deref(), crate::state::now_secs());
    if cleared > 0 {
        renderer.note(format_args!(
            "trusting {cleared} repositories' copies on hosts again"
        ));
    }
}

/// `goway gc`.
pub fn gc(
    paths: &Paths,
    renderer: Renderer,
    args: &GcArgs,
    lookup: &(dyn Lookup + Sync),
    prober: &(dyn Prober + Sync),
) -> Result<u8> {
    let config = Config::load(&paths.config_file())?;
    let hosts: Vec<&HostConfig> = match &args.host {
        Some(name) => vec![config.host(name)?],
        None => config.hosts.iter().collect(),
    };
    let now = crate::state::now_secs();
    // Fail on a bad flag before any host is asked.
    command(&config, None, args, now)?;
    let mut state = State::load(&paths.state_file())?;
    let results = pool::on_hosts(&hosts, &mut state, |host, local| {
        let cmd = command(&config, Some(host), args, now)?;
        resolve::resolve_call(
            &config,
            host,
            local,
            lookup,
            prober,
            KeyPolicy::Strict,
            &cmd,
        )
        .map(|found| parse(&found.output))
    });
    if !args.dry_run {
        forget_local_state(&mut state, renderer, args);
    }
    if let Err(e) = state.save(&paths.state_file()) {
        tracing::warn!(error = %e, "cannot cache host addresses");
    }
    let (verb, evict_verb) = if args.dry_run {
        ("would remove", "would evict")
    } else {
        ("removed", "evicted")
    };
    let mut rows = vec![
        ["host", "action", "kind", "repo", "idle", "size", "path"]
            .map(str::to_owned)
            .to_vec(),
    ];
    let mut failed = false;
    for (host, result) in &results {
        match result {
            Ok(entries) => {
                let mut freed = 0;
                let mut count = 0;
                let mut evicted = 0;
                for e in entries.iter().filter(|e| e.action != Action::Keep) {
                    if matches!(e.action, Action::Remove | Action::Evict) {
                        freed += e.bytes;
                        count += 1;
                    }
                    if e.action == Action::Evict {
                        evicted += 1;
                    }
                    rows.push(vec![
                        host.name.clone(),
                        match e.action {
                            Action::Busy => "busy",
                            Action::Evict => evict_verb,
                            _ => verb,
                        }
                        .to_owned(),
                        e.kind.clone(),
                        e.repo.clone(),
                        human_age(e.age_secs),
                        human_bytes(e.bytes),
                        e.path.clone(),
                    ]);
                }
                tracing::info!(host = %host.name, count, freed, dry_run = args.dry_run, "gc done");
                let budget = if evicted > 0 {
                    format!(" ({evicted} of them for the disk budget)")
                } else {
                    String::new()
                };
                renderer.note(format_args!(
                    "{}: {verb} {count} entries ({}){budget}, kept {}",
                    host.name,
                    human_bytes(freed),
                    entries
                        .iter()
                        .filter(|e| !matches!(e.action, Action::Remove | Action::Evict))
                        .count()
                ));
            }
            Err(e) => {
                failed = true;
                renderer.warn(format_args!("{}: {e}", host.name));
            }
        }
    }
    if rows.len() > 1 {
        renderer.table(&rows);
    }
    Ok(u8::from(failed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> GcArgs {
        GcArgs {
            host: None,
            repo: None,
            older_than: None,
            all: false,
            dry_run: false,
        }
    }

    #[test]
    fn parses_gc_lines() {
        let e = parse(
            "remove\twork\t90000\t1024\tgoway\tabcd\t/r/work/1\nbusy\tcache\t5\t0\t-\t-\t/r/cache/x\nnoise\n",
        );
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].action, Action::Remove);
        assert_eq!(
            (e[0].age_secs, e[0].bytes, e[0].repo.as_str()),
            (90000, 1024, "goway")
        );
        assert_eq!(e[1].action, Action::Busy);
        let e = parse("evict\tslot\t5\t2048\tgoway\tabcd\t/r/cache/x/tree-0\n");
        assert_eq!((e[0].action, e[0].bytes), (Action::Evict, 2048));
    }

    #[test]
    fn ttl_overrides() {
        assert_eq!(override_ttl(&args()).unwrap(), None);
        let all = GcArgs {
            all: true,
            ..args()
        };
        assert_eq!(override_ttl(&all).unwrap(), Some(Duration::ZERO));
        let older = GcArgs {
            older_than: Some("12h".to_owned()),
            ..args()
        };
        assert_eq!(
            override_ttl(&older).unwrap(),
            Some(Duration::from_hours(12))
        );
        let bad = GcArgs {
            older_than: Some("soon".to_owned()),
            ..args()
        };
        assert!(override_ttl(&bad).is_err());
        let cmd = command(&Config::default(), None, &older, 100)
            .unwrap()
            .args
            .join(" ");
        assert!(cmd.contains(" 43200"));
        // The budget (automatic max, 10 GiB free) follows the filter words.
        assert!(cmd.contains(" 43200 0 10737418240"), "{cmd}");
        assert_eq!(human_age(90_000), "1d");
    }
}
