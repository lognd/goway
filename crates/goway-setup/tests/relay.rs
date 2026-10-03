//! NAT mode: the choice of networking, the relay entries of the host plan, the refresh script,
//! the elevated validator's view of them, and the install/uninstall round trip.

use std::path::{Path, PathBuf};

use goway_journal::{
    Change, Journal, ModelSystem, Outcome, Prior, ResourceKind, System, apply, revert,
};
use goway_setup::error::SetupError;
use goway_setup::host::{
    self, HostFacts, HostParams, HostSettings, Keepalive, MIRRORED_MIN_BUILD, NetworkChoice,
    NetworkMode, host_plan,
};
use goway_setup::hostsys::{HostSystem, Invocation, Output, Runner, parse_networking_mode};
use goway_setup::layout::Layout;
use goway_setup::ps;
use goway_setup::relay::{
    self, PortProxyRule, PortProxySpec, RelayTaskSpec, netsh_args, parse_portproxy_table,
    parse_relay_name, parse_wsl_ip, refresh_script, relay_name, rule_on_port,
};
use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};

const HOME: &str = "/home/u";
const PORT: u16 = 2299;

fn layout() -> Layout {
    Layout::new(Path::new("/Local"), Path::new("/ProgramData"), "goway-test").unwrap()
}

fn params(port: u16, keepalive: Keepalive, network: NetworkMode) -> HostParams {
    HostParams {
        port,
        distro: "Ubuntu".into(),
        keepalive,
        harden: true,
        allow_from: Vec::new(),
        home: PathBuf::from(HOME),
        network,
    }
}

fn settings(network: NetworkMode) -> HostSettings {
    HostSettings {
        distro: "Ubuntu".into(),
        port: PORT,
        allow_from: Vec::new(),
        network,
    }
}

fn machine() -> ModelSystem {
    let mut m = ModelSystem::new();
    for d in [
        "/home",
        HOME,
        "/etc",
        "/ProgramData",
        "/ProgramData/goway",
        "/ProgramData/goway/goway-test",
    ] {
        m.dirs.insert(d.into());
    }
    m
}

fn nat_plan(port: u16, keepalive: Keepalive) -> Vec<Change> {
    host_plan(
        &layout(),
        &params(port, keepalive, NetworkMode::Nat),
        &HostFacts::assumed(),
    )
}

fn facts(build: Option<u32>, wslconfig: Option<&str>) -> HostFacts {
    HostFacts {
        windows_build: build,
        wslconfig_network: wslconfig.map(str::to_owned),
        ..HostFacts::assumed()
    }
}

fn resources(plan: &[Change], kind: ResourceKind) -> Vec<&Change> {
    plan.iter()
        .filter(|c| matches!(c, Change::EnsureResource { kind: k, .. } if *k == kind))
        .collect()
}

fn is_networking_mode(c: &Change) -> bool {
    matches!(c, Change::SetIniKey { key, .. } if key == "networkingMode")
}

// frob:tests crates/goway-setup/src/host.rs::resolve_network
#[test]
fn the_networking_mode_follows_the_flag_the_build_and_an_explicit_nat_setting() {
    use NetworkChoice::{Auto, Mirrored, Nat};
    use NetworkMode as M;
    let win10 = 19045;
    let win11_21h2 = 22000;
    let win11 = MIRRORED_MIN_BUILD;
    let cases = [
        // (choice, build, existing networkingMode, expected)
        (Auto, Some(win11), None, Some(M::Mirrored)),
        (Auto, Some(win11 + 100), Some("mirrored"), Some(M::Mirrored)),
        (Auto, Some(win11), Some("nat"), Some(M::Nat)),
        (Auto, Some(win11), Some("NAT"), Some(M::Nat)),
        (Auto, Some(win11 - 1), None, Some(M::Nat)),
        (Auto, Some(win11_21h2), None, Some(M::Nat)),
        (Auto, Some(win10), None, Some(M::Nat)),
        (Auto, None, None, Some(M::Nat)),
        (Nat, Some(win11), None, Some(M::Nat)),
        (Nat, Some(win10), None, Some(M::Nat)),
        (Mirrored, Some(win11), None, Some(M::Mirrored)),
        (Mirrored, Some(win11), Some("nat"), Some(M::Mirrored)),
        (Mirrored, Some(win10), None, None),
        (Mirrored, None, None, None),
    ];
    for (choice, build, existing, expected) in cases {
        let got = host::resolve_network(choice, &facts(build, existing));
        match expected {
            Some(mode) => assert_eq!(got.unwrap(), mode, "{choice:?} {build:?} {existing:?}"),
            None => assert!(
                matches!(got, Err(SetupError::MirroredUnsupported { .. })),
                "{choice:?} {build:?}: {got:?}"
            ),
        }
    }
}

