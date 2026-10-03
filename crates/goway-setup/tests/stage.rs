//! The UAC relaunch elevates a hashed private copy, never the user-writable download.

use goway_setup::stage::{stage_in, valid_sha256, verify_image};

// frob:tests crates/goway-setup/src/stage.rs::stage_in
#[test]
fn staging_copies_the_exe_hashes_it_and_cleans_up_on_drop() {
    let tmp = tempfile::tempdir().unwrap();
    let exe = tmp.path().join("goway-setup.exe");
    std::fs::write(&exe, b"MZ original bytes").unwrap();
    let staged = stage_in(tmp.path(), &exe).unwrap();
    assert_ne!(staged.path, exe, "the copy is elevated, not the download");
    assert!(staged.path.starts_with(tmp.path()));
    assert_eq!(std::fs::read(&staged.path).unwrap(), b"MZ original bytes");
    assert!(valid_sha256(&staged.sha256));
    // Swapping the download afterwards changes nothing about what is elevated.
    std::fs::write(&exe, b"MZ attacker bytes").unwrap();
    assert_eq!(std::fs::read(&staged.path).unwrap(), b"MZ original bytes");
    assert!(verify_image(&staged.path, &staged.sha256).is_ok());
    let (dir, path) = (
        staged.path.parent().unwrap().to_path_buf(),
        staged.path.clone(),
    );
    drop(staged);
    assert!(!path.exists() && !dir.exists());
}

// frob:tests crates/goway-setup/src/stage.rs::stage_in
#[test]
#[cfg(unix)]
fn each_staging_gets_its_own_owner_only_directory() {
    use std::os::unix::fs::PermissionsExt as _;
    let tmp = tempfile::tempdir().unwrap();
    let exe = tmp.path().join("setup.exe");
    std::fs::write(&exe, b"x").unwrap();
    let a = stage_in(tmp.path(), &exe).unwrap();
    let b = stage_in(tmp.path(), &exe).unwrap();
    assert_ne!(a.path.parent(), b.path.parent());
    let mode = std::fs::metadata(a.path.parent().unwrap())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o077, 0, "no access for group or others");
}

// frob:tests crates/goway-setup/src/stage.rs::stage_in
#[test]
fn a_missing_source_fails_and_leaves_no_directory() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(stage_in(tmp.path(), &tmp.path().join("nope.exe")).is_err());
    assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
}

// frob:tests crates/goway-setup/src/stage.rs::verify_image
#[test]
fn the_elevated_side_refuses_an_image_that_differs_from_the_locked_digest() {
    let tmp = tempfile::tempdir().unwrap();
    let exe = tmp.path().join("a.exe");
    std::fs::write(&exe, b"genuine").unwrap();
    let staged = stage_in(tmp.path(), &exe).unwrap();
    let evil = tmp.path().join("evil.exe");
    std::fs::write(&evil, b"evil").unwrap();
    assert!(verify_image(&evil, &staged.sha256).is_err());
    for bad in ["", "abc", &"G".repeat(64), &staged.sha256.to_uppercase()] {
        assert!(verify_image(&staged.path, bad).is_err(), "{bad}");
    }
}

// frob:tests crates/goway-setup/src/stage.rs::stage_in
#[test]
#[cfg(windows)]
fn while_staged_the_copy_cannot_be_written_renamed_or_deleted() {
    let tmp = tempfile::tempdir().unwrap();
    let exe = tmp.path().join("a.exe");
    std::fs::write(&exe, b"genuine").unwrap();
    let staged = stage_in(tmp.path(), &exe).unwrap();
    assert!(std::fs::write(&staged.path, b"evil").is_err(), "write");
    assert!(std::fs::remove_file(&staged.path).is_err(), "delete");
    assert!(
        std::fs::rename(&staged.path, tmp.path().join("moved.exe")).is_err(),
        "rename"
    );
    assert!(std::fs::read(&staged.path).is_ok(), "reading still works");
    assert_eq!(std::fs::read(&staged.path).unwrap(), b"genuine");
}
