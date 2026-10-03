//! What the helper laptop's user is told: the WSL precheck, the next-steps block and the
//! restart question, all pure text and decisions over probe results.

use std::cell::RefCell;

use goway_journal::{Change, Entry, Journal, Prior};
use goway_setup::helper::{
    HelperInfo, WslCheck, WslProbe, add_command, check_wsl, helper_name, next_steps,
    parse_fingerprint, public_network_warning, wsl_steps,
};
use goway_setup::host::{
    NetworkMode, RestartAction, RestartNeed, is_yes, restart_action, restart_need,
};
use goway_setup::hostsys::{Invocation, Output, Runner, parse_distro_list, probe_wsl};

const FP: &str = "SHA256:Qk3nW0xk3b0nS0meFingerprintValue+/abc123XYZ";

fn info(fingerprint: Option<&str>) -> HelperInfo {
    HelperInfo {
        device_name: "Orion-Notebook".into(),
        fingerprint: fingerprint.map(str::to_owned),
        user: "user".into(),
        port: 2222,
        network: NetworkMode::Mirrored,
    }
}

// frob:tests crates/goway-setup/src/helper.rs::next_steps
// frob:tests crates/goway-setup/src/helper.rs::add_command
#[test]
fn the_block_names_the_helper_its_fingerprint_its_user_and_the_exact_command() {
    let text = next_steps(&info(Some(FP)));
    assert!(text.contains("orion-notebook"));
    assert!(text.contains(&format!("Its ssh host key fingerprint: {FP}")));
    assert!(text.contains("Its Linux user:               user"));
    assert!(text.contains(&format!(
        "\n    goway add orion-notebook --fingerprint {FP} --user user\n"
    )));
    assert!(!text.contains("--port"), "the default port is left out");
    assert!(text.is_ascii());
    let mut other = info(Some(FP));
    other.port = 2299;
    assert!(next_steps(&other).contains("--user user --port 2299\n"));
    assert_eq!(
        add_command("a", "SHA256:x", "u", 2222),
        "goway add a --fingerprint SHA256:x --user u"
    );
}

// frob:tests crates/goway-setup/src/helper.rs::next_steps
#[test]
fn a_missing_host_key_is_said_plainly_and_gives_no_command() {
    let text = next_steps(&info(None));
    assert!(text.contains("not available yet"));
    assert!(text.contains("status --host"));
    assert!(!text.contains("goway add"));
    let mut root = info(Some(FP));
    root.user = "root".into();
    assert!(next_steps(&root).contains("no ordinary user yet"));
}

// frob:tests crates/goway-setup/src/helper.rs::helper_name
#[test]
fn windows_device_names_become_names_goway_accepts() {
    assert_eq!(
        helper_name("Orion-Notebook").as_deref(),
        Some("orion-notebook")
    );
    assert_eq!(helper_name("DESKTOP_7K2").as_deref(), Some("desktop_7k2"));
    assert_eq!(helper_name("My PC!").as_deref(), Some("my-pc"));
    assert_eq!(helper_name("  ").as_deref(), None);
    assert_eq!(helper_name("---").as_deref(), None);
    let long = "a".repeat(80);
    assert_eq!(helper_name(&long).unwrap().len(), 63);
    for raw in ["Orion-Notebook", "-x", "a b c", "ÄÖ1"] {
        if let Some(n) = helper_name(raw) {
            assert!(
                !n.is_empty()
                    && n.len() <= 63
                    && !n.starts_with('-')
                    && n.chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                "{raw} -> {n}"
            );
        }
    }
}

// frob:tests crates/goway-setup/src/helper.rs::parse_fingerprint
#[test]
fn the_fingerprint_is_read_from_ssh_keygen_output() {
    assert_eq!(
        parse_fingerprint(&format!("256 {FP} root@host (ED25519)")).as_deref(),
        Some(FP)
    );
    assert_eq!(
        parse_fingerprint("/etc/ssh/x.pub: No such file or directory"),
        None
    );
    assert_eq!(parse_fingerprint("256 SHA256: x"), None);
}

fn probe(wsl_exe: bool, distros: Option<&[&str]>) -> WslProbe {
    WslProbe {
        wsl_exe,
        distros: distros.map(|d| d.iter().map(|s| (*s).to_owned()).collect()),
    }
}

