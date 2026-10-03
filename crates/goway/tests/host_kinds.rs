//! Host kinds: Windows over OpenSSH with PowerShell, and the Windows side of
//! this machine through powershell.exe with no ssh.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use base64::Engine as _;
use goway::config::{Config, HostConfig, Os, Transport};
use goway::ssh::{KeyPolicy, Settings, Target};
use goway::transport::{self, Kind, Script};

fn decode(encoded: &str) -> String {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .unwrap();
    let units: Vec<u16> = bytes
        .chunks(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16(&units).unwrap()
}

fn script(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{body}")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

// frob:tests crates/goway/src/interop.rs::command_with
#[test]
fn interop_reaches_windows_through_powershell_with_no_ssh() {
    let dir = tempfile::tempdir().unwrap();
    let ps = dir.path().join("powershell.exe");
    // The fake powershell prints its arguments, one per line.
    script(&ps, "for a in \"$@\"; do echo \"$a\"; done\n");
    // An ssh that must never run.
    let ssh_marker = dir.path().join("ssh-ran");
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    script(
        &bin.join("ssh"),
        &format!("touch {}\n", ssh_marker.display()),
    );

    let source = goway::transport::ps_call(&["cargo", "test", "it's $x"]);
    let mut cmd = goway::interop::command_with(&ps, &source);
    cmd.env("PATH", &bin);
    let out = cmd.output().unwrap();
    assert!(out.status.success());
    let lines: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(
        lines[..5],
        [
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-EncodedCommand"
        ]
    );
    assert_eq!(
        decode(&lines[5]),
        "& 'cargo' 'test' 'it''s $x'",
        "arguments arrive exactly"
    );
    assert!(!ssh_marker.exists(), "no ssh was involved");
    assert!(!Kind::WindowsInterop.uses_ssh());
}

fn target() -> Target {
    Target {
        name: "helios".to_owned(),
        address: "192.0.2.7".to_owned(),
        port: 22,
        user: None,
        identity: None,
    }
}

// frob:tests crates/goway/src/transport.rs::command
#[test]
fn windows_over_ssh_uses_openssh_with_powershell_and_the_pinned_key_rules() {
    let settings = Settings {
        known_hosts: std::path::PathBuf::from("/tmp/goway-test/known_hosts"),
        control_dir: None,
        connect_timeout_secs: 5,
    };
    let source = transport::ps_call(&["cmd", "/c", "echo \"a b\" & del *"]);
    let cmd = transport::command(
        Kind::WindowsSsh,
        &target(),
        &settings,
        KeyPolicy::Strict,
        Script::Ps(&source),
    )
    .unwrap();
    assert_eq!(cmd.get_program(), "ssh");
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let joined = args.join(" ");
    assert!(joined.contains("HostKeyAlias=goway-helios"), "{joined}");
    assert!(joined.contains("StrictHostKeyChecking=yes"), "{joined}");
    assert!(joined.contains("BatchMode=yes"), "no passwords: {joined}");
    let last = args.last().unwrap();
    let rest = last
        .strip_prefix(
            "powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand ",
        )
        .unwrap();
    assert_eq!(decode(rest), source, "PowerShell gets the script unchanged");
}

#[test]
fn host_kinds_come_from_the_config() {
    let c = Config::parse(
        "[[host]]\nname = \"a\"\n\n[[host]]\nname = \"w\"\nos = \"windows\"\n\n[[host]]\nname = \"me\"\nos = \"windows\"\ntransport = \"interop\"\n",
        Path::new("c.toml"),
    )
    .unwrap();
    let kinds: Vec<Kind> = c.hosts.iter().map(Kind::of).collect();
    assert_eq!(kinds, [Kind::Unix, Kind::WindowsSsh, Kind::WindowsInterop]);
    assert_eq!(c.port_of(&c.hosts[0]), 2222, "WSL sshd");
    assert_eq!(c.port_of(&c.hosts[1]), 22, "Windows OpenSSH");
    // Interop has no network, and needs Windows.
    for bad in [
        "[[host]]\nname = \"x\"\ntransport = \"interop\"\n",
        "[[host]]\nname = \"x\"\nos = \"windows\"\ntransport = \"interop\"\naddress = \"192.0.2.1\"\n",
    ] {
        assert!(Config::parse(bad, Path::new("c.toml")).is_err(), "{bad}");
    }
    let h = HostConfig {
        name: "n".to_owned(),
        os: Os::Windows,
        transport: Transport::Interop,
        ..HostConfig::default()
    };
    assert!(h.check_kind().is_ok());
}
