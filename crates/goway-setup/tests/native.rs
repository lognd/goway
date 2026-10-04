//! The native host component (`install --host --native`): the plan, its exact reversal on a
//! model machine, the journal check an elevated uninstall applies, and the real `System`
//! driven through a scripted fake instead of PowerShell.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use clap::Parser;
use goway_journal::{
    Change, Entry, ModelSystem, Outcome, Prior, RegValue, ResourceKind, System, apply, revert,
};
use goway_setup::cli::{Cli, run};
use goway_setup::helper::{NativeInfo, login_name, native_next_steps};
use goway_setup::host::{self, HostSettings};
use goway_setup::hostsys::{
    HostSystem, Invocation, Output, Runner, normalize_sddl, parse_native_probe,
};
use goway_setup::layout::Layout;
use goway_setup::native::{
    self, ADMIN_KEYS_SDDL, BUILTIN_RULE, BuiltinRule, CAPABILITY, KeyAccount, NativeFacts,
    NativeParams, NativeSettings, ServiceSpec, StartType, native_plan,
};
use goway_setup::render::{ColorWhen, Renderer, describe};
use goway_setup::sysapi::{Tool, tool_path};

/// A throwaway ed25519 public key made for these tests, and its `ssh-keygen -lf` fingerprint.
const KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIKoa4Uf5D+guuHaneQElqF6SlsJHvCOVrYm+DXnN1BYV goway-test";
const FINGERPRINT: &str = "SHA256:M4nfdlcSUN4pWbC86sXl1G+y6dycgM+01/Zm6+B7yAQ";
const OTHER_KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOtherKeyOtherKeyOtherKeyOtherKeyOtherK someone-else";

fn layout(profile: &str) -> Layout {
    Layout::new(Path::new("/Local"), Path::new("/ProgramData"), profile).unwrap()
}

fn account(admin: bool) -> KeyAccount {
    KeyAccount {
        name: r"HELIOS\user".to_owned(),
        sid: "S-1-5-21-1-2-3-1001".to_owned(),
        admin,
        profile_dir: PathBuf::from("/Users/user"),
    }
}

fn params(admin: bool, key: bool) -> NativeParams {
    NativeParams {
        allow_from: Vec::new(),
        authorized_key: key.then(|| KEY.to_owned()),
        account: account(admin),
        shell: tool_path(Tool::PowerShell),
    }
}

fn resources(plan: &[Change], kind: ResourceKind) -> Vec<String> {
    plan.iter()
        .filter_map(|c| match c {
            Change::EnsureResource { kind: k, name, .. } if *k == kind => Some(name.clone()),
            _ => None,
        })
        .collect()
}

fn settings(admin: bool, key: bool) -> HostSettings {
    HostSettings {
        native: Some(NativeSettings {
            authorized_key: key.then(|| KEY.to_owned()),
            account: account(admin),
        }),
        port: native::NATIVE_PORT,
        ..HostSettings::default()
    }
}

