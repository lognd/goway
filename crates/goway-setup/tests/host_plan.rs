//! The host component's plan: its content, and the round trip over a model machine in which
//! any subset of the targets already exists.

use std::path::{Path, PathBuf};

use goway_journal::{
    Change, ModelSystem, Outcome, Prior, ResourceKind, apply, revert, still_applied,
};
use goway_setup::app::{self, Retry};
use goway_setup::host::{
    self, DEFAULT_PORT, HostFacts, HostParams, HostSettings, Keepalive, host_plan,
};
use goway_setup::layout::Layout;
use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};

const HOME: &str = "/home/u";

fn layout(profile: &str) -> Layout {
    Layout::new(Path::new("/Local"), Path::new("/ProgramData"), profile).unwrap()
}

fn params(port: u16, keepalive: Keepalive, harden: bool) -> HostParams {
    HostParams {
        port,
        distro: "Ubuntu".into(),
        keepalive,
        harden,
        home: PathBuf::from(HOME),
    }
}

fn kinds(plan: &[Change], kind: ResourceKind) -> Vec<&str> {
    plan.iter()
        .filter_map(|c| match c {
            Change::EnsureResource { kind: k, name, .. } if *k == kind => Some(name.as_str()),
            _ => None,
        })
        .collect()
}

// frob:tests crates/goway-setup/src/host.rs::host_plan
// frob:tests crates/goway-setup/src/host.rs::firewall_rule_name
// frob:tests crates/goway-setup/src/host.rs::hyperv_rule_name
// frob:tests crates/goway-setup/src/host.rs::task_name
#[test]
fn the_default_profile_reuses_the_names_of_the_hand_made_setup() {
    let plan = host_plan(
        &layout("goway"),
        &params(2222, Keepalive::Logon, false),
        &HostFacts::assumed(),
    );
    assert_eq!(kinds(&plan, ResourceKind::FirewallRule), ["WSL SSH 2222"]);
    assert_eq!(
        kinds(&plan, ResourceKind::HyperVFirewallRule),
        ["WSL SSH 2222 (Hyper-V)"]
    );
    assert_eq!(kinds(&plan, ResourceKind::ScheduledTask), ["WSL Keepalive"]);
    assert_eq!(kinds(&plan, ResourceKind::WslPackage), ["openssh-server"]);
    assert_eq!(
        kinds(&plan, ResourceKind::WslUnit),
        ["ssh.socket", "ssh.service"]
    );
    assert!(
        matches!(&plan[0], Change::SetIniKey { path, section, key, value }
        if path == Path::new("/home/u/.wslconfig") && section == "wsl2" && key == "networkingMode" && value == "mirrored")
    );
}

// frob:tests crates/goway-setup/src/host.rs::host_plan
// frob:tests crates/goway-setup/src/host.rs::port_dropin_path
// frob:tests crates/goway-setup/src/host.rs::hardening_dropin_path
// frob:tests crates/goway-setup/src/host.rs::port_dropin
// frob:tests crates/goway-setup/src/host.rs::hardening_dropin
#[test]
fn a_test_profile_gets_its_own_names_and_files() {
    let plan = host_plan(
        &layout("goway-test"),
        &params(2299, Keepalive::Boot, true),
        &HostFacts::assumed(),
    );
    assert_eq!(
        kinds(&plan, ResourceKind::FirewallRule),
        ["goway-test WSL SSH 2299"]
    );
    assert_eq!(
        kinds(&plan, ResourceKind::ScheduledTask),
        ["goway-test WSL Keepalive (boot)"]
    );
    let written: Vec<String> = plan
        .iter()
        .filter_map(|c| match c {
            Change::WriteFile { path, contents } => Some(format!(
                "{}: {}",
                path.display(),
                contents.lines().last().unwrap()
            )),
            _ => None,
        })
        .collect();
    assert_eq!(
        written,
        [
            "/etc/ssh/sshd_config.d/20-goway-test-port.conf: Port 2299",
            "/etc/ssh/sshd_config.d/10-goway-test-hardening.conf: PasswordAuthentication no",
        ]
    );
}

// frob:tests crates/goway-setup/src/host.rs::host_plan
#[test]
fn optional_steps_follow_the_probed_facts() {
    let l = layout("goway");
    let p = params(2222, Keepalive::Logon, true);
    let none = HostFacts {
        hyperv_firewall: false,
        sshd_ports: vec![2222],
        authorized_keys: false,
    };
    let plan = host_plan(&l, &p, &none);
    assert!(kinds(&plan, ResourceKind::HyperVFirewallRule).is_empty());
    assert!(
        !plan.iter().any(|c| matches!(c, Change::WriteFile { .. })),
        "the port is already configured and no key is authorized, so no drop-in at all"
    );
}

