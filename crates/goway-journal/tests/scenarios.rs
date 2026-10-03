//! Worked examples of each change kind against the model system.

use std::path::PathBuf;

use goway_journal::{
    Change, Journal, ListPosition, ModelSystem, Outcome, Prior, RegValue, ResourceKind, System,
    apply, apply_with, revert, sha256_hex, still_applied,
};

fn p(s: &str) -> PathBuf {
    PathBuf::from(s)
}

fn base() -> ModelSystem {
    let mut m = ModelSystem::new();
    m.dirs.insert(p("/t"));
    m
}

fn line(path: &str, text: &str) -> Change {
    Change::EnsureLine {
        path: p(path),
        line: text.into(),
        marker: "# goway".into(),
    }
}

// frob:tests crates/goway-journal/src/apply.rs::apply
#[test]
fn ensure_line_reverts_only_its_line_and_keeps_later_edits() {
    let mut m = base();
    m.files.insert(p("/t/rc"), "keep\n".into());
    let mut j = apply(&[line("/t/rc", "export A=1")], &mut m).unwrap();
    assert_eq!(m.files[&p("/t/rc")], "keep\nexport A=1 # goway\n");
    m.files.get_mut(&p("/t/rc")).unwrap().push_str("later\n");
    revert(&mut j, &mut m).unwrap();
    assert_eq!(m.files[&p("/t/rc")], "keep\nlater\n");
}

#[test]
fn ensure_line_removes_a_file_goway_created() {
    let mut m = base();
    let mut j = apply(&[line("/t/rc", "x")], &mut m).unwrap();
    assert!(m.files.contains_key(&p("/t/rc")));
    revert(&mut j, &mut m).unwrap();
    assert!(m.files.is_empty());
}

#[test]
fn ensure_line_adds_and_removes_a_missing_final_newline() {
    let mut m = base();
    m.files.insert(p("/t/rc"), "keep".into());
    let mut j = apply(&[line("/t/rc", "x")], &mut m).unwrap();
    assert_eq!(m.files[&p("/t/rc")], "keep\nx # goway\n");
    revert(&mut j, &mut m).unwrap();
    assert_eq!(m.files[&p("/t/rc")], "keep");
}

#[test]
fn ensure_line_present_is_a_noop_and_survives_revert() {
    let mut m = base();
    m.files.insert(p("/t/rc"), "x # goway\n".into());
    let mut j = apply(&[line("/t/rc", "x")], &mut m).unwrap();
    assert_eq!(j.entries[0].prior, Prior::Noop);
    let report = revert(&mut j, &mut m).unwrap();
    assert_eq!(report.outcomes, vec![(0, Outcome::Noop)]);
    assert_eq!(m.files[&p("/t/rc")], "x # goway\n");
}

#[test]
fn ensure_line_rejects_embedded_newline() {
    let mut m = base();
    let err = apply(&[line("/t/rc", "a\nb")], &mut m).unwrap_err();
    assert_eq!(err.index, 0);
}

#[test]
fn ensure_dir_creates_ancestors_and_keeps_non_empty_ones() {
    let mut m = base();
    let plan = [Change::EnsureDir { path: p("/t/a/b") }];
    let mut j = apply(&plan, &mut m).unwrap();
    assert!(m.dirs.contains(&p("/t/a/b")));
    m.files.insert(p("/t/a/other"), "x".into());
    revert(&mut j, &mut m).unwrap();
    assert!(!m.dirs.contains(&p("/t/a/b")));
    assert!(m.dirs.contains(&p("/t/a")), "non-empty parent stays");
}

#[test]
fn ensure_dir_that_exists_is_never_removed() {
    let mut m = base();
    let mut j = apply(&[Change::EnsureDir { path: p("/t") }], &mut m).unwrap();
    revert(&mut j, &mut m).unwrap();
    assert!(m.dirs.contains(&p("/t")));
}

fn path_change(entry: &str, position: ListPosition) -> Change {
    Change::EnsureListEntry {
        var: "PATH".into(),
        entry: entry.into(),
        separator: ';',
        position,
    }
}

