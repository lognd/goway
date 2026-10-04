//! Doctor for Windows hosts (ssh to a Windows `OpenSSH` server, or the
//! Windows side of this machine through WSL interop).
//!
//! The host answers the same `doctor` verb as any helper (found through
//! [`crate::resolve::resolve_call`], so the transport is the one `run`
//! uses); a second small PowerShell script adds what the verb does not
//! report: whether rustup has an msvc toolchain, whether the Visual C++
//! Build Tools are installed, tools that sit as bare files in the cargo
//! `bin` directory without being on `PATH`, and the free space of the system
//! drive. The checks and the exact PowerShell fix commands are pure
//! functions of those facts.

use std::collections::BTreeMap;

use std::process::Stdio;

use super::{Check, Fix, Level, MIN_FREE, Undo, projneeds, tool};
use crate::resolve::{Found, Prober};
use crate::ssh::{self, KeyPolicy};
use crate::transport::{self, Kind};
use crate::winadmin::{self, WinStep};

/// PowerShell that prints the extra facts, one `key=value` per line. It
/// only reads: nothing is installed, written or started.
const FACTS_PS: &str = r#"
$ErrorActionPreference = 'SilentlyContinue'
$o = New-Object Text.StringBuilder
$ch = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
$bin = Join-Path $ch 'bin'
foreach ($t in 'rustup', 'cargo', 'cargo-nextest') {
  if (Test-Path -LiteralPath (Join-Path $bin "$t.exe")) { [void]$o.Append("cargobin.$t=1`n") }
}
$rustup = Join-Path $bin 'rustup.exe'
if (-not (Test-Path -LiteralPath $rustup)) {
  $c = Get-Command rustup.exe -CommandType Application | Select-Object -First 1
  $rustup = if ($c) { $c.Source } else { '' }
}
$msvc = 'no'
if ($rustup) {
  $inst = (& $rustup target list --installed 2>$null) -join ' '
  if ($inst -match 'windows-msvc') { $msvc = 'yes' }
}
[void]$o.Append("rust_msvc=$msvc`n")
$vw = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$bt = ''
if (Test-Path -LiteralPath $vw) {
  $bt = (& $vw -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath | Select-Object -First 1)
}
[void]$o.Append("build_tools=$bt`n")
$sd = $env:SystemDrive
if ($sd) {
  $d = Get-PSDrive -Name $sd.TrimEnd(':') -PSProvider FileSystem
  if ($d) { [void]$o.Append("system_drive=$sd`nsystem_free=$($d.Free)`n") }
}
[Console]::Out.Write($o.ToString())
"#;

/// The PowerShell expression for the cargo `bin` directory of the user.
pub(super) const CARGO_BIN_PS: &str = "$(if ($env:CARGO_HOME) { Join-Path $env:CARGO_HOME 'bin' } else { Join-Path $env:USERPROFILE '.cargo\\bin' })";

/// The winget package of the Visual Studio Build Tools.
const BUILD_TOOLS_ID: &str = "Microsoft.VisualStudio.2022.BuildTools";

/// What the Build Tools installer is asked for: the C++ workload (compiler,
/// linker, Windows SDK), unattended and without a reboot.
const BUILD_TOOLS_OVERRIDE: &str =
    "--quiet --wait --norestart --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended";

/// A pinned Windows download: its address and sha256.
struct Pinned {
    url: &'static str,
    sha256: &'static str,
}

/// rustup 1.28.2's installer per architecture (sha256 as published by the rustup project).
const RUSTUP_INIT: [(&str, Pinned); 2] = [
    (
        "x86_64",
        Pinned {
            url: "https://static.rust-lang.org/rustup/archive/1.28.2/x86_64-pc-windows-msvc/rustup-init.exe",
            sha256: "88d8258dcf6ae4f7a80c7d1088e1f36fa7025a1cfd1343731b4ee6f385121fc0",
        },
    ),
    (
        "aarch64",
        Pinned {
            url: "https://static.rust-lang.org/rustup/archive/1.28.2/aarch64-pc-windows-msvc/rustup-init.exe",
            sha256: "de9f7d29ccd39efa59a3dda3ec363b396e09b92681229b9b8f6aaa4c84285e9c",
        },
    ),
];

