//! The journal invariant, enforced: every change goway or goway-setup makes to a machine goes
//! through `goway-journal` (or the change log built on it), so it can be listed and undone.
//!
//! This test scans the source of every crate for script verbs that change a machine
//! (scheduled tasks, firewall, registry, services, packages, `wsl --shutdown`, ...) and for
//! `std::fs` calls that write or remove, and fails, naming the file and the verb, when one
//! appears outside the reviewed allowlist below. Test code is not scanned.
//!
//! To add a legitimate site: route the change through the journal (`Change`, or
//! `changelog::record_action` for what cannot be inverted), then, only when the site IS the
//! journal-backed implementation or goway's own run state, add it to [`ALLOW`] with a reason.
//! See `docs/changes.md`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// One reviewed exception: `labels` of the scanner that may appear in `file`, and why.
struct Allow {
    file: &'static str,
    /// Verb labels, or `fs` (every file-system mutation call) or `*` (anything).
    labels: &'static [&'static str],
    why: &'static str,
}

/// The reviewed allowlist. Each entry is checked for staleness: it must still match something.
const ALLOW: &[Allow] = &[
    // ---- goway-journal: the journal and its System implementation
    Allow {
        file: "crates/goway-journal/src/journal.rs",
        labels: &["fs"],
        why: "the journal's own atomic save (exclusive temp file, rename): the record itself",
    },
    Allow {
        file: "crates/goway-journal/src/local.rs",
        labels: &["*"],
        why: "LocalSystem, the System implementation: runs only inside apply/revert of recorded changes",
    },
    // ---- goway-setup: the System implementation and what it builds
    Allow {
        file: "crates/goway-setup/src/hostsys.rs",
        labels: &["*"],
        why: "HostSystem, the System implementation: its primitives run only inside apply/revert; sshd activation and task start run only after app::record_action",
    },
    Allow {
        file: "crates/goway-setup/src/ps.rs",
        labels: &["*"],
        why: "PowerShell script builders consumed only by HostSystem (firewall, tasks, services, capabilities)",
    },
    Allow {
        file: "crates/goway-setup/src/relay.rs",
        labels: &["*"],
        why: "netsh argument and relay-task script builders consumed only by HostSystem and the journaled relay task",
    },
    Allow {
        file: "crates/goway-setup/src/sysapi.rs",
        labels: &["netsh"],
        why: "names the netsh.exe tool path; HostSystem is its only caller",
    },
    Allow {
        file: "crates/goway-setup/src/native.rs",
        labels: &["set-netfirewallrule"],
        why: "the native plan's firewall-scope change, applied through HostSystem as a journaled Change",
    },
    Allow {
        file: "crates/goway-setup/src/host.rs",
        labels: &["netsh", "wsl --shutdown", "wsl --terminate"],
        why: "user-facing text naming the restart a changed .wslconfig or wsl.conf needs; nothing runs here",
    },
    Allow {
        file: "crates/goway-setup/src/error.rs",
        labels: &["netsh", "wsl --terminate"],
        why: "error text telling a person what to run; nothing runs here",
    },
    Allow {
        file: "crates/goway-setup/src/render.rs",
        labels: &["chmod"],
        why: "describes a journaled SetUnixMode change in words",
    },
    Allow {
        file: "crates/goway-setup/src/cli.rs",
        labels: &["--shutdown", "--terminate", "fs"],
        why: "restart_wsl runs only after app::record_action wrote the restart to the tune journal; the file calls are goway-setup's own staging, relaunch and log files",
    },
    Allow {
        file: "crates/goway-setup/src/admin.rs",
        labels: &["fs"],
        why: "goway-setup's own administrator-only state directory and protected exe copy, purged by uninstall (purge_admin_state)",
    },
    Allow {
        file: "crates/goway-setup/src/app.rs",
        labels: &["fs"],
        why: "goway-setup's own settings file and the journal files themselves, removed by uninstall",
    },
    Allow {
        file: "crates/goway-setup/src/safefile.rs",
        labels: &["fs"],
        why: "link-safe writes of goway-setup's own state files",
    },
    Allow {
        file: "crates/goway-setup/src/stage.rs",
        labels: &["fs"],
        why: "staging of the payload that an InstallFile change then copies; removed when the install ends",
    },
    Allow {
        file: "crates/goway-setup/src/tune.rs",
        labels: &["fs"],
        why: "creates and removes the tune journal's own directory and file",
    },
    // ---- goway: the change log and the sites that record before they act
    Allow {
        file: "crates/goway/src/changelog.rs",
        labels: &["fs"],
        why: "the change log itself: creates its directory and removes the log once nothing in it is live",
    },
    Allow {
        file: "crates/goway/src/config.rs",
        labels: &["fs"],
        why: "write_atomic: the primitive under changelog::write_file (config.toml, known_hosts are recorded first); its other callers write goway's own state files",
    },
    Allow {
        file: "crates/goway/src/add.rs",
        labels: &[
            "apt-get install",
            "apt-get remove",
            "add-windowscapability",
            "remove-windowscapability",
            "winget install",
        ],
        why: "local tool install: run only after changelog::record_action (RunFix); the remove-* and winget lines are advice printed for the person",
    },
    Allow {
        file: "crates/goway/src/doctor.rs",
        labels: &[
            "brew install",
            "apt-get install",
            "dnf install",
            "apt-get remove",
            "systemctl reload",
            "wsl --shutdown",
        ],
        why: "fix and undo command text: fixes run only through SshFixRunner/HostRunner, which record a RunFix action first; the rest is advice text",
    },
    Allow {
        file: "crates/goway/src/doctor/logout.rs",
        labels: &[
            "loginctl enable-linger",
            "enable-linger",
            "loginctl disable-linger",
        ],
        why: "the linger fix command text: run only through SshFixRunner, which records it first; disable-linger is the undo hint",
    },
    Allow {
        file: "crates/goway/src/doctor/output.rs",
        labels: &["apt-get install", "dnf install"],
        why: "shows the fix commands a person can run; nothing runs here",
    },
    Allow {
        file: "crates/goway/src/doctor/prereq.rs",
        labels: &["apt-get install", "dnf install"],
        why: "prerequisite fix command text, run only through the recording fix runners",
    },
    Allow {
        file: "crates/goway/src/doctor/windows.rs",
        labels: &["winget install"],
        why: "Windows fix command text, run only through HostRunner, which records it first",
    },
    Allow {
        file: "crates/goway/src/facts.rs",
        labels: &["wsl --shutdown"],
        why: "advice text; nothing runs here",
    },
    Allow {
        file: "crates/goway/src/facts/clock.rs",
        labels: &["wsl --shutdown"],
        why: "advice text; nothing runs here",
    },
    Allow {
        file: "crates/goway/src/remotesys.rs",
        labels: &["chmod"],
        why: "RemoteSystem, the System implementation over ssh: runs only inside apply/revert of recorded changes",
    },
    Allow {
        file: "crates/goway/src/sshsetup.rs",
        labels: &["fs", "icacls", "chmod"],
        why: "scratch known_hosts of one setup run and removal of the setup's own record; icacls narrows the ACL of the key pair the journaled SshKeyPair change created; chmod lines are printed for the person to run by hand",
    },
    Allow {
        file: "crates/goway/src/sshenv.rs",
        labels: &["chmod"],
        why: "advice text about a key's permissions; nothing runs here",
    },
    Allow {
        file: "crates/goway/src/uninstall.rs",
        labels: &["fs"],
        why: "the undo executor: removes what install, setup and doctor recorded, and reverses the install journal; it deletes the change log with the rest",
    },
    Allow {
        file: "crates/goway/src/paths.rs",
        labels: &["fs"],
        why: "one-time move of goway's own config directory out of the roaming profile; not a machine change, and a failed move keeps the old directory so no key is lost",
    },
    // ---- goway's own run state: scratch, locks, queue, caches (gc owns it; docs/changes.md)
    Allow {
        file: "crates/goway/src/ecotools.rs",
        labels: &["fs"],
        why: "creates goway's own state directory",
    },
    Allow {
        file: "crates/goway/src/gitmeta.rs",
        labels: &["fs"],
        why: "the .git stand-in built in goway's own state directory for a run (run state)",
    },
    Allow {
        file: "crates/goway/src/hosts.rs",
        labels: &["ssh-keygen", "fs"],
        why: "scratch known_hosts of one probe run (ssh-keygen -R/-l on it); the pinned known_hosts is edited only through changelog::write_file",
    },
    Allow {
        file: "crates/goway/src/local.rs",
        labels: &["fs"],
        why: "lock and marker files of local runs (run state)",
    },
    Allow {
        file: "crates/goway/src/queue.rs",
        labels: &["fs"],
        why: "queue ticket files of waiting runs (run state)",
    },
    Allow {
        file: "crates/goway/src/ssh/mux.rs",
        labels: &["fs"],
        why: "ssh ControlMaster sockets and their lock (run state)",
    },
    Allow {
        file: "crates/goway/src/ssh.rs",
        labels: &["fs"],
        why: "the private ssh runtime directory and the scratch known_hosts copy (run state)",
    },
    Allow {
        file: "crates/goway/src/remote.sh",
        labels: &["chmod"],
        why: "inside a run's own work directory on the helper (the env file, a probe script): run state",
    },
    Allow {
        file: "crates/goway/src/remote.ps1",
        labels: &["chmod"],
        why: "marks files of a run's own synced work tree executable on the helper: run state",
    },
];