#[test]
fn list_entry_preserves_order_and_never_removes_a_preexisting_entry() {
    let mut m = base();
    m.vars.insert("PATH".into(), "a;b;c".into());
    let mut j = apply(
        &[
            path_change("b", ListPosition::Back),
            path_change("z", ListPosition::Front),
        ],
        &mut m,
    )
    .unwrap();
    assert_eq!(m.vars["PATH"], "z;a;b;c");
    revert(&mut j, &mut m).unwrap();
    assert_eq!(m.vars["PATH"], "a;b;c");
}

#[test]
fn list_entry_unsets_a_variable_goway_created() {
    let mut m = base();
    let mut j = apply(&[path_change("z", ListPosition::Back)], &mut m).unwrap();
    assert_eq!(m.vars["PATH"], "z");
    revert(&mut j, &mut m).unwrap();
    assert!(m.vars.is_empty());
}

#[test]
fn list_entry_rejects_empty_or_separator_containing_entries() {
    let mut m = base();
    assert!(apply(&[path_change("", ListPosition::Back)], &mut m).is_err());
    assert!(apply(&[path_change("a;b", ListPosition::Back)], &mut m).is_err());
}

#[test]
fn registry_value_restores_prior_type_and_value_or_deletes() {
    let mut m = base();
    m.registry
        .insert(("K".into(), "a".into()), RegValue::Dword(1));
    let set = |name: &str, value| Change::SetRegistryValue {
        key: "K".into(),
        name: name.into(),
        value,
    };
    let mut j = apply(
        &[
            set("a", RegValue::ExpandString("%X%".into())),
            set("b", RegValue::String("s".into())),
        ],
        &mut m,
    )
    .unwrap();
    revert(&mut j, &mut m).unwrap();
    assert_eq!(m.registry.len(), 1);
    assert_eq!(m.registry[&("K".into(), "a".into())], RegValue::Dword(1));
}

fn ini(path: &str, section: &str, key: &str, value: &str) -> Change {
    Change::SetIniKey {
        path: p(path),
        section: section.into(),
        key: key.into(),
        value: value.into(),
    }
}

#[test]
fn ini_creates_file_and_section_then_removes_both() {
    let mut m = base();
    let mut j = apply(
        &[ini("/t/.wslconfig", "wsl2", "networkingMode", "mirrored")],
        &mut m,
    )
    .unwrap();
    assert_eq!(
        m.files[&p("/t/.wslconfig")],
        "[wsl2]\nnetworkingMode=mirrored\n"
    );
    revert(&mut j, &mut m).unwrap();
    assert!(m.files.is_empty());
}

#[test]
fn ini_replaces_a_value_and_restores_the_original_line() {
    let mut m = base();
    let original = "[wsl2]\nmemory=8GB\nnetworkingMode = nat\n\n[other]\nx=1\n";
    m.files.insert(p("/t/.wslconfig"), original.into());
    let mut j = apply(
        &[ini("/t/.wslconfig", "wsl2", "networkingMode", "mirrored")],
        &mut m,
    )
    .unwrap();
    assert!(m.files[&p("/t/.wslconfig")].contains("networkingMode=mirrored"));
    revert(&mut j, &mut m).unwrap();
    assert_eq!(m.files[&p("/t/.wslconfig")], original);
}

#[test]
fn ini_inserts_into_an_existing_section_and_keeps_a_section_with_other_keys() {
    let mut m = base();
    m.files
        .insert(p("/t/c"), "[wsl2]\nmemory=8GB\n\n[other]\nx=1".into());
    let mut j = apply(&[ini("/t/c", "wsl2", "k", "v")], &mut m).unwrap();
    assert_eq!(
        m.files[&p("/t/c")],
        "[wsl2]\nmemory=8GB\nk=v\n\n[other]\nx=1"
    );
    revert(&mut j, &mut m).unwrap();
    assert_eq!(m.files[&p("/t/c")], "[wsl2]\nmemory=8GB\n\n[other]\nx=1");
}

#[test]
fn ini_appends_a_new_section_after_a_blank_separator() {
    let mut m = base();
    m.files.insert(p("/t/c"), "[other]\nx=1".into());
    let mut j = apply(&[ini("/t/c", "wsl2", "k", "v")], &mut m).unwrap();
    assert_eq!(m.files[&p("/t/c")], "[other]\nx=1\n\n[wsl2]\nk=v\n");
    revert(&mut j, &mut m).unwrap();
    assert_eq!(m.files[&p("/t/c")], "[other]\nx=1");
}