// frob:tests crates/goway-setup/src/host.rs::validate_distro
#[test]
fn distro_names_are_restricted_to_safe_characters() {
    for ok in ["Ubuntu", "Ubuntu-24.04", "my_distro"] {
        host::validate_distro(ok).unwrap();
    }
    for bad in ["", "a b", "a'b", "a\"b", "a;b", "\u{e9}"] {
        assert!(host::validate_distro(bad).is_err(), "{bad:?}");
    }
}

fn base_machine() -> ModelSystem {
    let mut m = ModelSystem::new();
    for d in ["/home", HOME, "/etc"] {
        m.dirs.insert(d.into());
    }
    m
}

/// A machine as the two test laptops are: everything the plan wants is already there.
fn laptop() -> ModelSystem {
    let mut m = base_machine();
    m.files.insert(
        "/home/u/.wslconfig".into(),
        "[wsl2]\r\ndnsTunneling = false\r\nnetworkingMode = mirrored".into(),
    );
    m.files.insert(
        "/etc/wsl.conf".into(),
        "[boot]\nsystemd=true\n[user]\ndefault=u\n".into(),
    );
    m.dirs.insert("/etc/ssh".into());
    m.dirs.insert("/etc/ssh/sshd_config.d".into());
    m.files.insert(
        "/etc/ssh/sshd_config.d/port.conf".into(),
        "Port 2222\n".into(),
    );
    let p = params(2222, Keepalive::Logon, false);
    for c in host_plan(&layout("goway"), &p, &HostFacts::assumed()) {
        if let Change::EnsureResource { kind, name, spec } = c {
            m.resources.insert((kind, name), spec);
        }
    }
    m
}

// frob:tests crates/goway-setup/src/host.rs::host_plan
#[test]
fn on_a_machine_set_up_by_hand_the_default_install_changes_nothing_and_uninstall_keeps_it_all() {
    let mut sys = laptop();
    let before = sys.clone();
    let facts = HostFacts {
        hyperv_firewall: false,
        sshd_ports: vec![2222],
        authorized_keys: true,
    };
    let plan = host_plan(
        &layout("goway"),
        &params(2222, Keepalive::Logon, false),
        &facts,
    );
    let mut journal = apply(&plan, &mut sys).unwrap();
    assert_eq!(sys, before);
    assert!(journal.entries.iter().all(|e| e.prior == Prior::Noop));
    assert!(!host::sshd_changed(&journal));
    assert!(host::created_task(&journal).is_none());
    assert!(host::restart_notices(&journal).is_empty());
    let report = revert(&mut journal, &mut sys).unwrap();
    assert!(report.outcomes.iter().all(|(_, o)| *o == Outcome::Noop));
    assert_eq!(sys, before);
}

// frob:tests crates/goway-setup/src/host.rs::sshd_changed
// frob:tests crates/goway-setup/src/host.rs::created_task
// frob:tests crates/goway-setup/src/host.rs::restart_notices
// frob:tests crates/goway-setup/src/host.rs::dropin_in_journal
// frob:tests crates/goway-setup/src/host.rs::needs_admin
#[test]
fn a_new_port_on_a_set_up_machine_is_a_pure_addition_that_uninstall_removes() {
    let mut sys = laptop();
    let before = sys.clone();
    let p = params(2299, Keepalive::Logon, true);
    let facts = HostFacts {
        hyperv_firewall: true,
        sshd_ports: vec![2222],
        authorized_keys: true,
    };
    let plan = host_plan(&layout("goway-test"), &p, &facts);
    let mut journal = apply(&plan, &mut sys).unwrap();
    assert_ne!(sys, before);
    assert!(host::sshd_changed(&journal));
    assert_eq!(
        host::created_task(&journal),
        Some("goway-test WSL Keepalive")
    );
    assert!(
        host::restart_notices(&journal).is_empty(),
        "mirrored and systemd were already set"
    );
    assert!(host::dropin_in_journal(&journal, "goway-test"));
    assert!(!host::dropin_in_journal(&journal, "other"));
    assert!(host::needs_admin(&journal));
    assert!(
        sys.files
            .contains_key(Path::new("/etc/ssh/sshd_config.d/20-goway-test-port.conf"))
    );
    revert(&mut journal, &mut sys).unwrap();
    assert_eq!(sys, before);
    assert!(!host::needs_admin(&journal), "everything is reverted");
}