/// cargo-nextest 0.9.146's Windows zip per architecture (the same release goway pins on Linux).
const NEXTEST_ZIP: [(&str, Pinned); 2] = [
    (
        "x86_64",
        Pinned {
            url: "https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-0.9.146/cargo-nextest-0.9.146-x86_64-pc-windows-msvc.zip",
            sha256: "0fa689815c8157e4633225b6b173184b3d546eb6ffb754c3d0e6ea5973284a20",
        },
    ),
    (
        "aarch64",
        Pinned {
            url: "https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-0.9.146/cargo-nextest-0.9.146-aarch64-pc-windows-msvc.zip",
            sha256: "8a071bd8190b287863c8aaa0b20fc56485f9190f0fc02206f48a0b0af4416de4",
        },
    ),
];

/// The Rust toolchain rustup is asked for on `arch` (its msvc host).
fn msvc_toolchain(arch: &str) -> Option<&'static str> {
    match arch {
        "x86_64" => Some("stable-x86_64-pc-windows-msvc"),
        "aarch64" => Some("stable-aarch64-pc-windows-msvc"),
        _ => None,
    }
}

/// PowerShell that downloads `pin` to `$f` inside a fresh temp directory,
/// verifies its sha256 (a mismatch throws before anything runs), runs
/// `then`, and removes the directory again.
fn verified_download(pin: &Pinned, file: &str, then: &str) -> String {
    format!(
        "$ErrorActionPreference = 'Stop'; \
         [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12; \
         $t = Join-Path ([IO.Path]::GetTempPath()) ('goway-' + [guid]::NewGuid().ToString('N')); \
         New-Item -ItemType Directory -Path $t | Out-Null; \
         try {{ \
         $f = Join-Path $t '{file}'; \
         Invoke-WebRequest -UseBasicParsing -Uri '{url}' -OutFile $f; \
         if ((Get-FileHash -Algorithm SHA256 -LiteralPath $f).Hash -ne '{sha}') {{ throw 'sha256 mismatch for {file}; nothing was run' }}; \
         {then} \
         }} finally {{ Remove-Item -Recurse -Force -LiteralPath $t -ErrorAction SilentlyContinue }}",
        url = pin.url,
        sha = pin.sha256,
    )
}

/// The pinned entry of `table` for `arch`; the host's own report of its
/// architecture is never pasted into a command.
fn pinned<'a>(table: &'a [(&str, Pinned)], arch: &str) -> Option<&'a Pinned> {
    table.iter().find(|(a, _)| *a == arch).map(|(_, p)| p)
}

/// The command that installs rustup with the msvc host toolchain.
fn rustup_command(arch: &str) -> Option<String> {
    let pin = pinned(&RUSTUP_INIT, arch)?;
    let host = msvc_toolchain(arch)?.trim_start_matches("stable-");
    Some(verified_download(
        pin,
        "rustup-init.exe",
        &format!(
            "& $f -y --profile minimal --default-host {host}; \
             if ($LASTEXITCODE -ne 0) {{ throw \"rustup-init failed (exit $LASTEXITCODE)\" }}"
        ),
    ))
}

/// The command that adds the msvc toolchain (and so its target) to rustup.
fn msvc_target_command(arch: &str) -> Option<String> {
    let toolchain = msvc_toolchain(arch)?;
    Some(format!(
        "$ErrorActionPreference = 'Stop'; & (Join-Path {CARGO_BIN_PS} 'rustup.exe') toolchain install {toolchain} --profile minimal; \
         if ($LASTEXITCODE -ne 0) {{ throw \"rustup failed (exit $LASTEXITCODE)\" }}"
    ))
}

/// The command that puts the pinned cargo-nextest into the cargo bin.
fn nextest_command(arch: &str) -> Option<String> {
    let pin = pinned(&NEXTEST_ZIP, arch)?;
    Some(verified_download(
        pin,
        "nextest.zip",
        &format!(
            "Expand-Archive -LiteralPath $f -DestinationPath $t -Force; \
             $bin = {CARGO_BIN_PS}; New-Item -ItemType Directory -Force -Path $bin | Out-Null; \
             Copy-Item -LiteralPath (Join-Path $t 'cargo-nextest.exe') -Destination (Join-Path $bin 'cargo-nextest.exe') -Force;"
        ),
    ))
}

/// The command that installs the Visual C++ Build Tools with winget; it
/// needs administrator rights.
fn build_tools_command() -> String {
    format!(
        "winget install --id {BUILD_TOOLS_ID} --exact --silent --accept-package-agreements --accept-source-agreements --override '{BUILD_TOOLS_OVERRIDE}'"
    )
}

