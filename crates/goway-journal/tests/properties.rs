//! Property tests: revert inverts apply over arbitrary model systems and plans.

use std::path::PathBuf;

use goway_journal::{
    Change, Journal, ListPosition, ModelSystem, RegValue, ResourceKind, apply, revert,
};
use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};

const FILES: [&str; 4] = ["/t/f0", "/t/f1", "/t/d0/f0", "/t/d0/e0/f0"];
const DIRS: [&str; 4] = ["/t/d0", "/t/d0/e0", "/t/d1", "/t/d1/e1"];

fn pick(items: &'static [&'static str]) -> impl Strategy<Value = String> {
    proptest::sample::select(items).prop_map(str::to_owned)
}

fn path_of(items: &'static [&'static str]) -> impl Strategy<Value = PathBuf> {
    pick(items).prop_map(PathBuf::from)
}

fn any_path() -> impl Strategy<Value = PathBuf> {
    prop_oneof![path_of(&FILES), path_of(&DIRS)]
}

fn text() -> impl Strategy<Value = String> {
    let line = pick(&[
        "[wsl2]",
        "[other]",
        "networkingMode=nat",
        "networkingMode = mirrored",
        "x=1",
        "; c",
        "",
        "a",
        "b",
        "a # goway",
        "x=1 #m",
    ]);
    (proptest::collection::vec(line, 0..6), any::<bool>()).prop_map(|(lines, nl)| {
        let mut s = lines.join("\n");
        if nl && !lines.is_empty() {
            s.push('\n');
        }
        s
    })
}

fn reg_value() -> impl Strategy<Value = RegValue> {
    prop_oneof![
        pick(&["p", "q"]).prop_map(RegValue::String),
        pick(&["%A%", "%B%"]).prop_map(RegValue::ExpandString),
        (0u32..3).prop_map(RegValue::Dword),
    ]
}

fn kind() -> impl Strategy<Value = ResourceKind> {
    prop_oneof![
        Just(ResourceKind::FirewallRule),
        Just(ResourceKind::HyperVFirewallRule),
        Just(ResourceKind::ScheduledTask),
        Just(ResourceKind::Service),
    ]
}

fn mode() -> impl Strategy<Value = u32> {
    proptest::sample::select(&[0o644u32, 0o600, 0o755][..])
}

fn sddl() -> impl Strategy<Value = String> {
    pick(&["", "D:a", "D:b"])
}

fn change() -> impl Strategy<Value = Change> {
    prop_oneof![
        (path_of(&FILES), text()).prop_map(|(path, contents)| Change::WriteFile { path, contents }),
        (
            path_of(&FILES),
            pick(&["a", "b", "x=1"]),
            pick(&["# goway", "", "#m"])
        )
            .prop_map(|(path, line, marker)| Change::EnsureLine { path, line, marker }),
        path_of(&DIRS).prop_map(|path| Change::EnsureDir { path }),
        (
            pick(&["/a", "/b", "/c"]),
            prop_oneof![Just(ListPosition::Front), Just(ListPosition::Back)]
        )
            .prop_map(|(entry, position)| Change::EnsureListEntry {
                var: "PATH".into(),
                entry,
                separator: ':',
                position
            }),
        (pick(&["K1", "K2"]), pick(&["n1", "n2"]), reg_value())
            .prop_map(|(key, name, value)| Change::SetRegistryValue { key, name, value }),
        (
            path_of(&FILES),
            pick(&["wsl2", "other"]),
            pick(&["networkingMode", "x"]),
            pick(&["mirrored", "nat", "1"])
        )
            .prop_map(|(path, section, key, value)| Change::SetIniKey {
                path,
                section,
                key,
                value
            }),
        (any_path(), mode()).prop_map(|(path, mode)| Change::SetUnixMode { path, mode }),
        (any_path(), sddl()).prop_map(|(path, sddl)| Change::SetAcl { path, sddl }),
        (kind(), pick(&["r1", "r2"]), pick(&["s1", "s2"]))
            .prop_map(|(kind, name, spec)| Change::EnsureResource { kind, name, spec }),
    ]
}

fn plan() -> impl Strategy<Value = Vec<Change>> {
    proptest::collection::vec(change(), 0..10)
}

