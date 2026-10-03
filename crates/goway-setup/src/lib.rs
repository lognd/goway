//! goway-setup: install and provably uninstall goway.
//!
//! Every change goes through `goway-journal`, so `uninstall` replays the recorded priors
//! backwards. Components (`client`, `host`) are planned independently in [`plan`] and [`host`], one journal each.

pub mod admin;
pub mod app;
pub mod cli;
pub mod elevate;
pub mod entry;
pub mod error;
pub mod helper;
pub mod host;
pub mod hostsys;
pub mod layout;
pub mod plan;
pub mod ps;
pub mod relay;
pub mod render;
pub mod stage;
pub mod sysapi;
pub mod windows;

/// The goway.exe embedded at build time (empty when built without `GOWAY_PAYLOAD`).
pub static PAYLOAD: &[u8] = include_bytes!(env!("GOWAY_PAYLOAD_FILE"));

/// Install the global tracing subscriber; `-v` raises the level (diagnostics go to stderr).
pub fn init_tracing(verbose: u8) {
    let level = match verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    let filter = tracing_subscriber::EnvFilter::try_from_env("GOWAY_LOG").unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new(format!("goway_setup={level},goway_journal={level}"))
    });
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}