/// Append the extra facts to the verb's answer in `found.output`; when the
/// script cannot run (a logged warning) the answer stays as it was, so the
/// checks that need the facts report the tool missing rather than fail the
/// whole report.
pub fn add_extra_facts(found: &mut Found, prober: &dyn Prober) {
    let extra = extra_facts(found, prober);
    if !extra.is_empty() {
        found.output.push('\n');
        found.output.push_str(&extra);
    }
}

/// The extra facts of a Windows host, as `key=value` text.
fn extra_facts(found: &Found, prober: &dyn Prober) -> String {
    let result = match found.kind {
        Kind::WindowsInterop => crate::interop::command(FACTS_PS)
            .and_then(|cmd| crate::sync::exchange_child(cmd, b""))
            .map(|out| String::from_utf8_lossy(&out).into_owned())
            .map_err(|e| e.to_string()),
        Kind::WindowsSsh => prober
            .probe(
                &found.target,
                KeyPolicy::Strict,
                &transport::windows_ssh_line(FACTS_PS),
            )
            .map_err(|(_, e)| e),
        Kind::Unix => return String::new(),
    };
    result.unwrap_or_else(|e| {
        tracing::warn!(host = %found.target.name, error = %e, "cannot read the Windows host's extra facts");
        String::new()
    })
}

/// Look `name` up as found by the verb (`tool.NAME`) or as a bare file in
/// the cargo bin directory (`cargobin.NAME`, how a hand-installed tool
/// looks). The detail says which.
fn found_tool(facts: &BTreeMap<String, String>, name: &str) -> Option<String> {
    if let Some(v) = tool(facts, name) {
        return Some(v.to_owned());
    }
    facts
        .get(&format!("cargobin.{name}"))
        .filter(|v| *v == "1")
        .map(|_| "present (a file in the cargo bin directory)".to_owned())
}

/// A check with an optional fix.
fn check(name: &str, level: Level, detail: String, fix: Option<Fix>) -> Check {
    Check {
        name: name.to_owned(),
        explain: None,
        level,
        detail,
        fix,
    }
}

/// A fix that is a PowerShell command run on the host.
fn fix(command: String, why: &str) -> Fix {
    Fix {
        command,
        root: false,
        why: why.to_owned(),
    }
}

/// The checks of a Windows host: how goway reaches it, the C++ Build Tools,
/// rustup with an msvc target, cargo-nextest and the system drive's space.
/// Pure so it can be tested.
pub fn assess(kind: Kind, facts: &BTreeMap<String, String>, wants_rust: bool) -> Vec<Check> {
    let arch = facts.get("arch").map_or("x86_64", String::as_str);
    let mut out = vec![check(
        "transport",
        Level::Ok,
        match kind {
            Kind::WindowsInterop => "powershell.exe through WSL interop (no ssh)".to_owned(),
            _ => "ssh to the Windows OpenSSH server, running powershell".to_owned(),
        },
        None,
    )];
    if wants_rust {
        out.extend(rust_checks(facts, arch));
    }
    if let Some(free) = facts.get("system_free").and_then(|v| v.parse::<u64>().ok()) {
        let drive = facts.get("system_drive").map_or("the system drive", |d| d);
        let level = if free < MIN_FREE {
            Level::Warn
        } else {
            Level::Ok
        };
        let human = crate::status::human_bytes(free);
        out.push(check(
            "disk",
            level,
            format!("{human} free on {drive}"),
            None,
        ));
    } else {
        tracing::debug!("no system drive facts from the Windows host");
    }
    out
}

