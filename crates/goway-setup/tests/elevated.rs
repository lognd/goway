//! The elevated host uninstall must not trust anything a non-administrator could have planted:
//! the state directory's owner and ACL are checked, and every journal entry must be one the host
//! plan itself produces (model system, so the refusals are proven without touching a machine).

use std::path::{Path, PathBuf};

use goway_journal::{Change, Journal, ModelSystem, Prior, ResourceKind};
use goway_setup::admin::{self, check_sddl};
use goway_setup::app::{self, Retry};
use goway_setup::error::SetupError;
use goway_setup::host::{
    self, DEFAULT_PORT, HostFacts, HostParams, HostSettings, Keepalive, has_wildcard, host_plan,
};
use goway_setup::layout::Layout;

const HOME: &str = "/home/u";

fn settings() -> HostSettings {
    HostSettings {
        distro: "Ubuntu".into(),
        port: 2299,
        allow_from: Vec::new(),
        network: host::NetworkMode::Mirrored,
    }
}

fn layout_in(root: &Path) -> Layout {
    // The real ProgramData always exists; only the goway directories below it are created.
    let program_data = root.join("ProgramData");
    std::fs::create_dir_all(&program_data).unwrap();
    let mut l = Layout::new(Path::new("/Local"), &program_data, "p").unwrap();
    l.state_dir = root.join("user-state");
    l.journal_path = l.state_dir.join("install-journal.json");
    l
}

fn machine() -> ModelSystem {
    let mut m = ModelSystem::new();
    for d in ["/home", HOME, "/etc"] {
        m.dirs.insert(d.into());
    }
    m
}

fn plan(layout: &Layout, harden: bool) -> Vec<Change> {
    host_plan(
        layout,
        &HostParams {
            port: 2299,
            distro: "Ubuntu".into(),
            keepalive: Keepalive::Logon,
            harden,
            allow_from: Vec::new(),
            home: PathBuf::from(HOME),
            network: host::NetworkMode::Mirrored,
        },
        &HostFacts::assumed(),
    )
}

/// Install the real host plan into a model machine with the journal in a real admin-style dir.
fn installed(root: &Path) -> (Layout, ModelSystem, Journal) {
    let layout = layout_in(root);
    let view = layout.host_view();
    let mut sys = machine();
    let journal = app::install(&mut sys, &view, &plan(&layout, true)).unwrap();
    (layout, sys, journal)
}

fn check(layout: &Layout) -> impl FnOnce(&Journal) -> Result<(), SetupError> + '_ {
    move |j| host::validate_journal(j, layout, &settings(), Path::new(HOME))
}

fn plant(layout: &Layout, change: Change, prior: Prior) {
    let mut journal = Journal::load(&layout.host_journal_path).unwrap();
    journal.entries.push(goway_journal::Entry {
        change,
        prior,
        reverted: false,
    });
    journal.save(&layout.host_journal_path).unwrap();
}

fn refused(result: &Result<Option<app::UninstallReport>, SetupError>) -> bool {
    matches!(result, Err(SetupError::UntrustedState { .. }))
}

// frob:tests crates/goway-setup/src/host.rs::validate_journal
// frob:tests crates/goway-setup/src/host.rs::expected_changes
// frob:tests crates/goway-setup/src/app.rs::uninstall_checked
#[test]
fn a_genuine_host_journal_is_accepted_and_reverts_to_the_start() {
    let tmp = tempfile::tempdir().unwrap();
    let (layout, mut sys, _) = installed(tmp.path());
    let before = machine();
    let report = app::uninstall_checked(&mut sys, &layout.host_view(), Retry::ONCE, check(&layout));
    assert!(report.unwrap().is_some());
    assert_eq!(sys, before);
}

// frob:tests crates/goway-setup/src/host.rs::validate_journal
// frob:tests crates/goway-setup/src/app.rs::uninstall_checked
#[test]
fn a_planted_write_file_outside_the_host_plan_is_refused_before_anything_is_reverted() {
    let tmp = tempfile::tempdir().unwrap();
    let (layout, mut sys, _) = installed(tmp.path());
    plant(
        &layout,
        Change::WriteFile {
            path: PathBuf::from("C:\\Windows\\x"),
            contents: "evil".into(),
        },
        Prior::File {
            contents: Some("payload".into()),
        },
    );
    let installed_state = sys.clone();
    let result = app::uninstall_checked(&mut sys, &layout.host_view(), Retry::ONCE, check(&layout));
    assert!(refused(&result), "{result:?}");
    assert_eq!(sys, installed_state, "nothing was reverted");
    assert!(
        layout.host_journal_path.exists(),
        "the journal is left for inspection"
    );
}