/// A script verb or call that changes a machine.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Hit {
    file: String,
    line: usize,
    label: String,
}

const CMDLETS: &[&str] = &[
    "register-scheduledtask",
    "unregister-scheduledtask",
    "start-scheduledtask",
    "stop-scheduledtask",
    "set-scheduledtask",
    "schtasks",
    "netsh",
    "new-netfirewallrule",
    "set-netfirewallrule",
    "remove-netfirewallrule",
    "enable-netfirewallrule",
    "disable-netfirewallrule",
    "set-itemproperty",
    "new-itemproperty",
    "remove-itemproperty",
    "reg add",
    "reg delete",
    "reg.exe add",
    "reg.exe delete",
    "add-windowscapability",
    "remove-windowscapability",
    "set-service",
    "new-service",
    "remove-service",
    "start-service",
    "stop-service",
    "restart-service",
    "loginctl enable-linger",
    "loginctl disable-linger",
    "enable-linger",
    "wsl --shutdown",
    "wsl --terminate",
    "wsl --unregister",
    "wsl.exe --shutdown",
    "icacls",
    "takeown",
    "chmod",
    "chown",
    "chgrp",
    "setfacl",
    "set-acl",
    "ssh-keygen",
    "setx",
];

const PACKAGE_MANAGERS: &[&str] = &["apt", "apt-get", "dnf", "yum", "brew", "winget", "zypper"];
const PACKAGE_VERBS: &[&str] = &["install", "remove", "purge", "uninstall", "upgrade"];
const SYSTEMCTL_VERBS: &[&str] = &[
    "enable",
    "disable",
    "mask",
    "unmask",
    "stop",
    "start",
    "restart",
    "reload",
    "daemon-reload",
];
/// Bare WSL arguments (an argv array holds them apart from `wsl`).
const WSL_ARGS: &[&str] = &["\"--shutdown\"", "\"--terminate\"", "\"--unregister\""];

