//! `goway doctor [HOST] [--fix] [--sudo]`: check what a host needs and fix
//! what can be fixed.
//!
//! Checks come from one remote `doctor` call (facts as `key=value`) plus the
//! local ssh setup. Every problem names the exact command that fixes it.
//! `--fix` runs the fixes that need no root, as the ordinary user. Fixes
//! that need root are never run silently: goway prints each command with
//! its reason and asks the user to rerun with `--sudo`, which runs them
//! through an interactive ssh session so sudo can ask for the password.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::process::Stdio;

mod cmakecheck;
mod logout;
mod mac;
pub mod output;
mod prereq;
mod projneeds;
pub mod windows;
pub mod wsl_down;

pub use prereq::Packages;
pub use projneeds::{Needs, Toolchain, first_version};

use crate::cli::DoctorArgs;
use crate::config::{Config, HostConfig, Os, Transport};
use crate::error::{Error, Result};
use crate::paths::Paths;
use crate::pool;
use crate::remote;
use crate::render::Renderer;
use crate::resolve::{self, Found, Lookup, Prober};
use crate::run;
use crate::spawn::CommandExt as _;
use crate::ssh::{self, KeyPolicy};
use crate::sshenv;
use crate::state::State;
use crate::transport::Kind;

/// How bad a finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Fine.
    Ok,
    /// Works, but slower or less safe than it should be.
    Warn,
    /// goway (or a Rust build) cannot work until fixed.
    Fail,
}

/// A command that fixes a finding, run on the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fix {
    /// The shell command (without `sudo`).
    pub command: String,
    /// Whether it needs root.
    pub root: bool,
    /// Why it is needed, shown when asking for sudo.
    pub why: String,
}

impl Fix {
    /// The exact command line to type on the host (what goway runs).
    pub fn display(&self) -> String {
        if self.root {
            format!("sudo bash -c {}", ssh::shell_quote(&self.command))
        } else {
            self.command.clone()
        }
    }
}

/// Checks whose fixes are hardening, not needs: listed apart and applied
/// only with `--harden`, never in the same confirmation as tool installs.
pub const HARDENING: &[&str] = &["sshd password login"];

/// One check result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    /// What was checked.
    pub name: String,
    /// The verdict.
    pub level: Level,
    /// Version found or what is wrong.
    pub detail: String,
    /// How to fix it, if goway knows.
    pub fix: Option<Fix>,
    /// The long explanation, shown only by `goway doctor --explain CHECK`.
    pub explain: Option<String>,
}

/// Parse `key=value` lines.
pub fn parse_facts(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect()
}

fn tool<'a>(facts: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
    facts
        .get(&format!("tool.{name}"))
        .map(String::as_str)
        .filter(|v| !v.is_empty())
}

/// `brew install PACKAGE` as a per-user fix, when Homebrew is there.
fn brew_fix(facts: &BTreeMap<String, String>, package: &str) -> Option<Fix> {
    tool(facts, "brew")?;
    Some(Fix {
        command: format!("brew install {package}"),
        root: false,
        why: "Homebrew installs per user, no sudo".to_owned(),
    })
}

/// Whether the host is a Mac (its helper side runs on Homebrew's GNU tools).
fn is_darwin(facts: &BTreeMap<String, String>) -> bool {
    facts.get("kernel").is_some_and(|k| k == "Darwin")
}

/// The Homebrew packages goway's remote side needs on a Mac.
const BREW_TOOLS: &str = "coreutils findutils gnu-sed gnu-tar grep util-linux flock";

/// The macOS checks: Homebrew itself, then the GNU tools goway's remote
/// side runs on (all installed per user, never with sudo).
fn darwin_checks(facts: &BTreeMap<String, String>) -> Vec<Check> {
    let missing: Vec<&str> = facts
        .get("gnu_missing")
        .map(String::as_str)
        .unwrap_or_default()
        .split(',')
        .filter(|t| !t.is_empty())
        .collect();
    let mut gone: Vec<String> = ["flock", "setsid"]
        .iter()
        .filter(|t| tool(facts, t).is_none())
        .map(|t| (*t).to_owned())
        .collect();
    gone.extend(missing.iter().map(|t| (*t).to_owned()));
    let brew = tool(facts, "brew").is_some();
    let check = |level, detail: String, fix| Check {
        name: "Homebrew GNU tools".to_owned(),
        explain: None,
        level,
        detail,
        fix,
    };
    if gone.is_empty() {
        return vec![check(
            Level::Ok,
            "GNU coreutils, findutils, sed, tar, grep, flock and setsid found".to_owned(),
            None,
        )];
    }
    let detail = format!(
        "missing: {}; goway's remote side needs the GNU versions",
        gone.join(", ")
    );
    if brew {
        vec![check(
            Level::Fail,
            detail,
            Some(Fix {
                command: format!("brew install {BREW_TOOLS}"),
                root: false,
                why: "Homebrew installs per user, no sudo".to_owned(),
            }),
        )]
    } else {
        vec![check(
            Level::Fail,
            format!(
                "{detail}; Homebrew is not installed: see https://brew.sh (its installer asks for sudo itself), then run `brew install {BREW_TOOLS}`"
            ),
            None,
        )]
    }
}

/// The package install command for this host's package manager.
fn install(facts: &BTreeMap<String, String>, apt: &str, dnf: &str, pacman: &str) -> String {
    if tool(facts, "apt-get").is_some() {
        format!("apt-get update && apt-get install -y {apt}")
    } else if tool(facts, "dnf").is_some() {
        format!("dnf install -y {dnf}")
    } else if tool(facts, "pacman").is_some() {
        format!("pacman -S --noconfirm {pacman}")
    } else {
        format!("install {apt} with the system package manager")
    }
}

/// Pinned releases goway installs, with their sha256 per architecture.
struct Pinned {
    url: &'static str,
    sha256: &'static str,
    /// The member to extract (sccache's tarball nests it in a directory).
    member: &'static str,
    strip: u8,
}

const NEXTEST: [(&str, Pinned); 2] = [
    (
        "x86_64",
        Pinned {
            url: "https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-0.9.146/cargo-nextest-0.9.146-x86_64-unknown-linux-gnu.tar.gz",
            sha256: "682c21b777c333e96fd532e114d3a5a894e0729ab88d94c0a9f20f8419695428",
            member: "cargo-nextest",
            strip: 0,
        },
    ),
    (
        "aarch64",
        Pinned {
            url: "https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-0.9.146/cargo-nextest-0.9.146-aarch64-unknown-linux-gnu.tar.gz",
            sha256: "b2e33d7c72de7ade0ff7b3a948ac37516b24f8a836b7a8870c1f634a94be9de9",
            member: "cargo-nextest",
            strip: 0,
        },
    ),
];

const SCCACHE: [(&str, Pinned); 2] = [
    (
        "x86_64",
        Pinned {
            url: "https://github.com/mozilla/sccache/releases/download/v0.18.0/sccache-v0.18.0-x86_64-unknown-linux-musl.tar.gz",
            sha256: "45f1447fbe231e3037bde351ef70677dd212216c8d62ae7ca409fecc4d6acc89",
            member: "sccache-v0.18.0-x86_64-unknown-linux-musl/sccache",
            strip: 1,
        },
    ),
    (
        "aarch64",
        Pinned {
            url: "https://github.com/mozilla/sccache/releases/download/v0.18.0/sccache-v0.18.0-aarch64-unknown-linux-musl.tar.gz",
            sha256: "2b3284d5da3b46a47dc4229e75bb7b88ac4aa99c8d754fb7d2f84997e5a4354a",
            member: "sccache-v0.18.0-aarch64-unknown-linux-musl/sccache",
            strip: 1,
        },
    ),
];

/// The install command for a pinned release on `arch`: download, verify
/// the sha256, then extract into the user's cargo bin. `None` for an
/// architecture goway has no pinned build for (the host's report of its
/// own arch is never pasted into a command).
fn pinned_install(table: &[(&str, Pinned)], arch: &str) -> Option<String> {
    let (_, p) = table.iter().find(|(a, _)| *a == arch)?;
    Some(format!(
        "t=$(mktemp -d) && curl -fsSL {url} -o \"$t/a.tgz\" && echo \"{sha}  $t/a.tgz\" | sha256sum -c --quiet && mkdir -p {bin} && tar xzf \"$t/a.tgz\" -C {bin} --strip-components={strip} {member}; rc=$?; rm -rf \"$t\"; exit $rc",
        url = p.url,
        sha = p.sha256,
        bin = CARGO_BIN,
        strip = p.strip,
        member = p.member,
    ))
}

/// Reload sshd whatever the distribution calls the unit (`ssh` on Debian and
/// Ubuntu, `sshd` on Fedora and Arch). Failing to reload is reported but
/// does not stop the root fixes that follow: the file is already in place
/// and sshd reads it at its next start.
const RELOAD_SSHD: &str = "{ systemctl reload ssh || systemctl reload sshd || echo 'goway: could not reload sshd; the change applies at its next restart' >&2; }";

/// The sshd drop-in goway writes (and removes again on uninstall).
const SSHD_DROPIN: &str = "/etc/ssh/sshd_config.d/10-goway-keys-only.conf";

/// Write the keys-only drop-in, check the whole sshd configuration with
/// `sshd -t` (removing the drop-in again if it does not pass), then reload.
fn sshd_keys_only_command() -> String {
    format!(
        "printf 'PasswordAuthentication no\\nKbdInteractiveAuthentication no\\n' > {SSHD_DROPIN} && {{ \"$(command -v sshd || echo /usr/sbin/sshd)\" -t || {{ rm -f {SSHD_DROPIN}; echo 'goway: sshd rejected the change; it was removed' >&2; false; }}; }} && {RELOAD_SSHD}"
    )
}

const CARGO_BIN: &str = "\"${CARGO_HOME:-$HOME/.cargo}/bin\"";
const MIN_FREE: u64 = 10 * 1024 * 1024 * 1024;

/// Turn remote facts into checks with fixes; pure so it can be tested.
pub fn assess(facts: &BTreeMap<String, String>) -> Vec<Check> {
    let mut out = system_checks(facts);
    out.extend(toolchain_checks(facts));
    out.extend(host_checks(facts));
    out
}

/// [`assess_project`] for a host of `kind`: a Windows host gets the Windows
/// checks (Build Tools, rustup with msvc, nextest, system drive).
fn assess_for(
    kind: Kind,
    facts: &BTreeMap<String, String>,
    needs: &projneeds::Needs,
) -> Vec<Check> {
    match kind {
        Kind::Unix => assess_project(facts, needs),
        Kind::WindowsSsh | Kind::WindowsInterop => windows::assess_project(kind, facts, needs),
    }
}

