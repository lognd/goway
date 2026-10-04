//! Golden tests for what `goway doctor` prints: one host, three hosts with
//! shared problems, `--all`, `--explain`, and the fix plan with and without
//! `--harden`. When the wording changes the diff shows it here.

use goway::doctor::output::{
    HostReport, Outcome, explain_lines, plan_lines, prompt_text, report_lines, script_lines,
};
use goway::doctor::{Check, Fix, Level};

fn ok(name: &str, detail: &str) -> Check {
    Check {
        name: name.to_owned(),
        level: Level::Ok,
        detail: detail.to_owned(),
        fix: None,
        explain: None,
    }
}

fn apt(name: &str, pkg: &str, level: Level, detail: &str, explain: Option<&str>) -> Check {
    Check {
        name: name.to_owned(),
        level,
        detail: detail.to_owned(),
        fix: Some(Fix {
            command: format!("apt-get update && apt-get install -y {pkg}"),
            root: true,
            why: format!("{name} is needed by cargo's linker setup; system packages need root"),
        }),
        explain: explain.map(str::to_owned),
    }
}

fn password(level: Level) -> Check {
    Check {
        name: "sshd password login".to_owned(),
        level,
        detail: if level == Level::Ok {
            "disabled (keys only)".to_owned()
        } else {
            "allowed (default-yes); goway needs keys only".to_owned()
        },
        fix: (level != Level::Ok).then(|| Fix {
            command: "printf 'PasswordAuthentication no\\n' > /etc/ssh/sshd_config.d/10-goway-keys-only.conf"
                .to_owned(),
            root: true,
            why: "password logins widen the attack surface".to_owned(),
        }),
        explain: None,
    }
}

fn host(name: &str, os: &str, checks: Vec<Check>) -> HostReport {
    HostReport {
        name: name.to_owned(),
        address: format!("192.0.2.{}", name.len()),
        os: os.to_owned(),
        arch: "x86_64".to_owned(),
        outcome: Outcome::Checked(checks),
    }
}

const LINKER_TEXT: &str = "cargo's configuration names this. To run once without installing it: goway run --env CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=cc --env \"RUSTFLAGS=-C link-arg=-fuse-ld=lld\" -- ...";

fn clang(level: Level) -> Check {
    if level == Level::Ok {
        return ok("clang", "clang version 18.1.3");
    }
    apt(
        "clang",
        "clang",
        level,
        "missing; cargo's linker for x86_64-unknown-linux-gnu",
        Some(LINKER_TEXT),
    )
}

fn mold(level: Level) -> Check {
    if level == Level::Ok {
        return ok("mold", "mold 2.30.0");
    }
    apt(
        "mold",
        "mold",
        level,
        "missing; cargo's -fuse-ld backend for x86_64-unknown-linux-gnu",
        Some(LINKER_TEXT),
    )
}

fn fleet() -> Vec<HostReport> {
    let common = |extra: Vec<Check>| {
        let mut v = vec![
            ok("bash", "GNU bash 5.2"),
            ok("git", "git 2.43.0"),
            ok("cargo", "cargo 1.90.0"),
        ];
        v.extend(extra);
        v
    };
    vec![
        host(
            "helios",
            "Ubuntu 24.04",
            common(vec![
                clang(Level::Fail),
                mold(Level::Fail),
                password(Level::Warn),
            ]),
        ),
        host(
            "apex",
            "Ubuntu 24.04",
            common(vec![
                clang(Level::Fail),
                mold(Level::Fail),
                password(Level::Ok),
            ]),
        ),
        host(
            "nova",
            "Debian 12",
            common(vec![
                clang(Level::Ok),
                mold(Level::Fail),
                password(Level::Ok),
            ]),
        ),
    ]
}

fn text(lines: &[String]) -> String {
    lines.join("\n")
}

#[test]
fn one_host_with_nothing_wrong_is_one_line() {
    let h = host(
        "helios",
        "Ubuntu 24.04",
        vec![ok("bash", "GNU bash 5.2"), ok("git", "git 2.43.0")],
    );
    assert_eq!(
        text(&report_lines(&[h], false, false)),
        "helios (192.0.2.6, Ubuntu 24.04, x86_64): all 2 ok"
    );
}

#[test]
fn problems_are_grouped_across_hosts_and_passing_checks_stay_hidden() {
    let got = text(&report_lines(&fleet(), false, false));
    assert_eq!(
        got,
        r"helios (192.0.2.6, Ubuntu 24.04, x86_64): 3 problems, 3 ok
apex (192.0.2.4, Ubuntu 24.04, x86_64): 2 problems, 4 ok
nova (192.0.2.4, Debian 12, x86_64): 1 problem, 5 ok

status  check                detail                                                          hosts
FAIL    clang                missing; cargo's linker for x86_64-unknown-linux-gnu            helios, apex
FAIL    mold                 missing; cargo's -fuse-ld backend for x86_64-unknown-linux-gnu  all 3 hosts
WARN    sshd password login  allowed (default-yes); goway needs keys only                    helios

goway doctor --fix applies the fixes; goway doctor --explain CHECK says more about one check."
    );
}