#[test]
fn ini_value_already_set_is_a_noop() {
    let mut m = base();
    m.files.insert(p("/t/c"), "[wsl2]\nk = v\n".into());
    let j = apply(&[ini("/t/c", "wsl2", "k", "v")], &mut m).unwrap();
    assert_eq!(j.entries[0].prior, Prior::Noop);
}

#[test]
fn mode_and_acl_restore_prior_values() {
    let mut m = base();
    m.files.insert(p("/t/f"), String::new());
    m.set_mode(&p("/t/f"), 0o755).unwrap();
    let plan = [
        Change::SetUnixMode {
            path: p("/t/f"),
            mode: 0o600,
        },
        Change::SetAcl {
            path: p("/t/f"),
            sddl: "D:P(A;;FA;;;SY)".into(),
        },
    ];
    let mut j = apply(&plan, &mut m).unwrap();
    assert_eq!(m.get_mode(&p("/t/f")).unwrap(), 0o600);
    assert_eq!(m.get_acl(&p("/t/f")).unwrap(), "D:P(A;;FA;;;SY)");
    revert(&mut j, &mut m).unwrap();
    assert_eq!(m.get_mode(&p("/t/f")).unwrap(), 0o755);
    assert_eq!(m.get_acl(&p("/t/f")).unwrap(), "");
}

#[test]
fn mode_on_a_missing_path_fails_with_a_typed_error() {
    let mut m = base();
    let err = apply(
        &[Change::SetUnixMode {
            path: p("/t/none"),
            mode: 0o600,
        }],
        &mut m,
    )
    .unwrap_err();
    assert!(matches!(
        err.source,
        goway_journal::JournalError::System(goway_journal::SystemError::NotFound(_))
    ));
}

#[test]
fn resource_is_deleted_only_if_goway_created_it() {
    let mut m = base();
    m.resources
        .insert((ResourceKind::Service, "old".into()), "spec".into());
    let ensure = |name: &str| Change::EnsureResource {
        kind: ResourceKind::Service,
        name: name.into(),
        spec: "new".into(),
    };
    let mut j = apply(&[ensure("old"), ensure("fresh")], &mut m).unwrap();
    assert_eq!(m.resources.len(), 2);
    revert(&mut j, &mut m).unwrap();
    assert_eq!(m.resources.len(), 1);
    assert_eq!(m.resources[&(ResourceKind::Service, "old".into())], "spec");
}

#[test]
fn write_file_changed_since_install_is_left_alone() {
    let mut m = base();
    let mut j = apply(
        &[Change::WriteFile {
            path: p("/t/f"),
            contents: "mine".into(),
        }],
        &mut m,
    )
    .unwrap();
    m.files.insert(p("/t/f"), "theirs".into());
    let report = revert(&mut j, &mut m).unwrap();
    assert!(matches!(report.outcomes[0].1, Outcome::LeftAlone(_)));
    assert_eq!(m.files[&p("/t/f")], "theirs");
}

#[test]
fn applying_the_same_plan_twice_records_only_noops_the_second_time() {
    let mut m = base();
    let plan = [
        Change::EnsureDir { path: p("/t/d") },
        Change::WriteFile {
            path: p("/t/d/f"),
            contents: "x".into(),
        },
        line("/t/rc", "a"),
        path_change("z", ListPosition::Back),
        ini("/t/c", "s", "k", "v"),
    ];
    let first = apply(&plan, &mut m).unwrap();
    let after_first = m.clone();
    let mut second = apply(&plan, &mut m).unwrap();
    assert!(second.entries.iter().all(|e| e.prior == Prior::Noop));
    revert(&mut second, &mut m).unwrap();
    assert_eq!(m, after_first);
    assert_ne!(first.id, second.id);
}

#[test]
fn revert_twice_reports_already_reverted() {
    let mut m = base();
    let mut j = apply(&[line("/t/rc", "a")], &mut m).unwrap();
    revert(&mut j, &mut m).unwrap();
    let report = revert(&mut j, &mut m).unwrap();
    assert_eq!(report.outcomes, vec![(0, Outcome::AlreadyReverted)]);
}