// frob:tests crates/goway-setup/src/host.rs::restart_notices
#[test]
fn changing_wsl_settings_asks_for_a_restart() {
    let mut sys = base_machine();
    let plan = host_plan(
        &layout("goway-test"),
        &params(2299, Keepalive::Logon, false),
        &HostFacts::assumed(),
    );
    let journal = apply(&plan, &mut sys).unwrap();
    let notes = host::restart_notices(&journal);
    assert_eq!(notes.len(), 2, "{notes:?}");
    assert!(notes[0].contains("wsl --shutdown"));
    assert!(notes[1].contains("wsl.conf"));
}

// ----- the round-trip property -----

#[derive(Debug, Clone)]
struct Scenario {
    wslconfig: Option<&'static str>,
    wsl_conf: Option<&'static str>,
    ssh_dir: u8, // 0 none, 1 /etc/ssh only, 2 with sshd_config.d
    existing: [bool; 6],
    port_dropin: Option<&'static str>,
    facts: HostFacts,
    params: HostParams,
    profile: &'static str,
}

fn scenario() -> impl Strategy<Value = Scenario> {
    let wslconfig = proptest::option::of(proptest::sample::select(vec![
        "",
        "[wsl2]",
        "[wsl2]\nnetworkingMode=mirrored\n",
        "[wsl2]\r\ndnsTunneling = false\r\nnetworkingMode = mirrored",
        "[wsl2]\nnetworkingMode=nat\n",
        "[experimental]\nx=1\n",
        "[wsl2]\nmemory=4GB",
    ]));
    let wsl_conf = proptest::option::of(proptest::sample::select(vec![
        "[boot]\nsystemd=true\n",
        "[boot]\nsystemd=false\n[user]\ndefault=u",
        "[user]\ndefault=u\n",
        "",
    ]));
    (
        wslconfig,
        wsl_conf,
        0u8..3,
        proptest::array::uniform6(any::<bool>()),
        proptest::option::of(proptest::sample::select(vec![
            "Port 2299\n",
            "Port 1\n",
            "other\n",
        ])),
        (any::<bool>(), any::<bool>(), any::<bool>()),
        (
            proptest::sample::select(vec![2222u16, 2299]),
            prop_oneof![Just(Keepalive::Logon), Just(Keepalive::Boot)],
            any::<bool>(),
            proptest::sample::select(vec!["goway", "goway-test"]),
        ),
    )
        .prop_map(
            |(
                wslconfig,
                wsl_conf,
                ssh_dir,
                existing,
                port_dropin,
                f,
                (port, ka, harden, profile),
            )| {
                Scenario {
                    wslconfig,
                    wsl_conf,
                    ssh_dir,
                    existing,
                    port_dropin,
                    facts: HostFacts {
                        hyperv_firewall: f.0,
                        sshd_ports: if f.1 { vec![2222] } else { vec![] },
                        authorized_keys: f.2,
                    },
                    params: params(port, ka, harden),
                    profile,
                }
            },
        )
}

fn machine_of(s: &Scenario) -> (ModelSystem, Vec<Change>) {
    let mut m = base_machine();
    if let Some(t) = s.wslconfig {
        m.files.insert("/home/u/.wslconfig".into(), t.into());
    }
    if let Some(t) = s.wsl_conf {
        m.files.insert("/etc/wsl.conf".into(), t.into());
    }
    if s.ssh_dir >= 1 {
        m.dirs.insert("/etc/ssh".into());
    }
    if s.ssh_dir == 2 {
        m.dirs.insert("/etc/ssh/sshd_config.d".into());
        if let Some(t) = s.port_dropin {
            m.files
                .insert(host::port_dropin_path(s.profile).into(), t.into());
        }
    }
    let plan = host_plan(&layout(s.profile), &s.params, &s.facts);
    let resources: Vec<(ResourceKind, String, String)> = plan
        .iter()
        .filter_map(|c| match c {
            Change::EnsureResource { kind, name, spec } => {
                Some((*kind, name.clone(), spec.clone()))
            }
            _ => None,
        })
        .collect();
    for (i, (k, n, sp)) in resources.into_iter().enumerate() {
        if s.existing[i % 6] {
            m.resources.insert((k, n), sp);
        }
    }
    (m, plan)
}