// frob:tests crates/goway-setup/src/hostsys.rs::parse_networking_mode
#[test]
fn networking_mode_is_read_from_the_wsl2_section_only() {
    assert_eq!(parse_networking_mode(""), None);
    assert_eq!(
        parse_networking_mode("[wsl2]\nnetworkingMode=nat\n").as_deref(),
        Some("nat")
    );
    assert_eq!(
        parse_networking_mode("[WSL2]\r\nmemory=4GB\r\nNetworkingMode = mirrored\r\n").as_deref(),
        Some("mirrored")
    );
    assert_eq!(
        parse_networking_mode("[experimental]\nnetworkingMode=nat\n[wsl2]\nx=1\n"),
        None,
        "another section's key is not the setting"
    );
}

// frob:tests crates/goway-setup/src/host.rs::host_plan
// frob:tests crates/goway-setup/src/host.rs::relay_task_name
// frob:tests crates/goway-setup/src/host.rs::relay_script_path
#[test]
fn the_relay_script_and_task_appear_only_in_nat_mode_and_nat_leaves_wslconfig_alone() {
    let l = layout();
    for keepalive in [Keepalive::Logon, Keepalive::Boot] {
        let mirrored = host_plan(
            &l,
            &params(PORT, keepalive, NetworkMode::Mirrored),
            &HostFacts::assumed(),
        );
        assert!(mirrored.iter().any(is_networking_mode));
        assert!(resources(&mirrored, ResourceKind::PortProxy).is_empty());
        assert_eq!(resources(&mirrored, ResourceKind::ScheduledTask).len(), 1);
        assert!(!mirrored.iter().any(|c| matches!(c,
            Change::WriteFile { path, .. } if path.ends_with(relay::SCRIPT_NAME))));

        let nat = nat_plan(PORT, keepalive);
        assert!(
            !nat.iter().any(is_networking_mode),
            "nat mode never touches networkingMode"
        );
        let proxies = resources(&nat, ResourceKind::PortProxy);
        assert_eq!(proxies.len(), 1);
        assert!(
            matches!(proxies[0], Change::EnsureResource { name, spec, .. }
            if name == "0.0.0.0:2299"
            && serde_json::from_str::<PortProxySpec>(spec).unwrap()
                == PortProxySpec { listen_address: "0.0.0.0".into(), port: PORT })
        );
        let tasks: Vec<&str> = resources(&nat, ResourceKind::ScheduledTask)
            .into_iter()
            .map(|c| match c {
                Change::EnsureResource { name, .. } => name.as_str(),
                _ => unreachable!(),
            })
            .collect();
        let suffix = if keepalive == Keepalive::Boot {
            " (boot)"
        } else {
            ""
        };
        assert_eq!(
            tasks,
            [
                format!("goway-test WSL Keepalive{suffix}"),
                format!("goway-test WSL Relay{suffix}")
            ]
        );
        let script_at = nat
            .iter()
            .position(|c| {
                matches!(c, Change::WriteFile { path, .. }
                if *path == host::relay_script_path(&l))
            })
            .expect("the script is written");
        let proxy_at = nat.iter().position(|c| *c == *proxies[0]).unwrap();
        let task_at = nat
            .iter()
            .position(|c| {
                matches!(c, Change::EnsureResource { name, .. }
                if *name == host::relay_task_name("goway-test", keepalive))
            })
            .unwrap();
        assert!(
            script_at < proxy_at && proxy_at < task_at,
            "reverted in the opposite order: the task goes before the script"
        );
        assert!(host::relay_script_path(&l).starts_with(&l.admin_dir));
        assert_eq!(
            host::network_in_journal(&apply(&nat, &mut machine()).unwrap()),
            NetworkMode::Nat
        );
    }
    assert_eq!(
        host::relay_task_name("goway", Keepalive::Logon),
        "WSL Relay"
    );
}

