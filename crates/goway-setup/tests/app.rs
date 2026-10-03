//! Install and uninstall against the model system, including the round-trip property.

use std::path::{Path, PathBuf};

use goway_journal::{Change, Journal, ModelSystem, Outcome, RegValue, sha256_hex};
use goway_setup::app::{self, Retry};
use goway_setup::error::SetupError;
use goway_setup::layout::Layout;
use goway_setup::plan::{Sources, client_plan};
use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};

/// The layout's bin directory as a Path entry, built the way the code
/// builds it (so the separator matches on every OS).
fn bin() -> String {
    Path::new("/Local")
        .join("Programs")
        .join("p")
        .join("bin")
        .to_string_lossy()
        .into_owned()
}

struct Fixture {
    _tmp: tempfile::TempDir,
    layout: Layout,
    plan: Vec<Change>,
}

/// A real temp directory for the journal and virtual (model) paths for everything else.
fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let mut layout = Layout::new(Path::new("/Local"), Path::new("/ProgramData"), "p").unwrap();
    layout.state_dir = tmp.path().join("state");
    layout.journal_path = layout.state_dir.join("install-journal.json");
    let src = Sources {
        goway_source: PathBuf::from("/stage/goway.exe"),
        goway_digest: sha256_hex(b"goway"),
        setup_source: PathBuf::from("/stage/setup.exe"),
        setup_digest: sha256_hex(b"setup"),
    };
    let plan = client_plan(&layout, &src, "9.9.9");
    Fixture {
        _tmp: tmp,
        layout,
        plan,
    }
}

fn machine(path_var: Option<&str>, uninstall_parent_exists: bool) -> ModelSystem {
    let mut m = ModelSystem::new();
    m.dirs.insert("/Local".into());
    m.dirs.insert("/stage".into());
    m.files.insert("/stage/goway.exe".into(), "goway".into());
    m.files.insert("/stage/setup.exe".into(), "setup".into());
    if let Some(p) = path_var {
        m.vars.insert("Path".into(), p.into());
    }
    if uninstall_parent_exists {
        m.dirs.insert("/Local/Programs".into());
    }
    m
}

// frob:tests crates/goway-setup/src/app.rs::install
// frob:tests crates/goway-setup/src/app.rs::uninstall
#[test]
fn install_then_uninstall_restores_the_machine_and_removes_the_journal() {
    let f = fixture();
    let initial = machine(Some(r"C:\Windows"), false);
    let mut sys = initial.clone();
    let journal = app::install(&mut sys, &f.layout, &f.plan).unwrap();
    assert_eq!(journal.entries.len(), f.plan.len());
    assert_eq!(sys.vars["Path"], format!(r"C:\Windows;{}", bin()));
    assert_eq!(
        sys.files[&PathBuf::from("/Local/Programs/p/bin/goway.exe")],
        "goway"
    );
    assert!(sys.reg_keys.contains(&f.layout.uninstall_key));
    assert!(f.layout.journal_path.exists());

    let report = app::uninstall(&mut sys, &f.layout, Retry::ONCE)
        .unwrap()
        .unwrap();
    assert!(report.outcomes.iter().all(|(_, o)| *o == Outcome::Restored));
    assert_eq!(sys, initial);
    assert!(!f.layout.journal_path.exists());
    assert!(!f.layout.state_dir.exists(), "empty state dir is removed");
}

// frob:tests crates/goway-setup/src/app.rs::install
#[test]
fn a_path_that_already_contains_the_directory_keeps_it_after_uninstall() {
    let f = fixture();
    let initial = machine(Some(&format!(r"C:\a;{};C:\b", bin())), true);
    let mut sys = initial.clone();
    app::install(&mut sys, &f.layout, &f.plan).unwrap();
    assert_eq!(sys.vars["Path"], initial.vars["Path"], "never duplicated");
    app::uninstall(&mut sys, &f.layout, Retry::ONCE).unwrap();
    assert_eq!(sys, initial);
}

// frob:tests crates/goway-setup/src/app.rs::install
#[test]
fn install_creates_a_missing_path_variable_and_uninstall_unsets_it() {
    let f = fixture();
    let initial = machine(None, false);
    let mut sys = initial.clone();
    app::install(&mut sys, &f.layout, &f.plan).unwrap();
    assert_eq!(sys.vars["Path"], bin());
    app::uninstall(&mut sys, &f.layout, Retry::ONCE).unwrap();
    assert_eq!(sys, initial);
}

// frob:tests crates/goway-setup/src/app.rs::install
#[test]
fn a_second_install_is_refused_until_uninstalled() {
    let f = fixture();
    let mut sys = machine(Some("x"), false);
    app::install(&mut sys, &f.layout, &f.plan).unwrap();
    let err = app::install(&mut sys, &f.layout, &f.plan).unwrap_err();
    assert!(matches!(err, SetupError::AlreadyInstalled { .. }), "{err}");
}

