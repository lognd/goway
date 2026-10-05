//! The real host `System`, driven through a scripted fake instead of `wsl.exe` and PowerShell.

use std::cell::RefCell;
use std::path::Path;

use goway_journal::{ResourceKind, System, SystemError};
use goway_setup::host::{FirewallSpec, HyperVSpec, Keepalive, Scope, TaskSpec};
use goway_setup::hostsys::{
    Activation, HostSystem, Invocation, Output, Runner, is_wsl_path, parse_listening_ports,
    parse_sshd_ports,
};
use goway_setup::ps;
use goway_setup::sysapi::{Tool, tool_path};

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
    assert_eq!(inv.program, tool_path(Tool::PowerShell));
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
    // PowerShell reads the typographic single quotes as quotes too.
    assert_eq!(
        ps::quote("a\u{2019}b\u{2018}"),
        "'a\u{2019}\u{2019}b\u{2018}\u{2018}'"
    );
    let script = "Write-Output 'caf\u{e9}'";
    let inv = Invocation {
        program: tool_path(Tool::PowerShell),
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
// frob:tests crates/goway-setup/src/ps.rs::task_outdated_snapshot
// frob:tests crates/goway-setup/src/ps.rs::task_restore
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
            scope: Scope::new(&[]),
        },
    );
    assert!(fw.contains("New-NetFirewallRule -Name 'it''s' -DisplayName 'it''s'"));
    assert!(fw.contains(
        "-Direction Inbound -Action Allow -Protocol TCP -LocalPort 2299 -Profile 'Private','Domain' -RemoteAddress 'LocalSubnet' -Enabled True"
    ));
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
            scope: Scope::new(&[]),
        },
    );
    assert!(hv.contains("New-NetFirewallHyperVRule -Name 'h'"));
    assert!(hv.contains("-VMCreatorId '{40E0AC32-46A5-438A-A0B2-2B479E8F2E90}' -Protocol TCP -LocalPorts 2299 -Profiles 'Private','Domain' -RemoteAddresses 'LocalSubnet' -Action Allow"));
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
    let (conhost, wsl) = (
        r"C:\Windows\System32\conhost.exe",
        r"C:\Windows\System32\wsl.exe",
    );
    let logon = ps::task_create("T", &spec(Keepalive::Logon), conhost, wsl);
    assert!(logon.contains("-AtLogOn -User $user"));
    assert!(logon.contains("-LogonType Interactive -RunLevel Limited"));
    assert!(logon.contains(
        "-Execute 'C:\\Windows\\System32\\conhost.exe' -Argument '--headless \"C:\\Windows\\System32\\wsl.exe\" -d Ubuntu --exec /bin/sh -c \"exec sleep infinity\"'"
    ));
    assert!(logon.contains("-MultipleInstances IgnoreNew"));
    let boot = ps::task_create("T", &spec(Keepalive::Boot), conhost, wsl);
    assert!(boot.contains("-AtStartup") && boot.contains("-LogonType S4U"));
    // Both triggers repeat so a distro shut down by WSL is started again, and the task replaces
    // registered the same way as before.
    for script in [&logon, &boot] {
        assert!(script.contains("-RepetitionInterval (New-TimeSpan -Minutes 5)"));
        assert!(script.contains("$trigger.Repetition = $repeat.Repetition"));
        assert!(script.contains("Register-ScheduledTask -TaskName 'T'"));
        assert!(script.contains("-MultipleInstances IgnoreNew"));
    }
    let stale = ps::task_outdated_snapshot("T");
    assert!(stale.contains("Get-ScheduledTask -TaskName ([WildcardPattern]::Escape('T'))"));
    assert!(stale.contains("Export-ScheduledTask"));
    let restore = ps::task_restore("T", "<Task a='b'/>");
    assert!(restore.contains("Register-ScheduledTask -TaskName 'T' -Xml '<Task a=''b''/>'"));
    assert!(stale.contains(
        "-not ($t.Triggers | Where-Object { $_.Repetition -and $_.Repetition.Interval })"
    ));
    assert!(
        ps::task_exists("T")
            .contains("Get-ScheduledTask -TaskName ([WildcardPattern]::Escape('T'))")
    );
    assert!(ps::task_delete("T").contains("Unregister-ScheduledTask"));
    assert!(ps::task_start("T").contains("| Start-ScheduledTask"));
    assert_eq!(
        ps::keepalive_arguments("U", wsl),
        "--headless \"C:\\Windows\\System32\\wsl.exe\" -d U --exec /bin/sh -c \"exec sleep infinity\""
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
    assert_eq!(last.program, tool_path(Tool::Wsl));
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
        scope: Scope::new(&[]),
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

// frob:ticket 01M44WPJWSD12YH6MEZKZE0GWF
// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.resource_create
// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.resource_delete
// frob:tests crates/goway-setup/src/ps.rs::wsl_sparse_exists
// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.resource_exists
#[test]
fn the_virtual_disk_is_made_sparse_through_wsl_manage_and_set_back_on_undo() {
    let fake = Fake::new(vec![("-EncodedCommand", 0, "0"), ("wsl.exe", 0, "")]);
    let mut s = sys(&fake);
    assert!(
        !s.resource_exists(ResourceKind::WslSparseVhd, "Ubuntu")
            .unwrap()
    );
    s.resource_create(ResourceKind::WslSparseVhd, "Ubuntu", "")
        .unwrap();
    s.resource_delete(ResourceKind::WslSparseVhd, "Ubuntu")
        .unwrap();
    let log = fake.log.borrow();
    let wsl: Vec<String> = log
        .iter()
        .filter(|i| i.program.to_lowercase().ends_with("wsl.exe"))
        .map(|i| i.args.join(" "))
        .collect();
    assert_eq!(
        wsl,
        [
            "--manage Ubuntu --set-sparse true",
            "--manage Ubuntu --set-sparse false"
        ]
    );
    let query = &log[0];
    assert!(
        query.program.to_lowercase().contains("powershell"),
        "{query:?}"
    );
    drop(log);
    // A refusal by wsl.exe is an error, never a silent success.
    let fake = Fake::new(vec![("wsl.exe", 1, "")]);
    let mut s = sys(&fake);
    assert!(
        s.resource_create(ResourceKind::WslSparseVhd, "Ubuntu", "")
            .is_err()
    );
    let flagged = Fake::new(vec![("-EncodedCommand", 0, "1")]);
    assert!(
        sys(&flagged)
            .resource_exists(ResourceKind::WslSparseVhd, "Ubuntu")
            .unwrap()
    );
}

// frob:ticket 01M44WPJWSD12YH6MEZKZE0GWF
// frob:tests crates/goway-setup/src/hostsys.rs::parse_wsl_version
// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.sparse_supported
#[test]
fn the_wsl_version_decides_whether_sparse_is_possible_and_an_old_wsl_is_skipped() {
    assert_eq!(
        goway_setup::hostsys::parse_wsl_version(
            "WSL version: 2.5.7.0\nKernel version: 6.6.87.1-1\n"
        ),
        Some((2, 5, 7))
    );
    assert_eq!(
        goway_setup::hostsys::parse_wsl_version("\u{feff}WSL-Version: 1.2.5.0\n"),
        Some((1, 2, 5))
    );
    assert_eq!(
        goway_setup::hostsys::parse_wsl_version("no version here"),
        None
    );
    for (out, code, want) in [
        ("WSL version: 2.5.7.0\n", 0, true),
        ("WSL version: 1.2.5.0\n", 0, false),
        ("Invalid command line argument: --version", 1, false),
    ] {
        let fake = Fake::new(vec![("wsl.exe --version", code, out)]);
        assert_eq!(sys(&fake).sparse_supported().unwrap(), want, "{out}");
    }
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
        ("netsh.exe", 0, ""),
    ]);
    let s = sys(&fake);
    assert!(s.distro_reachable().unwrap());
    assert!(s.systemd_running().unwrap());
    let facts = s.probe(Path::new("/nonexistent-home")).unwrap();
    assert!(facts.hyperv_firewall);
    assert_eq!(facts.sshd_ports, [2222]);
    assert!(facts.authorized_keys);

    let bare = Fake::new(vec![
        ("powershell.exe", 0, "0\n"),
        ("test -x /usr/sbin/sshd", 1, ""),
        ("printenv HOME", 0, "/home/user\n"),
        ("test -s", 1, ""),
        ("ps -p 1", 0, "init\n"),
        ("netsh.exe", 0, ""),
    ]);
    let b = sys(&bare);
    let facts = b.probe(Path::new("/nonexistent-home")).unwrap();
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