// frob:tests crates/goway-setup/src/helper.rs::check_wsl
// frob:tests crates/goway-setup/src/helper.rs::wsl_steps
#[test]
fn a_laptop_without_wsl_or_the_distro_stops_with_the_exact_steps() {
    assert_eq!(
        check_wsl(&probe(true, Some(&["Ubuntu"])), "Ubuntu"),
        WslCheck::Ready
    );
    assert_eq!(
        check_wsl(&probe(true, Some(&["ubuntu"])), "Ubuntu"),
        WslCheck::Ready
    );
    assert_eq!(check_wsl(&probe(false, None), "Ubuntu"), WslCheck::NoWsl);
    assert_eq!(check_wsl(&probe(true, None), "Ubuntu"), WslCheck::NoWsl);
    assert_eq!(
        check_wsl(&probe(true, Some(&[])), "Ubuntu"),
        WslCheck::NoWsl
    );
    let other = check_wsl(&probe(true, Some(&["docker-desktop"])), "Ubuntu");
    assert_eq!(
        other,
        WslCheck::NoDistro {
            installed: vec!["docker-desktop".into()]
        }
    );
    assert_eq!(wsl_steps(&WslCheck::Ready, "Ubuntu"), None);
    for check in [WslCheck::NoWsl, other] {
        let steps = wsl_steps(&check, "Ubuntu").unwrap();
        for need in [
            "Nothing was changed",
            "Run as administrator",
            "wsl --install -d Ubuntu",
            "Restart the laptop",
            "user name and a password",
            "goway-setup.exe install --host",
        ] {
            assert!(steps.contains(need), "{need}: {steps}");
        }
        assert!(steps.is_ascii());
    }
}

