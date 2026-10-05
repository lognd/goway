//! `goway doctor` on a WSL helper whose ssh port is closed: what it concludes from asking Windows
//! OpenSSH at the same address. A fake prober stands in for ssh, so no machine is touched.

use std::collections::BTreeMap;

use goway::doctor::wsl_down::{
    KEEPALIVE_TASK, clean_windows_text, describe, diagnose, parse_keepalive_tasks, parse_wsl_list,
};
use goway::doctor::{Check, Level};
use goway::resolve::{ProbeResult, Prober};
use goway::ssh::{Failure, KeyPolicy, Target};
use goway::transport::windows_ssh_line;

/// What the Windows ssh answers per command line; a missing entry is unreachable.
struct FakeWindows {
    answers: BTreeMap<String, ProbeResult>,
}

impl Prober for FakeWindows {
    fn probe(&self, _target: &Target, _policy: KeyPolicy, remote: &str) -> ProbeResult {
        self.answers.get(remote).cloned().unwrap_or_else(|| {
            Err((
                Failure::Unreachable,
                "ssh: connect to host 192.0.2.7 port 22: Connection timed out".to_owned(),
            ))
        })
    }
}

fn windows() -> Target {
    Target {
        name: "helios-windows".to_owned(),
        address: "192.0.2.7".to_owned(),
        port: 22,
        user: Some("alice".to_owned()),
        identity: None,
    }
}

/// `wsl.exe -l -v` as it arrives: UTF-16 text seen as bytes, a NUL after every character.
fn utf16ish(text: &str) -> String {
    text.chars().flat_map(|c| [c, '\0']).collect()
}

fn list_with(state: &str) -> String {
    utf16ish(&format!(
        "  NAME      STATE           VERSION\r\n* Ubuntu    {state}         2\r\n"
    ))
}

fn tasks_with(repeat: &str) -> String {
    format!(
        "Folder: \\\r\nHostName:      HELIOS\r\nTaskName:      \\WSL Keepalive\r\nStatus:        Ready\r\nLast Result:   0\r\nRepeat: Every: {repeat}\r\n\r\nHostName:      HELIOS\r\nTaskName:      \\Other task\r\nStatus:        Ready\r\nRepeat: Every: 0 Hour(s), 5 Minute(s)\r\n"
    )
}

fn fake(list: Option<String>, tasks: Option<String>) -> FakeWindows {
    let mut answers = BTreeMap::new();
    if let Some(l) = list {
        answers.insert(windows_ssh_line("wsl.exe -l -v"), Ok(l));
    }
    if let Some(t) = tasks {
        answers.insert(windows_ssh_line("schtasks /query /v /fo list"), Ok(t));
    }
    FakeWindows { answers }
}

fn find<'a>(checks: &'a [Check], name: &str) -> &'a Check {
    checks
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("no {name} check in {checks:?}"))
}

// frob:tests crates/goway/src/doctor/wsl_down.rs::diagnose
// frob:tests crates/goway/src/doctor/wsl_down.rs::describe
// frob:tests crates/goway/src/doctor/wsl_down.rs::parse_wsl_list
// frob:tests crates/goway/src/doctor/wsl_down.rs::clean_windows_text
#[test]
fn a_stopped_distro_is_named_with_the_command_that_starts_the_keepalive() {
    let prober = fake(
        Some(list_with("Stopped")),
        Some(tasks_with("0 Hour(s), 5 Minute(s)")),
    );
    let checks = diagnose(&prober, &windows(), 2222);
    let distro = find(&checks, "wsl distro");
    assert_eq!(distro.level, Level::Fail);
    assert!(
        distro.detail.contains("Ubuntu is stopped"),
        "{}",
        distro.detail
    );
    assert!(
        distro
            .detail
            .contains("ssh -p 22 alice@192.0.2.7 schtasks /run /tn \"WSL Keepalive\""),
        "{}",
        distro.detail
    );
    assert_eq!(find(&checks, "keepalive task").level, Level::Ok);
    let text = describe("cannot reach host `helios`", &checks);
    assert!(text.starts_with("cannot reach host `helios`\n    wsl distro: "));
    assert_eq!(
        parse_wsl_list(&clean_windows_text(&list_with("Running")))[0].state,
        "Running"
    );
    assert_eq!(KEEPALIVE_TASK, "WSL Keepalive");
}

// frob:tests crates/goway/src/doctor/wsl_down.rs::parse_keepalive_tasks
#[test]
fn a_keepalive_with_no_repetition_is_a_warning_that_points_at_the_installer() {
    let prober = fake(Some(list_with("Stopped")), Some(tasks_with("Disabled")));
    let checks = diagnose(&prober, &windows(), 2222);
    let task = find(&checks, "keepalive task");
    assert_eq!(task.level, Level::Warn);
    assert!(task.detail.contains("only a boot or logon trigger"));
    assert!(task.detail.contains("goway-setup install"));
    // Another task's repetition does not count for the keepalive.
    let tasks = parse_keepalive_tasks(&tasks_with("Disabled"));
    assert_eq!(tasks.len(), 1);
    assert!(!tasks[0].repeats);
}

// frob:tests crates/goway/src/doctor/wsl_down.rs::diagnose
#[test]
fn an_address_that_does_not_answer_is_unreachable_not_a_stopped_wsl() {
    let checks = diagnose(&fake(None, None), &windows(), 2222);
    assert_eq!(checks.len(), 1);
    let machine = find(&checks, "machine");
    assert_eq!(machine.level, Level::Fail);
    assert!(
        machine.detail.starts_with("unreachable:"),
        "{}",
        machine.detail
    );
    assert!(machine.detail.contains("not the same as WSL being stopped"));
    assert!(!machine.detail.contains("is stopped"));
}

// frob:tests crates/goway/src/doctor/wsl_down.rs::diagnose
#[test]
fn a_running_distro_with_a_closed_port_and_a_refused_login_are_told_apart() {
    let running = diagnose(
        &fake(Some(list_with("Running")), Some(tasks_with("Disabled"))),
        &windows(),
        2222,
    );
    let distro = find(&running, "wsl distro");
    assert_eq!(distro.level, Level::Warn);
    assert!(distro.detail.contains("is running"));
    let mut refused = fake(None, None);
    refused.answers.insert(
        windows_ssh_line("wsl.exe -l -v"),
        Err((Failure::HostKeyUnknown, "host key not pinned".to_owned())),
    );
    let checks = diagnose(&refused, &windows(), 2222);
    assert!(
        find(&checks, "windows ssh")
            .detail
            .contains("host key is not pinned")
    );
    // No keepalive task at all.
    let none = diagnose(
        &fake(Some(list_with("Stopped")), Some(String::new())),
        &windows(),
        2222,
    );
    assert!(
        find(&none, "keepalive task")
            .detail
            .contains("no WSL Keepalive")
    );
}