/// Find `host` and read its doctor facts: a Unix host through the raw
/// command `cmd` (with the project's wrappers), any other kind through the
/// `doctor` verb in its own language (the transport `run` uses), plus the
/// Windows-only extra facts.
fn probe_host(
    config: &Config,
    host: &HostConfig,
    state: &mut State,
    lookup: &dyn Lookup,
    prober: &dyn Prober,
    (cmd, call): (&str, &remote::Call),
) -> Result<Found> {
    let (found, sent, rtt) = crate::facts::clock::timed(|| {
        if Kind::of(host) == Kind::Unix {
            resolve::resolve(config, host, state, lookup, prober, KeyPolicy::Strict, cmd)
        } else {
            resolve::resolve_call(config, host, state, lookup, prober, KeyPolicy::Strict, call)
        }
    });
    let mut found = found?;
    if let Some(ms) = crate::facts::clock::measure(&found.output, sent, rtt) {
        // frob:ticket 01M42RAM7D56M1KH49NTGZTRVF
        let _ = writeln!(found.output, "\n{}={ms}", crate::facts::clock::FACT);
    }
    if Kind::of(host) != Kind::Unix {
        windows::add_extra_facts(&mut found, prober);
    }
    Ok(found)
}

/// The clock check for a host whose measured offset is in `facts`, if over tolerance.
fn clock_check(facts: &BTreeMap<String, String>, windows: bool) -> Option<Check> {
    let ms: i64 = facts.get(crate::facts::clock::FACT)?.parse().ok()?;
    Some(Check {
        name: "clock".to_owned(),
        explain: None,
        level: Level::Warn,
        detail: crate::facts::clock::detail(ms, windows)?,
        fix: None,
    })
}

/// Checks for one project: goway's own system tools, the Rust toolchain
/// only when the project is Rust (or nothing was detected), the host checks,
/// and every tool the project's files and `goway.toml` ask for.
pub fn assess_project(facts: &BTreeMap<String, String>, needs: &projneeds::Needs) -> Vec<Check> {
    let mut out = system_checks(facts);
    if !needs.wants_rust() {
        // cargo links through cc, but only Rust and C/C++ projects need one.
        let cc_wanted = needs.ecosystems.contains(&projneeds::Eco::Cpp);
        out.retain(|c| c.name != "cc (linker)" || cc_wanted);
    }
    if needs.wants_rust() {
        out.extend(toolchain_checks(facts));
    }
    out.extend(host_checks(facts));
    out.extend(logout::checks(facts));
    out.extend(mac::checks(facts));
    let mut have: Vec<String> = out.iter().map(|c| c.name.clone()).collect();
    if have.iter().any(|n| n == "cc (linker)") {
        have.push("cc".to_owned());
    }
    out.extend(projneeds::checks(needs, facts, &have));
    out
}

/// `base` followed by `script` (bash source) run on the host. The script
/// travels as base64 like [`remote::invocation`]'s payload, so a login shell
/// that rejects newlines, backslashes or bangs (fish, csh) still accepts it.
fn append_script(base: &str, script: &str) -> String {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD.encode(script);
    format!("{base}; bash -c 'eval \"$(printf %s {b64} | base64 -d)\"'")
}

/// Tools goway's remote side and the fixes need.
fn system_checks(facts: &BTreeMap<String, String>) -> Vec<Check> {
    let mut out = Vec::new();
    let mut push = |name: &str, level, detail: String, fix| {
        out.push(Check {
            name: name.to_owned(),
            explain: None,
            level,
            detail,
            fix,
        });
    };
    let present = |name: &str| tool(facts, name).map(str::to_owned);
    if is_darwin(facts) {
        // The package-manager fixes below are Linux's; a Mac has no
        // missing curl and its compiler comes with the Xcode tools.
        let mut checks = darwin_checks(facts);
        checks.push(match present("cc") {
            Some(v) => Check {
                name: "cc (linker)".to_owned(),
                explain: None,
                level: Level::Ok,
                detail: v,
                fix: None,
            },
            None => Check {
                name: "cc (linker)".to_owned(),
                explain: None,
                level: Level::Fail,
                detail: "missing; run `xcode-select --install` on the Mac (it opens a dialog)"
                    .to_owned(),
                fix: None,
            },
        });
        return checks;
    }
    for t in ["bash", "tar", "flock", "setsid"] {
        match present(t) {
            Some(v) => push(t, Level::Ok, v, None),
            None => push(
                t,
                Level::Fail,
                "missing; goway's remote side needs it".to_owned(),
                Some(Fix {
                    command: install(
                        facts,
                        "bash tar util-linux",
                        "bash tar util-linux",
                        "bash tar util-linux",
                    ),
                    root: true,
                    why: format!("goway runs its remote side with {t}; system packages need root"),
                }),
            ),
        }
    }
    let curl = present("curl");
    if curl.is_none() {
        push(
            "curl",
            Level::Warn,
            "missing; the toolchain fixes download with it".to_owned(),
            Some(Fix {
                command: install(facts, "curl ca-certificates", "curl", "curl"),
                root: true,
                why: "the rustup, nextest and sccache installers download with curl; system packages need root".to_owned(),
            }),
        );
    }
    match present("cc") {
        Some(v) => push("cc (linker)", Level::Ok, v, None),
        None => push(
            "cc (linker)",
            Level::Fail,
            "missing; cargo cannot link without a C toolchain".to_owned(),
            Some(Fix {
                command: install(facts, "build-essential", "gcc", "base-devel"),
                root: true,
                why: "cargo links through the system C compiler; system packages need root"
                    .to_owned(),
            }),
        ),
    }
    out
}

/// NVIDIA's CUDA toolkit for WSL-Ubuntu (apt on `x86_64` only); the Windows driver stays the only
/// driver, so the repository's `cuda-toolkit` package (no driver) is installed, never `cuda`.
fn cuda_fix(facts: &BTreeMap<String, String>) -> Option<Fix> {
    if tool(facts, "apt-get").is_none() || facts.get("arch").is_none_or(|a| a != "x86_64") {
        return None;
    }
    Some(Fix {
        command: format!(
            "cd \"$(mktemp -d)\" && curl -fsSLO {CUDA_KEYRING_URL} && dpkg -i cuda-keyring_1.1-1_all.deb && apt-get update && apt-get install -y cuda-toolkit"
        ),
        root: true,
        why: "installs NVIDIA's CUDA toolkit for WSL (no Linux GPU driver: the Windows driver serves WSL); system packages need root".to_owned(),
    })
}

/// NVIDIA's apt keyring package for WSL-Ubuntu on `x86_64`.
const CUDA_KEYRING_URL: &str = "https://developer.download.nvidia.com/compute/cuda/repos/wsl-ubuntu/x86_64/cuda-keyring_1.1-1_all.deb";

/// What WSL got of the laptop's RAM, swap and processors, and the command that changes it.
fn push_wsl_hardware(
    facts: &BTreeMap<String, String>,
    hw: &crate::facts::StaticFacts,
    push: &mut impl FnMut(&str, Level, String, Option<Fix>),
) {
    let (Some(win_ram), Some(win_cores)) = (hw.windows_ram, hw.windows_cores) else {
        tracing::debug!("no Windows hardware facts (interop off): skipping the WSL size check");
        return;
    };
    let ram = facts.get("mem_total").and_then(|v| v.parse::<u64>().ok());
    let cores = facts.get("cores").and_then(|v| v.parse::<u32>().ok());
    let short = crate::facts::shortfalls(ram, hw.swap_total, cores, win_ram, win_cores);
    if short.is_empty() {
        push(
            "wsl size",
            Level::Ok,
            "WSL has a fair share of the laptop's RAM, swap and processors".to_owned(),
            None,
        );
        return;
    }
    let command = crate::facts::suggest(win_ram, win_cores).command();
    push(
        "wsl size",
        Level::Warn,
        format!(
            "WSL gets much less than the laptop has ({}). Windows keeps at least 4 GiB or 25% of the RAM. \
             Run on the helper's Windows side (journaled; `goway-setup uninstall --host` restores the old values): `{command}`; \
             it asks before `wsl --shutdown` and refuses while goway jobs run",
            short.join("; ")
        ),
        None,
    );
}

/// The Rust toolchain.
fn toolchain_checks(facts: &BTreeMap<String, String>) -> Vec<Check> {
    let mut out = Vec::new();
    let mut push = |name: &str, level, detail: String, fix| {
        out.push(Check {
            name: name.to_owned(),
            explain: None,
            level,
            detail,
            fix,
        });
    };
    let present = |name: &str| tool(facts, name).map(str::to_owned);
    let arch = facts.get("arch").map_or("x86_64", String::as_str);
    match (present("rustup"), present("cargo")) {
        (_, Some(v)) => push("cargo", Level::Ok, v, None),
        (_, None) => push(
            "cargo",
            Level::Fail,
            "missing (no rustup toolchain for this user)".to_owned(),
            Some(Fix {
                command: "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal".to_owned(),
                root: false,
                why: "rustup installs per user".to_owned(),
            }),
        ),
    }
    match present("cargo-nextest") {
        Some(v) => push("cargo-nextest", Level::Ok, v, None),
        None => push(
            "cargo-nextest",
            Level::Warn,
            "missing; `cargo nextest run` will not work".to_owned(),
            if is_darwin(facts) {
                brew_fix(facts, "cargo-nextest")
            } else {
                pinned_install(&NEXTEST, arch).map(|command| Fix {
                    command,
                    root: false,
                    why: "installs the pinned, checksum-verified prebuilt binary into the user's cargo bin".to_owned(),
                })
            },
        ),
    }
    match present("sccache") {
        Some(v) => push("sccache", Level::Ok, v, None),
        None => push(
            "sccache",
            Level::Warn,
            "missing; cold builds in new target slots will be slower".to_owned(),
            if is_darwin(facts) {
                brew_fix(facts, "sccache")
            } else {
                pinned_install(&SCCACHE, arch).map(|command| Fix {
                    command,
                    root: false,
                    why: "installs the pinned, checksum-verified release binary into the user's cargo bin".to_owned(),
                })
            },
        ),
    }
    out
}

/// The warning for a helper whose shell startup files print text for non-interactive ssh.
fn startup_noise_check(facts: &BTreeMap<String, String>) -> Option<Check> {
    let sample = facts.get("startup_noise")?;
    Some(Check {
        name: "shell startup".to_owned(),
        explain: None,
        level: Level::Warn,
        detail: format!(
            "the helper's shell startup files print text for non-interactive ssh (first line: \"{sample}\"); goway ignores it, \
             but it is printed on every call. Guard it at the top of ~/.bashrc: case $- in *i*) ;; *) return ;; esac"
        ),
        fix: None,
    })
}

/// File systems whose locking, permissions or speed make builds unreliable.
const NETWORK_FS: &[&str] = &[
    "nfs",
    "nfs4",
    "cifs",
    "smb",
    "smb2",
    "smb3",
    "smbfs",
    "9p",
    "drvfs",
    "afs",
    "ceph",
    "glusterfs",
    "lustre",
    "vboxsf",
    "fuse.sshfs",
    "fuse.s3fs",
    "fuseblk",
];