// frob:tests crates/goway-setup/src/relay.rs::refresh_script
// frob:tests crates/goway-setup/src/relay.rs::netsh_args
#[test]
fn the_refresh_script_validates_the_address_and_builds_the_exact_netsh_command() {
    let script = refresh_script("goway-test", "Ubuntu", PORT);
    assert!(script.is_ascii());
    assert!(script.contains("$distro = 'Ubuntu'") && script.contains("$port = 2299"));
    // The one command that changes the relay, with the validated variable and nothing else.
    assert!(script.contains(
        "& $netsh interface portproxy set v4tov4 listenaddress=0.0.0.0 listenport=2299 connectaddress=$ip connectport=2299\n"
    ));
    assert_eq!(
        netsh_args("set", "0.0.0.0", PORT, "$ip").join(" "),
        "interface portproxy set v4tov4 listenaddress=0.0.0.0 listenport=2299 connectaddress=$ip connectport=2299"
    );
    assert_eq!(script.matches("& $netsh").count(), 2, "show and set only");
    // The address must pass the dotted-quad match before it can reach netsh, and the set only runs after it.
    let validate = script.find("-match $quad").expect("validates the address");
    let set = script.find("portproxy set").unwrap();
    assert!(validate < set);
    // Private ranges only, inside the adapter's subnet, never the adapter's own address.
    assert!(script.contains("$private = '^(10\\.|172\\.(1[6-9]|2[0-9]|3[01])\\.|192\\.168\\.)'"));
    assert!(script.contains("vEthernet (WSL*"));
    let subnet = script.find("-band $mask").expect("checks the subnet");
    assert!(subnet < set && script.contains("$token -ne $gateway"));
    assert!(script.contains("no address inside the WSL adapter subnet from the distro"));
    // System tools by absolute path, never a search path; no dynamic evaluation.
    assert!(script.contains("$system = [Environment]::SystemDirectory"));
    assert!(
        !script.contains("SystemRoot") && !script.contains("$env:Path"),
        "no user-overridable environment variable picks the tools"
    );
    assert!(script.contains("$env:PSModulePath = Join-Path $system"));
    assert!(
        !script.contains("Get-NetIPAddress"),
        "no module autoload in an elevated task"
    );
    assert!(script.contains("Join-Path $system 'netsh.exe'"));
    assert!(script.contains("Join-Path $system 'wsl.exe'"));
    assert!(!script.to_ascii_lowercase().contains("invoke-expression"));
    assert!(!script.contains("iex "));
    // It changes nothing when the relay already points at the address.
    assert!(script.contains("if ($current -eq $ip) { exit 0 }"));
    // Parameters are literals taken from validated settings.
    assert!(refresh_script("p", "Ubuntu-22.04", 22).contains("$distro = 'Ubuntu-22.04'"));
}

// frob:tests crates/goway-setup/src/relay.rs::parse_wsl_ip
#[test]
fn only_a_real_dotted_quad_is_taken_from_hostname_output() {
    let ip = |s: &str| parse_wsl_ip(s).map(|a| a.to_string());
    assert_eq!(ip("172.20.1.5 \n").as_deref(), Some("172.20.1.5"));
    assert_eq!(
        ip("fe80::1 172.20.1.5 172.17.0.1").as_deref(),
        Some("172.20.1.5"),
        "first IPv4, IPv6 skipped"
    );
    for bad in [
        "",
        "   ",
        "fe80::215:5dff:fe00:1",
        "127.0.0.1",
        "0.0.0.0",
        "169.254.3.4",
        "256.1.1.1",
        "172.20.1",
        "172.20.1.5.6",
        "172.20.1.5;calc",
        "$(calc)",
        "172.020.1.5",
    ] {
        assert_eq!(ip(bad), None, "{bad:?}");
    }
}

// frob:tests crates/goway-setup/src/relay.rs::parse_portproxy_table
// frob:tests crates/goway-setup/src/relay.rs::rule_on_port
// frob:tests crates/goway-setup/src/relay.rs::parse_relay_name
// frob:tests crates/goway-setup/src/relay.rs::relay_name
#[test]
fn the_portproxy_table_and_relay_names_parse() {
    let table = "\nListen on ipv4:             Connect to ipv4:\n\nAddress         Port        Address         Port\n--------------- ----------  --------------- ----------\n0.0.0.0         2299        172.20.1.5      2299\n127.0.0.1       8080        192.0.2.7       80\n";
    let rules = parse_portproxy_table(table);
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0].listen_port, 2299);
    assert_eq!(rules[0].connect_address.to_string(), "172.20.1.5");
    assert_eq!(
        rule_on_port(&rules, 8080)
            .unwrap()
            .listen_address
            .to_string(),
        "127.0.0.1"
    );
    assert!(rule_on_port(&rules, 2222).is_none());
    assert!(parse_portproxy_table("").is_empty());
    assert_eq!(relay_name(2299), "0.0.0.0:2299");
    assert_eq!(
        parse_relay_name("0.0.0.0:2299").map(|(a, p)| (a.to_string(), p)),
        Some(("0.0.0.0".into(), 2299))
    );
    for bad in ["", "2299", "0.0.0.*:2299", "0.0.0.0:x", "host:22"] {
        assert!(parse_relay_name(bad).is_none(), "{bad:?}");
    }
}

fn rule(listen: &str, port: u16) -> PortProxyRule {
    PortProxyRule {
        listen_address: listen.parse().unwrap(),
        listen_port: port,
        connect_address: "172.20.1.5".parse().unwrap(),
        connect_port: port,
    }
}