// frob:tests crates/goway-setup/src/native.rs::native_plan
#[test]
fn a_fresh_machine_gets_the_capability_a_narrowed_rule_the_shell_the_key_and_the_service() {
    let l = layout("goway");
    let plan = native_plan(&l, &params(true, true), &NativeFacts::assumed());
    assert_eq!(
        resources(&plan, ResourceKind::WindowsCapability),
        [CAPABILITY]
    );
    assert_eq!(
        resources(&plan, ResourceKind::FirewallScope),
        [BUILTIN_RULE]
    );
    assert_eq!(
        resources(&plan, ResourceKind::FirewallRule),
        ["OpenSSH SSH 22"]
    );
    assert_eq!(
        resources(&plan, ResourceKind::Service),
        ["sshd:Manual:stopped"]
    );
    // The rule is limited to local networks (Private and Domain, the local subnet).
    let rule = plan
        .iter()
        .find_map(|c| match c {
            Change::EnsureResource {
                kind: ResourceKind::FirewallRule,
                spec,
                ..
            } => Some(spec.clone()),
            _ => None,
        })
        .unwrap();
    assert!(rule.contains(r#""port":22"#), "{rule}");
    assert!(rule.contains("Private,Domain"), "{rule}");
    assert!(rule.contains("LocalSubnet"), "{rule}");
    // PowerShell becomes the login shell.
    assert!(plan.iter().any(|c| matches!(c,
        Change::SetRegistryValue { key, name, value: RegValue::String(v) }
        if key == native::OPENSSH_KEY && name == "DefaultShell" && *v == tool_path(Tool::PowerShell))));
    // The service is last, so sshd starts with everything else in place.
    assert!(matches!(
        plan.last(),
        Some(Change::EnsureResource {
            kind: ResourceKind::Service,
            ..
        })
    ));
    // An administrator's key goes to administrators_authorized_keys with SYSTEM and Administrators only.
    let file = Path::new("/ProgramData")
        .join("ssh")
        .join("administrators_authorized_keys");
    assert!(plan.iter().any(|c| matches!(c,
        Change::EnsureLine { path, line, marker } if *path == file && line == KEY && marker == "# goway-setup goway")));
    assert!(plan.iter().any(|c| matches!(c,
        Change::SetAcl { path, sddl } if *path == file && sddl == ADMIN_KEYS_SDDL)));
    assert_eq!(ADMIN_KEYS_SDDL, "D:P(A;;FA;;;SY)(A;;FA;;;BA)");
}

// frob:tests crates/goway-setup/src/native.rs::native_plan
// frob:tests crates/goway-setup/src/native.rs::KeyAccount.keys_file
// frob:tests crates/goway-setup/src/native.rs::KeyAccount.keys_sddl
#[test]
fn a_standard_account_gets_its_own_authorized_keys_with_a_user_only_acl() {
    let l = layout("goway");
    let plan = native_plan(&l, &params(false, true), &NativeFacts::assumed());
    let file = Path::new("/Users/user")
        .join(".ssh")
        .join("authorized_keys");
    assert!(
        plan.iter()
            .any(|c| matches!(c, Change::EnsureLine { path, .. } if *path == file))
    );
    let acl = plan
        .iter()
        .find_map(|c| match c {
            Change::SetAcl { path, sddl } if *path == file => Some(sddl.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(acl, "D:P(A;;FA;;;S-1-5-21-1-2-3-1001)(A;;FA;;;SY)");
    // Nothing is written where administrators' keys live.
    assert!(!plan.iter().any(|c| matches!(c,
        Change::EnsureLine { path, .. } if path.ends_with("administrators_authorized_keys"))));
}

// frob:tests crates/goway-setup/src/native.rs::native_plan
#[test]
fn without_a_key_no_key_file_is_touched() {
    let plan = native_plan(
        &layout("goway"),
        &params(true, false),
        &NativeFacts::assumed(),
    );
    assert!(
        !plan
            .iter()
            .any(|c| matches!(c, Change::EnsureLine { .. } | Change::SetAcl { .. }))
    );
}

// frob:tests crates/goway-setup/src/native.rs::native_plan
#[test]
fn an_existing_capability_leaves_its_firewall_rule_alone() {
    let facts = NativeFacts {
        capability_installed: true,
        service: Some((StartType::Automatic, true)),
        default_shell: Some(r"C:\Windows\System32\cmd.exe".to_owned()),
        builtin_rule: BuiltinRule::Open,
    };
    let plan = native_plan(&layout("goway"), &params(true, false), &facts);
    assert!(resources(&plan, ResourceKind::FirewallScope).is_empty());
    assert_eq!(
        resources(&plan, ResourceKind::Service),
        ["sshd:Automatic:running"]
    );
    assert!(
        native::open_rule_warning(&facts)
            .unwrap()
            .contains(BUILTIN_RULE)
    );
    assert!(
        native::shell_replaced_notice(&facts, &tool_path(Tool::PowerShell))
            .unwrap()
            .contains("cmd.exe")
    );
    let scoped = NativeFacts {
        builtin_rule: BuiltinRule::Scoped,
        ..facts
    };
    assert!(native::open_rule_warning(&scoped).is_none());
}

/// A machine as a user who already runs OpenSSH might have it.
fn model_with_user_setup() -> ModelSystem {
    let mut m = ModelSystem::new();
    let ssh = Path::new("/ProgramData").join("ssh");
    m.dirs.insert(PathBuf::from("/ProgramData"));
    m.dirs.insert(ssh.clone());
    m.files.insert(
        ssh.join("administrators_authorized_keys"),
        format!("{OTHER_KEY}\n"),
    );
    m.acls.insert(
        ssh.join("administrators_authorized_keys"),
        "D:P(A;;FA;;;BA)".to_owned(),
    );
    m.reg_keys.insert(native::OPENSSH_KEY.to_owned());
    m.registry.insert(
        (native::OPENSSH_KEY.to_owned(), "DefaultShell".to_owned()),
        RegValue::String(r"C:\Windows\System32\cmd.exe".to_owned()),
    );
    m
}

// frob:tests crates/goway-setup/src/native.rs::native_plan
#[test]
fn uninstall_restores_a_fresh_model_machine_exactly() {
    let l = layout("goway");
    let plan = native_plan(&l, &params(true, true), &NativeFacts::assumed());
    let mut m = ModelSystem::new();
    m.dirs.insert(PathBuf::from("/ProgramData"));
    let before = m.clone();
    let mut journal = apply(&plan, &mut m).unwrap();
    assert_ne!(m, before);
    let report = revert(&mut journal, &mut m).unwrap();
    assert!(
        report
            .outcomes
            .iter()
            .all(|(_, o)| matches!(o, Outcome::Restored | Outcome::Noop))
    );
    assert_eq!(m, before);
}

// frob:tests crates/goway-setup/src/native.rs::native_plan
#[test]
fn uninstall_keeps_what_the_user_already_had_and_restores_what_install_replaced() {
    let l = layout("goway");
    let facts = NativeFacts {
        capability_installed: true,
        service: Some((StartType::Automatic, true)),
        default_shell: Some(r"C:\Windows\System32\cmd.exe".to_owned()),
        builtin_rule: BuiltinRule::Open,
    };
    let mut m = model_with_user_setup();
    // The user's own capability and running sshd exist before the install.
    m.resources.insert(
        (ResourceKind::WindowsCapability, CAPABILITY.to_owned()),
        String::new(),
    );
    m.resources.insert(
        (ResourceKind::Service, "sshd:Automatic:running".to_owned()),
        String::new(),
    );
    let before = m.clone();
    let plan = native_plan(&l, &params(true, true), &facts);
    let mut journal = apply(&plan, &mut m).unwrap();
    // Our key line is appended to the user's file, their shell is replaced.
    let keys = m
        .files
        .get(
            &Path::new("/ProgramData")
                .join("ssh")
                .join("administrators_authorized_keys"),
        )
        .unwrap();
    assert!(keys.contains(OTHER_KEY) && keys.contains(KEY), "{keys}");
    assert_eq!(
        m.registry[&(native::OPENSSH_KEY.to_owned(), "DefaultShell".to_owned())],
        RegValue::String(tool_path(Tool::PowerShell))
    );
    // Capability and service were already there: recorded as no-ops, so never removed.
    let noops: Vec<_> = journal
        .entries
        .iter()
        .filter(|e| {
            matches!(
                &e.change,
                Change::EnsureResource {
                    kind: ResourceKind::WindowsCapability | ResourceKind::Service,
                    ..
                }
            )
        })
        .map(|e| e.prior.clone())
        .collect();
    assert!(noops.iter().all(|p| *p == Prior::Noop), "{noops:?}");
    revert(&mut journal, &mut m).unwrap();
    assert_eq!(m, before);
}

// frob:tests crates/goway-setup/src/native.rs::ServiceSpec.resource_name
// frob:tests crates/goway-setup/src/native.rs::ServiceSpec.from_resource_name
#[test]
fn the_service_resource_name_carries_what_removal_restores() {
    for restore in [StartType::Automatic, StartType::Manual, StartType::Disabled] {
        for was_running in [true, false] {
            let spec = ServiceSpec {
                restore,
                was_running,
            };
            assert_eq!(
                ServiceSpec::from_resource_name(&spec.resource_name()),
                Some(spec)
            );
        }
    }
    for bad in [
        "sshd",
        "other:Manual:stopped",
        "sshd:Fast:stopped",
        "sshd:Manual:stopped:x",
        "sshd:Manual:paused",
    ] {
        assert_eq!(ServiceSpec::from_resource_name(bad), None, "{bad}");
    }
}

// frob:tests crates/goway-setup/src/native.rs::validate_public_key
// frob:tests crates/goway-setup/src/native.rs::base64_decode
// frob:tests crates/goway-setup/src/native.rs::fingerprint
// frob:tests crates/goway-setup/src/native.rs::key_from_argument
#[test]
fn only_one_plain_public_key_line_is_ever_authorized() {
    native::validate_public_key(KEY).unwrap();
    assert_eq!(native::fingerprint(KEY).as_deref(), Some(FINGERPRINT));
    for bad in [
        "",
        "ssh-ed25519",
        "ssh-ed25519 !!!notbase64",
        "ssh-rsa AAAAC3NzaC1lZDI1NTE5AAAAIKoa4Uf5D+guuHaneQElqF6SlsJHvCOVrYm+DXnN1BYV",
        r#"command="calc" ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIKoa4Uf5D+guuHaneQElqF6SlsJHvCOVrYm+DXnN1BYV"#,
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIKoa4Uf5D+guuHaneQElqF6SlsJHvCOVrYm+DXnN1BYV x\nssh-ed25519 AAAA",
        "-----BEGIN OPENSSH PRIVATE KEY-----",
    ] {
        assert!(native::validate_public_key(bad).is_err(), "{bad:?}");
    }
    // A key given as a line is used as is; anything else is read as a file's text.
    let line =
        native::key_from_argument(KEY, |_| unreachable!("a line is not read as a path")).unwrap();
    assert_eq!(line, KEY);
    let from_file = native::key_from_argument("some.pub", |p| {
        assert_eq!(p, Path::new("some.pub"));
        Some(format!("\n{KEY}\n"))
    })
    .unwrap();
    assert_eq!(from_file, KEY);
    assert!(native::key_from_argument("missing.pub", |_| None).is_err());
}

// frob:tests crates/goway-setup/src/host.rs::validate_journal
// frob:tests crates/goway-setup/src/host.rs::expected_changes
// frob:tests crates/goway-setup/src/native.rs::expected_changes
#[test]
fn the_elevated_uninstall_accepts_what_the_native_install_wrote_and_nothing_else() {
    let l = layout("goway");
    let s = settings(true, true);
    let home = Path::new("/home/u");
    for facts in [
        NativeFacts::assumed(),
        NativeFacts {
            capability_installed: true,
            service: Some((StartType::Disabled, false)),
            default_shell: None,
            builtin_rule: BuiltinRule::Scoped,
        },
    ] {
        let plan = native_plan(&l, &params(true, true), &facts);
        let mut m = ModelSystem::new();
        m.dirs.insert(PathBuf::from("/ProgramData"));
        let journal = apply(&plan, &mut m).unwrap();
        host::validate_journal(&journal, &l, &s, home).unwrap();
    }
    // A planted entry outside the plan is refused: a different file, another service, a capability.
    let plan = native_plan(&l, &params(true, true), &NativeFacts::assumed());
    let mut m = ModelSystem::new();
    m.dirs.insert(PathBuf::from("/ProgramData"));
    let good = apply(&plan, &mut m).unwrap();
    for evil in [
        Change::EnsureLine {
            path: PathBuf::from("/ProgramData/ssh/sshd_config"),
            line: KEY.to_owned(),
            marker: "# goway-setup goway".to_owned(),
        },
        Change::EnsureResource {
            kind: ResourceKind::Service,
            name: "spooler:Manual:stopped".to_owned(),
            spec: String::new(),
        },
        Change::EnsureResource {
            kind: ResourceKind::WindowsCapability,
            name: "Browser.InternetExplorer~~~~0.0.11.0".to_owned(),
            spec: String::new(),
        },
        Change::SetAcl {
            path: PathBuf::from("/ProgramData/ssh/sshd_config"),
            sddl: ADMIN_KEYS_SDDL.to_owned(),
        },
    ] {
        let mut bad = good.clone();
        bad.entries.push(Entry {
            change: evil.clone(),
            prior: Prior::ResourceCreated,
            reverted: false,
        });
        assert!(
            host::validate_journal(&bad, &l, &s, home).is_err(),
            "{evil:?} must be refused"
        );
    }
    // The settings an elevated process rebuilds the plan from are checked too.
    let mut hostile = settings(true, true);
    hostile.native.as_mut().unwrap().authorized_key =
        Some(r#"command="x" ssh-ed25519 AAAA"#.into());
    assert!(hostile.validate(Path::new("s.json")).is_err());
    let mut hostile = settings(true, true);
    hostile.native.as_mut().unwrap().account.sid = "S-1-5; calc".into();
    assert!(hostile.validate(Path::new("s.json")).is_err());
    settings(true, true).validate(Path::new("s.json")).unwrap();
    // Native settings round-trip, and a WSL journal's settings (no native field) still load.
    let json = serde_json::to_string(&settings(false, true)).unwrap();
    assert_eq!(
        serde_json::from_str::<HostSettings>(&json).unwrap(),
        settings(false, true)
    );
    let legacy = r#"{"distro":"Ubuntu","port":2222}"#;
    assert!(
        serde_json::from_str::<HostSettings>(legacy)
            .unwrap()
            .native
            .is_none()
    );
}

// frob:tests crates/goway-setup/src/hostsys.rs::parse_native_probe
// frob:tests crates/goway-setup/src/hostsys.rs::normalize_sddl
#[test]
fn probe_output_and_acl_text_are_parsed_exactly() {
    let text = "account=HELIOS\\user\nsid=S-1-5-21-1-2-3-1001\nadmin=1\nprofile=C:\\Users\\user\ncapability=1\nservice_start=Auto\nservice_running=0\ndefault_shell=C:\\Windows\\System32\\cmd.exe\nbuiltin_rule=open\n";
    let (facts, acct) = parse_native_probe(text).unwrap();
    assert_eq!(
        facts,
        NativeFacts {
            capability_installed: true,
            service: Some((StartType::Automatic, false)),
            default_shell: Some(r"C:\Windows\System32\cmd.exe".to_owned()),
            builtin_rule: BuiltinRule::Open,
        }
    );
    assert!(acct.admin);
    assert_eq!(acct.name, r"HELIOS\user");
    // Nothing of sshd present.
    let (facts, _) = parse_native_probe(
        "account=A\\b\nsid=S-1-5-21-9\nadmin=0\nprofile=C:\\Users\\b\ncapability=0\nbuiltin_rule=absent\n",
    )
    .unwrap();
    assert_eq!(facts.service, None);
    assert_eq!(facts.builtin_rule, BuiltinRule::Absent);
    // Missing or hostile values are refused.
    assert!(parse_native_probe("capability=1\n").is_err());
    assert!(
        parse_native_probe(
            "account=A\\b\nsid=S-1-5; calc\nadmin=1\nprofile=C:\\Users\\b\ncapability=1\n"
        )
        .is_err()
    );
    // Windows adds auto-inheritance flags when it reads a DACL back.
    assert_eq!(
        normalize_sddl("D:PAI(A;;FA;;;SY)(A;;FA;;;BA)"),
        ADMIN_KEYS_SDDL
    );
    assert_eq!(normalize_sddl("D:PARAI(A;;FA;;;BA)"), "D:P(A;;FA;;;BA)");
    assert_eq!(normalize_sddl("D:(A;;FA;;;BA)"), "D:(A;;FA;;;BA)");
}

/// Answers each command from a script of `(substring of the script text, exit code, stdout)`.
struct Fake {
    script: Vec<(&'static str, i32, &'static str)>,
    scripts: RefCell<Vec<String>>,
}

impl Fake {
    fn new(script: Vec<(&'static str, i32, &'static str)>) -> Self {
        Self {
            script,
            scripts: RefCell::new(Vec::new()),
        }
    }
}

impl Runner for &Fake {
    fn run(&self, inv: &Invocation) -> std::io::Result<Output> {
        assert_eq!(inv.program, tool_path(Tool::PowerShell));
        let bytes = native::base64_decode(&inv.args[3]).unwrap();
        let units: Vec<u16> = bytes
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let text = String::from_utf16(&units).unwrap();
        self.scripts.borrow_mut().push(text.clone());
        let (code, out) = self
            .script
            .iter()
            .find(|(needle, _, _)| text.contains(needle))
            .map_or((127, ""), |(_, c, o)| (*c, *o));
        Ok(Output {
            code: Some(code),
            stdout: out.as_bytes().to_vec(),
            stderr: Vec::new(),
        })
    }
}

fn sys(fake: &Fake) -> HostSystem<&Fake> {
    HostSystem::with_runner("Ubuntu", fake)
}

// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.probe_native
// frob:tests crates/goway-setup/src/ps.rs::native_probe
#[test]
fn the_probe_script_is_read_only_and_names_exactly_the_things_it_reads() {
    let fake = Fake::new(vec![(
        "capability=",
        0,
        "account=H\\u\nsid=S-1-5-21-1\nadmin=1\nprofile=C:\\Users\\u\ncapability=0\nbuiltin_rule=absent\n",
    )]);
    let (facts, acct) = sys(&fake).probe_native().unwrap();
    assert!(!facts.capability_installed && acct.admin);
    let script = fake.scripts.borrow()[0].clone();
    assert!(script.contains("Get-WindowsCapability -Online -Name 'OpenSSH.Server~~~~0.0.1'"));
    assert!(script.contains("Win32_Service -Filter \"Name='sshd'\""));
    assert!(script.contains("OpenSSH-Server-In-TCP"));
    for write in [
        "Set-",
        "Add-",
        "Remove-",
        "New-",
        "Start-",
        "Stop-",
        "Register-",
    ] {
        assert!(
            !script.contains(write),
            "a probe must not use {write}: {script}"
        );
    }
}

// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.resource_create
// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.resource_delete
// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.resource_exists
// frob:tests crates/goway-setup/src/ps.rs::capability_add
// frob:tests crates/goway-setup/src/ps.rs::capability_remove
// frob:tests crates/goway-setup/src/ps.rs::capability_exists
// frob:tests crates/goway-setup/src/ps.rs::sshd_exists
// frob:tests crates/goway-setup/src/ps.rs::sshd_enable
// frob:tests crates/goway-setup/src/ps.rs::sshd_restore
// frob:tests crates/goway-setup/src/ps.rs::firewall_scope_exists
// frob:tests crates/goway-setup/src/ps.rs::firewall_scope_set
// frob:tests crates/goway-setup/src/ps.rs::firewall_scope_restore
#[test]
fn each_resource_runs_exactly_its_scripts_and_refuses_any_other_name() {
    let fake = Fake::new(vec![("-eq 'Installed'", 0, "1\n"), ("", 0, "")]);
    let mut s = sys(&fake);
    assert!(
        s.resource_exists(ResourceKind::WindowsCapability, CAPABILITY)
            .unwrap()
    );
    s.resource_create(ResourceKind::WindowsCapability, CAPABILITY, "")
        .unwrap();
    s.resource_delete(ResourceKind::WindowsCapability, CAPABILITY)
        .unwrap();
    s.resource_create(ResourceKind::Service, "sshd:Manual:stopped", "")
        .unwrap();
    s.resource_delete(ResourceKind::Service, "sshd:Manual:stopped")
        .unwrap();
    s.resource_delete(ResourceKind::Service, "sshd:Automatic:running")
        .unwrap();
    let scope =
        r#"{"profiles":"Private,Domain","remote_addresses":["LocalSubnet","100.64.0.0/10"]}"#;
    s.resource_create(ResourceKind::FirewallScope, BUILTIN_RULE, scope)
        .unwrap();
    s.resource_delete(ResourceKind::FirewallScope, BUILTIN_RULE)
        .unwrap();
    let scripts = fake.scripts.borrow().clone();
    let joined = scripts.join("\n---\n");
    assert!(joined.contains("Add-WindowsCapability -Online -Name 'OpenSSH.Server~~~~0.0.1'"));
    assert!(joined.contains("Remove-WindowsCapability -Online -Name 'OpenSSH.Server~~~~0.0.1'"));
    assert!(
        joined.contains(
            "Set-Service -Name 'sshd' -StartupType Automatic\nStart-Service -Name 'sshd'"
        )
    );
    // Restoring a stopped, manual service stops it and sets Manual; a running automatic one is only set.
    let stopped = scripts
        .iter()
        .find(|t| t.contains("-StartupType Manual"))
        .unwrap();
    assert!(stopped.contains("Stop-Service -Name 'sshd'"), "{stopped}");
    let running = scripts
        .iter()
        .find(|t| t.contains("-StartupType Automatic\n}"))
        .unwrap();
    assert!(!running.contains("Stop-Service"), "{running}");
    assert!(joined.contains("Set-NetFirewallRule -Name 'OpenSSH-Server-In-TCP' -Profile 'Private','Domain' -RemoteAddress 'LocalSubnet','100.64.0.0/10'"));
    assert!(joined.contains("-Profile Any -RemoteAddress Any"));
    // Any other capability, service or rule is refused before a script runs.
    let before = fake.scripts.borrow().len();
    for (kind, name) in [
        (
            ResourceKind::WindowsCapability,
            "Browser.InternetExplorer~~~~0.0.11.0",
        ),
        (ResourceKind::WindowsCapability, "OpenSSH.*"),
        (ResourceKind::Service, "spooler:Manual:stopped"),
        (ResourceKind::Service, "sshd"),
        (ResourceKind::FirewallScope, "*"),
        (ResourceKind::FirewallScope, "RemoteDesktop-UserMode-In-TCP"),
    ] {
        assert!(s.resource_exists(kind, name).is_err(), "{name}");
        assert!(s.resource_create(kind, name, scope).is_err(), "{name}");
        assert!(s.resource_delete(kind, name).is_err(), "{name}");
    }
    assert_eq!(fake.scripts.borrow().len(), before);
}

// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.get_acl
// frob:tests crates/goway-setup/src/hostsys.rs::HostSystem.set_acl
// frob:tests crates/goway-setup/src/ps.rs::acl_get
// frob:tests crates/goway-setup/src/ps.rs::acl_set
#[test]
fn acls_are_read_normalized_and_written_as_a_dacl_only() {
    let fake = Fake::new(vec![
        ("GetAccessControl", 0, "D:PAI(A;;FA;;;SY)(A;;FA;;;BA)\n"),
        ("", 0, ""),
    ]);
    let mut s = sys(&fake);
    let file = Path::new(r"C:\ProgramData\ssh\administrators_authorized_keys");
    assert_eq!(s.get_acl(file).unwrap(), ADMIN_KEYS_SDDL);
    s.set_acl(file, ADMIN_KEYS_SDDL).unwrap();
    let script = fake.scripts.borrow().last().unwrap().clone();
    assert!(
        script.contains("SetSecurityDescriptorSddlForm('D:P(A;;FA;;;SY)(A;;FA;;;BA)', 'Access')"),
        "{script}"
    );
    assert!(
        script.contains("SetAccessControl('C:\\ProgramData\\ssh\\administrators_authorized_keys'"),
        "{script}"
    );
    // A path with a quote cannot break out of the literal.
    s.set_acl(Path::new("C:\\a'b"), ADMIN_KEYS_SDDL).unwrap();
    assert!(fake.scripts.borrow().last().unwrap().contains("'C:\\a''b'"));
}

// frob:tests crates/goway-setup/src/helper.rs::native_next_steps
// frob:tests crates/goway-setup/src/helper.rs::login_name
#[test]
fn the_hand_over_block_names_the_helper_its_fingerprint_and_the_goway_add_line() {
    let info = NativeInfo {
        device_name: "Helios".to_owned(),
        fingerprint: Some(FINGERPRINT.to_owned()),
        account: r"HELIOS\user".to_owned(),
        key_authorized: true,
        key_file: Some(r"C:\ProgramData\ssh\administrators_authorized_keys".to_owned()),
    };
    let text = native_next_steps(&info);
    assert!(
        text.contains(&format!(
            "goway add helios --fingerprint {FINGERPRINT} --user user --port 22"
        )),
        "{text}"
    );
    assert!(text.contains("administrators_authorized_keys"), "{text}");
    assert!(text.contains("local network only"), "{text}");
    assert_eq!(login_name(r"HELIOS\user"), "user");
    assert_eq!(login_name("user"), "user");
    let early = native_next_steps(&NativeInfo {
        fingerprint: None,
        key_authorized: false,
        key_file: None,
        ..info
    });
    assert!(
        early.contains("not available yet") && early.contains("authorized no key"),
        "{early}"
    );
    assert!(!early.contains("goway add"), "{early}");
}

// frob:tests crates/goway-setup/src/cli.rs::run
#[test]
fn a_native_dry_run_lists_the_plan_and_changes_nothing() {
    goway_setup::init_tracing(0);
    let cli = Cli::try_parse_from([
        "goway-setup",
        "--color",
        "never",
        "install",
        "--host",
        "--native",
        "--dry-run",
        "--authorized-key",
        KEY,
        "--profile",
        "native-dry-run-test",
    ])
    .unwrap();
    run(&cli, Renderer::new(ColorWhen::Never)).unwrap();
    let layout = Layout::from_environment("native-dry-run-test").unwrap();
    assert!(!layout.host_journal_path.exists());
    // --native needs --host, and a native install keeps port 22.
    assert!(Cli::try_parse_from(["goway-setup", "install", "--native"]).is_err());
    assert!(
        Cli::try_parse_from(["goway-setup", "install", "--host", "--authorized-key", KEY]).is_err()
    );
    let moved = Cli::try_parse_from([
        "goway-setup",
        "install",
        "--host",
        "--native",
        "--dry-run",
        "--port",
        "2222",
    ])
    .unwrap();
    assert!(run(&moved, Renderer::new(ColorWhen::Never)).is_err());
    // describe() names every native change in words.
    let plan = native_plan(&layout, &params(true, true), &NativeFacts::assumed());
    let words: Vec<String> = plan.iter().map(describe).collect();
    assert!(
        words.iter().any(|w| w.contains("Windows capability")),
        "{words:?}"
    );
    assert!(
        words
            .iter()
            .any(|w| w.contains("scope of the built-in Windows Firewall rule")),
        "{words:?}"
    );
    assert!(words.iter().any(|w| w.contains("service")), "{words:?}");
}