// frob:tests crates/goway-setup/src/ps.rs::firewall_create
// frob:tests crates/goway-setup/src/ps.rs::hyperv_create
#[test]
fn firewall_rules_are_scoped_to_local_networks_and_never_open_to_everyone() {
    let spec = |scope| FirewallSpec {
        port: 2299,
        description: "d".into(),
        scope,
    };
    let default = ps::firewall_create("r", &spec(Scope::new(&[])));
    assert!(default.contains("-RemoteAddress"), "{default}");
    assert!(!default.contains("-Profile Any"), "{default}");
    assert!(!default.contains("Public"), "{default}");
    // Widening is explicit and keeps the local subnet.
    let wide = ps::firewall_create("r", &spec(Scope::new(&["100.64.0.0/10".to_owned()])));
    assert!(
        wide.contains("-RemoteAddress 'LocalSubnet','100.64.0.0/10'"),
        "{wide}"
    );
    // Scopes read from an old journal (or damaged ones) never produce an open rule.
    for scope in [
        Scope::default(),
        Scope {
            profiles: Some("Any".into()),
            remote_addresses: Vec::new(),
        },
        Scope {
            profiles: Some("Private,Public; calc".into()),
            remote_addresses: Vec::new(),
        },
    ] {
        let script = ps::firewall_create("r", &spec(scope));
        assert!(script.contains("-RemoteAddress 'LocalSubnet'"), "{script}");
        assert!(
            !script.contains("-Profile Any") && !script.contains("calc"),
            "{script}"
        );
    }
    let hv = ps::hyperv_create(
        "h",
        &HyperVSpec {
            port: 2299,
            vm_creator_id: "{40E0AC32-46A5-438A-A0B2-2B479E8F2E90}".into(),
            description: String::new(),
            scope: Scope::new(&["10.0.0.0/8".to_owned()]),
        },
    );
    assert!(
        hv.contains("-RemoteAddresses 'LocalSubnet','10.0.0.0/8'")
            && hv.contains("-Profiles 'Private','Domain'"),
        "{hv}"
    );
}