// frob:tests crates/goway-setup/src/app.rs::install
#[test]
fn a_failing_install_rolls_back_everything_it_did() {
    let f = fixture();
    let initial = machine(Some("x"), false);
    let mut sys = initial.clone();
    // The setup payload is missing, so the third change fails after two succeeded.
    sys.files.remove(&PathBuf::from("/stage/setup.exe"));
    let initial = sys.clone();
    let err = app::install(&mut sys, &f.layout, &f.plan).unwrap_err();
    assert!(
        matches!(err, SetupError::InstallFailed { index: 2, .. }),
        "{err}"
    );
    assert_eq!(sys, initial);
    assert!(!f.layout.journal_path.exists());
}

// frob:tests crates/goway-setup/src/app.rs::uninstall
#[test]
fn uninstall_without_a_journal_reports_nothing_to_do() {
    let f = fixture();
    let mut sys = machine(None, false);
    assert!(
        app::uninstall(&mut sys, &f.layout, Retry::ONCE)
            .unwrap()
            .is_none()
    );
}

// frob:tests crates/goway-setup/src/app.rs::uninstall
// frob:tests crates/goway-setup/src/app.rs::status
#[test]
fn edits_made_after_install_are_kept_and_reported() {
    let f = fixture();
    let mut sys = machine(Some("x"), false);
    let journal = app::install(&mut sys, &f.layout, &f.plan).unwrap();
    sys.files
        .insert("/Local/Programs/p/bin/goway.exe".into(), "newer".into());
    sys.vars.insert("Path".into(), format!("x;{};later", bin()));
    let rows = app::status(&sys, &journal).unwrap();
    let exe_row = rows.iter().find(|r| matches!(&r.change, Change::InstallFile { path, .. } if path.ends_with("goway.exe"))).unwrap();
    assert!(!exe_row.holds);
    assert!(rows.iter().filter(|r| r.holds).count() >= 6);

    let report = app::uninstall(&mut sys, &f.layout, Retry::ONCE)
        .unwrap()
        .unwrap();
    assert!(
        report
            .outcomes
            .iter()
            .any(|(_, o)| matches!(o, Outcome::LeftAlone(_)))
    );
    assert_eq!(
        sys.files["/Local/Programs/p/bin/goway.exe".as_ref() as &Path],
        "newer"
    );
    assert_eq!(
        sys.vars["Path"], "x;later",
        "only goway's entry is removed from Path"
    );
}

// frob:tests crates/goway-setup/src/app.rs::runs_from_installed_file
#[test]
fn the_installed_setup_copy_is_detected_so_it_can_relaunch_from_temp() {
    let f = fixture();
    let mut sys = machine(None, false);
    let journal = app::install(&mut sys, &f.layout, &f.plan).unwrap();
    assert!(app::runs_from_installed_file(&journal, &f.layout.setup_exe));
    assert!(app::runs_from_installed_file(
        &journal,
        Path::new("/LOCAL/programs/P/goway-setup.exe")
    ));
    assert!(!app::runs_from_installed_file(
        &journal,
        Path::new("/tmp/goway-setup.exe")
    ));
    let empty = Journal::new("x");
    assert!(!app::runs_from_installed_file(&empty, &f.layout.setup_exe));
}

fn arbitrary_path() -> impl Strategy<Value = Option<String>> {
    proptest::option::of(
        proptest::collection::vec(
            proptest::sample::select(vec![
                r"C:\Windows".to_owned(),
                bin(),
                r"C:\x".to_owned(),
                String::new(),
            ]),
            0..5,
        )
        .prop_map(|v| v.join(";")),
    )
}

// frob:tests crates/goway-setup/src/plan.rs::client_plan
// frob:tests crates/goway-setup/src/app.rs::uninstall
/// Uninstall after install is the identity over arbitrary Path values and registry states.
#[test]
fn client_plan_reverts_exactly() {
    let inputs = (
        arbitrary_path(),
        any::<bool>(),
        any::<bool>(),
        proptest::option::of("[a-z]{1,6}"),
    );
    TestRunner::new(Config::with_cases(300))
        .run(&inputs, |(path, programs, key_exists, old_name)| {
            let f = fixture();
            let mut initial = machine(path.as_deref(), programs);
            if key_exists {
                initial.reg_keys.insert(f.layout.uninstall_key.clone());
            }
            if let Some(n) = old_name {
                initial.registry.insert(
                    (f.layout.uninstall_key.clone(), n),
                    RegValue::String("keep".into()),
                );
            }
            let mut sys = initial.clone();
            app::install(&mut sys, &f.layout, &f.plan).unwrap();
            prop_assert!(sys.vars["Path"].split(';').any(|e| e == bin()));
            app::uninstall(&mut sys, &f.layout, Retry::ONCE)
                .unwrap()
                .unwrap();
            prop_assert_eq!(sys, initial);
            Ok(())
        })
        .unwrap();
}

// frob:tests crates/goway-setup/src/app.rs::load_journal
#[test]
fn load_journal_is_none_before_install_and_some_after() {
    let f = fixture();
    assert!(app::load_journal(&f.layout).unwrap().is_none());
    let mut sys = machine(None, false);
    app::install(&mut sys, &f.layout, &f.plan).unwrap();
    assert!(app::load_journal(&f.layout).unwrap().is_some());
}