// frob:tests crates/goway-setup/src/host.rs::host_plan
#[test]
fn reverting_the_host_plan_restores_any_machine_including_ones_where_parts_pre_exist() {
    let mut runner = TestRunner::new(Config::with_cases(400));
    runner
        .run(&scenario(), |s| {
            let (initial, plan) = machine_of(&s);
            let mut sys = initial.clone();
            let mut journal =
                apply(&plan, &mut sys).map_err(|e| TestCaseError::fail(e.to_string()))?;
            // Everything wanted is in place after the install.
            for c in &plan {
                prop_assert!(still_applied(c, &sys).unwrap(), "not satisfied: {c:?}");
            }
            // Re-applying is a no-op.
            let installed = sys.clone();
            let again = apply(&plan, &mut sys).unwrap();
            prop_assert!(again.entries.iter().all(|e| e.prior == Prior::Noop));
            prop_assert_eq!(&sys, &installed);
            // Reverting restores the start exactly; pre-existing things were never removed.
            let report = revert(&mut journal, &mut sys).unwrap();
            prop_assert!(
                report
                    .outcomes
                    .iter()
                    .all(|(_, o)| matches!(o, Outcome::Restored | Outcome::Noop))
            );
            prop_assert_eq!(&sys, &initial);
            Ok(())
        })
        .unwrap();
}

// frob:tests crates/goway-setup/src/host.rs::host_plan
#[test]
fn a_user_change_after_install_is_left_alone_and_reported() {
    let mut sys = base_machine();
    let plan = host_plan(
        &layout("goway-test"),
        &params(2299, Keepalive::Logon, false),
        &HostFacts::assumed(),
    );
    let mut journal = apply(&plan, &mut sys).unwrap();
    let dropin: PathBuf = host::port_dropin_path("goway-test").into();
    sys.files
        .insert(dropin.clone(), "Port 2299\nPort 4000\n".into());
    sys.resources.remove(&(
        ResourceKind::ScheduledTask,
        "goway-test WSL Keepalive".into(),
    ));
    let report = revert(&mut journal, &mut sys).unwrap();
    let left: Vec<_> = report
        .outcomes
        .iter()
        .filter(|(_, o)| matches!(o, Outcome::LeftAlone(_)))
        .collect();
    assert_eq!(left.len(), 2, "{report:?}");
    assert_eq!(sys.files[&dropin], "Port 2299\nPort 4000\n");
}

// frob:tests crates/goway-setup/src/app.rs::save_settings
// frob:tests crates/goway-setup/src/app.rs::load_settings
// frob:tests crates/goway-setup/src/app.rs::remove_settings
// frob:tests crates/goway-setup/src/layout.rs::Layout.host_view
// frob:tests crates/goway-setup/src/app.rs::ensure_not_installed
#[test]
fn the_host_journal_and_settings_live_in_the_admin_dir_apart_from_the_client_journal_and_are_cleaned_up()
 {
    let tmp = tempfile::tempdir().unwrap();
    let mut l = layout("p");
    l.state_dir = tmp.path().join("state");
    l.journal_path = l.state_dir.join("install-journal.json");
    l.admin_root = tmp.path().join("ProgramData").join("goway");
    l.admin_dir = l.admin_root.join("p");
    l.host_journal_path = l.admin_dir.join("host-journal.json");
    l.host_settings_path = l.admin_dir.join("host-settings.json");
    let view = l.host_view();
    assert_eq!(view.journal_path, l.host_journal_path);
    assert_eq!(
        view.state_dir, l.admin_dir,
        "host state is administrator-only"
    );
    assert!(view.admin_only && !l.admin_only);

    assert!(app::load_settings(&l).unwrap().is_none());
    let settings = HostSettings {
        distro: "Ubuntu".into(),
        port: DEFAULT_PORT,
    };
    app::save_settings(&l, &settings).unwrap();
    assert_eq!(app::load_settings(&l).unwrap(), Some(settings));

    let mut sys = base_machine();
    let plan = host_plan(
        &l,
        &params(2299, Keepalive::Logon, false),
        &HostFacts::assumed(),
    );
    app::install(&mut sys, &view, &plan).unwrap();
    assert!(l.host_journal_path.exists());
    assert!(!l.journal_path.exists(), "the client journal is separate");
    assert!(app::ensure_not_installed(&view).is_err());
    let report = app::uninstall(&mut sys, &view, Retry::ONCE)
        .unwrap()
        .unwrap();
    assert!(!report.outcomes.is_empty());
    assert!(!l.host_journal_path.exists());
    app::remove_settings(&l);
    assert!(!l.admin_dir.exists(), "empty admin dir is removed");
    assert!(!l.admin_root.exists(), "empty admin root is removed");
    assert_eq!(sys, base_machine());
}