// frob:tests crates/goway-setup/src/ps.rs::public_networks
// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.public_networks
#[test]
fn public_networks_are_listed_one_per_line() {
    let fake = Fake::new(vec![("powershell.exe", 0, "Cafe Wi-Fi\r\n\r\nOther\n")]);
    let s = sys(&fake);
    assert_eq!(s.public_networks().unwrap(), ["Cafe Wi-Fi", "Other"]);
    assert!(ps::public_networks().contains("NetworkCategory -eq 'Public'"));
}

// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.host_key_fingerprint
// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.default_user
#[test]
fn the_fingerprint_comes_from_the_ed25519_key_as_root_and_the_user_from_the_default_user() {
    let fake = Fake::new(vec![
        (
            "ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub",
            0,
            "256 SHA256:abc123 root@h (ED25519)\n",
        ),
        ("--exec id -un", 0, "user\n"),
    ]);
    let s = sys(&fake);
    assert_eq!(
        s.host_key_fingerprint().unwrap().as_deref(),
        Some("SHA256:abc123")
    );
    assert_eq!(s.default_user().unwrap(), "user");
    let log = fake.log.borrow();
    assert!(
        log[0].args.join(" ").contains("-u root"),
        "the key is read as root"
    );
    assert!(
        !log[1].args.join(" ").contains("-u root"),
        "the user is the default user"
    );
    let missing = Fake::new(vec![("ssh-keygen", 1, "")]);
    assert_eq!(sys(&missing).host_key_fingerprint().unwrap(), None);
}

// frob:tests crates/goway-setup/src/safefile.rs::write
// frob:tests crates/goway-setup/src/safefile.rs::read_to_string
#[test]
#[cfg(unix)]
fn the_elevated_side_never_writes_or_reads_through_a_link() {
    use goway_setup::safefile::{is_link_refusal, read_to_string, write};
    let tmp = tempfile::tempdir().unwrap();
    let secret = tmp.path().join("admin-only.txt");
    std::fs::write(&secret, "secret\n").unwrap();
    let link = tmp.path().join(".wslconfig");
    std::os::unix::fs::symlink(&secret, &link).unwrap();
    let err = write(&link, "[wsl2]\nnetworkingMode=mirrored\n").unwrap_err();
    assert!(is_link_refusal(&err), "{err}");
    assert!(err.to_string().contains("refusing to follow"), "{err}");
    assert_eq!(std::fs::read_to_string(&secret).unwrap(), "secret\n");
    assert!(is_link_refusal(&read_to_string(&link).unwrap_err()));
    // A dangling link is refused as well, not created through.
    let dangling = tmp.path().join("dangling");
    std::os::unix::fs::symlink(tmp.path().join("new-target"), &dangling).unwrap();
    assert!(write(&dangling, "x").is_err());
    assert!(!tmp.path().join("new-target").exists());
    // Ordinary files still work, and an absent one reads as None.
    let plain = tmp.path().join("plain");
    assert_eq!(read_to_string(&plain).unwrap(), None);
    write(&plain, "a long first version\n").unwrap();
    write(&plain, "b\n").unwrap();
    assert_eq!(read_to_string(&plain).unwrap().as_deref(), Some("b\n"));
}

