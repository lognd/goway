//! The embedded remote script and how to invoke its verbs over ssh.

use crate::ssh::shell_join;

/// The remote side of goway, sent inline with each call.
pub const SCRIPT: &str = include_str!("remote.sh");

/// The remote command line running `verb` with `args`.
pub fn invocation(verb: &str, args: &[&str]) -> String {
    let mut words = vec!["bash", "-c", SCRIPT, "goway", verb];
    words.extend_from_slice(args);
    shell_join(&words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invocation_runs_locally_through_a_shell() {
        let out = std::process::Command::new("sh")
            .args(["-c", &invocation("ping", &[])])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "goway-remote ok\n");
        let out = std::process::Command::new("sh")
            .args(["-c", &invocation("nope", &["it's"])])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(125));
        assert!(String::from_utf8_lossy(&out.stderr).contains("unknown verb"));
    }
}