struct Script(Option<(i32, &'static str)>, RefCell<Vec<Invocation>>);
impl Runner for &Script {
    fn run(&self, inv: &Invocation) -> std::io::Result<Output> {
        self.1.borrow_mut().push(inv.clone());
        let (code, out) = self.0.unwrap();
        Ok(Output {
            code: Some(code),
            stdout: out.as_bytes().to_vec(),
            stderr: Vec::new(),
        })
    }
}

// frob:tests crates/goway-setup/src/hostsys.rs::probe_wsl
// frob:tests crates/goway-setup/src/hostsys.rs::parse_distro_list
#[test]
fn the_wsl_probe_lists_distros_only_when_wsl_answers() {
    let ok = Script(
        Some((0, "\u{feff}Ubuntu\r\ndocker-desktop\r\n")),
        RefCell::default(),
    );
    let p = probe_wsl(&&ok, true);
    assert_eq!(
        p.distros,
        Some(vec!["Ubuntu".to_owned(), "docker-desktop".to_owned()])
    );
    assert_eq!(ok.1.borrow()[0].args, ["-l", "-q"]);
    let failed = Script(Some((1, "")), RefCell::default());
    assert_eq!(probe_wsl(&&failed, true).distros, None);
    let none = Script(None, RefCell::default());
    let p = probe_wsl(&&none, false);
    assert!(
        !p.wsl_exe && p.distros.is_none(),
        "wsl.exe is never run when absent"
    );
    assert!(none.1.borrow().is_empty());
    assert_eq!(
        parse_distro_list("WSL is not installed. Run it\r\nUbuntu\r\n"),
        ["Ubuntu"]
    );
}

fn journal(changes: Vec<Change>) -> Journal {
    let mut j = Journal::new("t");
    for change in changes {
        j.entries.push(Entry {
            change,
            prior: Prior::File { contents: None },
            reverted: false,
        });
    }
    j
}

fn ini(path: &str, key: &str) -> Change {
    Change::SetIniKey {
        path: path.into(),
        section: "s".into(),
        key: key.into(),
        value: "v".into(),
    }
}

// frob:tests crates/goway-setup/src/host.rs::restart_need
// frob:tests crates/goway-setup/src/host.rs::restart_action
// frob:tests crates/goway-setup/src/host.rs::is_yes
// frob:tests crates/goway-setup/src/host.rs::RestartNeed.command
// frob:tests crates/goway-setup/src/host.rs::RestartNeed.consequence
#[test]
fn a_needed_wsl_restart_is_asked_on_a_console_and_printed_otherwise() {
    let shutdown = restart_need(&journal(vec![ini("C:\\u\\.wslconfig", "networkingMode")]));
    let terminate = restart_need(&journal(vec![ini("/etc/wsl.conf", "systemd")]));
    let none = restart_need(&journal(vec![ini("/etc/other.conf", "x")]));
    assert!(none.is_none());
    assert_eq!(
        shutdown.command("Ubuntu").as_deref(),
        Some("wsl --shutdown")
    );
    assert_eq!(
        terminate.command("Ubuntu").as_deref(),
        Some("wsl --terminate Ubuntu")
    );
    assert!(shutdown.consequence().contains("every open Linux"));
    // (need, yes, activate, console)
    assert_eq!(
        restart_action(none, false, true, true),
        RestartAction::Nothing
    );
    assert_eq!(
        restart_action(shutdown, false, true, true),
        RestartAction::Ask
    );
    assert_eq!(
        restart_action(shutdown, true, true, true),
        RestartAction::PrintCommand
    );
    assert_eq!(
        restart_action(shutdown, false, false, true),
        RestartAction::PrintCommand
    );
    assert_eq!(
        restart_action(shutdown, false, true, false),
        RestartAction::PrintCommand
    );
    let both = RestartNeed {
        shutdown: true,
        terminate: true,
    };
    assert_eq!(both.command("U").as_deref(), Some("wsl --shutdown"));
    // The default answer is no.
    assert!(is_yes("y") && is_yes(" YES\r\n"));
    assert!(!is_yes("") && !is_yes("\n") && !is_yes("n") && !is_yes("yep"));
    // Entries that were already in place (no-op) need no restart.
    let mut j = journal(vec![ini("C:\\u\\.wslconfig", "networkingMode")]);
    j.entries[0].prior = Prior::Noop;
    assert!(restart_need(&j).is_none());
}

// frob:tests crates/goway-setup/src/helper.rs::public_network_warning
#[test]
fn the_public_network_warning_explains_in_plain_words_and_gives_the_fix() {
    let w = public_network_warning("Cafe's WiFi");
    assert!(w.contains("cafe and hotel Wi-Fi"));
    assert!(w.contains("home or work (Private)"));
    assert!(w.contains("Set-NetConnectionProfile -Name 'Cafe''s WiFi' -NetworkCategory Private"));
    assert!(w.contains("administrator"));
}

// frob:tests crates/goway-setup/src/helper.rs::next_steps
#[test]
fn a_hostile_linux_user_never_reaches_the_command_line() {
    for bad in [
        "a b",
        "x;y",
        "x\ny",
        "$(id)",
        "me;touch INJECTED;#",
        "-x",
        "Root",
        "",
        "a`b`",
        "x\u{1b}[31m",
    ] {
        let mut i = info(Some(FP));
        i.user = bad.into();
        let text = next_steps(&i);
        let command = text
            .lines()
            .find(|l| l.trim_start().starts_with("goway add"))
            .unwrap_or_else(|| panic!("no command for {bad:?}: {text}"));
        assert!(
            command.ends_with("--user YOUR-LINUX-USER"),
            "{bad:?} reached {command}"
        );
        assert!(
            text.lines()
                .all(|l| !l.contains('\u{1b}') && !l.contains("INJECTED") && !l.contains("$(id)")),
            "{bad:?} leaked into the block"
        );
        assert!(text.contains("could not read a usable Linux user"));
    }
}

// frob:tests crates/goway-setup/src/helper.rs::valid_linux_user
#[test]
fn only_names_useradd_accepts_are_valid_linux_users() {
    use goway_setup::helper::valid_linux_user;
    for ok in [
        "user",
        "a",
        "_x",
        "dev-ops_1",
        "machine$",
        "a23456789012345678901234567890_",
    ] {
        assert!(valid_linux_user(ok), "{ok}");
    }
    for bad in [
        "",
        "A",
        "1a",
        "-a",
        "a b",
        "a;b",
        "a\n",
        "$",
        "a$b",
        "a23456789012345678901234567890123",
    ] {
        assert!(!valid_linux_user(bad), "{bad}");
    }
}

// frob:tests crates/goway-setup/src/helper.rs::add_command
#[test]
fn the_add_command_is_shell_quoted() {
    assert_eq!(
        add_command("n", "SHA256:a+/=", "u", 2222),
        "goway add n --fingerprint SHA256:a+/= --user u"
    );
    let cmd = add_command("n; id", "SHA256:x", "a b", 2222);
    assert!(cmd.contains("'n; id'"));
    assert!(cmd.contains("'a b'"));
}