// frob:tests crates/goway-setup/src/host.rs::validate_journal
// frob:tests crates/goway-setup/src/host.rs::has_wildcard
#[test]
fn a_wildcard_resource_name_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let (layout, mut sys, _) = installed(tmp.path());
    for kind in [
        ResourceKind::FirewallRule,
        ResourceKind::ScheduledTask,
        ResourceKind::HyperVFirewallRule,
    ] {
        plant(
            &layout,
            Change::EnsureResource {
                kind,
                name: "*".into(),
                spec: String::new(),
            },
            Prior::ResourceCreated,
        );
        let result =
            app::uninstall_checked(&mut sys, &layout.host_view(), Retry::ONCE, check(&layout));
        assert!(refused(&result), "{kind:?}: {result:?}");
    }
    assert!(
        has_wildcard("*")
            && has_wildcard("a?")
            && has_wildcard("[a]")
            && !has_wildcard("WSL SSH 2299")
    );
}

// frob:tests crates/goway-setup/src/host.rs::validate_journal
#[test]
fn registry_acl_install_file_and_other_planted_kinds_are_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let layout = layout_in(tmp.path());
    let genuine = plan(&layout, true);
    let ok = |change: &Change| {
        let mut j = Journal::new("t");
        j.entries.push(goway_journal::Entry {
            change: change.clone(),
            prior: Prior::Noop,
            reverted: false,
        });
        host::validate_journal(&j, &layout, &settings(), Path::new(HOME)).is_ok()
    };
    assert!(genuine.iter().all(ok), "every genuine change is accepted");
    let planted = [
        Change::SetRegistryValue {
            key: r"HKLM\Software\Microsoft\Windows\CurrentVersion\Run".into(),
            name: "x".into(),
            value: goway_journal::RegValue::String("evil.exe".into()),
        },
        Change::SetAcl {
            path: PathBuf::from("C:\\Windows"),
            sddl: "D:(A;;GA;;;WD)".into(),
        },
        Change::InstallFile {
            path: PathBuf::from("C:\\Windows\\System32\\x.dll"),
            source: PathBuf::from("C:\\u\\x"),
            digest: "00".into(),
        },
        Change::EnsureDir {
            path: PathBuf::from("C:\\Windows"),
        },
        Change::EnsureResource {
            kind: ResourceKind::FirewallRule,
            name: "Core Networking".into(),
            spec: String::new(),
        },
        // A genuine-looking write to the wrong place: the port drop-in of another profile.
        Change::WriteFile {
            path: PathBuf::from("/etc/ssh/sshd_config.d/20-other-port.conf"),
            contents: "Port 2299\n".into(),
        },
    ];
    for change in &planted {
        assert!(!ok(change), "{change:?} must be refused");
    }
}

// frob:tests crates/goway-setup/src/host.rs::validate_journal
#[test]
fn a_directory_removal_prior_cannot_reach_outside_its_own_path() {
    let tmp = tempfile::tempdir().unwrap();
    let layout = layout_in(tmp.path());
    let mut j = Journal::new("t");
    j.entries.push(goway_journal::Entry {
        change: Change::EnsureDir {
            path: PathBuf::from("/etc/ssh/sshd_config.d"),
        },
        prior: Prior::DirsCreated {
            created: vec![PathBuf::from("/etc/ssh"), PathBuf::from("/etc")],
        },
        reverted: false,
    });
    assert!(host::validate_journal(&j, &layout, &settings(), Path::new(HOME)).is_ok());
    j.entries[0].prior = Prior::DirsCreated {
        created: vec![PathBuf::from("/usr/lib")],
    };
    assert!(host::validate_journal(&j, &layout, &settings(), Path::new(HOME)).is_err());
}

