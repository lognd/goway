//! `.wslconfig` tuning: validation, the journaled plan, exact restoration and the restart need.

use clap::Parser;
use goway_journal::{Change, Journal, LocalSystem};
use goway_setup::cli::{Cli, run};
use goway_setup::error::SetupError;
use goway_setup::host::restart_need;
use goway_setup::render::{ColorWhen, Renderer};
use goway_setup::tune::{
    TuneRequest, apply_tune, parse_processors, parse_size, revert_tune, tune_plan, wslconfig_path,
};

// frob:tests crates/goway-setup/src/tune.rs::parse_size
// frob:tests crates/goway-setup/src/tune.rs::parse_processors
#[test]
fn sizes_and_processor_counts_are_validated_before_anything_is_written() {
    assert_eq!(parse_size("8GB").unwrap(), "8GB");
    assert_eq!(parse_size(" 8 g ").unwrap(), "8GB");
    assert_eq!(parse_size("2048mb").unwrap(), "2048MB");
    assert_eq!(parse_size("0").unwrap(), "0");
    for bad in [
        "",
        "GB",
        "8",
        "8TB",
        "-1GB",
        "1.5GB",
        "100MB",
        "9999999GB",
        "8GB\nx=1",
    ] {
        assert!(
            matches!(parse_size(bad), Err(SetupError::BadTuneValue(_))),
            "{bad:?}"
        );
    }
    assert_eq!(parse_processors(4).unwrap(), 4);
    assert!(parse_processors(0).is_err() && parse_processors(5000).is_err());
}

// frob:tests crates/goway-setup/src/tune.rs::tune_plan
// frob:tests crates/goway-setup/src/host.rs::restart_need
#[test]
fn the_plan_sets_only_what_was_asked_and_needs_a_wsl_shutdown() {
    let path = std::path::Path::new("C:/Users/u/.wslconfig");
    assert!(TuneRequest::default().is_empty());
    assert!(tune_plan(path, &TuneRequest::default()).is_empty());
    let req = TuneRequest {
        memory: Some("12GB".into()),
        swap: None,
        processors: Some(14),
        nested_virtualization: Some(true),
    };
    let plan = tune_plan(path, &req);
    let keys: Vec<(&str, &str)> = plan
        .iter()
        .map(|c| match c {
            Change::SetIniKey {
                section,
                key,
                value,
                ..
            } => {
                assert_eq!(section, "wsl2");
                (key.as_str(), value.as_str())
            }
            other => panic!("unexpected change {other:?}"),
        })
        .collect();
    assert_eq!(
        keys,
        [
            ("memory", "12GB"),
            ("processors", "14"),
            ("nestedVirtualization", "true")
        ]
    );
}

// frob:tests crates/goway-setup/src/tune.rs::apply_tune
// frob:tests crates/goway-setup/src/tune.rs::revert_tune
// frob:tests crates/goway-setup/src/tune.rs::wslconfig_path
// frob:tests crates/goway-setup/src/host.rs::restart_need
#[test]
fn tuning_twice_then_reverting_restores_the_exact_original_file() {
    let home = tempfile::tempdir().unwrap();
    let config = wslconfig_path(home.path());
    let original = "[wsl2]\nmemory=2GB\nnetworkingMode=mirrored\n\n[experimental]\nautoMemoryReclaim=gradual\n";
    std::fs::write(&config, original).unwrap();
    let journal_path = home.path().join("state").join("tune-journal.json");
    let mut sys = LocalSystem;
    let first = TuneRequest {
        memory: Some("12GB".into()),
        swap: Some("0".into()),
        ..TuneRequest::default()
    };
    let j = apply_tune(&mut sys, &journal_path, &tune_plan(&config, &first)).unwrap();
    assert_eq!(j.entries.len(), 2);
    let second = TuneRequest {
        memory: Some("8GB".into()),
        processors: Some(6),
        ..TuneRequest::default()
    };
    let j = apply_tune(&mut sys, &journal_path, &tune_plan(&config, &second)).unwrap();
    assert_eq!(
        j.entries.len(),
        4,
        "the second run extends the same journal"
    );
    let text = std::fs::read_to_string(&config).unwrap();
    assert!(
        text.contains("memory=8GB") && text.contains("processors=6") && text.contains("swap=0")
    );
    assert!(
        text.contains("networkingMode=mirrored"),
        "other keys are kept"
    );
    assert!(restart_need(&Journal::load(&journal_path).unwrap()).shutdown);

    assert!(revert_tune(&mut sys, &journal_path).unwrap());
    assert_eq!(std::fs::read_to_string(&config).unwrap(), original);
    assert!(!journal_path.exists());
    assert!(
        !revert_tune(&mut sys, &journal_path).unwrap(),
        "nothing left to revert"
    );
}

// frob:tests crates/goway-setup/src/cli.rs::run_tune
#[test]
fn the_tune_command_parses_and_a_dry_run_changes_nothing() {
    let cli = Cli::try_parse_from([
        "goway-setup",
        "--color",
        "never",
        "tune",
        "--memory",
        "8GB",
        "--processors",
        "4",
        "--nested-virtualization",
        "true",
        "--dry-run",
        "--profile",
        "tune-dry-run-test",
    ])
    .unwrap();
    run(&cli, Renderer::new(ColorWhen::Never)).unwrap();
    // A bad size is refused before anything runs; so is a tune that asks for nothing.
    let bad =
        Cli::try_parse_from(["goway-setup", "tune", "--memory", "lots", "--dry-run"]).unwrap();
    assert!(run(&bad, Renderer::new(ColorWhen::Never)).is_err());
    let none = Cli::try_parse_from(["goway-setup", "tune", "--dry-run"]).unwrap();
    assert!(run(&none, Renderer::new(ColorWhen::Never)).is_err());
}