const FS_CALLS: &[&str] = &[
    "fs::write(",
    "fs::remove_file(",
    "fs::remove_dir(",
    "fs::remove_dir_all(",
    "fs::create_dir(",
    "fs::create_dir_all(",
    "fs::rename(",
    "fs::copy(",
    "fs::set_permissions(",
    "fs::hard_link(",
    "fs::soft_link(",
    "file::create(",
    "file::create_new(",
    "openoptions::new(",
    "dirbuilder::new(",
    ".set_permissions(",
    "os::unix::fs::symlink(",
    "os::windows::fs::symlink_file(",
    "os::windows::fs::symlink_dir(",
];

/// Whether `c` could be part of a word, so a match is not the tail of a longer one.
fn word_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '-' | '_' | '.')
}

/// Whether `needle` occurs in `hay` as a whole word: not the tail of a longer one (`xchmod`) and
/// not the head of one (`TakeOwnership` is not `takeown`).
fn has_word(hay: &str, needle: &str) -> bool {
    hay.match_indices(needle).any(|(i, _)| {
        let before = hay[..i].chars().next_back().is_none_or(|c| !word_char(c));
        let after = hay[i + needle.len()..]
            .chars()
            .next()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
        before && after
    })
}

/// The labels of machine-changing verbs on one (lowercased, whitespace-collapsed) line.
fn labels_of(line: &str, is_rust: bool) -> Vec<String> {
    // An argv array (`["systemctl", "restart", unit]`) reads as the command line it stands for.
    let flat = line.replace("\", \"", " ").replace("\",\"", " ");
    let line = flat.as_str();
    let mut out = Vec::new();
    for verb in CMDLETS {
        if has_word(line, verb) {
            out.push((*verb).to_owned());
        }
    }
    for pm in PACKAGE_MANAGERS {
        for v in PACKAGE_VERBS {
            if has_word(line, &format!("{pm} {v}")) {
                out.push(format!("{pm} {v}"));
            }
        }
    }
    for v in SYSTEMCTL_VERBS {
        if has_word(line, &format!("systemctl {v}")) {
            out.push(format!("systemctl {v}"));
        }
    }
    for arg in WSL_ARGS {
        if line.contains(arg) {
            out.push(arg.trim_matches('"').to_owned());
        }
    }
    if is_rust {
        for call in FS_CALLS {
            if line.contains(call) {
                out.push("fs".to_owned());
            }
        }
    }
    out.dedup();
    out
}