// frob:tests crates/goway-setup/src/host.rs::check_relay_port
#[test]
fn a_foreign_portproxy_rule_on_the_port_refuses_a_nat_install_and_explains() {
    let existing = [rule("0.0.0.0", PORT)];
    let err = host::check_relay_port(NetworkMode::Nat, PORT, &existing).unwrap_err();
    assert!(matches!(err, SetupError::RelayPortBusy { port: PORT, .. }));
    let text = err.to_string();
    assert!(text.contains("never changes rules it did not create") && text.contains("--port"));
    // Any listen address on that port conflicts; other ports and mirrored mode do not.
    assert!(host::check_relay_port(NetworkMode::Nat, PORT, &[rule("127.0.0.1", PORT)]).is_err());
    assert!(host::check_relay_port(NetworkMode::Nat, PORT, &[rule("0.0.0.0", 2222)]).is_ok());
    assert!(host::check_relay_port(NetworkMode::Mirrored, PORT, &existing).is_ok());
    assert!(host::check_relay_port(NetworkMode::Nat, PORT, &[]).is_ok());
}

fn installed_nat() -> (Journal, ModelSystem) {
    let mut sys = machine();
    let journal = apply(&nat_plan(PORT, Keepalive::Logon), &mut sys).unwrap();
    (journal, sys)
}

fn validate(journal: &Journal, network: NetworkMode) -> Result<(), SetupError> {
    host::validate_journal(journal, &layout(), &settings(network), Path::new(HOME))
}

fn with_entry(change: Change) -> Journal {
    let mut j = Journal::new("t");
    j.entries.push(goway_journal::Entry {
        change,
        prior: Prior::Noop,
        reverted: false,
    });
    j
}

// frob:tests crates/goway-setup/src/host.rs::validate_journal
// frob:tests crates/goway-setup/src/host.rs::expected_changes
#[test]
fn the_validator_accepts_exactly_the_nat_plans_entries() {
    let (journal, _) = installed_nat();
    validate(&journal, NetworkMode::Nat).expect("the genuine nat journal is accepted");
    for keepalive in [Keepalive::Logon, Keepalive::Boot] {
        for c in nat_plan(PORT, keepalive) {
            validate(&with_entry(c.clone()), NetworkMode::Nat)
                .unwrap_or_else(|e| panic!("{c:?}: {e}"));
        }
    }
    // The recorded mode is the whole story: nat entries under mirrored settings, and the
    // mirrored plan's networkingMode under nat settings, are both refused.
    assert!(validate(&journal, NetworkMode::Mirrored).is_err());
    let mirrored = host_plan(
        &layout(),
        &params(PORT, Keepalive::Logon, NetworkMode::Mirrored),
        &HostFacts::assumed(),
    );
    let mirrored_journal = apply(&mirrored, &mut machine()).unwrap();
    validate(&mirrored_journal, NetworkMode::Mirrored).unwrap();
    assert!(validate(&mirrored_journal, NetworkMode::Nat).is_err());
}

fn proxy(name: &str, spec: &PortProxySpec) -> Change {
    Change::EnsureResource {
        kind: ResourceKind::PortProxy,
        name: name.into(),
        spec: serde_json::to_string(spec).unwrap(),
    }
}

fn spec(listen: &str, port: u16) -> PortProxySpec {
    PortProxySpec {
        listen_address: listen.into(),
        port,
    }
}