/// A tmp smaller than this is not trusted with build scratch space (3.9 GiB tmpfs seen live).
const SMALL_TMP: u64 = 8 << 30;

/// The file system of goway's remote root and of the helper's temp directory.
fn filesystem_checks(facts: &BTreeMap<String, String>) -> Vec<Check> {
    let mut out = Vec::new();
    let mut push = |name: &str, level, detail: String| {
        out.push(Check {
            name: name.to_owned(),
            explain: None,
            level,
            detail,
            fix: None,
        });
    };
    let num = |k: &str| facts.get(k).and_then(|v| v.parse::<u64>().ok());
    let flag = |k: &str| facts.get(k).is_some_and(|v| v == "1");
    if let Some(fs) = facts.get("root_fs") {
        let free = num("root_free").map_or_else(String::new, |f| {
            format!(", {} free", crate::status::human_bytes(f))
        });
        let (level, note) = if flag("root_noexec") {
            (
                Level::Fail,
                " and mounted noexec, so builds cannot run programs there: set remote_root to a directory on another file system".to_owned(),
            )
        } else if flag("root_case_insensitive") {
            (
                Level::Warn,
                " and it ignores case: goway refuses repositories with paths that differ only in case; use a case-sensitive remote_root for those".to_owned(),
            )
        } else if NETWORK_FS.contains(&fs.as_str()) {
            (
                Level::Warn,
                " is a network or translated file system: builds are slow and file locks may not hold; set remote_root on a local disk".to_owned(),
            )
        } else {
            (Level::Ok, String::new())
        };
        push("goway root", level, format!("file system {fs}{free}{note}"));
    }
    let small = num("tmp_size").is_some_and(|s| s < SMALL_TMP);
    if flag("tmp_noexec") || small {
        let fs = facts.get("tmp_fs").map_or("unknown", String::as_str);
        let why = if flag("tmp_noexec") {
            "mounted noexec"
        } else {
            "small"
        };
        push(
            "temp dir",
            Level::Ok,
            format!(
                "the helper's temp directory ({fs}) is {why}; goway keeps its scratch files in its own root instead"
            ),
        );
    }
    out
}

/// The proxy variables set on the host, by name only: a proxy URL may carry credentials.
fn proxy_check(facts: &BTreeMap<String, String>) -> Option<Check> {
    let names = facts.get("proxy_vars").filter(|v| !v.is_empty())?;
    Some(Check {
        name: "proxy".to_owned(),
        explain: None,
        level: Level::Ok,
        detail: format!("{names} set on the host (values are never shown)"),
        fix: None,
    })
}

/// Disk and sshd hardening.
fn host_checks(facts: &BTreeMap<String, String>) -> Vec<Check> {
    let mut out: Vec<Check> = startup_noise_check(facts).into_iter().collect();
    out.extend(proxy_check(facts));
    out.extend(filesystem_checks(facts));
    let mut push = |name: &str, level, detail: String, fix| {
        out.push(Check {
            name: name.to_owned(),
            explain: None,
            level,
            detail,
            fix,
        });
    };
    match facts.get("disk_free").and_then(|v| v.parse::<u64>().ok()) {
        Some(free) if free < MIN_FREE => push(
            "disk",
            Level::Warn,
            format!(
                "{} free in the home directory",
                crate::status::human_bytes(free)
            ),
            None,
        ),
        Some(free) => push(
            "disk",
            Level::Ok,
            format!("{} free", crate::status::human_bytes(free)),
            None,
        ),
        None => {}
    }
    if let Some(hw) = crate::facts::parse_static(facts) {
        match hw.interop {
            Some(crate::facts::Interop::Elevated) if hw.wsl => push(
                "wsl interop",
                Level::Warn,
                format!("SECURITY: {}", crate::facts::ELEVATED_INTEROP_WARNING),
                None,
            ),
            Some(crate::facts::Interop::Off) if hw.wsl => {
                push("wsl interop", Level::Ok, "disabled (safe)".to_owned(), None);
            }
            Some(crate::facts::Interop::Limited) if hw.wsl => push(
                "wsl interop",
                Level::Ok,
                "runs Windows programs without administrator rights".to_owned(),
                None,
            ),
            _ => {}
        }
        if hw.wsl {
            push_wsl_hardware(facts, &hw, &mut push);
        }
        if let Some(name) = crate::facts::gpu_invisible_to_wsl(&hw) {
            push(
                "gpu",
                Level::Warn,
                format!(
                    "Windows has {name}, but WSL cannot see it (no nvidia-smi or rocm-smi GPU). \
                     Fix on Windows, not in WSL: install the current NVIDIA (or AMD) Windows driver \
                     with WSL support, do not install a Linux GPU driver inside WSL, run \
                     `wsl --shutdown`, reopen WSL and check that `nvidia-smi` lists the GPU"
                ),
                None,
            );
        } else if hw.gpus.is_empty() {
            tracing::debug!("host has no GPU visible");
        } else {
            let list: Vec<String> = hw.gpus.iter().map(crate::facts::Gpu::summary).collect();
            push("gpu", Level::Ok, list.join(", "), None);
            if hw.wsl && hw.gpus.iter().any(|g| g.vendor == "nvidia") {
                push(
                    "cuda toolkit",
                    if hw.nvcc { Level::Ok } else { Level::Warn },
                    if hw.nvcc {
                        "nvcc found".to_owned()
                    } else {
                        "the GPU is visible but the CUDA toolkit (nvcc) is not installed".to_owned()
                    },
                    (!hw.nvcc).then(|| cuda_fix(facts)).flatten(),
                );
            }
        }
    }
    match facts.get("password_auth").map(String::as_str) {
        Some("no") => push("sshd password login", Level::Ok, "disabled (keys only)".to_owned(), None),
        Some(other) => push(
            "sshd password login",
            Level::Warn,
            format!("allowed ({other}); goway needs keys only"),
            Some(Fix {
                command: sshd_keys_only_command(),
                root: true,
                why: "password logins widen the attack surface and goway only uses keys; sshd config is owned by root (undo: remove /etc/ssh/sshd_config.d/10-goway-keys-only.conf)".to_owned(),
            }),
        ),
        None => {}
    }
    out
}

/// Runs fix commands on a host; injectable so the sudo policy is testable.
pub trait FixRunner {
    /// Run `command` as the user (`sudo == false`) or under sudo with a tty.
    fn run(&self, command: &str, sudo: bool) -> bool;
}

/// What `apply_fixes` did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Applied {
    /// User-level fixes that ran and succeeded (commands).
    pub done: Vec<String>,
    /// User-level fixes that ran and failed (commands).
    pub failed: Vec<String>,
    /// Root fixes not run because `--sudo` was not given.
    pub need_sudo: Vec<Fix>,
    /// The root fixes that went into the one sudo session, as (check, fix).
    /// The session reports each step itself; whether a step worked is judged
    /// by checking again afterwards, never inferred from the session's status.
    pub root_ran: Vec<(String, Fix)>,
}

/// The one root script for several fixes: each runs as its own step with its
/// own result line, a failing step never stops the others, and `apt-get
/// update` runs once for the whole session instead of once per package.
fn root_script(steps: &[(String, Fix)]) -> String {
    use std::fmt::Write as _;
    const UPDATE: &str = "apt-get update && ";
    let mut script = String::from("set +e\nfailed=0\n");
    if steps.iter().any(|(_, f)| f.command.starts_with(UPDATE)) {
        script.push_str(
            "echo '==> apt-get update'\napt-get update || echo 'goway: apt-get update failed; going on with the package lists the host has'\n",
        );
    }
    for (name, fix) in steps {
        let command = fix.command.strip_prefix(UPDATE).unwrap_or(&fix.command);
        let label = format!("'{}'", name.replace('\'', ""));
        let _ = write!(
            script,
            "echo\necho '==>' {label}\nrc=0\n(\n{command}\n) || rc=$?\nif [ $rc -ne 0 ]; then echo \"goway: step failed (exit $rc):\" {label}; failed=$((failed+1)); else echo 'goway: step ok:' {label}; fi\n"
        );
    }
    let _ = writeln!(
        script,
        "if [ $failed -ne 0 ]; then echo \"goway: $failed of {} steps failed; the others ran\"; exit 1; fi",
        steps.len()
    );
    script
}

/// Asks whether to run the root steps of one host, given their script.
pub type Confirm<'a> = dyn Fn(&[(String, Fix)], &str) -> bool + 'a;

/// Run the fixes `--fix` allows: user fixes always; root fixes only with
/// `sudo` and only after `confirm` approves the whole list, and then all
/// together in ONE sudo session (one password, typed into sudo itself), each
/// as its own step. Each distinct command runs once.
pub fn apply_fixes(
    checks: &[Check],
    sudo: bool,
    confirm: &Confirm,
    runner: &dyn FixRunner,
) -> Applied {
    let mut applied = Applied::default();
    let mut seen = std::collections::BTreeSet::new();
    let mut root: Vec<(String, Fix)> = Vec::new();
    let mut user = Vec::new();
    for (name, fix) in checks
        .iter()
        .filter(|c| c.level != Level::Ok)
        .filter_map(|c| c.fix.as_ref().map(|f| (&c.name, f)))
    {
        if seen.insert(fix.command.clone()) {
            if fix.root {
                root.push((name.clone(), fix.clone()));
            } else {
                user.push(fix.clone());
            }
        }
    }
    // Root fixes first: they provide what user fixes need (curl, cc).
    if !root.is_empty() {
        let fixes: Vec<Fix> = root.iter().map(|(_, f)| f.clone()).collect();
        let script = root_script(&root);
        if sudo && confirm(&root, &script) {
            tracing::info!(fixes = root.len(), "running root fixes in one sudo session");
            let ok = runner.run(&script, true);
            if !ok {
                tracing::warn!("the root session reported failing steps");
            }
            applied.root_ran = root;
        } else {
            applied.need_sudo = fixes;
        }
    }
    for fix in user {
        tracing::info!(command = %fix.command, "running fix");
        if runner.run(&fix.command, false) {
            applied.done.push(fix.command.clone());
        } else {
            applied.failed.push(fix.command.clone());
        }
    }
    applied
}

/// Real fix runner: ssh to the found host, `sudo` with a tty when needed.
pub(crate) struct SshFixRunner<'a> {
    pub(crate) found: &'a Found,
    pub(crate) settings: &'a ssh::Settings,
    /// The config directory whose change log records each command before it runs; `None` for
    /// an undo, which takes earlier changes back and is not a new change.
    pub(crate) record_in: Option<&'a std::path::Path>,
}

/// How a person takes back what `doctor --fix` ran, for the change log.
pub(crate) const FIX_UNDO_HINT: &str = "`goway uninstall` takes back what doctor --fix installed";

