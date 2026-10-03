//! The client component's plan, layout and Uninstall entry values (pure logic).

use std::path::{Path, PathBuf};

use goway_journal::{Change, ListPosition, RegValue, sha256_hex};
use goway_setup::entry::{display_name, uninstall_string, uninstall_values};
use goway_setup::layout::{Layout, validate_profile};
use goway_setup::plan::{Component, Sources, build, client_plan};

fn layout(profile: &str) -> Layout {
    Layout::new(Path::new("/local"), profile).unwrap()
}

fn sources() -> Sources {
    Sources {
        goway_source: PathBuf::from("/stage/goway.exe"),
        goway_digest: sha256_hex(b"goway"),
        setup_source: PathBuf::from("/stage/goway-setup.exe"),
        setup_digest: sha256_hex(b"setup"),
    }
}

// frob:tests crates/goway-setup/src/layout.rs::Layout.new
#[test]
fn layout_places_everything_under_the_profile() {
    let l = layout("goway-test");
    assert_eq!(l.install_root, Path::new("/local/Programs/goway-test"));
    assert_eq!(l.bin_dir, Path::new("/local/Programs/goway-test/bin"));
    assert_eq!(
        l.goway_exe,
        Path::new("/local/Programs/goway-test/bin/goway.exe")
    );
    assert_eq!(
        l.setup_exe,
        Path::new("/local/Programs/goway-test/goway-setup.exe")
    );
    assert_eq!(
        l.journal_path,
        Path::new("/local/goway-test/install-journal.json")
    );
    assert_eq!(
        l.uninstall_key,
        r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\goway-test"
    );
}

// frob:tests crates/goway-setup/src/layout.rs::validate_profile
#[test]
fn profile_names_are_restricted_to_safe_characters() {
    for ok in ["goway", "goway-test", "a.b_c", "X1"] {
        validate_profile(ok).unwrap();
    }
    for bad in ["", ".", "..", "a/b", "a\\b", "a b", "a;b", "é"] {
        assert!(validate_profile(bad).is_err(), "{bad:?} must be rejected");
    }
    assert!(Layout::new(Path::new("/l"), "../x").is_err());
}

// frob:tests crates/goway-setup/src/entry.rs::uninstall_values
// frob:tests crates/goway-setup/src/entry.rs::uninstall_string
// frob:tests crates/goway-setup/src/entry.rs::display_name
#[test]
fn uninstall_entry_has_the_add_remove_programs_values() {
    let l = layout("goway-test");
    let values = uninstall_values(&l, "1.2.3");
    let get = |n: &str| values.iter().find(|(k, _)| *k == n).map(|(_, v)| v.clone());
    assert_eq!(
        get("DisplayName"),
        Some(RegValue::String("goway (goway-test)".into()))
    );
    assert_eq!(
        get("DisplayVersion"),
        Some(RegValue::String("1.2.3".into()))
    );
    assert_eq!(get("Publisher"), Some(RegValue::String("goway".into())));
    assert_eq!(
        get("InstallLocation"),
        Some(RegValue::String("/local/Programs/goway-test".into()))
    );
    assert_eq!(
        get("UninstallString"),
        Some(RegValue::String(
            "\"/local/Programs/goway-test/goway-setup.exe\" uninstall --profile goway-test".into()
        ))
    );
    assert_eq!(get("NoModify"), Some(RegValue::Dword(1)));
    assert_eq!(get("NoRepair"), Some(RegValue::Dword(1)));
    assert_eq!(
        uninstall_string(&l),
        "\"/local/Programs/goway-test/goway-setup.exe\" uninstall --profile goway-test"
    );
    assert_eq!(display_name("goway"), "goway");
}

// frob:tests crates/goway-setup/src/plan.rs::client_plan
#[test]
fn client_plan_orders_directory_files_path_then_registry() {
    let l = layout("goway");
    let plan = client_plan(&l, &sources(), "0.1.0");
    assert!(matches!(&plan[0], Change::EnsureDir { path } if *path == l.bin_dir));
    assert!(matches!(&plan[1], Change::InstallFile { path, .. } if *path == l.goway_exe));
    assert!(matches!(&plan[2], Change::InstallFile { path, .. } if *path == l.setup_exe));
    assert_eq!(
        plan[3],
        Change::EnsureListEntry {
            var: "Path".into(),
            entry: l.bin_dir.display().to_string(),
            separator: ';',
            position: ListPosition::Back,
        }
    );
    assert!(matches!(&plan[4], Change::EnsureRegKey { key } if *key == l.uninstall_key));
    assert!(
        plan[5..]
            .iter()
            .all(|c| matches!(c, Change::SetRegistryValue { key, .. } if *key == l.uninstall_key))
    );
    assert_eq!(plan.len(), 5 + uninstall_values(&l, "0.1.0").len());
}

// frob:tests crates/goway-setup/src/plan.rs::build
#[test]
fn build_dedups_components() {
    let l = layout("goway");
    let once = build(&l, &[Component::Client], &sources(), "0.1.0");
    let twice = build(
        &l,
        &[Component::Client, Component::Client],
        &sources(),
        "0.1.0",
    );
    assert_eq!(once, twice);
}