/// Build Tools, rustup, its msvc target and nextest.
fn rust_checks(facts: &BTreeMap<String, String>, arch: &str) -> Vec<Check> {
    let mut out = Vec::new();
    let tools = facts.get("build_tools").filter(|v| !v.is_empty());
    let linker = tool(facts, "cc");
    out.push(match (tools, linker) {
        (Some(path), _) => check("msvc build tools", Level::Ok, path.clone(), None),
        (None, Some(_)) => check(
            "msvc build tools",
            Level::Ok,
            "a C compiler is on PATH".to_owned(),
            None,
        ),
        (None, None) => check(
            "msvc build tools",
            Level::Fail,
            "missing; cargo cannot link for msvc without the Visual C++ Build Tools".to_owned(),
            Some(fix(
                build_tools_command(),
                "the Visual Studio Build Tools with the C++ workload install system-wide; Windows asks for administrator rights",
            )),
        ),
    });
    let have_rustup = found_tool(facts, "rustup");
    out.push(match &have_rustup {
        Some(v) => check("rustup", Level::Ok, v.clone(), None),
        None => check(
            "rustup",
            Level::Fail,
            "missing (no Rust toolchain for this user)".to_owned(),
            rustup_command(arch).map(|c| {
                fix(
                    c,
                    "installs the pinned, checksum-verified rustup-init for the user, with the msvc host toolchain",
                )
            }),
        ),
    });
    out.push(
        match (facts.get("rust_msvc").map(String::as_str), &have_rustup) {
            (Some("yes"), _) => check("rust msvc target", Level::Ok, "installed".to_owned(), None),
            (_, None) => check(
                "rust msvc target",
                Level::Fail,
                "missing; rustup-init installs it with rustup".to_owned(),
                None,
            ),
            (_, Some(_)) => check(
                "rust msvc target",
                Level::Fail,
                "rustup has no windows-msvc toolchain".to_owned(),
                msvc_target_command(arch)
                    .map(|c| fix(c, "adds the msvc toolchain with rustup, for the user")),
            ),
        },
    );
    out.push(match found_tool(facts, "cargo-nextest") {
        Some(v) => check("cargo-nextest", Level::Ok, v, None),
        None => check(
            "cargo-nextest",
            Level::Warn,
            "missing; `cargo nextest run` will not work".to_owned(),
            nextest_command(arch).map(|c| {
                fix(
                    c,
                    "installs the pinned, checksum-verified prebuilt binary into the user's cargo bin",
                )
            }),
        ),
    });
    out
}

/// [`assess`] for one project: Rust checks only when the project is Rust.
pub fn assess_project(
    kind: Kind,
    facts: &BTreeMap<String, String>,
    needs: &projneeds::Needs,
) -> Vec<Check> {
    assess(kind, facts, needs.wants_rust())
}

/// Whether the fix of the check `name` needs Windows administrator rights
/// (everything else installs for the user).
pub fn needs_admin(name: &str) -> bool {
    ADMIN_CHECKS.contains(&name)
}

/// The checks whose fixes need administrator rights.
const ADMIN_CHECKS: &[&str] = &["msvc build tools"];

/// The prefix of the install records of a Windows host: its checks share
/// names with the Unix ones (`cargo-nextest`), but undo differently.
pub const RECORD_PREFIX: &str = "win:";

/// How a Windows host's fixes run; the real one uses the host's transport and
/// [`winadmin`], tests use a script.
pub trait Runner {
    /// Run the PowerShell `command` as the host's user; whether it succeeded.
    fn user(&self, command: &str) -> bool;
    /// Run `step` with administrator rights by the best route there is.
    fn admin(&self, step: &WinStep) -> winadmin::Outcome;
}

/// Asks once, before anything is installed, whether to go ahead with
/// these steps (check name, fix, needs administrator rights).
pub type Confirm<'a> = dyn Fn(&[Step]) -> bool + 'a;

/// One install step of a Windows host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// The check it fixes.
    pub check: String,
    /// The fix: PowerShell, exactly as it runs.
    pub fix: Fix,
    /// Whether it needs administrator rights.
    pub admin: bool,
}

/// What [`apply`] did, by check name.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Applied {
    /// Steps that ran and reported success (judged again by a re-check).
    pub ran: Vec<String>,
    /// Steps that ran and failed.
    pub failed: Vec<String>,
    /// Administrator steps not run: no `--rsudo`, or no route to elevate; each
    /// with the lines explaining why and what to run by hand.
    pub manual: Vec<(Step, Vec<String>)>,
    /// Steps nobody confirmed.
    pub declined: bool,
}

/// The steps for `checks` that are not fine and have a fix: the
/// administrator ones first (the Build Tools are what cargo links with),
/// each distinct command once.
pub fn plan(checks: &[Check]) -> Vec<Step> {
    let mut steps: Vec<Step> = Vec::new();
    for c in checks.iter().filter(|c| c.level != Level::Ok) {
        let Some(fix) = &c.fix else { continue };
        if steps.iter().any(|s| s.fix.command == fix.command) {
            continue;
        }
        steps.push(Step {
            check: c.name.clone(),
            fix: fix.clone(),
            admin: needs_admin(&c.name),
        });
    }
    steps.sort_by_key(|s| !s.admin);
    steps
}