impl FixRunner for SshFixRunner<'_> {
    fn run(&self, command: &str, sudo: bool) -> bool {
        if let Some(dir) = self.record_in {
            let reason = if sudo {
                "doctor --fix, as root"
            } else {
                "doctor --fix"
            };
            if let Err(e) = crate::changelog::record_action(
                dir,
                goway_journal::ActionKind::RunFix,
                command,
                &self.found.target.name,
                reason,
                Some(FIX_UNDO_HINT),
            ) {
                tracing::error!(error = %e, "could not record the fix; not running it");
                return false;
            }
        }
        let script = if sudo {
            Fix {
                command: command.to_owned(),
                root: true,
                why: String::new(),
            }
            .display()
        } else {
            format!("bash -c {}", ssh::shell_quote(command))
        };
        let mut cmd = ssh::command(
            &self.found.target,
            self.settings,
            KeyPolicy::Strict,
            &script,
        );
        if sudo {
            ssh::force_tty(&mut cmd);
        }
        cmd.stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status_locked()
            .is_ok_and(|s| s.success())
    }
}

/// Something `doctor --fix` installed on a host. Only the name of the check
/// is stored: the undo command is derived from it when needed, never read
/// from the record, so a tampered record cannot make goway run its text.
/// (Older records also carried `undo` and `root`; they are ignored.)
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Installed {
    /// The check it fixed (`cargo`, `cargo-nextest`, ...).
    pub check: String,
    /// For `cargo`: whether `~/.cargo` existed before goway installed
    /// rustup. `None` (an older record) counts as "existed".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cargo_home_existed: Option<bool>,
}

/// What uninstall does about one recorded install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Undo {
    /// Run this command (as root when `root`).
    Run {
        /// The command text, from [`undo_of`].
        command: String,
        /// Whether it needs administrator rights.
        root: bool,
    },
    /// Run this PowerShell command on the Windows host (as administrator when `admin`).
    Windows {
        /// The command text, from [`windows::undo`].
        command: String,
        /// Whether it needs administrator rights.
        admin: bool,
    },
    /// A system package: listed, not removed.
    KeepPackage,
    /// rustup was set up on top of a `~/.cargo` that existed before.
    KeepCargo,
    /// Not a check goway knows; ignored.
    Unknown,
}

/// System-package checks: their installs are listed, never undone.
const PACKAGE_CHECKS: &[&str] = &["bash", "tar", "flock", "setsid", "curl", "cc (linker)"];

impl Installed {
    /// The action that takes this install back, derived from the check name.
    pub fn undo(&self) -> Undo {
        if let Some(check) = self.check.strip_prefix(windows::RECORD_PREFIX) {
            return windows::undo(check, self.cargo_home_existed);
        }
        if self.check == "cargo" && self.cargo_home_existed != Some(false) {
            return Undo::KeepCargo;
        }
        if let Some((command, root)) = undo_of(&self.check) {
            return Undo::Run { command, root };
        }
        if PACKAGE_CHECKS.contains(&self.check.as_str())
            || projneeds::is_package_check(&self.check)
            || self.check.starts_with("pkg:")
        {
            Undo::KeepPackage
        } else {
            Undo::Unknown
        }
    }
}

/// How to take back the fix of `check`; `None` for system packages, which
/// other software may have come to rely on (they are listed instead).
pub fn undo_of(check: &str) -> Option<(String, bool)> {
    match check {
        "cargo" => Some((format!("{CARGO_BIN}/rustup self uninstall -y"), false)),
        // Also the cargo bin directories the install created, if now empty.
        "cargo-nextest" => Some((
            format!(
                "rm -f {CARGO_BIN}/cargo-nextest && {{ rmdir {CARGO_BIN} 2>/dev/null && rmdir \"${{CARGO_HOME:-$HOME/.cargo}}\" 2>/dev/null; true; }}"
            ),
            false,
        )),
        "sccache" => Some((
            format!(
                "rm -f {CARGO_BIN}/sccache && {{ rmdir {CARGO_BIN} 2>/dev/null && rmdir \"${{CARGO_HOME:-$HOME/.cargo}}\" 2>/dev/null; true; }}"
            ),
            false,
        )),
        "sshd password login" => Some((format!("rm -f {SSHD_DROPIN} && {RELOAD_SSHD}"), true)),
        // The toolkit and NVIDIA's keyring; the Windows GPU driver is not ours to touch.
        "cuda toolkit" => Some((
            "apt-get remove -y cuda-toolkit cuda-keyring && apt-get autoremove -y".to_owned(),
            true,
        )),
        other => projneeds::undo_of(other),
    }
}

/// Where the record of what goway installed on `host` lives.
pub fn installed_path(paths: &Paths, host: &str) -> std::path::PathBuf {
    paths
        .config_dir
        .join(format!("installed-{}.json", host.to_ascii_lowercase()))
}

/// Load the record of what goway installed on `host` (empty if none).
pub fn load_installed(paths: &Paths, host: &str) -> Vec<Installed> {
    std::fs::read_to_string(installed_path(paths, host))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Add the checks whose fixes just ran to the host's record.
fn record_installed(
    paths: &Paths,
    host: &str,
    facts: &BTreeMap<String, String>,
    checks: &[Check],
    done: &[String],
) -> Result<()> {
    let mut items = load_installed(paths, host);
    let before = items.len();
    for c in checks {
        let ran = c.fix.as_ref().is_some_and(|f| done.contains(&f.command));
        if ran && !items.iter().any(|i| i.check == c.name) {
            items.push(Installed {
                check: c.name.clone(),
                cargo_home_existed: (c.name == "cargo")
                    .then(|| facts.get("cargo_home").is_none_or(|v| v != "0")),
            });
        }
    }
    if items.len() == before {
        return Ok(());
    }
    let text = serde_json::to_string_pretty(&items).map_err(|e| Error::Usage(e.to_string()))?;
    crate::config::write_atomic(&installed_path(paths, host), text.as_bytes())?;
    tracing::info!(host, items = items.len(), "recorded what doctor installed");
    Ok(())
}

/// List the root fixes with their reasons and ask once (or accept with --yes).
pub(crate) fn confirm_root(renderer: Renderer, host: &str, fixes: &[Fix], yes: bool) -> bool {
    renderer.headline(format_args!(
        "{} change(s) on {host} need administrator rights:",
        fixes.len()
    ));
    for f in fixes {
        renderer.line(format_args!("  {}\n    why: {}", f.display(), f.why));
    }
    if yes {
        return true;
    }
    match crate::render::ask(&format!(
        "Run them on {host} now? sudo there asks for {host}'s password once [y/N]: "
    )) {
        Some(answer) => matches!(answer.trim(), "y" | "Y" | "yes" | "Yes" | "YES"),
        None => false,
    }
}

/// The checks of `root_ran` that `after` (a re-check) shows fixed, and the
/// ones it does not: each fix gets its own verdict, never the session's.
type Judged<'a> = Vec<&'a (String, Fix)>;

fn judge_root<'a>(root_ran: &'a [(String, Fix)], after: &[Check]) -> (Judged<'a>, Judged<'a>) {
    root_ran.iter().partition(|(name, _)| {
        after
            .iter()
            .any(|c| c.name == *name && c.level == Level::Ok)
    })
}

fn show_applied(renderer: Renderer, host: &HostConfig, applied: &Applied) {
    for c in &applied.done {
        renderer.ok(format_args!("{}: fixed: {c}", host.name));
    }
    for c in &applied.failed {
        renderer.warn(format_args!("{}: fix failed: {c}", host.name));
    }
    if !applied.need_sudo.is_empty() {
        renderer.warn(format_args!(
            "{}: {} fix(es) need administrator rights and were not run; rerun with --rsudo (sudo on {} asks for its password once; goway never sees it)",
            host.name,
            applied.need_sudo.len(),
            host.name
        ));
    }
}

/// What the project in the current directory needs (nothing detected when
/// the directory is not in a git project: goway's own Rust-first checks).
pub(crate) fn project_needs() -> Result<projneeds::Needs> {
    let Ok(cwd) = std::env::current_dir() else {
        return Ok(projneeds::Needs::default());
    };
    let Ok(repo) = crate::repo::Repo::discover(&cwd) else {
        tracing::debug!("not in a git project; no project needs");
        return Ok(projneeds::Needs::default());
    };
    let toolchain = crate::project::Rules::load(&repo.root)?
        .map(|r| r.toolchain)
        .unwrap_or_default();
    let mut needs = projneeds::analyse(&repo.root, &toolchain)?;
    needs.prereqs.repo = repo.name;
    Ok(needs)
}

/// For a `CMake` project, add what `CMake` itself says it needs (its File API
/// replies on every helper, and with `--configure` a traced configure there).
fn add_cmake_checks(
    env: &run::Env<'_>,
    renderer: Renderer,
    config: &Config,
    configure: bool,
    reports: &mut [output::HostReport],
    reached: &mut [Probed<'_>],
) {
    let repo = crate::repo::Repo::discover(env.cwd)
        .ok()
        .filter(cmakecheck::is_cmake_project);
    let Some(repo) = repo else {
        if configure {
            renderer
                .note("--configure: this is not a CMake project (no CMakeLists.txt at its root)");
        }
        return;
    };
    let ask = cmakecheck::Ask {
        env,
        config,
        repo: &repo,
        configure,
    };
    for p in reached.iter_mut() {
        if configure {
            renderer.note(format_args!(
                "configuring on {} (a snapshot, in goway's own scratch directory)",
                p.host.name
            ));
        }
        let extra = cmakecheck::host_checks(&ask, &p.found, &p.facts);
        if !extra.is_empty() {
            p.checks.extend(extra);
            reports[p.index].outcome = output::Outcome::Checked(p.checks.clone());
        }
    }
}

/// What every host's fixes share.
struct FixCtx<'a> {
    paths: &'a Paths,
    renderer: Renderer,
    args: &'a DoctorArgs,
    config: &'a Config,
    lookup: &'a (dyn Lookup + Sync),
    prober: &'a (dyn Prober + Sync),
    settings: &'a ssh::Settings,
    needs: &'a projneeds::Needs,
    cmd: (&'a str, &'a remote::Call),
    state: &'a State,
}

/// A reachable host with what doctor found there.
struct Probed<'a> {
    host: &'a HostConfig,
    found: Found,
    facts: BTreeMap<String, String>,
    checks: Vec<Check>,
    /// Its place in the report list.
    index: usize,
}

/// Ask before root fixes run; `s` shows the exact script first. The exact
/// script is always shown before it runs, whatever the answer was.
fn confirm_plan(
    renderer: Renderer,
    host: &str,
    steps: &[(String, Fix)],
    script: &str,
    yes: bool,
    hardening: bool,
) -> bool {
    let show = || {
        for line in output::script_lines(host, script) {
            renderer.line(line);
        }
    };
    if yes {
        show();
        return true;
    }
    let mut shown = false;
    loop {
        let Some(answer) = crate::render::ask(&output::prompt_text(host, steps, hardening)) else {
            return false;
        };
        match answer.trim().to_ascii_lowercase().as_str() {
            "s" | "show" => {
                show();
                shown = true;
            }
            "y" | "yes" => {
                if !shown {
                    show();
                }
                return true;
            }
            _ => return false,
        }
    }
}

