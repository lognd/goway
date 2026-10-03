//! The `System` contract, exercised method by method on the model.

use std::path::Path;

use goway_journal::{ModelSystem, RegValue, ResourceKind, System};

// frob:tests crates/goway-journal/src/system.rs::System.read_file
// frob:tests crates/goway-journal/src/system.rs::System.write_file
// frob:tests crates/goway-journal/src/system.rs::System.remove_file
// frob:tests crates/goway-journal/src/system.rs::System.dir_exists
// frob:tests crates/goway-journal/src/system.rs::System.create_dir
// frob:tests crates/goway-journal/src/system.rs::System.dir_is_empty
// frob:tests crates/goway-journal/src/system.rs::System.remove_dir
// frob:tests crates/goway-journal/src/system.rs::System.get_var
// frob:tests crates/goway-journal/src/system.rs::System.set_var
// frob:tests crates/goway-journal/src/system.rs::System.remove_var
// frob:tests crates/goway-journal/src/system.rs::System.reg_get
// frob:tests crates/goway-journal/src/system.rs::System.reg_set
// frob:tests crates/goway-journal/src/system.rs::System.reg_delete
// frob:tests crates/goway-journal/src/system.rs::System.get_mode
// frob:tests crates/goway-journal/src/system.rs::System.set_mode
// frob:tests crates/goway-journal/src/system.rs::System.get_acl
// frob:tests crates/goway-journal/src/system.rs::System.set_acl
// frob:tests crates/goway-journal/src/system.rs::System.resource_exists
// frob:tests crates/goway-journal/src/system.rs::System.resource_create
// frob:tests crates/goway-journal/src/system.rs::System.resource_delete
#[test]
fn model_system_honours_the_system_contract() {
    let mut s = ModelSystem::new();
    let d = Path::new("/d");
    let f = Path::new("/d/f");
    assert!(!s.dir_exists(d).unwrap());
    s.create_dir(d).unwrap();
    assert!(s.dir_exists(d).unwrap());
    assert!(s.dir_is_empty(d).unwrap());
    assert_eq!(s.read_file(f).unwrap(), None);
    s.write_file(f, "x").unwrap();
    assert_eq!(s.read_file(f).unwrap().as_deref(), Some("x"));
    assert!(!s.dir_is_empty(d).unwrap());
    assert!(
        s.remove_dir(d).is_err(),
        "non-empty directories are refused"
    );
    assert!(
        s.create_dir(Path::new("/missing/child")).is_err(),
        "parent must exist"
    );

    assert_eq!(s.get_mode(f).unwrap(), 0o644);
    s.set_mode(f, 0o600).unwrap();
    assert_eq!(s.get_mode(f).unwrap(), 0o600);
    assert_eq!(s.get_acl(f).unwrap(), "");
    s.set_acl(f, "D:x").unwrap();
    assert_eq!(s.get_acl(f).unwrap(), "D:x");

    s.remove_file(f).unwrap();
    s.remove_file(f).unwrap();
    assert!(s.get_mode(f).is_err());
    s.remove_dir(d).unwrap();
    s.remove_dir(d).unwrap();

    assert_eq!(s.get_var("P").unwrap(), None);
    s.set_var("P", "a:b").unwrap();
    assert_eq!(s.get_var("P").unwrap().as_deref(), Some("a:b"));
    s.remove_var("P").unwrap();
    s.remove_var("P").unwrap();
    assert_eq!(s.get_var("P").unwrap(), None);

    assert_eq!(s.reg_get("K", "n").unwrap(), None);
    s.reg_set("K", "n", &RegValue::Dword(7)).unwrap();
    assert_eq!(s.reg_get("K", "n").unwrap(), Some(RegValue::Dword(7)));
    s.reg_delete("K", "n").unwrap();
    s.reg_delete("K", "n").unwrap();
    assert_eq!(s.reg_get("K", "n").unwrap(), None);

    assert!(!s.resource_exists(ResourceKind::ScheduledTask, "t").unwrap());
    s.resource_create(ResourceKind::ScheduledTask, "t", "spec")
        .unwrap();
    assert!(s.resource_exists(ResourceKind::ScheduledTask, "t").unwrap());
    s.resource_delete(ResourceKind::ScheduledTask, "t").unwrap();
    s.resource_delete(ResourceKind::ScheduledTask, "t").unwrap();
    assert_eq!(s, ModelSystem::new());
}
