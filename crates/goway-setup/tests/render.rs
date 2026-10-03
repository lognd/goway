//! Change descriptions shown by the dry run and status.

use goway_journal::{Change, RegValue};
use goway_setup::render::describe;

// frob:tests crates/goway-setup/src/render.rs::describe
#[test]
fn describe_names_the_target_of_each_change() {
    let c = Change::SetRegistryValue {
        key: "HKCU\\K".into(),
        name: "NoModify".into(),
        value: RegValue::Dword(1),
    };
    assert_eq!(describe(&c), "set registry value HKCU\\K\\NoModify = 1");
    let c = Change::EnsureListEntry {
        var: "Path".into(),
        entry: "C:\\bin".into(),
        separator: ';',
        position: goway_journal::ListPosition::Back,
    };
    assert_eq!(describe(&c), "add C:\\bin to user Path");
    let c = Change::InstallFile {
        path: "/x".into(),
        source: "/s".into(),
        digest: "0123456789abcdef".into(),
    };
    assert_eq!(describe(&c), "install file /x (sha256 0123456789ab)");
}

// frob:tests crates/goway-setup/src/render.rs::clean_line
#[test]
fn system_text_loses_control_and_bidi_characters() {
    use goway_setup::render::{clean_line, clean_text};
    assert_eq!(clean_line("a\x1b[31mb\x07c"), "a?[31mb?c");
    assert_eq!(clean_line("a\nb\r\nc"), "a / b / c");
    assert_eq!(
        clean_line("a\u{202e}b\u{2066}c\u{200b}d\u{feff}"),
        "a?b?c?d?"
    );
    assert_eq!(clean_line("\u{9b}31m"), "?31m");
    assert_eq!(clean_line("caf\u{e9}"), "caf\u{e9}");
    assert_eq!(clean_text("a\nb\tc\x1b"), "a\nb\tc?");
    let long = "x".repeat(5000);
    assert!(clean_line(&long).chars().count() <= 1024);
}