/// Apply one host's fixes (tools first, hardening only with `--harden`),
/// judge each by a re-check, record what was verified, and update `probed`.
/// Returns whether anything ran.
fn fix_host(ctx: &FixCtx<'_>, probed: &mut Probed<'_>) -> bool {
    if probed.found.kind != Kind::Unix {
        return fix_windows_host(ctx, probed);
    }
    let (renderer, args) = (ctx.renderer, ctx.args);
    let host = probed.host;
    let runner = SshFixRunner {
        found: &probed.found,
        settings: ctx.settings,
        record_in: Some(&ctx.paths.config_dir),
    };
    let (hard, tools): (Vec<Check>, Vec<Check>) = probed
        .checks
        .iter()
        .cloned()
        .partition(output::is_hardening);
    let confirm_tools = |steps: &[(String, Fix)], script: &str| {
        confirm_plan(renderer, &host.name, steps, script, args.yes, false)
    };
    let mut applied = apply_fixes(&tools, args.rsudo, &confirm_tools, &runner);
    if args.harden {
        let confirm_hard = |steps: &[(String, Fix)], script: &str| {
            confirm_plan(renderer, &host.name, steps, script, args.yes, true)
        };
        let more = apply_fixes(&hard, args.rsudo, &confirm_hard, &runner);
        applied.root_ran.extend(more.root_ran);
        applied.need_sudo.extend(more.need_sudo);
    }
    show_applied(renderer, host, &applied);
    let ran =
        !applied.done.is_empty() || !applied.failed.is_empty() || !applied.root_ran.is_empty();
    if !ran {
        return false;
    }
    // Re-check after fixing; each root fix is judged by it.
    let mut fixed: Vec<String> = applied.done.clone();
    let mut local = ctx.state.clone();
    let after = probe_host(
        ctx.config, host, &mut local, ctx.lookup, ctx.prober, ctx.cmd,
    )
    .map(|f| assess_for(f.kind, &parse_facts(&f.output), ctx.needs));
    if let Ok(after) = &after {
        let (ok, bad) = judge_root(&applied.root_ran, after);
        for (name, fix) in &ok {
            renderer.ok(format_args!("{}: fixed: {name}", host.name));
            fixed.push(fix.command.clone());
        }
        for (name, _) in &bad {
            renderer.warn(format_args!(
                "{}: not fixed: {name} (its output is above)",
                host.name
            ));
        }
    } else if !applied.root_ran.is_empty() {
        renderer.warn(format_args!(
            "{}: cannot re-check, so the root fixes are not recorded",
            host.name
        ));
    }
    if let Err(e) = record_installed(ctx.paths, &host.name, &probed.facts, &probed.checks, &fixed) {
        renderer.warn(format_args!(
            "cannot record what was installed on {}: {e}",
            host.name
        ));
    }
    if let Ok(after) = after {
        probed.checks = after;
    }
    true
}

/// [`fix_host`] for a Windows host: the installs that need no administrator
/// rights, and with `--rsudo` the ones that do (through [`crate::winadmin`]),
/// all confirmed once, each judged by a re-check and recorded for uninstall.
fn fix_windows_host(ctx: &FixCtx<'_>, probed: &mut Probed<'_>) -> bool {
    let (renderer, args) = (ctx.renderer, ctx.args);
    let host = probed.host;
    let steps = windows::plan(&probed.checks);
    let runner = windows::HostRunner {
        found: &probed.found,
        settings: ctx.settings,
        admin_user: args.windows_admin.as_deref(),
        record_in: &ctx.paths.config_dir,
    };
    let confirm = |steps: &[windows::Step]| {
        for line in output::windows_plan_lines(&host.name, steps, args.rsudo) {
            renderer.line(line);
        }
        args.yes
            || crate::render::ask(&format!("Run these on {} now? [y/N]: ", host.name))
                .is_some_and(|a| matches!(a.trim(), "y" | "Y" | "yes" | "Yes" | "YES"))
    };
    let applied = windows::apply(&steps, args.rsudo, &confirm, &runner);
    for (step, notes) in &applied.manual {
        renderer.warn(format_args!(
            "{}: {} needs administrator rights and was not run: {}",
            host.name, step.check, step.fix.why
        ));
        for note in notes {
            renderer.note(format_args!("not elevated: {note}"));
        }
        renderer.next(format_args!(
            "in an administrator PowerShell on {}: {}",
            host.name, step.fix.command
        ));
    }
    for name in &applied.failed {
        renderer.warn(format_args!(
            "{}: the step for {name} failed (its output is above)",
            host.name
        ));
    }
    if applied.ran.is_empty() && applied.failed.is_empty() {
        return false;
    }
    let mut local = ctx.state.clone();
    let after = probe_host(
        ctx.config, host, &mut local, ctx.lookup, ctx.prober, ctx.cmd,
    )
    .map(|f| assess_for(f.kind, &parse_facts(&f.output), ctx.needs));
    let mut fixed = Vec::new();
    match &after {
        Ok(after) => {
            for name in &applied.ran {
                if after
                    .iter()
                    .any(|c| c.name == *name && c.level == Level::Ok)
                {
                    renderer.ok(format_args!("{}: fixed: {name}", host.name));
                    fixed.push(name.clone());
                } else {
                    renderer.warn(format_args!("{}: not fixed: {name}", host.name));
                }
            }
        }
        Err(e) => {
            tracing::warn!(host = %host.name, error = %e, "cannot re-check the Windows host");
            renderer.warn(format_args!(
                "{}: cannot re-check, so the installs are not recorded",
                host.name
            ));
        }
    }
    if let Err(e) = record_windows(ctx.paths, &host.name, &probed.facts, &fixed) {
        renderer.warn(format_args!(
            "cannot record what was installed on {}: {e}",
            host.name
        ));
    }
    if let Ok(after) = after {
        probed.checks = after;
    }
    true
}

/// Add the Windows checks that were just fixed to the host's record, under
/// their [`windows::RECORD_PREFIX`] names.
fn record_windows(
    paths: &Paths,
    host: &str,
    facts: &BTreeMap<String, String>,
    fixed: &[String],
) -> Result<()> {
    let mut items = load_installed(paths, host);
    let before = items.len();
    for name in fixed {
        let check = format!("{}{name}", windows::RECORD_PREFIX);
        if items.iter().any(|i| i.check == check) {
            continue;
        }
        items.push(Installed {
            check,
            cargo_home_existed: (name == "rustup")
                .then(|| facts.get("cargo_home").is_none_or(|v| v != "0")),
        });
    }
    if items.len() == before {
        return Ok(());
    }
    let text = serde_json::to_string_pretty(&items).map_err(|e| Error::Usage(e.to_string()))?;
    crate::config::write_atomic(&installed_path(paths, host), text.as_bytes())?;
    tracing::info!(host, items = items.len(), "recorded what doctor installed");
    Ok(())
}

/// Turn the probes of every host into the report and the reachable hosts
/// (with their checks); also the local ssh findings of unreachable hosts.
fn collect<'a>(
    config: &Config,
    prober: &dyn Prober,
    needs: &projneeds::Needs,
    results: Vec<(&'a HostConfig, Result<Found>)>,
) -> (
    Vec<output::HostReport>,
    Vec<Probed<'a>>,
    std::collections::BTreeSet<String>,
) {
    let mut reports: Vec<output::HostReport> = Vec::new();
    let mut reached: Vec<Probed<'_>> = Vec::new();
    let mut local_ssh = std::collections::BTreeSet::new();
    for (host, result) in results {
        let found = match result {
            Ok(found) => found,
            Err(e) => {
                reports.push(output::HostReport {
                    name: host.name.clone(),
                    address: host.address.clone().unwrap_or_default(),
                    os: "?".to_owned(),
                    arch: "?".to_owned(),
                    outcome: output::Outcome::Down(down_reason(config, host, prober, &e)),
                });
                for finding in sshenv::check(
                    host.address.as_deref().unwrap_or(&host.name),
                    config.port_of(host),
                ) {
                    local_ssh.insert(finding.to_string());
                }
                continue;
            }
        };
        let facts = parse_facts(&found.output);
        let mut checks = assess_for(found.kind, &facts, needs);
        checks.extend(clock_check(&facts, found.kind != Kind::Unix));
        // An interop host has no ssh setup to check.
        let ssh_findings = if found.kind.uses_ssh() {
            sshenv::check(&found.target.address, found.target.port)
        } else {
            Vec::new()
        };
        for finding in ssh_findings {
            checks.push(Check {
                name: "local ssh".to_owned(),
                explain: None,
                level: Level::Warn,
                detail: finding.to_string(),
                fix: None,
            });
        }
        reports.push(output::HostReport {
            name: host.name.clone(),
            address: found.target.address.clone(),
            os: facts
                .get("os")
                .cloned()
                .unwrap_or_else(|| "unknown OS".to_owned()),
            arch: facts.get("arch").cloned().unwrap_or_else(|| "?".to_owned()),
            outcome: output::Outcome::Checked(checks.clone()),
        });
        reached.push(Probed {
            host,
            found,
            facts,
            checks,
            index: reports.len() - 1,
        });
    }
    (reports, reached, local_ssh)
}

/// Why `host` is down: goway's error, plus a look through Windows OpenSSH at the same address
/// when the host is a WSL helper (a Windows or interop host has no WSL side to ask about).
fn down_reason(config: &Config, host: &HostConfig, prober: &dyn Prober, error: &Error) -> String {
    let why = error.to_string();
    if Kind::of(host) != Kind::Unix {
        return why;
    }
    let address = host.address.clone().unwrap_or_else(|| host.name.clone());
    // A configured Windows ssh host at the same address already has its key pinned and its own
    // port and login; otherwise ask Windows OpenSSH on its default port as the same user.
    let configured = config.hosts.iter().find(|h| {
        h.os == Os::Windows
            && h.transport == Transport::Ssh
            && h.address.as_deref() == Some(address.as_str())
    });
    let windows = match configured {
        Some(h) => ssh::Target {
            name: h.name.clone(),
            address,
            port: config.port_of(h),
            user: h.user.clone(),
            identity: h.identity.as_ref().map(std::path::PathBuf::from),
        },
        None => ssh::Target {
            name: format!("{}-windows", host.name),
            address,
            port: wsl_down::WINDOWS_SSH_PORT,
            user: host.user.clone(),
            identity: host.identity.as_ref().map(std::path::PathBuf::from),
        },
    };
    let checks = wsl_down::diagnose(prober, &windows, config.port_of(host));
    wsl_down::describe(&why, &checks)
}

