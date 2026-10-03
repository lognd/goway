//! goway: run a command on another machine, natively, from the current git
//! work tree. See docs/design.md for the problem tree.

pub mod cli;
pub mod error;
pub mod render;

use std::process::ExitCode;

use cli::{Cli, Command};
use error::{Error, Result};
use render::Renderer;

/// Install the tracing subscriber: `-v` levels, overridable by `GOWAY_LOG`.
pub fn init_tracing(verbose: u8) {
    let level = match verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    let filter = tracing_subscriber::EnvFilter::try_from_env("GOWAY_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(format!("goway={level}")));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .try_init();
}

/// Run a parsed command line and turn the outcome into the process exit code.
pub fn main_with(cli: &Cli) -> ExitCode {
    init_tracing(cli.verbose);
    let renderer = Renderer::new(cli.color);
    tracing::debug!(command = ?cli.command, "dispatch");
    match dispatch(&cli.command, renderer) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            tracing::debug!(?error, "goway failed");
            renderer.error(&error);
            ExitCode::from(error.exit_code())
        }
    }
}

fn dispatch(command: &Command, _renderer: Renderer) -> Result<u8> {
    match command {
        Command::Run(_) => Err(Error::NotImplemented("run")),
        Command::Status => Err(Error::NotImplemented("status")),
        Command::Gc(_) => Err(Error::NotImplemented("gc")),
        Command::Doctor(_) => Err(Error::NotImplemented("doctor")),
        Command::Host(_) => Err(Error::NotImplemented("host")),
        Command::Ssh(_) => Err(Error::NotImplemented("ssh")),
        Command::Config(_) => Err(Error::NotImplemented("config")),
    }
}