// frob:tests crates/goway-setup/src/host.rs::validate_journal
// frob:tests crates/goway-setup/src/host.rs::has_wildcard
#[test]
#[allow(clippy::too_many_lines)] // one table of variants reads better than splitting
fn the_validator_refuses_variants_of_the_relay_entries() {
    let l = layout();
    let nat = nat_plan(PORT, Keepalive::Logon);
    let script = nat
        .iter()
        .find_map(|c| match c {
            Change::WriteFile { path, contents } if path.ends_with(relay::SCRIPT_NAME) => {
                Some(contents.clone())
            }
            _ => None,
        })
        .unwrap();
    let task_spec = nat
        .iter()
        .find_map(|c| match c {
            Change::EnsureResource { name, spec, .. } if name.contains("WSL Relay") => {
                Some(serde_json::from_str::<RelayTaskSpec>(spec).unwrap())
            }
            _ => None,
        })
        .unwrap();
    let task = |name: &str, s: &RelayTaskSpec| Change::EnsureResource {
        kind: ResourceKind::ScheduledTask,
        name: name.into(),
        spec: serde_json::to_string(s).unwrap(),
    };
    let write = |path: PathBuf, contents: &str| Change::WriteFile {
        path,
        contents: contents.into(),
    };
    let variants: Vec<(&str, Change)> = vec![
        (
            "another port",
            proxy("0.0.0.0:2222", &spec("0.0.0.0", 2222)),
        ),
        (
            "another listen address",
            proxy("127.0.0.1:2299", &spec("127.0.0.1", PORT)),
        ),
        (
            "spec disagrees with name",
            proxy("0.0.0.0:2299", &spec("0.0.0.0", 2222)),
        ),
        (
            "wildcard port name",
            proxy("0.0.0.0:*", &spec("0.0.0.0", PORT)),
        ),
        (
            "wildcard address name",
            proxy("*:2299", &spec("0.0.0.0", PORT)),
        ),
        (
            "script outside the admin dir",
            write(
                PathBuf::from("/ProgramData/goway/other/relay-refresh.ps1"),
                &script,
            ),
        ),
        (
            "script in the user's profile",
            write(PathBuf::from("C:\\Users\\u\\relay-refresh.ps1"), &script),
        ),
        (
            "another file name in the admin dir",
            write(l.admin_dir.join("evil.ps1"), &script),
        ),
        (
            "tampered script",
            write(host::relay_script_path(&l), &format!("{script}calc.exe\n")),
        ),
        (
            "task pointing at another script",
            task(
                "goway-test WSL Relay",
                &RelayTaskSpec {
                    script: "C:\\Users\\u\\evil.ps1".into(),
                    ..task_spec.clone()
                },
            ),
        ),
        (
            "task with another interval",
            task(
                "goway-test WSL Relay",
                &RelayTaskSpec {
                    interval_minutes: 1,
                    ..task_spec.clone()
                },
            ),
        ),
        (
            "wildcard task name",
            task("goway-test WSL Relay*", &task_spec),
        ),
        ("other profile's task", task("other WSL Relay", &task_spec)),
        (
            "relay for another distro",
            task(
                "goway-test WSL Relay",
                &RelayTaskSpec {
                    distro: "Debian".into(),
                    ..task_spec.clone()
                },
            ),
        ),
    ];
    for (what, change) in variants {
        let result = validate(&with_entry(change), NetworkMode::Nat);
        assert!(
            matches!(result, Err(SetupError::UntrustedState { .. })),
            "{what} must be refused: {result:?}"
        );
    }
    assert!(has_wild("0.0.0.*:2299"));
}

fn has_wild(name: &str) -> bool {
    host::has_wildcard(name)
}

#[derive(Debug, Clone)]
struct Scenario {
    keepalive: Keepalive,
    port: u16,
    /// 0 none, 1 a rule on another port, 2 on the same port, 3 on the same port elsewhere.
    foreign: u8,
    existing: [bool; 8],
}

fn scenario() -> impl Strategy<Value = Scenario> {
    (
        prop_oneof![Just(Keepalive::Logon), Just(Keepalive::Boot)],
        proptest::sample::select(vec![2222u16, 2299]),
        0u8..4,
        proptest::array::uniform8(any::<bool>()),
    )
        .prop_map(|(keepalive, port, foreign, existing)| Scenario {
            keepalive,
            port,
            foreign,
            existing,
        })
}