/// Run the plan: confirm once for everything, then the administrator step
/// (only when `admin` is allowed) and the user steps. A failing step never
/// stops the others; whether each one worked is judged by the caller's
/// re-check, never inferred from the step's own report.
pub fn apply(steps: &[Step], admin: bool, confirm: &Confirm, runner: &dyn Runner) -> Applied {
    let mut applied = Applied::default();
    if steps.is_empty() {
        return applied;
    }
    if !confirm(steps) {
        tracing::info!("windows fixes not confirmed");
        applied.declined = true;
        return applied;
    }
    for step in steps {
        if step.admin {
            if !admin {
                applied.manual.push((
                    step.clone(),
                    vec![
                        "rerun with --rsudo to let goway ask Windows for administrator rights"
                            .to_owned(),
                    ],
                ));
                continue;
            }
            let win = WinStep {
                why: step.fix.why.clone(),
                command: step.fix.command.clone(),
                wsl_distro: None,
                setup_args: None,
            };
            tracing::info!(check = %step.check, "running an administrator step");
            match runner.admin(&win) {
                winadmin::Outcome::Ran(route) => {
                    tracing::info!(check = %step.check, ?route, "administrator step ran");
                    applied.ran.push(step.check.clone());
                }
                winadmin::Outcome::Failed(route, why) => {
                    tracing::warn!(check = %step.check, ?route, %why, "administrator step failed");
                    applied.failed.push(step.check.clone());
                }
                winadmin::Outcome::Manual { tried } => applied.manual.push((step.clone(), tried)),
            }
        } else {
            tracing::info!(check = %step.check, "running a user step");
            if runner.user(&step.fix.command) {
                applied.ran.push(step.check.clone());
            } else {
                applied.failed.push(step.check.clone());
            }
        }
    }
    applied
}

/// The real [`Runner`]: the host's transport for user steps, and
/// [`winadmin::elevate`] (administrator ssh) or this machine's UAC prompt
/// for administrator ones.
pub struct HostRunner<'a> {
    /// The host as found.
    pub found: &'a Found,
    /// ssh settings shared by every call.
    pub settings: &'a ssh::Settings,
    /// `--windows-admin`: the administrator account for the ssh route.
    pub admin_user: Option<&'a str>,
}

/// [`winadmin::run_local`]'s answer as the [`winadmin::Outcome`] of the route
/// it is: this machine's own UAC prompt.
fn local_outcome(local: winadmin::Local) -> winadmin::Outcome {
    match local {
        winadmin::Local::Done => winadmin::Outcome::Ran(winadmin::Route::DesktopUac),
        winadmin::Local::Failed(why) => winadmin::Outcome::Failed(winadmin::Route::DesktopUac, why),
        winadmin::Local::NoWindows => winadmin::Outcome::Manual {
            tried: vec!["PowerShell is not reachable from here, so no UAC prompt".to_owned()],
        },
    }
}

impl Runner for HostRunner<'_> {
    fn user(&self, command: &str) -> bool {
        let cmd = transport::command(
            self.found.kind,
            &self.found.target,
            self.settings,
            KeyPolicy::Strict,
            transport::Script::Ps(command),
        );
        let mut cmd = match cmd {
            Ok(cmd) => cmd,
            Err(e) => {
                tracing::warn!(error = %e, "cannot start the Windows fix");
                return false;
            }
        };
        cmd.stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .is_ok_and(|s| s.success())
    }

    fn admin(&self, step: &WinStep) -> winadmin::Outcome {
        if self.found.kind == Kind::WindowsInterop {
            return local_outcome(winadmin::run_local(step));
        }
        let target = &self.found.target;
        let runner = winadmin::SshWinRunner {
            key_name: &target.name,
            address: &target.address,
            port: target.port,
            settings: self.settings,
            // A native Windows host has no WSL to start the prompt from.
            wsl: None,
        };
        winadmin::elevate(step, self.admin_user, &runner)
    }
}

