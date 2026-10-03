//! The real host `System`, driven through a scripted fake instead of `wsl.exe` and PowerShell.

use std::cell::RefCell;
use std::path::Path;

use goway_journal::{ResourceKind, System, SystemError};
use goway_setup::host::{FirewallSpec, HyperVSpec, Keepalive, TaskSpec};
use goway_setup::hostsys::{
    Activation, HostSystem, Invocation, Output, Runner, is_wsl_path, parse_listening_ports,
    parse_sshd_ports,
};
use goway_setup::ps;

/// Answers each command from a script of `(substring of the command line, exit code, stdout)`.
struct Fake {
    script: Vec<(&'static str, i32, &'static str)>,
    log: RefCell<Vec<Invocation>>,
}

impl Fake {
    fn new(script: Vec<(&'static str, i32, &'static str)>) -> Self {
        Self {
            script,
            log: RefCell::new(Vec::new()),
        }
    }
}

impl Runner for &Fake {
    fn run(&self, inv: &Invocation) -> std::io::Result<Output> {
        self.log.borrow_mut().push(inv.clone());
        let line = format!("{} {}", inv.program, inv.args.join(" "));
        let (code, out) = self
            .script
            .iter()
            .find(|(needle, _, _)| line.contains(needle))
            .map_or((127, ""), |(_, c, o)| (*c, *o));
        Ok(Output {
            code: Some(code),
            stdout: out.as_bytes().to_vec(),
            stderr: Vec::new(),
        })
    }
}

struct Flip<'a>(&'a Fake, RefCell<u32>);
impl Runner for Flip<'_> {
    fn run(&self, inv: &Invocation) -> std::io::Result<Output> {
        if inv.args.contains(&"ss".to_owned()) {
            let mut n = self.1.borrow_mut();
            *n += 1;
            let out = if *n >= 2 {
                "LISTEN 0 1 0.0.0.0:2299 0.0.0.0:*\n"
            } else {
                ""
            };
            return Ok(Output {
                code: Some(0),
                stdout: out.as_bytes().to_vec(),
                stderr: vec![],
            });
        }
        self.0.run(inv)
    }
}

#[allow(clippy::cast_possible_truncation)] // bytes are masked by construction
fn decode_base64(text: &str) -> Vec<u8> {
    let val = |c: u8| match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        _ => 63,
    };
    let mut out = Vec::new();
    for chunk in text.as_bytes().chunks(4) {
        let n = chunk
            .iter()
            .take_while(|&&c| c != b'=')
            .fold((0u32, 0), |(acc, k), &c| {
                ((acc << 6) | u32::from(val(c)), k + 1)
            });
        let (acc, k) = n;
        let acc = acc << (6 * (4 - k));
        for i in 0..(k * 6 / 8) {
            out.push((acc >> (16 - 8 * i)) as u8);
        }
    }
    out
}

