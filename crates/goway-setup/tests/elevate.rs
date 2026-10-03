//! UAC relaunch command-line construction and the harmless off-Windows behaviour.

use std::path::Path;

use goway_setup::elevate::{can_prompt, elevated_parameters, is_elevated, quote_arg, run_elevated};

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

// frob:tests crates/goway-setup/src/elevate.rs::elevated_parameters
#[test]
fn the_elevated_command_line_redirects_output_to_the_log() {
    let args = ["install", "--host", "--profile", "goway test"].map(str::to_owned);
    let line = elevated_parameters(
        Path::new("C:\\Users\\a b\\goway-setup.exe"),
        &args,
        Path::new("C:\\Temp\\log.txt"),
    );
    assert_eq!(
        line,
        "/D /S /C \"\"C:\\Users\\a b\\goway-setup.exe\" install --host --profile \"goway test\" > C:\\Temp\\log.txt 2>&1\""
    );
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
        assert!(run_elevated("/C echo").is_err());
    }
}
