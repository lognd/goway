//! Command dispatch, rendering and platform helpers that run harmlessly on any host.

use std::path::Path;

use clap::Parser;
use goway_journal::{Change, Journal, Outcome};
use goway_setup::app::{StatusRow, UninstallReport, relaunch_dir};
use goway_setup::cli::{Cli, run, selected_components};
use goway_setup::layout::Layout;
use goway_setup::plan::Component;
use goway_setup::render::{ColorWhen, Renderer};
use goway_setup::windows::{broadcast_environment_change, schedule_self_delete, spawn_detached};

// frob:tests crates/goway-setup/src/cli.rs::run
// frob:tests crates/goway-setup/src/cli.rs::selected_components
// frob:tests crates/goway-setup/src/lib.rs::init_tracing
// frob:tests crates/goway-setup/src/layout.rs::Layout.from_environment
// frob:tests crates/goway-setup/src/render.rs::Renderer.new
// frob:tests crates/goway-setup/src/render.rs::Renderer.plan
#[test]
fn a_dry_run_install_succeeds_and_changes_nothing() {
    goway_setup::init_tracing(0);
    let cli = Cli::try_parse_from([
        "goway-setup",
        "--color",
        "never",
        "install",
        "--client",
        "--dry-run",
        "--profile",
        "dry-run-test",
    ])
    .unwrap();
    run(&cli, Renderer::new(ColorWhen::Never)).unwrap();
    assert_eq!(selected_components(false, false), vec![Component::Client]);
    assert_eq!(selected_components(false, true), vec![Component::Host]);
    assert_eq!(
        selected_components(true, true),
        vec![Component::Client, Component::Host]
    );
    let layout = Layout::from_environment("dry-run-test").unwrap();
    assert!(!layout.journal_path.exists());
}

#[test]
fn a_bad_profile_is_rejected_before_anything_happens() {
    let cli =
        Cli::try_parse_from(["goway-setup", "install", "--dry-run", "--profile", "a/b"]).unwrap();
    assert!(run(&cli, Renderer::new(ColorWhen::Never)).is_err());
}

// frob:tests crates/goway-setup/src/render.rs::Renderer.error
// frob:tests crates/goway-setup/src/render.rs::Renderer.installed
// frob:tests crates/goway-setup/src/render.rs::Renderer.relaunching
// frob:tests crates/goway-setup/src/render.rs::Renderer.nothing_installed
// frob:tests crates/goway-setup/src/render.rs::Renderer.uninstalled
// frob:tests crates/goway-setup/src/render.rs::Renderer.status
// frob:tests crates/goway-setup/src/render.rs::Renderer.not_installed
#[test]
fn every_message_renders_without_panicking() {
    let r = Renderer::new(ColorWhen::Never);
    let layout = Layout::new(Path::new("/l"), Path::new("/ProgramData"), "p").unwrap();
    let change = Change::EnsureDir { path: "/x".into() };
    let mut journal = Journal::new("j");
    journal.entries.push(goway_journal::Entry {
        change: change.clone(),
        prior: goway_journal::Prior::Noop,
        reverted: true,
    });
    r.error(&goway_setup::error::SetupError::NoPayload);
    r.installed(&layout, 3);
    r.relaunching(Path::new("/t/a.exe"), Path::new("/t/uninstall.log"));
    r.nothing_installed(&layout);
    r.not_installed(&layout);
    r.uninstalled(
        &layout,
        &UninstallReport {
            journal,
            outcomes: vec![(0, Outcome::LeftAlone("edited".into()))],
        },
    );
    r.status(
        &layout,
        &[StatusRow {
            index: 0,
            change,
            reverted: false,
            holds: true,
        }],
    );
}

// frob:tests crates/goway-setup/src/app.rs::relaunch_dir
// frob:tests crates/goway-setup/src/windows.rs::broadcast_environment_change
// frob:tests crates/goway-setup/src/windows.rs::schedule_self_delete
// frob:tests crates/goway-setup/src/windows.rs::spawn_detached
#[test]
fn platform_helpers_are_harmless_off_windows() {
    assert_eq!(
        relaunch_dir(Path::new("/tmp"), 7),
        Path::new("/tmp/goway-uninstall-7")
    );
    broadcast_environment_change();
    schedule_self_delete(Path::new("/nope/a.exe"), Path::new("/nope"));
    let mut cmd = std::process::Command::new("true");
    if let Ok(mut child) = spawn_detached(&mut cmd) {
        child.wait().unwrap();
    }
}

