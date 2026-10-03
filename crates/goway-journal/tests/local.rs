//! `LocalSystem` against a real temporary directory.

use goway_journal::{
    Change, JournalError, ListPosition, LocalSystem, RegValue, ResourceKind, System, SystemError,
    apply, revert,
};

#[test]
fn files_dirs_lines_and_ini_round_trip_on_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::write(root.join("rc"), "keep").unwrap();
    let plan = [
        Change::EnsureDir {
            path: root.join("a/b"),
        },
        Change::WriteFile {
            path: root.join("a/b/f"),
            contents: "hello".into(),
        },
        Change::EnsureLine {
            path: root.join("rc"),
            line: "x".into(),
            marker: "# goway".into(),
        },
        Change::SetIniKey {
            path: root.join(".wslconfig"),
            section: "wsl2".into(),
            key: "networkingMode".into(),
            value: "mirrored".into(),
        },
    ];
    let mut sys = LocalSystem;
    let mut j = apply(&plan, &mut sys).unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("rc")).unwrap(),
        "keep\nx # goway\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join(".wslconfig")).unwrap(),
        "[wsl2]\nnetworkingMode=mirrored\n"
    );
    revert(&mut j, &mut sys).unwrap();
    assert_eq!(std::fs::read_to_string(root.join("rc")).unwrap(), "keep");
    assert!(!root.join("a").exists());
    assert!(!root.join(".wslconfig").exists());
}

#[test]
fn non_empty_directory_is_kept_on_revert() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("d");
    let mut sys = LocalSystem;
    let mut j = apply(&[Change::EnsureDir { path: dir.clone() }], &mut sys).unwrap();
    std::fs::write(dir.join("user-file"), "x").unwrap();
    revert(&mut j, &mut sys).unwrap();
    assert!(dir.join("user-file").exists());
    assert!(matches!(
        sys.remove_dir(&dir),
        Err(SystemError::InvalidState(_))
    ));
}

#[cfg(unix)]
#[test]
fn unix_mode_is_set_and_restored() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("f");
    std::fs::write(&f, "x").unwrap();
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
    let mut sys = LocalSystem;
    let mut j = apply(
        &[Change::SetUnixMode {
            path: f.clone(),
            mode: 0o600,
        }],
        &mut sys,
    )
    .unwrap();
    assert_eq!(sys.get_mode(&f).unwrap(), 0o600);
    revert(&mut j, &mut sys).unwrap();
    assert_eq!(sys.get_mode(&f).unwrap(), 0o644);
    assert!(matches!(
        sys.get_mode(&tmp.path().join("none")),
        Err(SystemError::NotFound(_))
    ));
}

#[test]
fn registry_acl_vars_and_resources_are_unsupported() {
    let mut sys = LocalSystem;
    let unsupported = |e: SystemError| matches!(e, SystemError::Unsupported(_));
    assert!(unsupported(sys.reg_get("K", "n").unwrap_err()));
    assert!(unsupported(
        sys.reg_set("K", "n", &RegValue::Dword(1)).unwrap_err()
    ));
    assert!(unsupported(sys.reg_delete("K", "n").unwrap_err()));
    assert!(unsupported(sys.get_acl("/x".as_ref()).unwrap_err()));
    assert!(unsupported(sys.set_acl("/x".as_ref(), "D:").unwrap_err()));
    assert!(unsupported(sys.get_var("PATH").unwrap_err()));
    assert!(unsupported(sys.set_var("PATH", "a").unwrap_err()));
    assert!(unsupported(sys.remove_var("PATH").unwrap_err()));
    assert!(unsupported(
        sys.resource_exists(ResourceKind::Service, "s").unwrap_err()
    ));
    assert!(unsupported(
        sys.resource_create(ResourceKind::Service, "s", "x")
            .unwrap_err()
    ));
    assert!(unsupported(
        sys.resource_delete(ResourceKind::Service, "s").unwrap_err()
    ));
    let plan = [Change::EnsureListEntry {
        var: "PATH".into(),
        entry: "a".into(),
        separator: ':',
        position: ListPosition::Back,
    }];
    let err = apply(&plan, &mut sys).unwrap_err();
    assert!(matches!(
        err.source,
        JournalError::System(SystemError::Unsupported(_))
    ));
}

#[test]
fn install_file_copies_binary_bytes_and_reverts_on_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let bytes: Vec<u8> = (0..=255u8).collect();
    std::fs::write(root.join("src.bin"), &bytes).unwrap();
    let plan = [Change::InstallFile {
        path: root.join("dest.bin"),
        source: root.join("src.bin"),
        digest: goway_journal::sha256_hex(&bytes),
    }];
    let mut sys = LocalSystem;
    let mut journal = apply(&plan, &mut sys).unwrap();
    assert_eq!(std::fs::read(root.join("dest.bin")).unwrap(), bytes);
    revert(&mut journal, &mut sys).unwrap();
    assert!(!root.join("dest.bin").exists());
    assert!(root.join("src.bin").exists());
}

// frob:tests crates/goway-journal/src/local.rs::LocalSystem.resource_create
// frob:tests crates/goway-journal/src/local.rs::LocalSystem.resource_exists
// frob:tests crates/goway-journal/src/local.rs::LocalSystem.resource_delete
#[test]
fn ssh_key_pair_is_created_and_reverted_on_disk() {
    use goway_journal::{Change, LocalSystem, ResourceKind, apply, revert};
    let dir = tempfile::tempdir().unwrap();
    let key = dir.path().join("id_test");
    let name = key.to_string_lossy().into_owned();
    let mut sys = LocalSystem;
    let plan = [Change::EnsureResource {
        kind: ResourceKind::SshKeyPair,
        name: name.clone(),
        spec: "goway test".to_owned(),
    }];
    let mut journal = apply(&plan, &mut sys).unwrap();
    assert!(key.is_file());
    let public = std::fs::read_to_string(dir.path().join("id_test.pub")).unwrap();
    assert!(public.starts_with("ssh-ed25519 ") && public.trim_end().ends_with("goway test"));
    revert(&mut journal, &mut sys).unwrap();
    assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());

    // A key that existed before is never deleted by revert.
    std::fs::write(&key, "pre-existing").unwrap();
    std::fs::write(dir.path().join("id_test.pub"), "pre-existing").unwrap();
    let mut journal = apply(&plan, &mut sys).unwrap();
    revert(&mut journal, &mut sys).unwrap();
    assert_eq!(std::fs::read_to_string(&key).unwrap(), "pre-existing");
}