#[test]
fn all_adds_the_passing_rows_after_the_problems() {
    let got = text(&report_lines(&fleet(), true, false));
    assert_eq!(
        got,
        r"helios (192.0.2.6, Ubuntu 24.04, x86_64): 3 problems, 3 ok
apex (192.0.2.4, Ubuntu 24.04, x86_64): 2 problems, 4 ok
nova (192.0.2.4, Debian 12, x86_64): 1 problem, 5 ok

status  check                detail                                                          hosts
FAIL    clang                missing; cargo's linker for x86_64-unknown-linux-gnu            helios, apex
FAIL    mold                 missing; cargo's -fuse-ld backend for x86_64-unknown-linux-gnu  all 3 hosts
WARN    sshd password login  allowed (default-yes); goway needs keys only                    helios
ok      bash                 GNU bash 5.2                                                    all 3 hosts
ok      cargo                cargo 1.90.0                                                    all 3 hosts
ok      clang                clang version 18.1.3                                            nova
ok      git                  git 2.43.0                                                      all 3 hosts
ok      sshd password login  disabled (keys only)                                            apex, nova

goway doctor --fix applies the fixes; goway doctor --explain CHECK says more about one check."
    );
}

#[test]
fn an_unreachable_host_is_one_line() {
    let mut hosts = fleet();
    hosts.push(HostReport {
        name: "orion".to_owned(),
        address: String::new(),
        os: "?".to_owned(),
        arch: "?".to_owned(),
        outcome: Outcome::Down("ssh: connection refused".to_owned()),
    });
    let got = text(&report_lines(&hosts[3..], false, false));
    assert_eq!(got, "orion: unreachable: ssh: connection refused");
}

#[test]
fn explain_gives_the_long_text_for_one_check_only() {
    let got = text(&explain_lines(&fleet(), "mold"));
    assert_eq!(
        got,
        r#"FAIL mold: missing; cargo's -fuse-ld backend for x86_64-unknown-linux-gnu  [helios, apex, nova]
  cargo's configuration names this. To run once without installing it: goway run --env CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=cc --env "RUSTFLAGS=-C link-arg=-fuse-ld=lld" -- ...
  fix: install mold with the system package manager (administrator rights)
  why: mold is needed by cargo's linker setup; system packages need root
  exact: sudo bash -c 'apt-get update && apt-get install -y mold'"#
    );
    let none = text(&explain_lines(&fleet(), "nonsense"));
    assert_eq!(
        none,
        "no check named `nonsense`; the checks are: bash, git, cargo, clang, mold, sshd password login"
    );
}

#[test]
fn the_plan_lists_each_fix_once_and_hardening_apart_without_harden() {
    let got = text(&plan_lines(&fleet(), false, true));
    assert_eq!(
        got,
        r"Fixes:
  clang: install clang with the system package manager (administrator rights)  on helios, apex
  mold: install mold with the system package manager (administrator rights)  on helios, apex, nova
Optional hardening (not applied; rerun with --harden --rsudo):
  sshd password login: turn off ssh password login (keys only)  on helios"
    );
}

#[test]
fn with_harden_the_hardening_is_still_its_own_list() {
    let got = text(&plan_lines(&fleet(), true, true));
    assert_eq!(
        got,
        r"Fixes:
  clang: install clang with the system package manager (administrator rights)  on helios, apex
  mold: install mold with the system package manager (administrator rights)  on helios, apex, nova
Optional hardening (applied now, asked separately):
  sshd password login: turn off ssh password login (keys only)  on helios"
    );
}

#[test]
fn the_prompt_names_the_host_and_what_changes_and_the_script_is_shown_whole() {
    let steps: Vec<(String, Fix)> = ["clang", "mold"]
        .iter()
        .map(|n| {
            let c = apt(n, n, Level::Fail, "missing", None);
            (c.name, c.fix.unwrap())
        })
        .collect();
    let prompt = prompt_text("helios", &steps, false);
    assert_eq!(
        prompt,
        "On helios, as administrator, goway will:\n  - install clang with the system package manager (administrator rights)\n  - install mold with the system package manager (administrator rights)\nsudo on helios asks for its password (goway never sees it). Go ahead? [s]how exact commands / [y]es / [N]o: "
    );
    let script = "set +e\napt-get install -y clang";
    let shown = text(&script_lines("helios", script));
    assert_eq!(
        shown,
        "exact commands for helios, run as root in one sudo session:\n    set +e\n    apt-get install -y clang"
    );
}
