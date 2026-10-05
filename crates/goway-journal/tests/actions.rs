//! Non-invertible actions: recorded with time, host and reason; undo reports them, never skips.

use std::path::PathBuf;

use goway_journal::{
    ActionKind, Change, Journal, ModelSystem, Outcome, Prior, ResourceKind, apply, revert,
    still_applied,
};

fn start_task() -> Change {
    Change::Action {
        kind: ActionKind::StartScheduledTask,
        target: "goway keepalive".into(),
        host: "helios".into(),
        reason: "bring WSL back after the restart".into(),
        undo: None,
    }
}

// frob:tests crates/goway-journal/src/apply.rs::apply
#[test]
fn an_action_is_recorded_with_time_host_and_reason() {
    let mut m = ModelSystem::new();
    let before = m.clone();
    let j = apply(&[start_task()], &mut m).unwrap();
    assert_eq!(m, before, "recording an action changes nothing by itself");
    assert_eq!(j.entries.len(), 1);
    assert!(matches!(
        j.entries[0].prior,
        Prior::Action { at_unix_secs } if at_unix_secs > 0
    ));
    assert_eq!(j.entries[0].change, start_task());
}

// frob:tests crates/goway-journal/src/apply.rs::revert
#[test]
fn undo_reports_an_action_as_not_reversible_and_settles_it() {
    let mut m = ModelSystem::new();
    let mut j = apply(&[start_task()], &mut m).unwrap();
    let report = revert(&mut j, &mut m).unwrap();
    let [(0, Outcome::NotReversible(text))] = report.outcomes.as_slice() else {
        panic!(
            "expected one NotReversible outcome, got {:?}",
            report.outcomes
        );
    };
    for part in [
        "started a scheduled task",
        "helios",
        "goway keepalive",
        "WSL back",
    ] {
        assert!(text.contains(part), "{text:?} lacks {part:?}");
    }
    assert!(text.contains("cannot be undone"));
    assert!(j.entries[0].reverted, "settled so an uninstall can finish");
    let again = revert(&mut j, &mut m).unwrap();
    assert_eq!(again.outcomes, vec![(0, Outcome::AlreadyReverted)]);
}

#[test]
fn the_undo_hint_is_reported() {
    let mut m = ModelSystem::new();
    let change = Change::Action {
        kind: ActionKind::RunFix,
        target: "apt-get install -y mold".into(),
        host: "orion".into(),
        reason: "link faster".into(),
        undo: Some("goway uninstall orion".into()),
    };
    let mut j = apply(std::slice::from_ref(&change), &mut m).unwrap();
    assert!(still_applied(&change, &m).unwrap());
    let report = revert(&mut j, &mut m).unwrap();
    let (_, Outcome::NotReversible(text)) = &report.outcomes[0] else {
        panic!("{:?}", report.outcomes);
    };
    assert!(
        text.contains("to take it back: goway uninstall orion"),
        "{text}"
    );
}

#[test]
fn actions_interleave_with_invertible_changes_and_only_the_others_revert() {
    let mut m = ModelSystem::new();
    m.dirs.insert(PathBuf::from("/t"));
    let plan = [
        Change::EnsureResource {
            kind: ResourceKind::ScheduledTask,
            name: "t".into(),
            spec: String::new(),
        },
        start_task(),
    ];
    let mut j = apply(&plan, &mut m).unwrap();
    assert_eq!(m.resources.len(), 1);
    let report = revert(&mut j, &mut m).unwrap();
    assert!(m.resources.is_empty());
    assert!(matches!(report.outcomes[0].1, Outcome::NotReversible(_)));
    assert_eq!(report.outcomes[1].1, Outcome::Restored);
}

// A journal written before actions existed: one entry of every older kind. It must keep loading
// and reverting, so installs made by an older goway can still be undone.
const OLD_JOURNAL: &str = r##"{
  "id": "old-1",
  "created_unix_secs": 1700000000,
  "entries": [
    {"change": {"change": "write_file", "path": "/t/a", "contents": "x"},
     "prior": {"state": "file", "contents": null}},
    {"change": {"change": "ensure_line", "path": "/t/rc", "line": "l", "marker": "# m"},
     "prior": {"state": "line", "created_file": true, "fixed_newline": false}, "reverted": false},
    {"change": {"change": "ensure_dir", "path": "/t/d"},
     "prior": {"state": "dirs_created", "created": ["/t/d"]}},
    {"change": {"change": "set_registry_value", "key": "K", "name": "n",
                "value": {"type": "dword", "value": 1}},
     "prior": {"state": "registry", "value": null}},
    {"change": {"change": "ensure_resource", "kind": "scheduled_task", "name": "t", "spec": ""},
     "prior": {"state": "resource_created"}},
    {"change": {"change": "set_unix_mode", "path": "/t/a", "mode": 384},
     "prior": {"state": "mode", "mode": 420}},
    {"change": {"change": "ensure_reg_key", "key": "K"},
     "prior": {"state": "noop"}}
  ]
}"##;

// frob:tests crates/goway-journal/src/journal.rs::Journal
#[test]
fn a_journal_from_before_actions_still_loads_and_round_trips() {
    let journal: Journal = serde_json::from_str(OLD_JOURNAL).unwrap();
    assert_eq!(journal.entries.len(), 7);
    assert!(journal.entries.iter().all(|e| !e.reverted));
    let text = serde_json::to_string(&journal).unwrap();
    let again: Journal = serde_json::from_str(&text).unwrap();
    assert_eq!(journal, again);
}

#[test]
fn the_new_entry_kind_serializes_with_stable_names() {
    let mut m = ModelSystem::new();
    let j = apply(&[start_task()], &mut m).unwrap();
    let text = serde_json::to_string(&j).unwrap();
    assert!(text.contains(r#""change":"action""#), "{text}");
    assert!(text.contains(r#""kind":"start_scheduled_task""#), "{text}");
    assert!(text.contains(r#""state":"action""#), "{text}");
    assert!(!text.contains("\"undo\""), "an absent hint is not written");
}

#[test]
fn record_helpers_append_and_survive_a_reload() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("changes.json");
    Journal::record_action_at(
        &path,
        ActionKind::ShutdownWsl,
        "",
        "localhost",
        "apply .wslconfig",
        None,
    )
    .unwrap();
    let file = dir.path().join("config.toml");
    Journal::record_write_at(&path, &file, Some("old".into()), "new").unwrap();
    let j = Journal::load(&path).unwrap();
    assert_eq!(j.entries.len(), 2);
    assert!(matches!(j.entries[1].prior, Prior::File { contents: Some(ref c) } if c == "old"));
}