// frob:tests crates/goway-setup/src/host.rs::HostSettings.validate
// frob:tests crates/goway-setup/src/app.rs::load_settings
#[test]
fn settings_with_a_bad_distro_or_port_are_refused_when_loaded() {
    let tmp = tempfile::tempdir().unwrap();
    let layout = layout_in(tmp.path());
    app::prepare_admin_dir(&layout).unwrap();
    for text in [
        r#"{"distro":"Ubuntu';calc","port":2222}"#,
        r#"{"distro":"Ubuntu","port":0}"#,
        "not json",
    ] {
        std::fs::write(&layout.host_settings_path, text).unwrap();
        assert!(
            matches!(
                app::load_settings(&layout),
                Err(SetupError::UntrustedState { .. } | SetupError::BadDistro(_))
            ),
            "{text}"
        );
    }
    std::fs::write(
        &layout.host_settings_path,
        format!(r#"{{"distro":"Ubuntu","port":{DEFAULT_PORT}}}"#),
    )
    .unwrap();
    assert_eq!(
        app::load_settings(&layout).unwrap().unwrap().port,
        DEFAULT_PORT
    );
}

const GOOD: &str = "O:BAD:PAI(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)(A;OICI;0x1200a9;;;BU)";

// frob:tests crates/goway-setup/src/admin.rs::check_sddl
#[test]
fn the_admin_dir_acl_check_accepts_what_goway_creates() {
    assert!(check_sddl(GOOD).is_ok());
    assert!(check_sddl(admin::ADMIN_DIR_SDDL).is_ok());
    // The same grants as symbolic tokens, plus deny entries and a TrustedInstaller owner.
    // Inherit-only entries (the ProgramData default for creator-owner) do not apply to the directory.
    assert!(check_sddl("O:SYD:PAI(A;OICI;FA;;;SY)(A;OICIIO;FA;;;CO)").is_ok());
    assert!(check_sddl("O:BAD:P(D;;FW;;;WD)(A;OICI;FA;;;BA)(A;OICI;GRGX;;;BU)").is_ok());
    assert!(
        check_sddl(
            "O:S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464D:P(A;;FA;;;SY)"
        )
        .is_ok()
    );
}

// frob:tests crates/goway-setup/src/admin.rs::check_sddl
#[test]
fn the_admin_dir_acl_check_refuses_a_wrong_owner_or_a_writable_acl() {
    let bad = [
        // Owned by an ordinary user (a directory they pre-created): they could rewrite its ACL.
        (
            "O:S-1-5-21-1-2-3-1001D:P(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)",
            "owner",
        ),
        ("O:BUD:P(A;OICI;FA;;;BA)", "Users as owner"),
        // Users or Everyone may write, by token or by mask.
        ("O:BAD:P(A;OICI;FA;;;BA)(A;OICI;FA;;;BU)", "Users full"),
        ("O:BAD:P(A;OICI;FA;;;BA)(A;OICI;GRGW;;;WD)", "Everyone GW"),
        (
            "O:BAD:P(A;OICI;FA;;;BA)(A;OICI;0x1301bf;;;AU)",
            "Authenticated modify",
        ),
        (
            "O:BAD:P(A;OICI;0x100000;;;BU)(A;OICI;DCSD;;;BU)",
            "delete rights",
        ),
        // A specific user SID, and the creator-owner entry ProgramData subfolders inherit.
        ("O:BAD:P(A;OICI;FA;;;S-1-5-21-1-2-3-1001)", "user sid"),
        (
            "O:SYD:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;CO)",
            "creator owner on the directory itself",
        ),
        // No DACL at all means everyone has full access.
        ("O:BA", "no dacl"),
        (
            "O:BAD:P(A;OICI;FA;;;BA)(A;OICI;GRGW;;;BU)(A;IO;FA;;;WD)",
            "one bad entry among inherit-only",
        ),
        ("D:P(A;;FA;;;BA)", "no owner"),
        ("O:BAD:P(A;OICI;FA;;BA)", "short ace"),
    ];
    for (sddl, why) in bad {
        let reason = check_sddl(sddl);
        assert!(reason.is_err(), "{why}: {sddl} was accepted");
    }
}

// frob:tests crates/goway-setup/src/admin.rs::valid_log_name
// frob:tests crates/goway-setup/src/admin.rs::new_log_name
#[test]
fn elevated_log_names_are_plain_file_names() {
    assert!(admin::valid_log_name(&admin::new_log_name()));
    for bad in [
        "..\\x.log",
        "../x.log",
        "elevated-..log",
        "elevated-a\\b.log",
        "other.log",
        "elevated-x.txt",
        "elevated-C:x.log",
    ] {
        assert!(!admin::valid_log_name(bad), "{bad}");
    }
}

#[cfg(unix)]
// frob:tests crates/goway-setup/src/admin.rs::verify
// frob:tests crates/goway-setup/src/admin.rs::ensure
#[test]
fn a_loosened_or_linked_state_directory_is_refused_off_windows_too() {
    use std::os::unix::fs::PermissionsExt as _;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("a").join("b");
    admin::ensure(&dir).unwrap();
    admin::verify(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
    assert!(matches!(
        admin::verify(&dir),
        Err(SetupError::UntrustedState { .. })
    ));
    std::fs::write(dir.join("planted"), "x").unwrap();
    let err = admin::ensure(&dir).unwrap_err();
    assert!(
        err.to_string().contains("probably created by another user")
            && err.to_string().contains("rd /s /q"),
        "a loose dir with contents is not adopted and the way out is explained: {err}"
    );
    assert!(dir.join("planted").exists(), "nothing of theirs is deleted");
    std::fs::remove_file(dir.join("planted")).unwrap();
    // An empty pre-created directory is replaced by a proper one instead of blocking installs.
    admin::ensure(&dir).unwrap();
    admin::verify(&dir).unwrap();
    assert_eq!(
        std::fs::metadata(&dir).unwrap().permissions().mode() & 0o022,
        0
    );
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
    let link = tmp.path().join("link");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink(&dir, &link).unwrap();
    assert!(
        admin::verify(&link).is_err(),
        "a link is not a trusted directory"
    );
}

// frob:tests crates/goway-setup/src/app.rs::load_journal
#[test]
fn a_journal_in_an_untrusted_directory_is_not_read() {
    let tmp = tempfile::tempdir().unwrap();
    let (layout, _, _) = installed(tmp.path());
    let view = layout.host_view();
    assert!(app::load_journal(&view).unwrap().is_some());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&layout.admin_dir, std::fs::Permissions::from_mode(0o777))
            .unwrap();
        assert!(matches!(
            app::load_journal(&view),
            Err(SetupError::UntrustedState { .. })
        ));
        assert!(matches!(
            app::ensure_not_installed(&view),
            Err(SetupError::UntrustedState { .. })
        ));
    }
}

