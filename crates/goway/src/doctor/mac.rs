//! doctor's mandatory-access-control check: `SELinux` or `AppArmor` denials of
//! goway's own tools (`setsid`, `flock`, `bash`, `goway`) on a helper.
//!
//! A denied `setsid` or `flock` fails a run with a bare "Permission
//! denied" that looks like a goway bug. One extra probe appended to the
//! `doctor` call reads the module that is on and, when the audit log or the
//! kernel log is readable, the latest matching denial; the check names the
//! denied program and the label or profile change to make. Nothing here
//! changes the host: loosening a security policy is the owner's decision.

use std::collections::BTreeMap;

use super::{Check, Level};

/// The longest denial line kept (audit lines carry long paths).
const MAX_DENIAL: usize = 400;

/// The shell script that reports `mac.module` and `mac.denial`.
fn probe_script() -> String {
    format!(
        "m=\n\
         if command -v getenforce >/dev/null 2>&1; then\n\
         e=$(getenforce 2>/dev/null || true)\n\
         case \"$e\" in Enforcing|Permissive) m=selinux ;; esac\n\
         fi\n\
         if [ -z \"$m\" ] && [ \"$(cat /sys/module/apparmor/parameters/enabled 2>/dev/null || true)\" = Y ]; then m=apparmor; fi\n\
         if [ -n \"$m\" ]; then\n\
         printf 'mac.module=%s\\n' \"$m\"\n\
         w='comm=\"(setsid|flock|bash|goway)\"|setsid|flock|goway'\n\
         d=$({{ ausearch -m avc,user_avc -ts recent 2>/dev/null || true; }} | grep -E \"$w\" | tail -1)\n\
         if [ -z \"$d\" ]; then d=$({{ dmesg 2>/dev/null || true; }} | grep -E 'apparmor=\"DENIED\"|avc: +denied' | grep -E \"$w\" | tail -1); fi\n\
         d=$(printf %s \"$d\" | tr -d '\\n' | cut -c1-{MAX_DENIAL})\n\
         [ -n \"$d\" ] && printf 'mac.denial=%s\\n' \"$d\"\n\
         fi\n\
         true\n"
    )
}

/// `base` (the `doctor` call) followed by the security-module probe.
pub(super) fn wrap(base: &str) -> String {
    super::append_script(base, &probe_script())
}

/// The value of `key="..."` or `key=word` in an audit or kernel log line.
fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let at = line.find(&format!("{key}="))? + key.len() + 1;
    let rest = &line[at..];
    match rest.strip_prefix('"') {
        Some(quoted) => quoted.split('"').next(),
        None => rest.split_whitespace().next(),
    }
}

/// The `SELinux` type in a context such as `system_u:system_r:init_t:s0`.
fn selinux_type(context: &str) -> &str {
    context.split(':').nth(2).unwrap_or(context)
}

/// What to do about one denial of `module` (`selinux` or `apparmor`).
fn advice(module: &str, denial: &str) -> String {
    let program = field(denial, "comm").unwrap_or("a goway tool");
    if module == "selinux" {
        let source = field(denial, "scontext").map_or("its domain", selinux_type);
        format!(
            "SELinux denied {program} (domain {source}). Inspect it with `sudo ausearch -m avc -ts recent`; if the files are only mislabelled, `sudo restorecon -Rv` the goway root and ~/.local/bin; otherwise build a local policy module from the reviewed denial: `sudo ausearch -m avc -ts recent | audit2allow -M goway-local && sudo semodule -i goway-local.pp`"
        )
    } else {
        let profile = field(denial, "profile").unwrap_or("the profile named in the log");
        let name = field(denial, "name").map_or(String::new(), |n| format!(" on {n}"));
        format!(
            "AppArmor denied {program}{name} (profile {profile}). Allow it in /etc/apparmor.d/local/ for that profile, then `sudo apparmor_parser -r` the profile; to see what it blocks without enforcing, `sudo aa-complain` the profile first"
        )
    }
}

/// The check for a helper whose security module denied one of goway's tools; none otherwise.
pub(super) fn checks(facts: &BTreeMap<String, String>) -> Vec<Check> {
    let (Some(module), Some(denial)) = (facts.get("mac.module"), facts.get("mac.denial")) else {
        return Vec::new();
    };
    let name = if module == "selinux" {
        "selinux"
    } else {
        "apparmor"
    };
    vec![Check {
        name: name.to_owned(),
        explain: Some(
            "A security module can deny setsid, flock or the shell goway runs jobs with, and the run then fails with a plain 'Permission denied'. The latest denial that names one of goway's tools was read from the audit log (ausearch) or the kernel log (dmesg); both need root on many systems, so no denial shown does not prove there is none. goway never changes a security policy itself."
                .to_owned(),
        ),
        level: Level::Warn,
        detail: advice(module, denial),
        fix: None,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    // frob:tests crates/goway/src/doctor/mac.rs::checks
    #[test]
    fn an_selinux_denial_of_setsid_names_the_domain_and_the_label_change() {
        let c = checks(&facts(&[
            ("mac.module", "selinux"),
            (
                "mac.denial",
                "type=AVC msg=audit(1.2:3): avc:  denied  { execute } for  pid=9 comm=\"setsid\" scontext=system_u:system_r:sshd_t:s0 tcontext=unconfined_u:object_r:user_home_t:s0 tclass=file",
            ),
        ]));
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].level, Level::Warn);
        assert!(c[0].detail.contains("denied setsid"), "{}", c[0].detail);
        assert!(c[0].detail.contains("sshd_t"), "{}", c[0].detail);
        assert!(c[0].detail.contains("restorecon"), "{}", c[0].detail);
        assert!(c[0].detail.contains("audit2allow"), "{}", c[0].detail);
    }

    // frob:tests crates/goway/src/doctor/mac.rs::checks
    #[test]
    fn an_apparmor_denial_names_the_profile_and_the_path() {
        let c = checks(&facts(&[
            ("mac.module", "apparmor"),
            (
                "mac.denial",
                "audit: type=1400 apparmor=\"DENIED\" operation=\"exec\" profile=\"usr.sbin.sshd\" name=\"/usr/bin/flock\" comm=\"flock\"",
            ),
        ]));
        assert_eq!(c.len(), 1);
        assert!(c[0].detail.contains("usr.sbin.sshd"), "{}", c[0].detail);
        assert!(c[0].detail.contains("/usr/bin/flock"), "{}", c[0].detail);
        assert!(c[0].detail.contains("aa-complain"), "{}", c[0].detail);
    }

    // frob:tests crates/goway/src/doctor/mac.rs::checks
    #[test]
    fn a_module_without_a_denial_or_no_module_says_nothing() {
        assert!(checks(&facts(&[("mac.module", "selinux")])).is_empty());
        assert!(checks(&facts(&[])).is_empty());
    }

    // frob:tests crates/goway/src/doctor/mac.rs::wrap
    #[test]
    fn the_probe_script_runs_clean_where_no_module_is_on() {
        let out = std::process::Command::new("bash")
            .arg("-c")
            .arg(probe_script())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