/// How to take back the install recorded as `check` (without the
/// [`RECORD_PREFIX`]), derived from the name alone: goway never runs text
/// from a record. `cargo_home_existed` is false when rustup created the cargo
/// home. A bare cargo-nextest is only ever a file: its undo deletes the file
/// (never `cargo uninstall`, which does not know a hand-placed binary).
pub fn undo(check: &str, cargo_home_existed: Option<bool>) -> Undo {
    match check {
        "rustup" if cargo_home_existed != Some(false) => Undo::KeepCargo,
        "rustup" => Undo::Windows {
            command: format!("& (Join-Path {CARGO_BIN_PS} 'rustup.exe') self uninstall -y"),
            admin: false,
        },
        "cargo-nextest" => Undo::Windows {
            command: format!(
                "$f = Join-Path {CARGO_BIN_PS} 'cargo-nextest.exe'; if (Test-Path -LiteralPath $f) {{ Remove-Item -LiteralPath $f -Force }}"
            ),
            admin: false,
        },
        "rust msvc target" => Undo::Windows {
            command: format!(
                "$r = Join-Path {CARGO_BIN_PS} 'rustup.exe'; foreach ($l in (& $r toolchain list)) {{ $n = ($l -split ' ')[0]; if ($n -match '^stable-\\S+windows-msvc$') {{ & $r toolchain uninstall $n }} }}"
            ),
            admin: false,
        },
        // A system-wide package other software may use: listed, never removed.
        "msvc build tools" => Undo::KeepPackage,
        _ => Undo::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::super::parse_facts;
    use super::*;

    fn facts(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn named<'a>(checks: &'a [Check], name: &str) -> &'a Check {
        checks.iter().find(|c| c.name == name).unwrap()
    }

    // frob:tests crates/goway/src/doctor/windows.rs::assess
    #[test]
    fn a_windows_host_with_everything_is_all_ok() {
        let f = facts(&[
            ("arch", "x86_64"),
            ("tool.rustup", "rustup 1.28.2"),
            ("tool.cargo-nextest", "cargo-nextest 0.9.146"),
            ("rust_msvc", "yes"),
            ("build_tools", "C:\\BuildTools"),
            ("system_drive", "C:"),
            ("system_free", "900000000000"),
        ]);
        let checks = assess(Kind::WindowsSsh, &f, true);
        assert!(checks.iter().all(|c| c.level == Level::Ok), "{checks:?}");
        assert_eq!(
            named(&checks, "disk").detail,
            "838.2 GiB free on C:".to_owned()
        );
    }

    // frob:tests crates/goway/src/doctor/windows.rs::assess
    #[test]
    fn a_bare_nextest_file_in_the_cargo_bin_counts_as_present() {
        let f = facts(&[("arch", "aarch64"), ("cargobin.cargo-nextest", "1")]);
        let checks = assess(Kind::WindowsInterop, &f, true);
        let c = named(&checks, "cargo-nextest");
        assert_eq!(c.level, Level::Ok);
        assert!(c.detail.contains("cargo bin"));
        assert!(c.fix.is_none());
    }

    // frob:tests crates/goway/src/doctor/windows.rs::assess
    #[test]
    fn a_bare_host_gets_every_fix_as_an_exact_powershell_command() {
        let checks = assess(Kind::WindowsSsh, &facts(&[("arch", "x86_64")]), true);
        let tools = named(&checks, "msvc build tools").fix.as_ref().unwrap();
        assert!(
            tools
                .command
                .starts_with("winget install --id Microsoft.VisualStudio")
        );
        assert!(tools.command.contains("Workload.VCTools"));
        let rustup = named(&checks, "rustup").fix.as_ref().unwrap();
        assert!(
            rustup
                .command
                .contains("88d8258dcf6ae4f7a80c7d1088e1f36fa7025a1cfd1343731b4ee6f385121fc0")
        );
        assert!(rustup.command.contains("sha256 mismatch"));
        // rustup-init brings the target itself: no separate step.
        assert!(named(&checks, "rust msvc target").fix.is_none());
        let nextest = named(&checks, "cargo-nextest").fix.as_ref().unwrap();
        assert!(
            nextest
                .command
                .contains("0fa689815c8157e4633225b6b173184b3d546eb6ffb754c3d0e6ea5973284a20")
        );
    }

    // frob:tests crates/goway/src/doctor/windows.rs::assess
    #[test]
    fn rustup_without_an_msvc_toolchain_gets_a_toolchain_fix() {
        let f = facts(&[
            ("arch", "aarch64"),
            ("tool.rustup", "rustup 1.28.2"),
            ("rust_msvc", "no"),
        ]);
        let checks = assess(Kind::WindowsSsh, &f, true);
        let fix = named(&checks, "rust msvc target").fix.as_ref().unwrap();
        assert!(fix.command.contains("stable-aarch64-pc-windows-msvc"));
    }

    // frob:tests crates/goway/src/doctor/windows.rs::assess
    #[test]
    fn an_unknown_arch_never_reaches_a_command() {
        let f = facts(&[("arch", "x86_64'; calc; '")]);
        let checks = assess(Kind::WindowsSsh, &f, true);
        assert!(named(&checks, "rustup").fix.is_none());
        assert!(named(&checks, "cargo-nextest").fix.is_none());
    }

    // frob:tests crates/goway/src/doctor/windows.rs::assess
    #[test]
    fn a_non_rust_project_is_not_asked_for_rust() {
        let checks = assess(Kind::WindowsInterop, &facts(&[]), false);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].name, "transport");
    }

    // frob:tests crates/goway/src/doctor/windows.rs::assess
    #[test]
    fn a_low_system_drive_is_a_warning() {
        let f = facts(&[("system_drive", "C:"), ("system_free", "1073741824")]);
        let checks = assess(Kind::WindowsSsh, &f, false);
        assert_eq!(named(&checks, "disk").level, Level::Warn);
    }

    struct NoNames;

    impl crate::resolve::Lookup for NoNames {
        fn system(&self, _: &str) -> Vec<std::net::IpAddr> {
            Vec::new()
        }
        fn windows(&self, _: &str) -> Vec<std::net::IpAddr> {
            Vec::new()
        }
    }

    /// A Windows host that answers the verb and the extra-facts script.
    struct WinHost;

    impl crate::resolve::Prober for WinHost {
        fn probe(
            &self,
            _: &crate::ssh::Target,
            _: KeyPolicy,
            remote: &str,
        ) -> crate::resolve::ProbeResult {
            if remote == transport::windows_ssh_line(FACTS_PS) {
                Ok("cargobin.cargo-nextest=1\nrust_msvc=yes\nbuild_tools=C:\\BT\n".to_owned())
            } else if remote.starts_with("powershell -NoProfile") {
                Ok("arch=aarch64\nos=Microsoft Windows 11\ntool.rustup=rustup 1.29.0\n".to_owned())
            } else {
                Err((crate::ssh::Failure::Other, "unexpected command".to_owned()))
            }
        }
    }

    // frob:tests crates/goway/src/doctor.rs::probe_host
    #[test]
    fn doctor_reaches_a_windows_host_through_the_run_transport_and_merges_the_facts() {
        let host = crate::config::HostConfig {
            name: "winbox".to_owned(),
            os: crate::config::Os::Windows,
            address: Some("192.0.2.7".to_owned()),
            ..crate::config::HostConfig::default()
        };
        let call = crate::remote::Call::new("doctor", &["goway"]);
        let found = super::super::probe_host(
            &crate::config::Config::default(),
            &host,
            &mut crate::state::State::default(),
            &NoNames,
            &WinHost,
            ("unused for Windows", &call),
        )
        .unwrap();
        assert_eq!(found.kind, Kind::WindowsSsh);
        let facts = parse_facts(&found.output);
        let checks = assess(found.kind, &facts, true);
        assert!(checks.iter().all(|c| c.level == Level::Ok), "{checks:?}");
    }

    /// A runner that answers from a script and records what it was asked.
    struct Script {
        user_ok: bool,
        admin: winadmin::Outcome,
        calls: std::cell::RefCell<Vec<String>>,
    }

    impl Runner for Script {
        fn user(&self, command: &str) -> bool {
            self.calls.borrow_mut().push(format!("user:{command}"));
            self.user_ok
        }
        fn admin(&self, step: &WinStep) -> winadmin::Outcome {
            self.calls
                .borrow_mut()
                .push(format!("admin:{}", step.command));
            self.admin.clone()
        }
    }

    fn script(user_ok: bool, admin: winadmin::Outcome) -> Script {
        Script {
            user_ok,
            admin,
            calls: std::cell::RefCell::new(Vec::new()),
        }
    }

    fn bare_plan() -> Vec<Step> {
        plan(&assess(
            Kind::WindowsSsh,
            &facts(&[("arch", "x86_64")]),
            true,
        ))
    }

    // frob:tests crates/goway/src/doctor/windows.rs::plan
    #[test]
    fn the_plan_puts_the_administrator_step_first_and_lists_every_missing_tool() {
        let steps = bare_plan();
        let names: Vec<&str> = steps.iter().map(|s| s.check.as_str()).collect();
        assert_eq!(names, ["msvc build tools", "rustup", "cargo-nextest"]);
        assert_eq!(
            steps.iter().map(|s| s.admin).collect::<Vec<_>>(),
            [true, false, false]
        );
    }

    // frob:tests crates/goway/src/doctor/windows.rs::apply
    #[test]
    fn nothing_runs_until_the_whole_plan_is_confirmed() {
        let runner = script(true, winadmin::Outcome::Ran(winadmin::Route::AdminSsh));
        let applied = apply(&bare_plan(), true, &|_: &[Step]| false, &runner);
        assert!(applied.declined && applied.ran.is_empty());
        assert!(runner.calls.borrow().is_empty());
    }

    // frob:tests crates/goway/src/doctor/windows.rs::apply
    #[test]
    fn without_rsudo_the_administrator_step_is_left_for_the_owner_and_the_user_steps_run() {
        let runner = script(true, winadmin::Outcome::Ran(winadmin::Route::AdminSsh));
        let applied = apply(&bare_plan(), false, &|_: &[Step]| true, &runner);
        assert_eq!(applied.ran, ["rustup", "cargo-nextest"]);
        assert_eq!(applied.manual.len(), 1);
        assert_eq!(applied.manual[0].0.check, "msvc build tools");
        assert!(applied.manual[0].1[0].contains("--rsudo"));
        assert!(runner.calls.borrow().iter().all(|c| c.starts_with("user:")));
    }

    // frob:tests crates/goway/src/doctor/windows.rs::apply
    #[test]
    fn the_administrator_step_goes_through_the_elevation_and_failures_do_not_stop_the_rest() {
        let runner = script(
            false,
            winadmin::Outcome::Failed(winadmin::Route::DesktopUac, "declined".to_owned()),
        );
        let applied = apply(&bare_plan(), true, &|_: &[Step]| true, &runner);
        assert_eq!(
            applied.failed,
            ["msvc build tools", "rustup", "cargo-nextest"]
        );
        let calls = runner.calls.borrow();
        assert!(calls[0].starts_with("admin:winget install"));
        assert_eq!(calls.len(), 3);
    }

    // frob:tests crates/goway/src/doctor/windows.rs::apply
    #[test]
    fn no_route_to_administrator_rights_leaves_the_exact_command() {
        let runner = script(
            true,
            winadmin::Outcome::Manual {
                tried: vec!["nobody at the desktop".to_owned()],
            },
        );
        let applied = apply(&bare_plan(), true, &|_: &[Step]| true, &runner);
        assert_eq!(applied.manual.len(), 1);
        assert_eq!(applied.manual[0].1, ["nobody at the desktop"]);
        assert!(
            applied.manual[0]
                .0
                .fix
                .command
                .starts_with("winget install")
        );
    }

    // frob:tests crates/goway/src/doctor/windows.rs::HostRunner
    #[test]
    fn this_machines_uac_prompt_maps_onto_the_elevation_outcomes() {
        assert_eq!(
            local_outcome(winadmin::Local::Done),
            winadmin::Outcome::Ran(winadmin::Route::DesktopUac)
        );
        assert!(matches!(
            local_outcome(winadmin::Local::NoWindows),
            winadmin::Outcome::Manual { .. }
        ));
        assert!(matches!(
            local_outcome(winadmin::Local::Failed("no".to_owned())),
            winadmin::Outcome::Failed(..)
        ));
    }

    // frob:tests crates/goway/src/doctor/windows.rs::undo
    #[test]
    fn a_recorded_nextest_is_undone_by_deleting_the_file_never_cargo_uninstall() {
        let item = crate::doctor::Installed {
            check: format!("{RECORD_PREFIX}cargo-nextest"),
            cargo_home_existed: None,
        };
        let Undo::Windows { command, admin } = item.undo() else {
            panic!("{:?}", item.undo());
        };
        assert!(!admin);
        assert!(command.contains("Remove-Item") && command.contains("cargo-nextest.exe"));
        assert!(!command.contains("cargo uninstall"));
    }

    // frob:tests crates/goway/src/doctor/windows.rs::undo
    #[test]
    fn windows_undo_keeps_what_may_have_existed_and_the_system_package() {
        assert_eq!(undo("rustup", None), Undo::KeepCargo);
        assert_eq!(undo("rustup", Some(true)), Undo::KeepCargo);
        assert!(matches!(undo("rustup", Some(false)), Undo::Windows { .. }));
        assert_eq!(undo("msvc build tools", None), Undo::KeepPackage);
        assert_eq!(undo("something else", None), Undo::Unknown);
        // The Unix names never reach a Windows command.
        let unix = crate::doctor::Installed {
            check: "cargo-nextest".to_owned(),
            cargo_home_existed: None,
        };
        assert!(matches!(unix.undo(), Undo::Run { .. }));
    }
}