// frob:tests crates/goway-setup/src/host.rs::host_plan
// frob:tests crates/goway-setup/src/host.rs::check_relay_port
#[test]
fn installing_then_uninstalling_the_nat_plan_restores_any_machine_and_foreign_relays_survive() {
    let mut runner = TestRunner::new(Config::with_cases(400));
    runner
        .run(&scenario(), |s| {
            let plan = nat_plan(s.port, s.keepalive);
            let mut initial = machine();
            for (i, c) in plan.iter().enumerate() {
                if let Change::EnsureResource { kind, name, spec } = c
                    && *kind != ResourceKind::PortProxy
                    && s.existing[i % 8]
                {
                    initial
                        .resources
                        .insert((*kind, name.clone()), spec.clone());
                }
            }
            let foreign: Option<(String, u16)> = match s.foreign {
                1 => Some(("0.0.0.0".into(), if s.port == 2222 { 2299 } else { 2222 })),
                2 => Some(("0.0.0.0".into(), s.port)),
                3 => Some(("127.0.0.1".into(), s.port)),
                _ => None,
            };
            if let Some((addr, port)) = &foreign {
                initial.resources.insert(
                    (ResourceKind::PortProxy, format!("{addr}:{port}")),
                    serde_json::to_string(&spec(addr, *port)).unwrap(),
                );
            }
            // The facts the install would have probed from that machine.
            let rules: Vec<PortProxyRule> = initial
                .resources
                .keys()
                .filter(|(k, _)| *k == ResourceKind::PortProxy)
                .filter_map(|(_, n)| parse_relay_name(n))
                .map(|(a, p)| rule(&a.to_string(), p))
                .collect();
            let refused = host::check_relay_port(NetworkMode::Nat, s.port, &rules).is_err();
            prop_assert_eq!(
                refused,
                matches!(s.foreign, 2 | 3),
                "the same-port rule is refused"
            );

            // Applying anyway (a refused install never gets here) must still never touch it.
            let mut sys = initial.clone();
            let mut journal =
                apply(&plan, &mut sys).map_err(|e| TestCaseError::fail(e.to_string()))?;
            if let Some((addr, port)) = &foreign {
                let name = format!("{addr}:{port}");
                let kept = sys.resource_exists(ResourceKind::PortProxy, &name).unwrap();
                prop_assert!(kept);
            }
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

/// Answers each command from a script of `(substring of the command line, exit code, stdout)`.
struct Fake {
    script: Vec<(&'static str, i32, &'static str)>,
    log: std::cell::RefCell<Vec<Invocation>>,
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

// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.resource_exists
// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.resource_create
// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.resource_delete
// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.wsl_ip
// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.portproxy_rules
// frob:tests crates/goway-setup/src/relay.rs::netsh_delete_args
#[test]
fn the_portproxy_resource_runs_netsh_with_the_distros_current_address() {
    let table = "Listen on ipv4:             Connect to ipv4:\n\nAddress         Port        Address         Port\n--------------- ----------  --------------- ----------\n0.0.0.0         2299        172.20.1.5      2299\n";
    let fake = Fake {
        script: vec![
            ("hostname -I", 0, "172.20.9.9 \n"),
            ("powershell.exe", 0, "172.20.0.1/20\n"),
            ("show v4tov4", 0, table),
            ("netsh.exe", 0, ""),
        ],
        log: std::cell::RefCell::default(),
    };
    let mut s = HostSystem::with_runner("Ubuntu", &fake);
    assert!(
        s.resource_exists(ResourceKind::PortProxy, "0.0.0.0:2299")
            .unwrap()
    );
    assert!(
        !s.resource_exists(ResourceKind::PortProxy, "0.0.0.0:2222")
            .unwrap()
    );
    assert!(
        !s.resource_exists(ResourceKind::PortProxy, "127.0.0.1:2299")
            .unwrap()
    );
    assert!(s.resource_exists(ResourceKind::PortProxy, "bogus").is_err());
    let json = serde_json::to_string(&spec("0.0.0.0", 2299)).unwrap();
    s.resource_create(ResourceKind::PortProxy, "0.0.0.0:2299", &json)
        .unwrap();
    s.resource_delete(ResourceKind::PortProxy, "0.0.0.0:2299")
        .unwrap();
    assert!(
        s.resource_create(
            ResourceKind::PortProxy,
            "0.0.0.0:2299",
            &serde_json::to_string(&spec("0.0.0.0", 1)).unwrap()
        )
        .is_err(),
        "a spec that disagrees with its name is refused"
    );
    let netsh: Vec<String> = fake
        .log
        .borrow()
        .iter()
        .filter(|i| i.program.ends_with("netsh.exe") && !i.args.contains(&"show".to_owned()))
        .map(|i| i.args.join(" "))
        .collect();
    assert_eq!(
        netsh,
        [
            "interface portproxy add v4tov4 listenaddress=0.0.0.0 listenport=2299 connectaddress=172.20.9.9 connectport=2299",
            "interface portproxy delete v4tov4 listenaddress=0.0.0.0 listenport=2299",
        ]
    );

    let none = Fake {
        script: vec![("hostname -I", 0, "fe80::1\n")],
        log: std::cell::RefCell::default(),
    };
    let mut n = HostSystem::with_runner("Ubuntu", &none);
    assert!(
        n.resource_create(ResourceKind::PortProxy, "0.0.0.0:2299", &json)
            .is_err(),
        "no IPv4 address: nothing is created"
    );
    assert!(
        none.log
            .borrow()
            .iter()
            .all(|i| !i.program.ends_with("netsh.exe"))
    );
}

// frob:tests crates/goway-setup/src/ps.rs::relay_task_create
// frob:tests crates/goway-setup/src/ps.rs::relay_arguments
#[test]
fn the_relay_task_runs_the_script_as_the_user_with_highest_privileges_and_stores_no_password() {
    let l = layout();
    let spec = |keepalive| RelayTaskSpec {
        distro: "Ubuntu".into(),
        keepalive,
        script: host::relay_script_path(&l).display().to_string(),
        interval_minutes: relay::REFRESH_MINUTES,
        description: "d".into(),
    };
    let ps_exe = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe";
    let logon = ps::relay_task_create(
        "goway-test WSL Relay",
        &spec(Keepalive::Logon),
        r"C:\Windows\System32\conhost.exe",
        ps_exe,
    );
    assert!(logon.contains("-LogonType Interactive") && logon.contains("-RunLevel Highest"));
    assert!(logon.contains("-AtLogOn -User $user"));
    assert!(logon.contains("-RepetitionInterval (New-TimeSpan -Minutes 5)"));
    assert!(logon.contains("$trigger.Repetition = $repeat.Repetition"));
    assert!(logon.contains("-Execute 'C:\\Windows\\System32\\conhost.exe'"));
    let script = host::relay_script_path(&l).display().to_string();
    assert!(logon.contains(&format!(
        "--headless \"{ps_exe}\" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"{script}\""
    )));
    assert!(!logon.contains("-Password") && !logon.contains("Password"));
    let boot = ps::relay_task_create(
        "goway-test WSL Relay (boot)",
        &spec(Keepalive::Boot),
        "c",
        ps_exe,
    );
    assert!(
        boot.contains("-LogonType S4U")
            && boot.contains("-AtStartup")
            && boot.contains("-RunLevel Highest")
    );
}

// frob:tests crates/goway-setup/src/relay.rs::choose_wsl_ip
// frob:tests crates/goway-setup/src/relay.rs::parse_adapter_addrs
// frob:tests crates/goway-setup/src/relay.rs::in_adapter_subnet
#[test]
fn the_connect_address_must_be_private_inside_the_wsl_adapter_subnet_and_not_the_gateway() {
    use goway_setup::relay::{choose_wsl_ip, in_adapter_subnet, parse_adapter_addrs};
    let adapters =
        parse_adapter_addrs("172.20.0.1/20\nnot an address\n10.0.0.1/0\n10.0.0.1/33\n  \n");
    assert_eq!(
        adapters.len(),
        1,
        "only well-formed address/prefix lines count"
    );
    let pick = |text: &str| choose_wsl_ip(text, &adapters).map(|a| a.to_string());
    assert_eq!(pick("172.20.1.5\n").as_deref(), Some("172.20.1.5"));
    assert_eq!(
        pick("8.8.8.8 172.20.15.254").as_deref(),
        Some("172.20.15.254")
    );
    for steered in [
        "8.8.8.8",     // public
        "192.168.1.1", // private, but not the adapter's subnet (a LAN router)
        "172.21.0.5",  // private, next /20 over
        "172.20.0.1",  // the adapter's own address
        "127.0.0.1",
        "169.254.1.1",
        "10.0.0.5",
        "0.0.0.0",
        "100.64.0.9",  // CGNAT is not RFC 1918
        "172.20.16.1", // one past the /20
        "fe80::1",
        "172.20.1.5;calc",
    ] {
        assert_eq!(pick(steered), None, "{steered}");
    }
    assert_eq!(
        choose_wsl_ip("172.20.1.5", &[]),
        None,
        "no adapter, no relay"
    );
    let a = adapters[0];
    assert!(in_adapter_subnet("172.20.0.0".parse().unwrap(), a));
    assert!(in_adapter_subnet("172.20.15.255".parse().unwrap(), a));
    assert!(!in_adapter_subnet("172.20.16.0".parse().unwrap(), a));
}

// frob:tests crates/goway-setup/src/hostsys.rs::wsl_ip
#[test]
fn a_distro_that_prints_an_address_the_adapter_cannot_reach_gets_no_relay() {
    let json = serde_json::to_string(&spec("0.0.0.0", 2299)).unwrap();
    for (adapter, distro) in [
        ("172.20.0.1/20\n", "8.8.8.8\n"),
        ("172.20.0.1/20\n", "192.168.1.1\n"),
        ("172.20.0.1/20\n", "172.20.0.1\n"),
        ("", "172.20.1.5\n"),
    ] {
        let fake = Fake {
            script: vec![("hostname -I", 0, distro), ("powershell.exe", 0, adapter)],
            log: std::cell::RefCell::default(),
        };
        let mut s = HostSystem::with_runner("Ubuntu", &fake);
        assert!(
            s.resource_create(ResourceKind::PortProxy, "0.0.0.0:2299", &json)
                .is_err(),
            "{distro:?} with adapter {adapter:?}"
        );
        assert!(
            fake.log
                .borrow()
                .iter()
                .all(|i| !i.program.ends_with("netsh.exe"))
        );
    }
}

// frob:tests crates/goway-setup/src/relay.rs::is_goway_relay
// frob:tests crates/goway-setup/src/hostsys.rs::resource_exists
#[test]
fn only_a_rule_shaped_like_the_relay_is_reported_and_removed_as_goways() {
    let table = |connect: &str, port: u16| {
        format!(
            "Listen on ipv4:             Connect to ipv4:\n\nAddress         Port        Address         Port\n--------------- ----------  --------------- ----------\n0.0.0.0         2299        {connect}      {port}\n"
        )
    };
    let rule = |connect: &str, port: u16| PortProxyRule {
        listen_address: "0.0.0.0".parse().unwrap(),
        listen_port: 2299,
        connect_address: connect.parse().unwrap(),
        connect_port: port,
    };
    assert!(relay::is_goway_relay(&rule("172.20.1.5", 2299), 2299));
    assert!(
        !relay::is_goway_relay(&rule("172.20.1.5", 22), 2299),
        "other port"
    );
    assert!(
        !relay::is_goway_relay(&rule("8.8.8.8", 2299), 2299),
        "public target"
    );
    for (connect, port) in [("8.8.8.8", 2299), ("172.20.1.5", 22)] {
        let t: &'static str = Box::leak(table(connect, port).into_boxed_str());
        let fake = Fake {
            script: vec![("show v4tov4", 0, t), ("netsh.exe", 0, "")],
            log: std::cell::RefCell::default(),
        };
        let mut s = HostSystem::with_runner("Ubuntu", &fake);
        assert!(
            !s.resource_exists(ResourceKind::PortProxy, "0.0.0.0:2299")
                .unwrap()
        );
        assert!(
            s.resource_delete(ResourceKind::PortProxy, "0.0.0.0:2299")
                .is_err()
        );
        assert!(
            fake.log
                .borrow()
                .iter()
                .all(|i| !i.args.contains(&"delete".to_owned())),
            "someone else's rule is never deleted"
        );
    }
}

// frob:tests crates/goway-setup/src/relay.rs::check_script_path
// frob:tests crates/goway-setup/src/hostsys.rs::resource_create
#[test]
fn the_relay_task_only_runs_the_refresh_script_by_absolute_path() {
    let good = if cfg!(windows) {
        r"C:\ProgramData\goway\p\relay-refresh.ps1"
    } else {
        "/ProgramData/goway/p/relay-refresh.ps1"
    };
    assert!(relay::check_script_path(good).is_ok());
    for bad in [
        "relay-refresh.ps1",
        "goway/p/relay-refresh.ps1",
        "/ProgramData/goway/p/evil.ps1",
        "/ProgramData/goway/p/relay-refresh.ps1\" -Command calc \"",
        "/ProgramData/%TEMP%/relay-refresh.ps1",
        "/x/$y/relay-refresh.ps1",
    ] {
        assert!(relay::check_script_path(bad).is_err(), "{bad}");
    }
    let mut s = spec_task("relay-refresh.ps1");
    let fake = Fake {
        script: vec![("powershell.exe", 0, "")],
        log: std::cell::RefCell::default(),
    };
    let mut sys = HostSystem::with_runner("Ubuntu", &fake);
    assert!(
        sys.resource_create(
            ResourceKind::ScheduledTask,
            "t",
            &serde_json::to_string(&s).unwrap()
        )
        .is_err()
    );
    assert!(fake.log.borrow().is_empty(), "nothing is registered");
    s.script = good.to_owned();
    sys.resource_create(
        ResourceKind::ScheduledTask,
        "t",
        &serde_json::to_string(&s).unwrap(),
    )
    .unwrap();
}

fn spec_task(script: &str) -> RelayTaskSpec {
    RelayTaskSpec {
        distro: "Ubuntu".into(),
        keepalive: Keepalive::Logon,
        script: script.into(),
        interval_minutes: relay::REFRESH_MINUTES,
        description: "d".into(),
    }
}

// frob:tests crates/goway-setup/src/host.rs::host_plan
#[test]
fn the_firewall_rule_exists_before_the_relay_listens_and_the_script_lives_in_the_admin_dir() {
    let l = layout();
    let plan = host_plan(
        &l,
        &params(PORT, Keepalive::Logon, NetworkMode::Nat),
        &HostFacts::assumed(),
    );
    let index = |want: &dyn Fn(&Change) -> bool| plan.iter().position(want).unwrap();
    let firewall = index(&|c| {
        matches!(
            c,
            Change::EnsureResource {
                kind: ResourceKind::FirewallRule,
                ..
            }
        )
    });
    let proxy = index(&|c| {
        matches!(
            c,
            Change::EnsureResource {
                kind: ResourceKind::PortProxy,
                ..
            }
        )
    });
    assert!(
        firewall < proxy,
        "no window where the relay listens unscoped"
    );
    let script = plan
        .iter()
        .find_map(|c| match c {
            Change::WriteFile { path, .. } if path.ends_with(relay::SCRIPT_NAME) => {
                Some(path.clone())
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(
        script.parent().unwrap(),
        l.admin_dir,
        "only administrators can edit the script"
    );
}