/// `goway doctor`.
#[allow(clippy::too_many_lines)] // one pass over hosts: probe, report, record, fix
pub fn doctor(
    paths: &Paths,
    renderer: Renderer,
    args: &DoctorArgs,
    lookup: &(dyn Lookup + Sync),
    prober: &(dyn Prober + Sync),
    settings: &ssh::Settings,
) -> Result<u8> {
    if args.rsudo && !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        return Err(Error::Usage(
            "--rsudo needs an interactive terminal so the host's sudo can ask for the password"
                .to_owned(),
        ));
    }
    let config = Config::load(&paths.config_file())?;
    let hosts: Vec<&HostConfig> = match &args.host {
        Some(name) => vec![config.host(name)?],
        None => config.hosts.iter().collect(),
    };
    if hosts.is_empty() {
        renderer.note("no hosts configured; add one with `goway host add NAME`");
        return Ok(1);
    }
    let needs = project_needs()?;
    renderer.note(format_args!("{}", needs.summary()));
    let names = needs.probe_names();
    let mut cmd_args = vec![config.defaults.remote_root.as_str()];
    cmd_args.extend(names.iter().map(String::as_str));
    // The repository's targets and packages are asked in the same ssh call.
    let cmd = logout::wrap(
        &mac::wrap(&needs.prereqs.wrap(&remote::invocation("doctor", &cmd_args))),
        &config.defaults.remote_root,
    );
    let call = remote::Call::new("doctor", &cmd_args);
    let mut state = State::load(&paths.state_file())?;
    let results = pool::on_hosts(&hosts, &mut state, |host, local| {
        probe_host(&config, host, local, lookup, prober, (&cmd, &call))
    });
    if let Err(e) = state.save(&paths.state_file()) {
        tracing::warn!(error = %e, "cannot cache host addresses");
    }
    let (mut reports, mut reached, local_ssh) = collect(&config, prober, &needs, results);
    add_cmake_checks(
        &run::Env {
            paths,
            lookup,
            prober,
            settings,
            cwd: &std::env::current_dir().unwrap_or_default(),
        },
        renderer,
        &config,
        args.configure,
        &mut reports,
        &mut reached,
    );
    for finding in &local_ssh {
        renderer.warn(format_args!("local ssh: {finding}"));
    }
    let print = |lines: Vec<String>| {
        for line in lines {
            renderer.line(line);
        }
    };
    if let Some(check) = &args.explain {
        print(output::explain_lines(&reports, check));
        return Ok(exit_code(&reports));
    }
    print(output::report_lines(
        &reports,
        args.all,
        renderer.is_plain(),
    ));
    print_versions(renderer, args.all, &needs, &reached);
    let now = crate::state::now_secs();
    let repo_id = std::env::current_dir()
        .ok()
        .and_then(|cwd| crate::repo::Repo::discover(&cwd).ok())
        .map(|r| r.id)
        .unwrap_or_default();
    for o in observed_versions(&needs, &reached) {
        state.record_tools(&o.host, &repo_id, now, o.versions);
    }
    if let Err(e) = state.save(&paths.state_file()) {
        tracing::warn!(error = %e, "cannot cache tool versions");
    }
    if args.fix {
        print(output::plan_lines(&reports, args.harden, args.rsudo));
        let ctx = FixCtx {
            paths,
            renderer,
            args,
            config: &config,
            lookup,
            prober,
            settings,
            needs: &needs,
            cmd: (&cmd, &call),
            state: &state,
        };
        let mut changed = false;
        for p in &mut reached {
            if fix_host(&ctx, p) {
                changed = true;
                reports[p.index].outcome = output::Outcome::Checked(p.checks.clone());
            }
        }
        if changed {
            renderer.note("after fixes:");
            print(output::report_lines(
                &reports,
                args.all,
                renderer.is_plain(),
            ));
        }
    }
    Ok(exit_code(&reports))
}