#[test]
fn failed_apply_returns_a_journal_that_reverts_the_partial_work() {
    let mut m = base();
    let plan = [
        line("/t/rc", "a"),
        Change::SetUnixMode {
            path: p("/t/none"),
            mode: 0o600,
        },
    ];
    let err = apply(&plan, &mut m).unwrap_err();
    assert_eq!(err.index, 1);
    let mut j = *err.journal;
    assert_eq!(j.entries.len(), 1);
    revert(&mut j, &mut m).unwrap();
    assert_eq!(m, base());
}

#[test]
fn apply_with_calls_the_sink_before_each_mutation() {
    let mut m = base();
    let mut seen = Vec::new();
    let plan = [line("/t/rc", "a"), line("/t/rc", "b")];
    let mut sink = |j: &Journal| {
        seen.push(j.entries.len());
        Ok(())
    };
    let j = apply_with(&plan, &mut m, Journal::new("fixed-id"), &mut sink).unwrap();
    assert_eq!(j.id, "fixed-id");
    assert_eq!(seen, vec![1, 2]);
}

#[test]
fn journal_saves_and_loads_through_a_file() {
    let mut m = base();
    let j = apply(&[line("/t/rc", "a")], &mut m).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.json");
    j.save(&path).unwrap();
    assert_eq!(Journal::load(&path).unwrap(), j);
    assert!(Journal::load(&dir.path().join("missing.json")).is_err());
}

fn install_file(dest: &str) -> Change {
    Change::InstallFile {
        path: p(dest),
        source: p("/t/src"),
        digest: sha256_hex(b"payload"),
    }
}

fn with_source() -> ModelSystem {
    let mut m = base();
    m.files.insert(p("/t/src"), "payload".into());
    m
}

#[test]
fn install_file_copies_then_removes_only_while_unchanged() {
    let mut m = with_source();
    let mut j = apply(&[install_file("/t/dest")], &mut m).unwrap();
    assert_eq!(m.files[&p("/t/dest")], "payload");
    assert!(still_applied(&j.entries[0].change, &m).unwrap());
    revert(&mut j, &mut m).unwrap();
    assert!(!m.files.contains_key(&p("/t/dest")));

    let mut j = apply(&[install_file("/t/dest")], &mut m).unwrap();
    m.files.insert(p("/t/dest"), "edited".into());
    assert!(!still_applied(&j.entries[0].change, &m).unwrap());
    let report = revert(&mut j, &mut m).unwrap();
    assert!(matches!(report.outcomes[0].1, Outcome::LeftAlone(_)));
    assert_eq!(m.files[&p("/t/dest")], "edited");
}

#[test]
fn install_file_refuses_a_different_existing_file_and_keeps_an_identical_one() {
    let mut m = with_source();
    m.files.insert(p("/t/dest"), "foreign".into());
    assert!(apply(&[install_file("/t/dest")], &mut m).is_err());
    assert_eq!(m.files[&p("/t/dest")], "foreign");

    m.files.insert(p("/t/dest"), "payload".into());
    let mut j = apply(&[install_file("/t/dest")], &mut m).unwrap();
    assert_eq!(j.entries[0].prior, Prior::Noop);
    revert(&mut j, &mut m).unwrap();
    assert_eq!(m.files[&p("/t/dest")], "payload");
}

#[test]
fn registry_key_is_removed_only_when_goway_created_it_and_it_is_empty() {
    let key = || Change::EnsureRegKey { key: "K".into() };
    let value = Change::SetRegistryValue {
        key: "K".into(),
        name: "n".into(),
        value: RegValue::Dword(1),
    };
    let mut m = base();
    let mut j = apply(&[key(), value.clone()], &mut m).unwrap();
    assert!(still_applied(&value, &m).unwrap());
    revert(&mut j, &mut m).unwrap();
    assert_eq!(m, base());

    let mut j = apply(&[key()], &mut m).unwrap();
    m.registry
        .insert(("K".into(), "other".into()), RegValue::Dword(2));
    let report = revert(&mut j, &mut m).unwrap();
    assert!(matches!(report.outcomes[0].1, Outcome::LeftAlone(_)));
    assert!(m.reg_key_exists("K").unwrap());

    let mut m = base();
    m.reg_keys.insert("K".into());
    let mut j = apply(&[key()], &mut m).unwrap();
    revert(&mut j, &mut m).unwrap();
    assert!(m.reg_keys.contains("K"), "a pre-existing key survives");
}