// frob:tests crates/goway-setup/src/admin.rs::install_protected_exe
// frob:tests crates/goway-setup/src/admin.rs::purge
// frob:tests crates/goway-setup/src/admin.rs::remove_old_logs
// frob:tests crates/goway-setup/src/admin.rs::same_file
#[test]
fn the_protected_exe_is_copied_and_purged_with_the_state_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let layout = layout_in(tmp.path());
    app::prepare_admin_dir(&layout).unwrap();
    let exe = tmp.path().join("goway-setup.exe");
    std::fs::write(&exe, b"setup bytes").unwrap();
    let copy = admin::install_protected_exe(&layout.admin_dir, &exe).unwrap();
    assert_eq!(copy, admin::protected_exe(&layout.admin_dir));
    assert_eq!(std::fs::read(&copy).unwrap(), b"setup bytes");
    let log = layout.admin_dir.join(admin::new_log_name());
    std::fs::write(&log, "out").unwrap();
    std::fs::write(layout.admin_dir.join("elevated-keep.log"), "keep").unwrap();
    admin::remove_old_logs(&layout.admin_dir, Some("elevated-keep.log"));
    assert!(!log.exists() && layout.admin_dir.join("elevated-keep.log").exists());
    // The running exe is the protected copy and a log is in use: they stay, the rest goes.
    assert!(!admin::purge(
        &layout.admin_dir,
        &layout.admin_root,
        &copy,
        Some("elevated-keep.log")
    ));
    assert!(copy.exists() && layout.admin_dir.exists());
    // Once nothing is running from it, everything including both directories goes.
    assert!(admin::purge(
        &layout.admin_dir,
        &layout.admin_root,
        &exe,
        None
    ));
    assert!(!layout.admin_dir.exists() && !layout.admin_root.exists());
}

#[cfg(windows)]
// frob:tests crates/goway-setup/src/sysapi.rs::owner_and_dacl_sddl
// frob:tests crates/goway-setup/src/admin.rs::verify
#[test]
fn the_real_security_descriptor_reader_agrees_with_the_checker() {
    use goway_setup::sysapi::{owner_and_dacl_sddl, windows_dir};
    // C:\Windows is owned by TrustedInstaller and no ordinary user can write it.
    let windows = windows_dir().unwrap();
    let sddl = owner_and_dacl_sddl(&windows).unwrap();
    assert!(sddl.starts_with("O:"), "{sddl}");
    assert_eq!(check_sddl(&sddl), Ok(()), "{sddl}");
    assert!(admin::verify(&windows).is_ok());
    // A directory the current user just made is owned by that user: refused.
    let tmp = tempfile::tempdir().unwrap();
    assert!(
        matches!(
            admin::verify(tmp.path()),
            Err(SetupError::UntrustedState { .. })
        ),
        "{:?}",
        owner_and_dacl_sddl(tmp.path())
    );
}