// frob:tests crates/goway-setup/src/hostsys.rs::write_file
#[test]
#[cfg(windows)]
fn a_windows_path_that_is_a_symlink_is_refused_by_the_host_system() {
    let tmp = tempfile::tempdir().unwrap();
    let secret = tmp.path().join("admin-only.txt");
    std::fs::write(&secret, "secret\n").unwrap();
    let link = tmp.path().join(".wslconfig");
    if std::os::windows::fs::symlink_file(&secret, &link).is_err() {
        return; // no symlink privilege here
    }
    let fake = Fake::new(vec![]);
    let mut sys = HostSystem::with_runner("Ubuntu", &fake);
    let err = sys.write_file(&link, "x").unwrap_err();
    assert!(err.to_string().contains("refusing to follow"), "{err}");
    assert_eq!(std::fs::read_to_string(&secret).unwrap(), "secret\n");
    assert!(sys.read_file(&link).is_err());
}

// frob:tests crates/goway-setup/src/hostsys.rs::never_start_wsl
// frob:tests crates/goway-setup/src/hostsys.rs::distro_running
#[test]
fn an_elevated_system_never_starts_a_distro_that_is_not_running() {
    // Nothing running: the guard asks only `--list --running` and refuses; no command enters the distro.
    let fake = Fake::new(vec![("--list --running", 0, "\u{feff}Debian\r\n")]);
    let guarded = sys(&fake).never_start_wsl(true);
    let err = guarded.distro_reachable().unwrap_err();
    assert!(
        matches!(&err, SystemError::InvalidState(m) if m.contains("never starts WSL")),
        "{err}"
    );
    assert!(
        fake.log
            .borrow()
            .iter()
            .all(|i| i.args.first().map(String::as_str) == Some("--list")),
        "{:?}",
        fake.log.borrow()
    );
    // Running (any case): the command goes through.
    let fake = Fake::new(vec![
        ("--list --running", 0, "Debian\r\nubuntu\r\n"),
        ("--exec true", 0, ""),
    ]);
    assert!(sys(&fake).never_start_wsl(true).distro_reachable().unwrap());
    // An unguarded (non-elevated) system starts the distro as before, with no list query.
    let fake = Fake::new(vec![("--exec true", 0, "")]);
    assert!(sys(&fake).distro_reachable().unwrap());
    assert_eq!(fake.log.borrow().len(), 1);
}

// frob:tests crates/goway-setup/src/hostsys.rs::parse_interop_disabled
// frob:tests crates/goway-setup/src/hostsys.rs::ini_value
#[test]
fn wsl_conf_interop_setting_is_read_from_its_own_section() {
    use goway_setup::hostsys::parse_interop_disabled;
    assert!(parse_interop_disabled(
        "[boot]\nsystemd=true\n[Interop]\nEnabled = False\nappendWindowsPath=false\n"
    ));
    assert!(!parse_interop_disabled("[interop]\nenabled=true\n"));
    assert!(!parse_interop_disabled("[boot]\nenabled=false\n"));
    assert!(!parse_interop_disabled(""));
}

// frob:tests crates/goway-setup/src/hostsys.rs::admin_account
// frob:tests crates/goway-setup/src/ps.rs::admin_account
#[test]
fn the_administrator_probe_reads_the_token_groups_and_defaults_to_no() {
    let fake = Fake::new(vec![("EncodedCommand", 0, "True\r\n")]);
    assert!(sys(&fake).admin_account());
    assert!(ps::admin_account().contains("S-1-5-32-544"));
    assert!(!ps::admin_account().contains("IsInRole"));
    let fake = Fake::new(vec![("EncodedCommand", 0, "False\r\n")]);
    assert!(!sys(&fake).admin_account());
}

// frob:tests crates/goway-setup/src/hostsys.rs::goway_jobs_running
#[test]
fn goway_jobs_are_counted_in_a_running_distro_and_a_stopped_one_has_none() {
    // Not running: zero, and nothing enters the distro.
    let fake = Fake::new(vec![("--list --running", 0, "Debian\r\n")]);
    assert_eq!(sys(&fake).goway_jobs_running(".cache/goway").unwrap(), 0);
    assert!(fake.log.borrow().iter().all(|i| i.args[0] == "--list"));
    // Running: the count the lock probe printed; the root is an argument, never part of the script.
    let fake = Fake::new(vec![
        ("--list --running", 0, "Ubuntu\r\n"),
        ("goway-jobs", 0, "2\n"),
    ]);
    assert_eq!(sys(&fake).goway_jobs_running(".cache/goway").unwrap(), 2);
    let log = fake.log.borrow();
    let probe = log.last().unwrap();
    assert!(probe.args.contains(&".cache/goway".to_owned()));
    assert!(probe.args.iter().any(|a| a.contains("flock -n")));
    assert!(!probe.args.iter().any(|a| a == "--user"));
}