fn system() -> impl Strategy<Value = ModelSystem> {
    let dirs = proptest::collection::vec(any::<bool>(), 4);
    let files = proptest::collection::vec(proptest::option::of(text()), 4);
    let path_var = proptest::option::of(proptest::collection::vec(
        pick(&["/a", "/b", "/z", ""]),
        0..4,
    ));
    let regs = proptest::collection::vec(
        (pick(&["K1", "K2"]), pick(&["n1", "n2"]), reg_value()),
        0..3,
    );
    let attrs = proptest::collection::vec((any_path(), mode(), sddl()), 0..4);
    let resources = proptest::collection::vec((kind(), pick(&["r1", "r2"]), pick(&["old"])), 0..3);
    (dirs, files, path_var, regs, attrs, resources).prop_map(
        |(dirs, files, path_var, regs, attrs, resources)| {
            let mut m = ModelSystem::new();
            m.dirs.insert("/t".into());
            for (d, on) in DIRS.iter().zip(dirs) {
                let parent = PathBuf::from(d).parent().unwrap().to_path_buf();
                if on && m.dirs.contains(&parent) {
                    m.dirs.insert(PathBuf::from(d));
                }
            }
            for (f, c) in FILES.iter().zip(files) {
                let parent = PathBuf::from(f).parent().unwrap().to_path_buf();
                if let Some(c) = c
                    && m.dirs.contains(&parent)
                {
                    m.files.insert(PathBuf::from(f), c);
                }
            }
            if let Some(parts) = path_var {
                m.vars.insert("PATH".into(), parts.join(":"));
            }
            for (k, n, v) in regs {
                m.registry.insert((k, n), v);
            }
            for (p, mo, s) in attrs {
                if m.files.contains_key(&p) || m.dirs.contains(&p) {
                    use goway_journal::System;
                    m.set_mode(&p, mo).unwrap();
                    m.set_acl(&p, &s).unwrap();
                }
            }
            for (k, n, s) in resources {
                m.resources.insert((k, n), s);
            }
            m
        },
    )
}

/// Run `body` over arbitrary (initial system, plan) pairs.
fn check(body: impl Fn(ModelSystem, Vec<Change>) -> Result<(), TestCaseError>) {
    let mut runner = TestRunner::new(Config::with_cases(2000));
    runner
        .run(&(system(), plan()), |(initial, plan)| body(initial, plan))
        .unwrap();
}

/// Apply, keeping the journal even when apply fails part-way.
fn apply_any(sys: &mut ModelSystem, plan: &[Change]) -> Journal {
    match apply(plan, sys) {
        Ok(j) => j,
        Err(e) => *e.journal,
    }
}

// frob:tests crates/goway-journal/src/apply.rs::apply
// frob:tests crates/goway-journal/src/apply.rs::revert
// frob:tests crates/goway-journal/src/model.rs::ModelSystem
/// Install then uninstall restores the initial system, also when apply fails part-way.
#[test]
fn revert_after_apply_restores_initial_state() {
    check(|initial, plan| {
        let mut sys = initial.clone();
        let mut journal = apply_any(&mut sys, &plan);
        revert(&mut journal, &mut sys).unwrap();
        prop_assert_eq!(sys, initial);
        Ok(())
    });
}

/// A second install over the first, then reverting only the second, leaves the first intact.
#[test]
fn second_apply_revert_keeps_first_install() {
    check(|initial, plan| {
        let mut sys = initial;
        if apply(&plan, &mut sys).is_err() {
            return Ok(());
        }
        let after_first = sys.clone();
        let Ok(mut second) = apply(&plan, &mut sys) else {
            return Ok(());
        };
        revert(&mut second, &mut sys).unwrap();
        prop_assert_eq!(sys, after_first);
        Ok(())
    });
}

/// Reverting a journal twice is harmless.
#[test]
fn revert_is_idempotent() {
    check(|initial, plan| {
        let mut sys = initial.clone();
        let mut journal = apply_any(&mut sys, &plan);
        revert(&mut journal, &mut sys).unwrap();
        let once = sys.clone();
        revert(&mut journal, &mut sys).unwrap();
        prop_assert_eq!(&sys, &once);
        prop_assert_eq!(sys, initial);
        Ok(())
    });
}

/// A journal survives a JSON round trip unchanged and still reverts.
#[test]
fn journal_json_round_trips() {
    check(|initial, plan| {
        let mut sys = initial.clone();
        let journal = apply_any(&mut sys, &plan);
        let json = serde_json::to_string(&journal).unwrap();
        let mut back: Journal = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(&back, &journal);
        revert(&mut back, &mut sys).unwrap();
        prop_assert_eq!(sys, initial);
        Ok(())
    });
}