#[cfg(windows)]
// frob:tests crates/goway-setup/src/sysapi.rs::create_dir_with_sddl
#[test]
fn a_directory_is_created_with_its_acl_from_the_start() {
    use goway_setup::sysapi::{create_dir_with_sddl, owner_and_dacl_sddl};
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("protected");
    create_dir_with_sddl(&dir, "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)").unwrap();
    let sddl = owner_and_dacl_sddl(&dir).unwrap();
    assert!(
        sddl.contains("D:P"),
        "protected, no inherited entries: {sddl}"
    );
    assert!(
        sddl.contains("(A;OICI;FA;;;BA)") && sddl.contains("(A;OICI;FA;;;SY)"),
        "{sddl}"
    );
    let again = create_dir_with_sddl(&dir, "D:P(A;OICI;FA;;;SY)");
    assert_eq!(again.unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
}

// frob:tests crates/goway-setup/src/host.rs::validate_journal
// frob:tests crates/goway-setup/src/host.rs::expected_changes
#[test]
fn the_elevated_validator_accepts_exactly_the_host_arp_entry() {
    use goway_journal::RegValue;
    let tmp = tempfile::tempdir().unwrap();
    let layout = layout_in(tmp.path());
    let ok = |change: Change| {
        let mut j = Journal::new("t");
        j.entries.push(goway_journal::Entry {
            change,
            prior: Prior::Noop,
            reverted: false,
        });
        host::validate_journal(&j, &layout, &settings(), Path::new(HOME)).is_ok()
    };
    let value = |key: &str, name: &str, v: RegValue| Change::SetRegistryValue {
        key: key.into(),
        name: name.into(),
        value: v,
    };
    let key = layout.host_uninstall_key.clone();
    assert!(ok(Change::EnsureRegKey { key: key.clone() }));
    // An uninstaller of another version still accepts the entry an older install wrote.
    assert!(ok(value(
        &key,
        "DisplayVersion",
        RegValue::String("0.0.9-rc.1".into())
    )));
    // A version string is not a way to smuggle anything else in.
    assert!(!ok(value(
        &key,
        "DisplayVersion",
        RegValue::String("1; evil".into())
    )));
    // The command the entry runs is exact: never another exe, never other arguments.
    let evil = r#""C:\Users\u\evil.exe" uninstall --host --profile p"#;
    assert!(!ok(value(
        &key,
        "UninstallString",
        RegValue::String(evil.into())
    )));
    assert!(!ok(value(&key, "Run", RegValue::String("x".into()))));
    // The client's per-user key is not the host's to touch.
    assert!(!ok(Change::EnsureRegKey {
        key: layout.uninstall_key.clone()
    }));
    assert!(!ok(Change::EnsureRegKey {
        key: r"HKLM\Software\Microsoft\Windows\CurrentVersion\Uninstall\other-host".into()
    }));
}

// frob:tests crates/goway-setup/src/host.rs::validate_journal
#[test]
fn a_planted_prior_is_refused_before_anything_is_reverted() {
    let tmp = tempfile::tempdir().unwrap();
    let (layout, _sys, journal) = installed(tmp.path());
    let ini = journal
        .entries
        .iter()
        .position(|e| matches!(&e.change, Change::SetIniKey { key, .. } if key == "networkingMode"))
        .expect("the mirrored plan sets networkingMode");
    let write = journal
        .entries
        .iter()
        .position(|e| matches!(e.change, Change::WriteFile { .. }))
        .unwrap();
    let with = |index: usize, prior: Prior| {
        let mut j = journal.clone();
        j.entries[index].prior = prior;
        host::validate_journal(&j, &layout, &settings(), Path::new(HOME))
    };
    // Honest priors pass, whatever their kind.
    assert!(
        with(
            ini,
            Prior::IniReplaced {
                original_line: "networkingMode=nat".into()
            }
        )
        .is_ok()
    );
    assert!(with(ini, Prior::Noop).is_ok());
    // A different key, several lines, or the wrong kind of prior is refused.
    for bad in [
        "processors=99",
        "networkingMode=nat\n[boot]\ncommand=calc",
        "networkingMode=nat\r",
        "no equals sign",
    ] {
        let err = with(
            ini,
            Prior::IniReplaced {
                original_line: bad.into(),
            },
        );
        assert!(
            matches!(err, Err(SetupError::UntrustedState { .. })),
            "{bad:?}"
        );
    }
    assert!(with(ini, Prior::Mode { mode: 0o777 }).is_err());
    assert!(with(write, Prior::Mode { mode: 0o777 }).is_err());
    assert!(
        with(
            write,
            Prior::Acl {
                sddl: "D:(A;;GA;;;WD)".into()
            }
        )
        .is_err()
    );
    assert!(
        with(
            write,
            Prior::File {
                contents: Some("x".repeat(2 << 20))
            }
        )
        .is_err()
    );
    assert!(
        with(
            write,
            Prior::File {
                contents: Some("old".into())
            }
        )
        .is_ok()
    );
}
