//! UAC relaunch command-line construction and the harmless off-Windows behaviour.

use std::path::Path;

use goway_setup::elevate::{can_prompt, command_line, is_elevated, quote_arg, run_elevated};

// frob:tests crates/goway-setup/src/elevate.rs::quote_arg
#[test]
fn arguments_are_quoted_only_when_needed() {
    assert_eq!(quote_arg("install"), "install");
    assert_eq!(
        quote_arg("C:\\Program Files\\x.exe"),
        "\"C:\\Program Files\\x.exe\""
    );
    assert_eq!(quote_arg(""), "\"\"");
    assert_eq!(quote_arg("a\"b"), "\"a\\\"b\"");
}

// frob:tests crates/goway-setup/src/elevate.rs::quote_arg
#[test]
fn quoting_follows_the_windows_rules_for_backslashes_and_quotes() {
    // Backslashes are literal unless they precede a quote or the closing quote.
    assert_eq!(quote_arg(r"C:\dir\file"), r"C:\dir\file");
    assert_eq!(quote_arg(r"C:\my dir\"), r#""C:\my dir\\""#);
    assert_eq!(quote_arg(r#"a\"b"#), r#""a\\\"b""#);
    // cmd metacharacters need no escaping because no shell is involved (L7).
    assert_eq!(quote_arg("a&b|c^d%e(f)"), "a&b|c^d%e(f)");
}

// frob:tests crates/goway-setup/src/elevate.rs::command_line
#[test]
fn the_elevated_command_line_is_the_quoted_arguments_and_nothing_else() {
    let args = [
        "install",
        "--host",
        "--profile",
        "goway test",
        "--distro",
        "a&b",
    ]
    .map(str::to_owned);
    assert_eq!(
        command_line(&args),
        "install --host --profile \"goway test\" --distro a&b"
    );
    assert!(!command_line(&args).contains("cmd"), "no shell is involved");
}

// frob:tests crates/goway-setup/src/elevate.rs::is_elevated
// frob:tests crates/goway-setup/src/elevate.rs::can_prompt
// frob:tests crates/goway-setup/src/elevate.rs::run_elevated
#[test]
fn elevation_probes_do_not_panic_and_relaunch_is_unavailable_off_windows() {
    let _ = can_prompt();
    if cfg!(windows) {
        let _ = is_elevated();
    } else {
        assert!(is_elevated(), "nothing to elevate off Windows");
        assert!(run_elevated(Path::new("goway-setup.exe"), "install").is_err());
    }
}

// frob:tests crates/goway-setup/src/elevate.rs::redirect_output
// frob:tests crates/goway-setup/src/windows.rs::restrict_dll_search
#[test]
fn output_redirection_and_dll_restriction_are_harmless_off_windows() {
    goway_setup::windows::restrict_dll_search();
    if !cfg!(windows) {
        let tmp = tempfile::tempdir().unwrap();
        let file = std::fs::File::create(tmp.path().join("log")).unwrap();
        assert!(goway_setup::elevate::redirect_output(file).is_err());
    }
}