/// The 0-based lines of `lines` that sit inside `#[cfg(test)]` items (a module or a function).
fn test_lines(lines: &[&str]) -> Vec<bool> {
    let mut skip = vec![false; lines.len()];
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim() != "#[cfg(test)]" {
            i += 1;
            continue;
        }
        let indent = lines[i].len() - lines[i].trim_start().len();
        // The item runs from the attribute to the closing brace at the same indentation, or to
        // the end of its single line when it has no body (a `use`).
        let mut end = i;
        let mut j = i + 1;
        while j < lines.len() && lines[j].trim_start().starts_with("#[") {
            j += 1;
        }
        if j < lines.len() && !lines[j].trim_end().ends_with(';') {
            let close = format!("{}}}", " ".repeat(indent));
            end = (j..lines.len())
                .find(|&k| {
                    lines[k].trim_end() == close || lines[k].starts_with(&format!("{close} "))
                })
                .unwrap_or(lines.len() - 1);
        } else if j < lines.len() {
            end = j;
        }
        for flag in &mut skip[i..=end] {
            *flag = true;
        }
        i = end + 1;
    }
    skip
}

/// Every machine-changing verb or call in the source text of `file`, test code and comments aside.
fn scan_text(file: &str, text: &str) -> Vec<Hit> {
    let is_rust = Path::new(file).extension().is_some_and(|e| e == "rs");
    let lines: Vec<&str> = text.lines().collect();
    let skip = if is_rust {
        test_lines(&lines)
    } else {
        vec![false; lines.len()]
    };
    let mut hits = Vec::new();
    for (n, raw) in lines.iter().enumerate() {
        let trimmed = raw.trim_start();
        let comment = if is_rust {
            trimmed.starts_with("//")
        } else {
            trimmed.starts_with('#')
        };
        if skip[n] || comment {
            continue;
        }
        let line = raw
            .to_ascii_lowercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        for label in labels_of(&line, is_rust) {
            hits.push(Hit {
                file: file.to_owned(),
                line: n + 1,
                label,
            });
        }
    }
    hits
}

/// Whether `allow` covers `label` in its file.
fn covers(allow: &Allow, label: &str) -> bool {
    allow.labels.contains(&"*") || allow.labels.contains(&label)
}