/// What each reachable host reported for the project's tools.
fn observed_versions(
    needs: &projneeds::Needs,
    reached: &[Probed<'_>],
) -> Vec<crate::drift::Observed> {
    let tools = needs.probe_names();
    reached
        .iter()
        .map(|p| crate::drift::Observed {
            host: p.host.name.clone(),
            versions: crate::drift::versions_from_facts(&p.facts, &tools),
        })
        .collect()
}

/// The versions table: with `--all`, or whenever hosts disagree.
fn print_versions(renderer: Renderer, all: bool, needs: &projneeds::Needs, reached: &[Probed<'_>]) {
    let tools = needs.probe_names();
    let observed = observed_versions(needs, reached);
    let pinned = needs.pinned_tools();
    if tools.is_empty() || observed.is_empty() {
        return;
    }
    let drift = crate::drift::drifting(&tools, &pinned, &observed);
    if !all && drift.is_empty() {
        return;
    }
    let laptop = crate::drift::laptop_versions(&tools);
    renderer.line("");
    for line in crate::drift::table_lines(&tools, &pinned, &observed, &laptop, renderer.is_plain())
    {
        renderer.line(line);
    }
    if !drift.is_empty() {
        let names: Vec<&str> = drift.iter().map(|(t, _)| t.as_str()).collect();
        renderer.line(format!(
            "{} differ across the hosts; builds may behave differently depending on where they run.",
            names.join(", ")
        ));
    }
}

/// 1 when a host is unreachable or has a failing check, else 0.
fn exit_code(reports: &[output::HostReport]) -> u8 {
    let bad = reports.iter().any(|r| match &r.outcome {
        output::Outcome::Down(_) => true,
        output::Outcome::Checked(c) => c.iter().any(|c| c.level == Level::Fail),
    });
    u8::from(bad)
}

#[cfg(test)]
mod tests {
    use super::*;
    /// A fix runner pointed at a host that cannot be reached, recording into `dir`.
    fn recording_runner<'a>(
        found: &'a Found,
        settings: &'a ssh::Settings,
        dir: &'a std::path::Path,
    ) -> SshFixRunner<'a> {
        SshFixRunner {
            found,
            settings,
            record_in: Some(dir),
        }
    }

    fn unreachable_found() -> (Found, ssh::Settings) {
        let found = Found {
            kind: crate::transport::Kind::Unix,
            target: ssh::Target {
                name: "helios".to_owned(),
                address: "192.0.2.1".to_owned(),
                port: 22,
                user: None,
                identity: None,
            },
            source: crate::resolve::Source::Cached,
            output: String::new(),
        };
        let settings = ssh::Settings {
            known_hosts: std::path::PathBuf::from("/nonexistent/known_hosts"),
            control_dir: None,
            connect_timeout_secs: 1,
        };
        (found, settings)
    }

    // frob:tests crates/goway/src/doctor.rs::SshFixRunner
    #[test]
    fn a_fix_is_recorded_in_the_change_log_before_it_runs() {
        let dir = tempfile::tempdir().unwrap();
        let (found, settings) = unreachable_found();
        let runner = recording_runner(&found, &settings, dir.path());
        // The host is unreachable, so the fix fails; the record is there all the same.
        let _ = runner.run("true", false);
        let rows = crate::changelog::rows(dir.path()).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].what.contains("helios") && rows[0].what.contains("doctor --fix"));
    }

    // frob:tests crates/goway/src/doctor.rs::SshFixRunner
    #[test]
    fn a_fix_that_cannot_be_recorded_does_not_run() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(crate::changelog::FILE_NAME)).unwrap();
        let (found, settings) = unreachable_found();
        let runner = recording_runner(&found, &settings, dir.path());
        // The early return is the guard: a failed record is an error log and a `false`.
        assert!(!runner.run("touch /tmp/never", false));
    }

    use std::cell::RefCell;

    // frob:tests crates/goway/src/doctor.rs::clock_check
    #[test]
    fn doctor_warns_about_a_clock_over_two_seconds_off_with_the_os_fix() {
        let fact = |ms: &str| BTreeMap::from([("clock_offset_ms".to_owned(), ms.to_owned())]);
        assert!(clock_check(&fact("1500"), false).is_none());
        assert!(clock_check(&BTreeMap::new(), false).is_none());
        let c = clock_check(&fact("-4000"), false).unwrap();
        assert_eq!((c.name.as_str(), c.level), ("clock", Level::Warn));
        assert!(c.detail.contains("4.0 s behind") && c.detail.contains("sudo hwclock -s"));
        assert!(
            clock_check(&fact("90000"), true)
                .unwrap()
                .detail
                .contains("resync")
        );
    }

    // frob:tests crates/goway/src/doctor.rs::host_checks
    #[test]
    fn proxy_variable_names_are_shown_and_a_host_without_any_says_nothing() {
        let mut f = BTreeMap::new();
        assert!(host_checks(&f).iter().all(|c| c.name != "proxy"));
        f.insert("proxy_vars".to_owned(), "HTTPS_PROXY,no_proxy".to_owned());
        let checks = host_checks(&f);
        let c = checks
            .iter()
            .find(|c| c.name == "proxy")
            .expect("a proxy check");
        assert_eq!(c.level, Level::Ok);
        assert!(c.detail.contains("HTTPS_PROXY,no_proxy"), "{}", c.detail);
    }

    fn facts(missing: &[&str]) -> BTreeMap<String, String> {
        let mut f = BTreeMap::new();
        for t in [
            "bash",
            "git",
            "tar",
            "flock",
            "setsid",
            "cc",
            "curl",
            "rustup",
            "cargo",
            "cargo-nextest",
            "sccache",
            "apt-get",
        ] {
            let v = if missing.contains(&t) {
                String::new()
            } else {
                format!("{t} 1.0")
            };
            f.insert(format!("tool.{t}"), v);
        }
        f.insert("arch".to_owned(), "x86_64".to_owned());
        f.insert("disk_free".to_owned(), (500u64 << 30).to_string());
        f.insert("password_auth".to_owned(), "no".to_owned());
        f
    }

    // frob:tests crates/goway/src/doctor.rs::assess_project
    #[test]
    fn a_cargo_config_naming_clang_and_mold_makes_doctor_check_and_plan_both() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".cargo")).unwrap();
        std::fs::write(
            dir.path().join(".cargo/config.toml"),
            "[target.x86_64-unknown-linux-gnu]\nlinker = \"clang\"\nrustflags = [\"-C\", \"link-arg=-fuse-ld=mold\"]\n",
        )
        .unwrap();
        let needs = projneeds::Needs {
            linking: crate::ecotools::cargo_linking(dir.path(), None, &|_| None),
            ..Default::default()
        };
        assert!(needs.probe_names().contains(&"clang".to_owned()));
        assert!(needs.probe_names().contains(&"mold".to_owned()));
        // A helper with gcc only: no clang, no mold.
        let mut f = facts(&[]);
        f.insert("want.clang".to_owned(), String::new());
        f.insert("want.mold".to_owned(), String::new());
        let checks = assess_project(&f, &needs);
        let fixes: Vec<String> = ["clang", "mold"]
            .iter()
            .map(|name| {
                let c = checks.iter().find(|c| c.name == *name).unwrap();
                assert_eq!(c.level, Level::Fail, "{name}");
                assert!(!c.detail.contains("--env"), "the detail stays short");
                assert!(
                    c.explain
                        .as_deref()
                        .unwrap()
                        .contains("--env CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=cc")
                );
                let fix = c.fix.clone().unwrap();
                assert!(fix.root);
                fix.command
            })
            .collect();
        assert_eq!(
            fixes,
            [
                "apt-get update && apt-get install -y clang",
                "apt-get update && apt-get install -y mold"
            ]
        );
        // One sudo session runs both, and both are recorded for uninstall by
        // check name only (system packages are listed, never removed).
        assert!(projneeds::is_package_check("clang"));
        // A helper that has them is fine.
        f.insert("want.clang".to_owned(), "clang version 18.1.3".to_owned());
        f.insert("want.mold".to_owned(), "mold 2.30.0".to_owned());
        assert!(
            assess_project(&f, &needs)
                .iter()
                .all(|c| c.level == Level::Ok)
        );
    }

    // frob:tests crates/goway/src/doctor.rs::darwin_checks
    #[test]
    fn a_mac_missing_gnu_tools_gets_a_per_user_brew_fix() {
        let mut f = facts(&["flock", "setsid", "cargo-nextest"]);
        f.insert("kernel".to_owned(), "Darwin".to_owned());
        f.insert("gnu_missing".to_owned(), "find,tar".to_owned());
        f.insert("tool.brew".to_owned(), "Homebrew 4".to_owned());
        let checks = assess(&f);
        let gnu = checks
            .iter()
            .find(|c| c.name == "Homebrew GNU tools")
            .unwrap();
        assert_eq!(gnu.level, Level::Fail);
        assert!(
            gnu.detail.contains("flock, setsid, find, tar"),
            "{}",
            gnu.detail
        );
        let fix = gnu.fix.as_ref().unwrap();
        assert!(!fix.root && fix.command.starts_with("brew install coreutils"));
        let nextest = checks.iter().find(|c| c.name == "cargo-nextest").unwrap();
        assert_eq!(
            nextest.fix.as_ref().unwrap().command,
            "brew install cargo-nextest"
        );
        assert!(
            checks
                .iter()
                .all(|c| c.fix.as_ref().is_none_or(|x| !x.root))
        );
        // Without Homebrew there is nothing goway may run: it explains.
        f.remove("tool.brew");
        let checks = assess(&f);
        let gnu = checks
            .iter()
            .find(|c| c.name == "Homebrew GNU tools")
            .unwrap();
        assert!(gnu.fix.is_none() && gnu.detail.contains("https://brew.sh"));
    }

    #[test]
    fn an_unknown_arch_never_reaches_a_command() {
        let mut f = facts(&["cargo-nextest", "sccache"]);
        f.insert("arch".to_owned(), "x86_64; touch /tmp/pwned".to_owned());
        for c in assess(&f) {
            if let Some(fix) = &c.fix {
                assert!(!fix.command.contains("pwned"), "{}", fix.command);
            }
        }
    }

    #[test]
    fn tool_undo_removes_only_empty_directories_it_left() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let bin = home.join(".cargo/bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("sccache"), "x").unwrap();
        let (undo, root) = undo_of("sccache").unwrap();
        assert!(!root);
        let ok = std::process::Command::new("sh")
            .args(["-c", &undo])
            .env("HOME", home)
            .env_remove("CARGO_HOME")
            .status()
            .unwrap();
        assert!(ok.success());
        assert!(!home.join(".cargo").exists(), "empty dirs removed");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("cargo"), "x").unwrap();
        std::fs::write(bin.join("cargo-nextest"), "x").unwrap();
        let (undo, _) = undo_of("cargo-nextest").unwrap();
        let ok = std::process::Command::new("sh")
            .args(["-c", &undo])
            .env("HOME", home)
            .env_remove("CARGO_HOME")
            .status()
            .unwrap();
        assert!(ok.success());
        assert!(bin.join("cargo").exists(), "other tools stay");
    }

    #[test]
    fn records_never_supply_the_command_uninstall_runs() {
        // A hostile or legacy record: the stored undo and root are ignored.
        let items: Vec<Installed> = serde_json::from_str(
            r#"[{"check":"evil","undo":"touch /tmp/pwned","root":true},
                {"check":"sccache","undo":"touch /tmp/pwned","root":true},
                {"check":"tar","undo":"touch /tmp/pwned","root":false}]"#,
        )
        .unwrap();
        assert_eq!(items[0].undo(), Undo::Unknown);
        let (command, root) = undo_of("sccache").unwrap();
        assert_eq!(items[1].undo(), Undo::Run { command, root });
        assert_eq!(items[2].undo(), Undo::KeepPackage);
        let text = serde_json::to_string(&items[1]).unwrap();
        assert!(!text.contains("undo") && !text.contains("pwned"), "{text}");
    }

    #[test]
    fn cargo_undo_spares_a_cargo_home_that_existed_before() {
        let item = |existed| Installed {
            check: "cargo".to_owned(),
            cargo_home_existed: existed,
        };
        assert_eq!(item(Some(true)).undo(), Undo::KeepCargo);
        assert_eq!(item(None).undo(), Undo::KeepCargo, "old records are safe");
        assert!(matches!(item(Some(false)).undo(), Undo::Run { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn sshd_reload_falls_back_to_the_sshd_unit_and_never_aborts() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log");
        let fake = dir.path().join("systemctl");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\necho \"$@\" >> {}\n[ \"$2\" = sshd ]\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let run = |script: &str| {
            std::process::Command::new("sh")
                .args(["-c", script])
                .env("PATH", format!("{}:/usr/bin:/bin", dir.path().display()))
                .output()
                .unwrap()
        };
        assert!(run(RELOAD_SSHD).status.success());
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "reload ssh\nreload sshd\n"
        );
        // A host where neither unit reloads still lets `set -e` scripts go on.
        std::fs::write(&fake, "#!/bin/sh\nexit 1\n").unwrap();
        let out = run(&format!("set -e\n{RELOAD_SSHD}\necho later"));
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("later"));
        // The write command checks the configuration before reloading.
        let fix = sshd_keys_only_command();
        assert!(
            fix.find("-t").unwrap() < fix.find("systemctl reload").unwrap(),
            "{fix}"
        );
        assert!(
            undo_of("sshd password login")
                .unwrap()
                .0
                .contains("reload sshd")
        );
    }

    // frob:tests crates/goway/src/doctor.rs::push_wsl_hardware
    // frob:tests crates/goway/src/doctor.rs::host_checks
    #[test]
    fn doctor_compares_what_wsl_got_with_the_laptop_and_names_the_tune_command() {
        let g = 1u64 << 30;
        let run = |wsl_ram: u64| {
            let mut f = facts(&[]);
            f.insert("static".to_owned(), "1".to_owned());
            f.insert("wsl".to_owned(), "1".to_owned());
            f.insert("winhw".to_owned(), format!("{};16", 16 * g));
            f.insert("swap_total".to_owned(), (6 * g).to_string());
            f.insert("mem_total".to_owned(), wsl_ram.to_string());
            f.insert("cores".to_owned(), "16".to_owned());
            assess(&f)
                .into_iter()
                .find(|c| c.name == "wsl size")
                .unwrap()
        };
        let low = run(g * 36 / 10);
        assert_eq!(low.level, Level::Warn);
        assert!(
            low.detail
                .contains("goway-setup.exe tune --memory 12GB --swap 6GB --processors 14")
                && low.detail.contains("4 GiB or 25%"),
            "{}",
            low.detail
        );
        assert_eq!(run(10 * g).level, Level::Ok);
    }

    // frob:tests crates/goway/src/doctor.rs::cuda_fix
    // frob:tests crates/goway/src/doctor.rs::undo_of
    #[test]
    fn a_visible_nvidia_gpu_without_cuda_gets_the_wsl_toolkit_fix_and_an_undo() {
        let run = |nvcc: &str, apt: bool| {
            let mut f = facts(if apt { &[] } else { &["apt-get"] });
            f.insert("static".to_owned(), "1".to_owned());
            f.insert("wsl".to_owned(), "1".to_owned());
            f.insert(
                "gpu.0".to_owned(),
                "nvidia|RTX 3060|12288|555.42|12.5".to_owned(),
            );
            f.insert("nvcc".to_owned(), nvcc.to_owned());
            assess(&f)
                .into_iter()
                .find(|c| c.name == "cuda toolkit")
                .unwrap()
        };
        let c = run("0", true);
        assert_eq!(c.level, Level::Warn);
        let fix = c.fix.unwrap();
        assert!(fix.root && fix.command.contains("wsl-ubuntu/x86_64/cuda-keyring"));
        assert!(
            fix.command.contains("install -y cuda-toolkit"),
            "never the driver package"
        );
        assert!(!fix.command.contains("cuda-drivers") && !fix.command.contains("install -y cuda "));
        let (undo, root) = undo_of("cuda toolkit").unwrap();
        assert!(root && undo.contains("remove -y cuda-toolkit"));
        assert!(run("0", false).fix.is_none(), "no apt, no automatic fix");
        assert_eq!(run("1", true).level, Level::Ok);
    }

    // frob:tests crates/goway/src/doctor.rs::host_checks
    #[test]
    fn doctor_warns_when_wsl_interop_is_elevated_and_reports_off_as_safe() {
        let level = |interop: &str| {
            let mut f = facts(&[]);
            f.insert("static".to_owned(), "1".to_owned());
            f.insert("wsl".to_owned(), "1".to_owned());
            f.insert("interop".to_owned(), interop.to_owned());
            assess(&f).into_iter().find(|c| c.name == "wsl interop")
        };
        let bad = level("elevated").unwrap();
        assert_eq!(bad.level, Level::Warn);
        assert!(bad.detail.contains("SECURITY") && bad.detail.contains("wsl --shutdown"));
        assert_eq!(level("off").unwrap().level, Level::Ok);
        assert_eq!(level("limited").unwrap().level, Level::Ok);
        assert!(level("bogus").is_none());
    }

    // frob:tests crates/goway/src/doctor.rs::host_checks
    #[test]
    fn doctor_flags_a_gpu_invisible_to_wsl() {
        let mut f = facts(&[]);
        f.insert("static".to_owned(), "1".to_owned());
        f.insert("wsl".to_owned(), "1".to_owned());
        f.insert("winvideo".to_owned(), "NVIDIA GeForce RTX 3060".to_owned());
        let gpu = assess(&f).into_iter().find(|c| c.name == "gpu").unwrap();
        assert_eq!(gpu.level, Level::Warn);
        assert!(gpu.detail.contains("cannot see"), "{}", gpu.detail);
        assert!(gpu.detail.contains("wsl --shutdown"), "{}", gpu.detail);
        f.insert(
            "gpu.0".to_owned(),
            "nvidia|RTX 3060|12288|555.1|12.5".to_owned(),
        );
        let gpu = assess(&f).into_iter().find(|c| c.name == "gpu").unwrap();
        assert_eq!(gpu.level, Level::Ok);
    }

    #[test]
    fn healthy_host_is_all_ok() {
        assert!(assess(&facts(&[])).iter().all(|c| c.level == Level::Ok));
    }

    #[test]
    fn missing_nextest_names_the_exact_fix() {
        let checks = assess(&facts(&["cargo-nextest"]));
        let c = checks.iter().find(|c| c.name == "cargo-nextest").unwrap();
        assert_eq!(c.level, Level::Warn);
        let fix = c.fix.as_ref().unwrap();
        assert!(!fix.root);
        assert!(
            fix.command
                .contains("682c21b777c333e96fd532e114d3a5a894e0729ab88d94c0a9f20f8419695428")
        );
        assert!(fix.command.contains("sha256sum -c"), "{}", fix.command);
        assert!(!fix.command.contains("latest"), "{}", fix.command);
    }

    #[test]
    fn missing_linker_and_password_login_need_root() {
        let mut f = facts(&["cc"]);
        f.insert("password_auth".to_owned(), "default-yes".to_owned());
        let checks = assess(&f);
        let cc = checks.iter().find(|c| c.name == "cc (linker)").unwrap();
        assert_eq!(cc.level, Level::Fail);
        let fix = cc.fix.as_ref().unwrap();
        assert!(fix.root && fix.command.contains("apt-get install -y build-essential"));
        assert!(fix.why.contains("root"));
        assert_eq!(
            fix.display(),
            "sudo bash -c 'apt-get update && apt-get install -y build-essential'"
        );
        let pw = checks
            .iter()
            .find(|c| c.name == "sshd password login")
            .unwrap();
        assert!(pw.fix.as_ref().unwrap().root);
    }

    // frob:tests crates/goway/src/doctor.rs::apply_fixes
    #[test]
    fn each_root_fix_is_its_own_step_with_its_own_result_and_one_apt_update() {
        let mut f = facts(&["curl"]);
        f.insert("want.clang".to_owned(), String::new());
        f.insert("want.mold".to_owned(), String::new());
        let steps = vec![
            (
                "clang".to_owned(),
                Fix {
                    command: "apt-get update && apt-get install -y clang".to_owned(),
                    root: true,
                    why: String::new(),
                },
            ),
            (
                "mold".to_owned(),
                Fix {
                    command: "apt-get update && apt-get install -y mold".to_owned(),
                    root: true,
                    why: String::new(),
                },
            ),
        ];
        let script = root_script(&steps);
        assert_eq!(script.matches("apt-get update ||").count(), 1, "{script}");
        assert!(script.contains("apt-get install -y clang"), "{script}");
        assert!(script.contains("apt-get install -y mold"), "{script}");
        assert!(
            script.contains("step failed (exit $rc):\" 'mold'"),
            "{script}"
        );
        assert!(
            script.starts_with("set +e"),
            "a failing step never stops the rest"
        );
        // Run it for real with apt-get faked: mold has no package.
        #[cfg(unix)]
        {
            let dir = tempfile::tempdir().unwrap();
            let apt = dir.path().join("apt-get");
            std::fs::write(
                &apt,
                "#!/bin/sh\n[ \"$1\" = update ] && exit 0\ncase \"$*\" in *mold*) echo 'E: Unable to locate package mold' >&2; exit 100;; esac\necho installed \"$*\"\n",
            )
            .unwrap();
            std::fs::set_permissions(&apt, std::os::unix::fs::PermissionsExt::from_mode(0o755))
                .unwrap();
            let out = std::process::Command::new("bash")
                .arg("-c")
                .arg(&script)
                .env("PATH", format!("{}:/usr/bin:/bin", dir.path().display()))
                .output()
                .unwrap();
            let text = String::from_utf8_lossy(&out.stdout);
            assert!(text.contains("step ok: clang"), "{text}");
            assert!(text.contains("step failed (exit 100): mold"), "{text}");
            assert!(!text.contains("step failed (exit 100): clang"), "{text}");
            assert!(text.contains("1 of 2 steps failed"), "{text}");
        }
    }

    // frob:tests crates/goway/src/doctor/projneeds.rs::fix_for
    #[test]
    fn mold_is_a_package_where_one_exists_and_a_pinned_user_install_where_not() {
        let want = |os: Option<&str>| {
            let mut f = facts(&[]);
            f.insert("want.mold".to_owned(), String::new());
            if let Some(os) = os {
                f.insert("os".to_owned(), os.to_owned());
            }
            let needs = projneeds::Needs {
                linking: vec![crate::ecotools::CargoLinking {
                    triple: "x86_64-unknown-linux-gnu".to_owned(),
                    linker: None,
                    backend: Some("mold".to_owned()),
                    source: "test".to_owned(),
                }],
                ..Default::default()
            };
            assess_project(&f, &needs)
                .into_iter()
                .find(|c| c.name == "mold")
                .and_then(|c| c.fix)
                .unwrap()
        };
        for old in ["Ubuntu 20.04.6 LTS", "Debian GNU/Linux 11 (bullseye)"] {
            let fix = want(Some(old));
            assert!(!fix.root, "{old}: {}", fix.command);
            assert!(
                fix.command
                    .contains("6ff270c9bf07d2bec5c98aa324eb7c4daf6a1a4d815c05ff1708049616047855")
            );
            assert!(fix.command.contains("ld.mold"), "{}", fix.command);
        }
        for new in [
            "Ubuntu 22.04.4 LTS",
            "Ubuntu 24.04 LTS",
            "Debian GNU/Linux 12 (bookworm)",
        ] {
            let fix = want(Some(new));
            assert!(
                fix.root && fix.command.contains("apt-get install -y mold"),
                "{new}"
            );
        }
        assert!(
            want(None).root,
            "an unknown distribution is asked for the package"
        );
        assert!(!projneeds::is_package_check("mold"));
        assert!(projneeds::undo_of("mold").is_some());
    }

    struct Recorder(RefCell<Vec<(String, bool)>>);
    impl FixRunner for Recorder {
        fn run(&self, command: &str, sudo: bool) -> bool {
            self.0.borrow_mut().push((command.to_owned(), sudo));
            true
        }
    }

    #[test]
    fn root_fixes_never_run_without_sudo() {
        let checks = assess(&facts(&["cc", "cargo-nextest", "sccache"]));
        let rec = Recorder(RefCell::new(Vec::new()));
        let applied = apply_fixes(&checks, false, &|_, _| true, &rec);
        assert_eq!(applied.done.len(), 2, "nextest and sccache run as the user");
        assert!(rec.0.borrow().iter().all(|(_, sudo)| !sudo));
        assert_eq!(applied.need_sudo.len(), 1);
        assert!(applied.need_sudo[0].command.contains("build-essential"));
        assert!(applied.need_sudo[0].why.contains("root"));

        let rec = Recorder(RefCell::new(Vec::new()));
        let applied = apply_fixes(&checks, true, &|_, _| true, &rec);
        assert!(applied.need_sudo.is_empty());
        let calls = rec.0.borrow();
        assert!(calls[0].1, "root fixes first, under sudo");
        assert_eq!(calls.len(), 3);
    }

    #[test]
    fn root_fixes_need_confirmation_and_share_one_sudo_session() {
        let mut f = facts(&["cc", "curl"]);
        f.insert("password_auth".to_owned(), "default-yes".to_owned());
        let checks = assess(&f);
        let rec = Recorder(RefCell::new(Vec::new()));
        let declined = apply_fixes(&checks, true, &|_, _| false, &rec);
        assert!(
            rec.0.borrow().is_empty(),
            "nothing runs when the user says no"
        );
        assert_eq!(declined.need_sudo.len(), 3);
        let applied = apply_fixes(&checks, true, &|_, _| true, &rec);
        let calls = rec.0.borrow();
        let sudo_calls: Vec<&(String, bool)> = calls.iter().filter(|(_, s)| *s).collect();
        assert_eq!(sudo_calls.len(), 1, "one sudo session for all root fixes");
        let script = &sudo_calls[0].0;
        assert_eq!(
            script.matches("apt-get update ||").count(),
            1,
            "one update for the whole session: {script}"
        );
        assert!(!script.contains("apt-get update &&"), "{script}");
        assert!(
            script.contains("step failed (exit $rc):\" 'cc (linker)'"),
            "{script}"
        );
        assert_eq!(applied.root_ran.len(), 3);
    }

    #[test]
    fn other_package_managers_and_arm() {
        let mut f = facts(&["cc", "apt-get", "cargo-nextest"]);
        f.insert("tool.dnf".to_owned(), "dnf 4".to_owned());
        f.insert("arch".to_owned(), "aarch64".to_owned());
        let checks = assess(&f);
        assert!(checks.iter().any(|c| {
            c.fix
                .as_ref()
                .is_some_and(|x| x.command == "dnf install -y gcc")
        }));
        assert!(checks.iter().any(|c| {
            c.fix
                .as_ref()
                .is_some_and(|x| x.command.contains("aarch64-unknown-linux-gnu"))
        }));
    }

    // frob:tests crates/goway/src/doctor.rs::filesystem_checks
    #[test]
    fn doctor_reports_the_root_file_system_and_unusual_temp_dirs() {
        let mut f = facts(&[]);
        f.insert("root_fs".to_owned(), "ext4".to_owned());
        f.insert("root_free".to_owned(), (50u64 << 30).to_string());
        f.insert("tmp_fs".to_owned(), "tmpfs".to_owned());
        f.insert("tmp_size".to_owned(), (4u64 << 30).to_string());
        f.insert("tmp_noexec".to_owned(), "1".to_owned());
        let checks = filesystem_checks(&f);
        let root = checks.iter().find(|c| c.name == "goway root").unwrap();
        assert_eq!(root.level, Level::Ok);
        assert!(
            root.detail.contains("ext4") && root.detail.contains("50.0 GiB"),
            "{}",
            root.detail
        );
        let tmp = checks.iter().find(|c| c.name == "temp dir").unwrap();
        assert!(
            tmp.detail.contains("noexec") && tmp.detail.contains("its own root"),
            "{}",
            tmp.detail
        );

        f.insert("root_case_insensitive".to_owned(), "1".to_owned());
        let c = filesystem_checks(&f);
        assert!(c.iter().any(|c| c.name == "goway root"
            && c.level == Level::Warn
            && c.detail.contains("ignores case")));
        f.insert("root_noexec".to_owned(), "1".to_owned());
        assert!(
            filesystem_checks(&f)
                .iter()
                .any(|c| c.name == "goway root" && c.level == Level::Fail)
        );
        f.insert("root_noexec".to_owned(), "0".to_owned());
        f.insert("root_case_insensitive".to_owned(), "0".to_owned());
        f.insert("root_fs".to_owned(), "nfs4".to_owned());
        assert!(filesystem_checks(&f).iter().any(|c| c.name == "goway root"
            && c.level == Level::Warn
            && c.detail.contains("network")));
    }
}