/// The script text a PowerShell invocation carried.
fn script_of(inv: &Invocation) -> String {
    assert_eq!(inv.program, "powershell.exe");
    assert_eq!(
        inv.args[..3],
        ["-NoProfile", "-NonInteractive", "-EncodedCommand"]
    );
    let bytes = decode_base64(&inv.args[3]);
    let units: Vec<u16> = bytes
        .chunks(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16(&units).unwrap()
}

fn sys(fake: &Fake) -> HostSystem<&Fake> {
    HostSystem::with_runner("Ubuntu", fake)
}

// frob:tests crates/goway-setup/src/ps.rs::base64
// frob:tests crates/goway-setup/src/ps.rs::encode_command
// frob:tests crates/goway-setup/src/ps.rs::quote
#[test]
fn powershell_encoding_and_quoting_are_exact() {
    assert_eq!(ps::base64(b""), "");
    assert_eq!(ps::base64(b"M"), "TQ==");
    assert_eq!(ps::base64(b"Ma"), "TWE=");
    assert_eq!(ps::base64(b"Man"), "TWFu");
    assert_eq!(ps::base64(b"Many hands"), "TWFueSBoYW5kcw==");
    assert_eq!(ps::encode_command("a"), "YQA=");
    assert_eq!(ps::quote("it's"), "'it''s'");
    assert_eq!(ps::quote("WSL SSH 2222"), "'WSL SSH 2222'");
    let script = "Write-Output 'caf\u{e9}'";
    let inv = Invocation {
        program: "powershell.exe".into(),
        args: vec![
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-EncodedCommand".into(),
            ps::encode_command(script),
        ],
        stdin: None,
    };
    assert_eq!(script_of(&inv), script);
}

// frob:tests crates/goway-setup/src/ps.rs::firewall_create
// frob:tests crates/goway-setup/src/ps.rs::firewall_exists
// frob:tests crates/goway-setup/src/ps.rs::firewall_delete
// frob:tests crates/goway-setup/src/ps.rs::hyperv_create
// frob:tests crates/goway-setup/src/ps.rs::hyperv_exists
// frob:tests crates/goway-setup/src/ps.rs::hyperv_delete
// frob:tests crates/goway-setup/src/ps.rs::task_create
// frob:tests crates/goway-setup/src/ps.rs::task_exists
// frob:tests crates/goway-setup/src/ps.rs::task_delete
// frob:tests crates/goway-setup/src/ps.rs::task_start
// frob:tests crates/goway-setup/src/ps.rs::keepalive_arguments
// frob:tests crates/goway-setup/src/ps.rs::hyperv_available
#[test]
fn scripts_name_the_right_cmdlets_and_quote_their_values() {
    let fw = ps::firewall_create(
        "it's",
        &FirewallSpec {
            port: 2299,
            description: "d".into(),
        },
    );
    assert!(fw.contains("New-NetFirewallRule -Name 'it''s' -DisplayName 'it''s'"));
    assert!(
        fw.contains("-Direction Inbound -Action Allow -Protocol TCP -LocalPort 2299 -Profile Any")
    );
    assert!(fw.starts_with("$ErrorActionPreference = 'Stop'"));
    assert!(
        ps::firewall_exists("x")
            .contains("Get-NetFirewallRule -DisplayName ([WildcardPattern]::Escape('x'))")
    );
    assert!(ps::firewall_delete("x").contains("| Remove-NetFirewallRule"));
    let hv = ps::hyperv_create(
        "h",
        &HyperVSpec {
            port: 2299,
            vm_creator_id: "{40E0AC32-46A5-438A-A0B2-2B479E8F2E90}".into(),
            description: String::new(),
        },
    );
    assert!(hv.contains("New-NetFirewallHyperVRule -Name 'h'"));
    assert!(hv.contains("-VMCreatorId '{40E0AC32-46A5-438A-A0B2-2B479E8F2E90}' -Protocol TCP -LocalPorts 2299 -Action Allow"));
    assert!(
        ps::hyperv_exists("h")
            .contains("Get-NetFirewallHyperVRule -Name ([WildcardPattern]::Escape('h'))")
    );
    assert!(ps::hyperv_delete("h").contains("Remove-NetFirewallHyperVRule"));
    assert!(ps::hyperv_available().contains("New-NetFirewallHyperVRule"));
    let spec = |keepalive| TaskSpec {
        distro: "Ubuntu".into(),
        keepalive,
        description: "k".into(),
    };
    let logon = ps::task_create("T", &spec(Keepalive::Logon));
    assert!(logon.contains("-AtLogOn -User $user"));
    assert!(logon.contains("-LogonType Interactive -RunLevel Limited"));
    assert!(
        logon.contains("--headless wsl.exe -d Ubuntu --exec /bin/sh -c \"exec sleep infinity\"")
    );
    assert!(logon.contains("-MultipleInstances IgnoreNew"));
    let boot = ps::task_create("T", &spec(Keepalive::Boot));
    assert!(boot.contains("-AtStartup") && boot.contains("-LogonType S4U"));
    assert!(
        ps::task_exists("T")
            .contains("Get-ScheduledTask -TaskName ([WildcardPattern]::Escape('T'))")
    );
    assert!(ps::task_delete("T").contains("Unregister-ScheduledTask"));
    assert!(ps::task_start("T").contains("| Start-ScheduledTask"));
    assert_eq!(
        ps::keepalive_arguments("U"),
        "--headless wsl.exe -d U --exec /bin/sh -c \"exec sleep infinity\""
    );
}

// frob:tests crates/goway-setup/src/hostsys.rs::is_wsl_path
// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.read_file
// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.write_file
#[test]
fn unix_paths_go_to_the_distro_as_root_and_windows_paths_stay_local() {
    assert!(is_wsl_path(Path::new("/etc/wsl.conf")));
    assert!(!is_wsl_path(Path::new("C:\\Users\\u\\.wslconfig")));
    assert!(!is_wsl_path(Path::new("relative/x")));

    let fake = Fake::new(vec![
        ("test -e /etc/absent", 1, ""),
        ("test -e /etc/wsl.conf", 0, ""),
        ("cat -- /etc/wsl.conf", 0, "[boot]\nsystemd=true\n"),
        ("tee -- /etc/x", 0, "echoed"),
    ]);
    let mut s = sys(&fake);
    assert_eq!(s.read_file(Path::new("/etc/absent")).unwrap(), None);
    assert_eq!(
        s.read_file(Path::new("/etc/wsl.conf")).unwrap().as_deref(),
        Some("[boot]\nsystemd=true\n")
    );
    s.write_file(Path::new("/etc/x"), "a\nb\n").unwrap();
    let log = fake.log.borrow();
    let last = log.last().unwrap();
    assert_eq!(last.program, "wsl.exe");
    assert_eq!(
        last.args,
        [
            "-d", "Ubuntu", "-u", "root", "--exec", "tee", "--", "/etc/x"
        ]
    );
    assert_eq!(last.stdin.as_deref(), Some(b"a\nb\n".as_slice()));
    drop(log);

    let tmp = tempfile::tempdir().unwrap();
    // A relative path is not a unix-absolute one, so it is a Windows path (nextest isolates
    // each test in its own process, so changing the directory is safe).
    std::env::set_current_dir(tmp.path()).unwrap();
    let local = Path::new("local-file");
    let before = fake.log.borrow().len();
    s.write_file(local, "windows side").unwrap();
    assert_eq!(s.read_file(local).unwrap().as_deref(), Some("windows side"));
    assert_eq!(
        fake.log.borrow().len(),
        before,
        "no command ran for a local path"
    );
}

// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.remove_file
// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.dir_exists
// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.create_dir
// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.dir_is_empty
// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.remove_dir
// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.file_digest
// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.get_mode
// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.set_mode
// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.copy_file
#[test]
fn directory_digest_and_mode_operations_map_to_plain_commands() {
    let fake = Fake::new(vec![
        ("test -d /d/empty", 0, ""),
        ("test -d /d/gone", 1, ""),
        ("find /d/empty", 0, ""),
        ("test -f /d/f", 0, ""),
        ("sha256sum -- /d/f", 0, "abc123  /d/f\n"),
        ("stat -c %a -- /d/f", 0, "644\n"),
        ("test -f /d/nofile", 1, ""),
        ("rm -f", 0, ""),
        ("mkdir", 0, ""),
        ("rmdir", 0, ""),
        ("chmod", 0, ""),
        ("cp --", 0, ""),
    ]);
    let mut s = sys(&fake);
    assert!(s.dir_exists(Path::new("/d/empty")).unwrap());
    assert!(!s.dir_exists(Path::new("/d/gone")).unwrap());
    assert!(s.dir_is_empty(Path::new("/d/empty")).unwrap());
    assert!(matches!(
        s.dir_is_empty(Path::new("/d/gone")),
        Err(SystemError::NotFound(_))
    ));
    assert_eq!(
        s.file_digest(Path::new("/d/f")).unwrap().as_deref(),
        Some("abc123")
    );
    assert_eq!(s.file_digest(Path::new("/d/nofile")).unwrap(), None);
    assert_eq!(s.get_mode(Path::new("/d/f")).unwrap(), 0o644);
    s.set_mode(Path::new("/d/f"), 0o600).unwrap();
    s.create_dir(Path::new("/d/new")).unwrap();
    s.remove_dir(Path::new("/d/gone")).unwrap(); // absent is success, nothing run
    s.remove_dir(Path::new("/d/empty")).unwrap();
    s.remove_file(Path::new("/d/f")).unwrap();
    s.copy_file(Path::new("/d/f"), Path::new("/d/g")).unwrap();
    assert!(s.copy_file(Path::new("/d/f"), Path::new("C:\\x")).is_err());
    let log = fake.log.borrow();
    assert!(log.iter().any(|i| i.args.contains(&"600".to_owned())));
    assert!(!log.iter().any(|i| i.args.contains(&"/d/gone".to_owned()) && i.args.contains(&"rmdir".to_owned())));
}

// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.resource_exists
#[test]
fn wsl_resources_exist_by_package_state_and_unit_enablement() {
    let fake = Fake::new(vec![
        (
            "dpkg-query -W -f=${Status} openssh-server",
            0,
            "install ok installed",
        ),
        (
            "dpkg-query -W -f=${Status} gone",
            0,
            "deinstall ok config-files",
        ),
        (
            "dpkg-query -W -f=${Status} half",
            0,
            "install ok half-configured",
        ),
        (
            "dpkg-query -W -f=${Status} notinst",
            0,
            "unknown ok not-installed",
        ),
        ("dpkg-query -W -f=${Status} never", 1, ""),
        ("is-enabled ssh.socket", 0, "enabled\n"),
        ("is-enabled ssh.service", 1, "disabled\n"),
        ("is-enabled old.socket", 1, ""),
        ("systemctl cat ssh.service", 0, "unit"),
        ("systemctl cat old.socket", 1, ""),
    ]);
    let s = sys(&fake);
    assert!(
        s.resource_exists(ResourceKind::WslPackage, "openssh-server")
            .unwrap()
    );
    assert!(
        s.resource_exists(ResourceKind::WslPackage, "gone").is_err(),
        "rc state is refused"
    );
    assert!(
        s.resource_exists(ResourceKind::WslPackage, "half").is_err(),
        "half-configured is refused"
    );
    assert!(
        !s.resource_exists(ResourceKind::WslPackage, "notinst")
            .unwrap()
    );
    assert!(
        !s.resource_exists(ResourceKind::WslPackage, "never")
            .unwrap()
    );
    assert!(
        s.resource_exists(ResourceKind::WslUnit, "ssh.socket")
            .unwrap()
    );
    assert!(
        !s.resource_exists(ResourceKind::WslUnit, "ssh.service")
            .unwrap()
    );
    assert!(
        s.resource_exists(ResourceKind::WslUnit, "old.socket")
            .unwrap(),
        "a unit that does not exist has nothing to enable"
    );
    assert!(matches!(
        s.resource_exists(ResourceKind::Service, "x"),
        Err(SystemError::Unsupported(_))
    ));
}

// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.resource_exists
// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.resource_create
// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.resource_delete
#[test]
fn windows_resources_run_the_matching_powershell_and_report_failures() {
    let fake = Fake::new(vec![("powershell.exe", 0, "1\n")]);
    let mut s = sys(&fake);
    assert!(s.resource_exists(ResourceKind::FirewallRule, "r").unwrap());
    let spec = serde_json::to_string(&FirewallSpec {
        port: 2299,
        description: "d".into(),
    })
    .unwrap();
    s.resource_create(ResourceKind::FirewallRule, "r", &spec)
        .unwrap();
    s.resource_delete(ResourceKind::FirewallRule, "r").unwrap();
    s.resource_delete(ResourceKind::ScheduledTask, "t").unwrap();
    s.resource_delete(ResourceKind::HyperVFirewallRule, "h")
        .unwrap();
    let scripts: Vec<String> = fake.log.borrow().iter().map(script_of).collect();
    assert!(
        scripts[0].contains("Get-NetFirewallRule -DisplayName ([WildcardPattern]::Escape('r'))")
    );
    assert!(scripts[1].contains("New-NetFirewallRule") && scripts[1].contains("2299"));
    assert!(scripts[2].contains("Remove-NetFirewallRule"));
    assert!(scripts[3].contains("Unregister-ScheduledTask"));
    assert!(scripts[4].contains("Remove-NetFirewallHyperVRule"));
    assert!(
        s.resource_create(ResourceKind::FirewallRule, "r", "not json")
            .is_err()
    );

    let failing = Fake::new(vec![("powershell.exe", 1, "")]);
    let mut f = sys(&failing);
    let err = f
        .resource_create(
            ResourceKind::ScheduledTask,
            "t",
            &serde_json::to_string(&TaskSpec {
                distro: "Ubuntu".into(),
                keepalive: Keepalive::Logon,
                description: String::new(),
            })
            .unwrap(),
        )
        .unwrap_err();
    assert!(matches!(err, SystemError::Command { .. }), "{err}");
}

// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.resource_create
// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.resource_exists
#[test]
fn packages_in_odd_dpkg_states_are_refused_and_no_apt_command_runs() {
    // iF (half-configured) and rc (removed, configuration left) are the states that went wrong
    // on a live machine once: both are refused outright.
    for status in [
        "install ok half-configured",
        "deinstall ok config-files",
        "install ok unpacked",
    ] {
        let fake = Fake::new(vec![("dpkg-query", 0, status), ("wsl.exe", 0, "")]);
        let mut s = sys(&fake);
        assert!(
            s.resource_exists(ResourceKind::WslPackage, "openssh-server")
                .is_err(),
            "{status}"
        );
        assert!(
            s.resource_create(ResourceKind::WslPackage, "openssh-server", "")
                .is_err(),
            "{status}"
        );
        assert!(
            fake.log
                .borrow()
                .iter()
                .all(|i| i.args.iter().all(|a| !a.starts_with("apt"))),
            "{status}: no apt command may run"
        );
    }
    // A cleanly installed package exists, so a plan records a no-op and uninstall never touches it.
    let fake = Fake::new(vec![("dpkg-query", 0, "install ok installed")]);
    let mut s = sys(&fake);
    assert!(
        s.resource_exists(ResourceKind::WslPackage, "openssh-server")
            .unwrap()
    );
    assert!(
        s.resource_create(ResourceKind::WslPackage, "openssh-server", "")
            .is_err()
    );
}

// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.resource_create
// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.resource_delete
#[test]
fn packages_install_noninteractively_and_are_removed_never_purged() {
    let fake = Fake::new(vec![("dpkg-query", 1, ""), ("wsl.exe", 0, "")]);
    let mut s = sys(&fake);
    s.resource_create(ResourceKind::WslPackage, "openssh-server", "")
        .unwrap();
    s.resource_create(ResourceKind::WslUnit, "ssh.socket", "")
        .unwrap();
    s.resource_delete(ResourceKind::WslUnit, "ssh.socket")
        .unwrap();
    s.resource_delete(ResourceKind::WslPackage, "openssh-server")
        .unwrap();
    let cmds: Vec<String> = fake
        .log
        .borrow()
        .iter()
        .map(|i| i.args[5..].join(" "))
        .collect();
    assert_eq!(
        cmds,
        [
            "dpkg-query -W -f=${Status} openssh-server",
            "apt-get update",
            "env DEBIAN_FRONTEND=noninteractive apt-get install -y openssh-server",
            "systemctl enable ssh.socket",
            "systemctl disable ssh.socket",
            "env DEBIAN_FRONTEND=noninteractive apt-get remove -y openssh-server",
        ]
    );
}

// frob:tests crates/goway-setup/src/hostsys.rs::parse_sshd_ports
// frob:tests crates/goway-setup/src/hostsys.rs::parse_listening_ports
#[test]
fn port_lists_are_parsed_from_sshd_and_ss_output() {
    assert_eq!(
        parse_sshd_ports("port 2299\nport 2222\nlistenaddress 0.0.0.0:22\nport 2222\n"),
        [2222, 2299]
    );
    assert!(parse_sshd_ports("").is_empty());
    let ss = "LISTEN 0 4096 127.0.0.53%lo:53 0.0.0.0:*\nLISTEN 0 4096 0.0.0.0:2222 0.0.0.0:*\nLISTEN 0 4096 [::]:2222 [::]:*\n";
    assert_eq!(
        parse_listening_ports(ss).into_iter().collect::<Vec<_>>(),
        [53, 2222]
    );
}

// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.probe
// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.hyperv_firewall_available
// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.sshd_ports
// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.default_user_has_authorized_keys
// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.distro_reachable
// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.systemd_running
#[test]
fn probing_collects_the_facts_the_plan_depends_on() {
    let fake = Fake::new(vec![
        ("powershell.exe", 0, "1\n"),
        ("test -x /usr/sbin/sshd", 0, ""),
        ("/usr/sbin/sshd -T", 0, "port 2222\n"),
        ("-d Ubuntu --exec printenv HOME", 0, "/home/user\n"),
        ("test -s /home/user/.ssh/authorized_keys", 0, ""),
        ("--exec true", 0, ""),
        ("ps -p 1 -o comm=", 0, "systemd\n"),
    ]);
    let s = sys(&fake);
    assert!(s.distro_reachable().unwrap());
    assert!(s.systemd_running().unwrap());
    let facts = s.probe().unwrap();
    assert!(facts.hyperv_firewall);
    assert_eq!(facts.sshd_ports, [2222]);
    assert!(facts.authorized_keys);

    let bare = Fake::new(vec![
        ("powershell.exe", 0, "0\n"),
        ("test -x /usr/sbin/sshd", 1, ""),
        ("printenv HOME", 0, "/home/user\n"),
        ("test -s", 1, ""),
        ("ps -p 1", 0, "init\n"),
    ]);
    let b = sys(&bare);
    let facts = b.probe().unwrap();
    assert!(!facts.hyperv_firewall && facts.sshd_ports.is_empty() && !facts.authorized_keys);
    assert!(!b.systemd_running().unwrap());
}

// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.activate_sshd
// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.listening_ports
#[test]
fn activation_restarts_only_when_the_port_is_not_yet_listening() {
    let listening = "LISTEN 0 4096 0.0.0.0:2222 0.0.0.0:*\n";
    let fake = Fake::new(vec![
        ("sshd -t", 0, ""),
        ("ss -Hltn", 0, listening),
        ("is-active --quiet ssh", 0, ""),
        ("systemctl", 0, ""),
    ]);
    let mut s = sys(&fake);
    assert_eq!(s.activate_sshd(2222).unwrap(), Activation::Reloaded);
    let cmds: Vec<String> = fake
        .log
        .borrow()
        .iter()
        .map(|i| i.args[5..].join(" "))
        .collect();
    assert!(cmds.contains(&"systemctl reload ssh".to_owned()));
    assert!(!cmds.iter().any(|c| c.contains("restart")), "{cmds:?}");

    // Not listening on 2299 yet: restart the socket (ssh.socket is enabled), then it shows up.
    let two = Fake::new(vec![("sshd -t", 0, ""), ("systemctl", 0, "")]);
    let mut s = HostSystem::with_runner("Ubuntu", Flip(&two, RefCell::new(0)));
    assert_eq!(s.activate_sshd(2299).unwrap(), Activation::Restarted);
    let cmds: Vec<String> = two
        .log
        .borrow()
        .iter()
        .map(|i| i.args[5..].join(" "))
        .collect();
    assert!(
        cmds.contains(&"systemctl restart ssh.socket".to_owned()),
        "{cmds:?}"
    );
}

// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.deactivate_sshd
// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.start_task
#[test]
fn deactivation_leaves_a_port_that_is_still_configured_and_starts_tasks_by_name() {
    let fake = Fake::new(vec![
        ("ss -Hltn", 0, "LISTEN 0 1 0.0.0.0:2222 0.0.0.0:*\n"),
        ("systemctl", 0, ""),
        ("test -x /usr/sbin/sshd", 0, ""),
        ("sshd -T", 0, "port 2222\n"),
        ("powershell.exe", 0, ""),
    ]);
    let mut s = sys(&fake);
    // Port 2299 is not listening: nothing to do.
    assert_eq!(s.deactivate_sshd(2299).unwrap(), None);
    // Port 2222 listens and is still configured: nothing to do either.
    assert_eq!(s.deactivate_sshd(2222).unwrap(), None);
    s.start_task("WSL Keepalive").unwrap();
    let last = fake.log.borrow().last().cloned().unwrap();
    assert!(script_of(&last).contains("| Start-ScheduledTask"));
}

// frob:tests crates/goway-setup/src/ps.rs::firewall_delete
// frob:tests crates/goway-setup/src/ps.rs::task_delete
// frob:tests crates/goway-setup/src/ps.rs::hyperv_delete
// frob:tests crates/goway-setup/src/ps.rs::task_exists
#[test]
fn lookups_match_a_name_exactly_and_never_as_a_wildcard() {
    for script in [
        ps::firewall_delete("*"),
        ps::firewall_exists("*"),
        ps::hyperv_delete("*"),
        ps::task_delete("*"),
        ps::task_exists("*"),
        ps::task_start("*"),
    ] {
        assert!(
            script.contains("[WildcardPattern]::Escape('*')"),
            "the name is escaped for the cmdlet: {script}"
        );
        assert!(
            script.contains("Where-Object { $_.") && script.contains("-ceq '*' }"),
            "and the result is filtered on exact equality: {script}"
        );
    }
}

// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem
#[test]
fn powershell_resources_with_wildcard_names_never_reach_powershell() {
    let fake = Fake::new(vec![("powershell.exe", 0, "1\n")]);
    let mut s = sys(&fake);
    for kind in [
        ResourceKind::FirewallRule,
        ResourceKind::HyperVFirewallRule,
        ResourceKind::ScheduledTask,
    ] {
        assert!(s.resource_exists(kind, "*").is_err());
        assert!(s.resource_delete(kind, "WSL*").is_err());
        assert!(s.resource_create(kind, "a?", "{}").is_err());
    }
    assert!(fake.log.borrow().is_empty(), "no command was started");
}

// frob:tests crates/goway-setup/src/sysapi.rs::tool_path
#[test]
fn system_tools_are_started_by_the_expected_path() {
    use goway_setup::sysapi::{Tool, tool_path};
    let ps = tool_path(Tool::PowerShell);
    let wsl = tool_path(Tool::Wsl);
    if cfg!(windows) {
        assert!(
            ps.to_lowercase()
                .ends_with(r"\system32\windowspowershell\v1.0\powershell.exe")
        );
        assert!(wsl.to_lowercase().ends_with(r"\system32\wsl.exe"));
    } else {
        assert_eq!((ps.as_str(), wsl.as_str()), ("powershell.exe", "wsl.exe"));
    }
}
