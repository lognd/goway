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