/// The hits not covered by `allowlist`.
fn violations(hits: &[Hit], allowlist: &[Allow]) -> Vec<Hit> {
    hits.iter()
        .filter(|h| {
            !allowlist
                .iter()
                .any(|a| a.file == h.file && covers(a, &h.label))
        })
        .cloned()
        .collect()
}

/// Allowlist labels that match no hit in their file (a stale entry hides nothing it should).
fn stale(hits: &[Hit], allowlist: &[Allow]) -> Vec<String> {
    let mut out = Vec::new();
    for a in allowlist {
        if a.why.trim().is_empty() {
            out.push(format!("{}: the allowlist entry has no reason", a.file));
        }
        let mine: Vec<&Hit> = hits.iter().filter(|h| h.file == a.file).collect();
        if mine.is_empty() {
            out.push(format!("{}: allowlisted but nothing to allow", a.file));
            continue;
        }
        for label in a.labels.iter().filter(|l| **l != "*") {
            if !mine.iter().any(|h| h.label == *label) {
                out.push(format!(
                    "{}: label `{label}` is allowlisted but absent",
                    a.file
                ));
            }
        }
    }
    out
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// Every source file under `crates/*/src` of the workspace.
fn sources() -> Vec<(String, String)> {
    sources_under(&workspace_root())
}

/// Every source file under `<root>/crates/*/src` (`.rs`, `.sh`, `.ps1`), as root-relative paths.
fn sources_under(root: &Path) -> Vec<(String, String)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else if path
                .extension()
                .is_some_and(|e| matches!(e.to_str(), Some("rs" | "sh" | "ps1")))
            {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    for krate in std::fs::read_dir(root.join("crates")).unwrap() {
        let src = krate.unwrap().path().join("src");
        if src.is_dir() {
            walk(&src, &mut files);
        }
    }
    files.sort();
    files
        .into_iter()
        .map(|p| {
            let rel = p
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let text = std::fs::read_to_string(&p).unwrap();
            (rel, text)
        })
        .collect()
}

fn report(hits: &[Hit]) -> String {
    let mut out = String::new();
    for h in hits {
        let _ = writeln!(out, "  {}:{}: `{}`", h.file, h.line, h.label);
    }
    out
}

// frob:tests crates/goway/src/changelog.rs::record_action
#[test]
fn no_machine_change_appears_outside_the_journal_backed_modules() {
    let all: Vec<Hit> = sources()
        .iter()
        .flat_map(|(file, text)| scan_text(file, text))
        .collect();
    let bad = violations(&all, ALLOW);
    assert!(
        bad.is_empty(),
        "these change a machine outside the reviewed journal-backed modules; route each through \
         goway-journal (Change, or changelog::record_action for what cannot be inverted), or \
         add the file to ALLOW in tests/journal_invariant.rs with a reason (docs/changes.md):\n{}",
        report(&bad)
    );
    let old = stale(&all, ALLOW);
    assert!(
        old.is_empty(),
        "stale allowlist entries:\n  {}",
        old.join("\n  ")
    );
}

#[test]
fn the_scanner_names_the_file_and_verb_of_a_planted_violation() {
    let planted = "fn f() {\n    run(\"Register-ScheduledTask -TaskName x\");\n    std::fs::write(p, b);\n    run(\"apt-get  install -y mold\");\n    run(\"systemctl enable ssh\");\n}\n";
    let hits = scan_text("crates/x/src/planted.rs", planted);
    let labels: Vec<(usize, &str)> = hits.iter().map(|h| (h.line, h.label.as_str())).collect();
    assert_eq!(
        labels,
        vec![
            (2, "register-scheduledtask"),
            (3, "fs"),
            (4, "apt-get install"),
            (5, "systemctl enable"),
        ]
    );
    let bad = violations(&hits, &[]);
    assert_eq!(bad.len(), 4);
    assert!(report(&bad).contains("crates/x/src/planted.rs:2: `register-scheduledtask`"));
}

#[test]
fn every_listed_verb_is_detected() {
    let probes = [
        "schtasks /create /tn x",
        "netsh interface portproxy add",
        "New-NetFirewallRule -Name x",
        "Set-ItemProperty -Path p",
        "reg add HKLM\\x",
        "dnf install -y gcc",
        "yum remove foo",
        "brew install coreutils",
        "apt purge x",
        "systemctl mask x",
        "systemctl disable x",
        "loginctl enable-linger me",
        "wsl --shutdown",
        "wsl --terminate d",
        "Add-WindowsCapability -Online",
        "Set-Service -Name x",
        "icacls p /grant",
        "chmod 600 f",
        "chown me f",
        "Unregister-ScheduledTask -InputObject t",
        "Remove-NetFirewallRule",
        "Set-NetFirewallRule -Name x",
        "New-ItemProperty -Path p",
        "reg delete HKLM\\x",
    ];
    for probe in probes {
        assert!(
            !scan_text("crates/x/src/a.ps1", probe).is_empty(),
            "not detected: {probe}"
        );
    }
    for fs in [
        "std::fs::remove_file(p)",
        "std::fs::create_dir_all(d)",
        "File::create(p)",
        "OpenOptions::new().write(true)",
        "std::fs::rename(a, b)",
        "std::fs::copy(a, b)",
        "std::fs::set_permissions(p, m)",
    ] {
        assert!(
            !scan_text("crates/x/src/a.rs", fs).is_empty(),
            "not detected: {fs}"
        );
    }
}

#[test]
fn test_code_comments_and_reads_are_not_flagged() {
    let text = "\
/// Runs `systemctl enable ssh` (docs only).
// chmod 600 in a comment
fn read() { let _ = std::fs::read_to_string(p); let _ = std::fs::metadata(p); }
fn is() { run(\"systemctl is-active ssh\"); }
#[cfg(test)]
mod tests {
    #[test]
    fn t() {
        std::fs::write(p, b).unwrap();
        run(\"netsh x\");
    }
}
#[cfg(test)]
use std::fs::write;
fn after() { std::fs::write(p, b); }
";
    let hits = scan_text("crates/x/src/a.rs", text);
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].line, 15);
}

#[test]
fn an_allowlist_entry_covers_only_its_file_and_labels_and_must_not_go_stale() {
    let hit = |file: &str, label: &str| Hit {
        file: file.to_owned(),
        line: 1,
        label: label.to_owned(),
    };
    let hits = vec![hit("a.rs", "fs"), hit("a.rs", "netsh"), hit("b.rs", "fs")];
    let allow = [Allow {
        file: "a.rs",
        labels: &["fs"],
        why: "journal-backed",
    }];
    let bad = violations(&hits, &allow);
    assert_eq!(bad.len(), 2, "{bad:?}");
    assert!(bad.iter().any(|h| h.file == "a.rs" && h.label == "netsh"));
    assert!(bad.iter().any(|h| h.file == "b.rs"));
    assert!(stale(&hits, &allow).is_empty());
    let gone = [Allow {
        file: "c.rs",
        labels: &["fs"],
        why: "x",
    }];
    assert_eq!(stale(&hits, &gone).len(), 1);
    let unreasoned = [Allow {
        file: "a.rs",
        labels: &["fs"],
        why: " ",
    }];
    assert_eq!(stale(&hits, &unreasoned).len(), 1);
    let absent = [Allow {
        file: "a.rs",
        labels: &["fs", "chmod"],
        why: "x",
    }];
    assert_eq!(stale(&hits, &absent).len(), 1);
}

#[test]
fn a_violation_planted_in_a_crate_fails_the_scan_with_its_file_and_verb() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("crates/demo/src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("sneaky.rs"),
        "pub fn f() {\n    run(\"schtasks /create /tn x\");\n}\n",
    )
    .unwrap();
    std::fs::write(src.join("ok.rs"), "pub fn g() -> u8 { 1 }\n").unwrap();
    let hits: Vec<Hit> = sources_under(tmp.path())
        .iter()
        .flat_map(|(file, text)| scan_text(file, text))
        .collect();
    let bad = violations(&hits, ALLOW);
    let text = report(&bad);
    assert_eq!(bad.len(), 1, "{text}");
    assert!(
        text.contains("crates/demo/src/sneaky.rs:2: `schtasks`"),
        "{text}"
    );
}