// frob:tests crates/goway-setup/src/cli.rs::run
#[test]
fn a_host_dry_run_prints_the_plan_and_a_real_host_install_needs_windows() {
    let r = Renderer::new(ColorWhen::Never);
    let dry = Cli::try_parse_from([
        "goway-setup",
        "install",
        "--host",
        "--dry-run",
        "--port",
        "2299",
        "--distro",
        "Ubuntu",
        "--keepalive",
        "boot",
        "--harden",
        "--no-activate",
        "--no-elevate",
        "--profile",
        "host-dry-run-test",
    ])
    .unwrap();
    run(&dry, r).unwrap();
    let layout = Layout::from_environment("host-dry-run-test").unwrap();
    assert!(!layout.host_journal_path.exists());

    let bad = Cli::try_parse_from([
        "goway-setup",
        "install",
        "--host",
        "--dry-run",
        "--distro",
        "a b",
    ])
    .unwrap();
    assert!(run(&bad, r).is_err());

    if !cfg!(windows) {
        let real = Cli::try_parse_from([
            "goway-setup",
            "install",
            "--host",
            "--profile",
            "host-dry-run-test",
        ])
        .unwrap();
        let err = run(&real, r).unwrap_err();
        assert!(
            matches!(err, goway_setup::error::SetupError::HostNeedsWindows),
            "{err}"
        );
    }
}

// frob:tests crates/goway-setup/src/cli.rs::run
#[test]
fn status_and_uninstall_of_an_unknown_profile_report_nothing_to_do() {
    let r = Renderer::new(ColorWhen::Never);
    for sub in ["status", "uninstall"] {
        let cli =
            Cli::try_parse_from(["goway-setup", sub, "--profile", "never-installed-test"]).unwrap();
        run(&cli, r).unwrap();
    }
}

// frob:tests crates/goway-setup/src/render.rs::Renderer.passthrough
// frob:tests crates/goway-setup/src/render.rs::Renderer.elevating
// frob:tests crates/goway-setup/src/render.rs::Renderer.notice
// frob:tests crates/goway-setup/src/render.rs::Renderer.host_installed
// frob:tests crates/goway-setup/src/render.rs::Renderer.plan_component
// frob:tests crates/goway-setup/src/render.rs::Renderer.uninstalled_component
// frob:tests crates/goway-setup/src/render.rs::describe_kind
#[test]
fn host_messages_render_without_panicking() {
    use goway_journal::ResourceKind;
    let r = Renderer::new(ColorWhen::Never);
    let layout = Layout::new(Path::new("/l"), Path::new("/ProgramData"), "p").unwrap();
    r.passthrough("child output\n");
    r.elevating("the host install");
    r.notice("restart WSL");
    r.host_installed(&layout, "Ubuntu", 2299, 11);
    let change = Change::EnsureResource {
        kind: ResourceKind::WslUnit,
        name: "ssh.socket".into(),
        spec: String::new(),
    };
    r.plan_component("host component", std::slice::from_ref(&change), None);
    r.plan_component(
        "host component",
        std::slice::from_ref(&change),
        Some(&[true]),
    );
    r.plan_component(
        "host component",
        std::slice::from_ref(&change),
        Some(&[false]),
    );
    let mut journal = Journal::new("j");
    journal.entries.push(goway_journal::Entry {
        change: change.clone(),
        prior: goway_journal::Prior::ResourceCreated,
        reverted: true,
    });
    r.uninstalled_component(
        &layout,
        "host component of profile",
        &UninstallReport {
            journal,
            outcomes: vec![(0, Outcome::Restored)],
        },
    );
    assert_eq!(
        goway_setup::render::describe(&change),
        "ensure enabled WSL systemd unit ssh.socket"
    );
    for kind in [
        ResourceKind::FirewallRule,
        ResourceKind::HyperVFirewallRule,
        ResourceKind::ScheduledTask,
        ResourceKind::Service,
        ResourceKind::WslPackage,
    ] {
        assert!(!goway_setup::render::describe_kind(kind).is_empty());
    }
}

// frob:tests crates/goway-setup/src/cli.rs::run
#[test]
fn the_elevated_rerun_refuses_the_client_component() {
    for args in [
        vec!["install", "--client", "--host", "--elevated-child"],
        vec!["uninstall", "--client", "--elevated-child"],
    ] {
        let mut argv = vec!["goway-setup", "--color", "never"];
        argv.extend(args);
        argv.extend(["--profile", "elevated-client-test"]);
        let cli = Cli::try_parse_from(argv).unwrap();
        let err = run(&cli, Renderer::new(ColorWhen::Never)).unwrap_err();
        assert!(
            matches!(err, goway_setup::error::SetupError::ClientNeverElevated),
            "{err:?}"
        );
    }
}
