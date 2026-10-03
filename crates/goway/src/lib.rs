//! goway: run a command on another machine, natively, from the current git
//! work tree. See docs/design.md for the problem tree.

pub mod cli;
pub mod config;
pub mod error;
pub mod hosts;
pub mod paths;
pub mod remote;
pub mod render;
pub mod repo;
pub mod resolve;
pub mod ssh;
pub mod sshenv;
pub mod state;
pub mod sync;

use std::process::ExitCode;

use cli::{Cli, Command, ConfigCommand, HostCommand};
use config::Config;
use error::{Error, Result};
use paths::Paths;
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

fn dispatch(command: &Command, renderer: Renderer) -> Result<u8> {
    let paths = Paths::from_env();
    match command {
        Command::Run(_) => Err(Error::NotImplemented("run")),
        Command::Status => Err(Error::NotImplemented("status")),
        Command::Gc(_) => Err(Error::NotImplemented("gc")),
        Command::Doctor(_) => Err(Error::NotImplemented("doctor")),
        Command::Host(HostCommand::List) => host_list(&paths, renderer),
        Command::Host(HostCommand::Remove { name }) => host_remove(&paths, renderer, name),
        Command::Host(HostCommand::Add(args)) => {
            hosts::add(&paths, renderer, args, &resolve::SystemLookup)
        }
        Command::Ssh(_) => Err(Error::NotImplemented("ssh")),
        Command::Config(ConfigCommand::Path) => {
            renderer.table(&[
                vec!["what".to_owned(), "path".to_owned()],
                vec![
                    "config".to_owned(),
                    paths.config_file().display().to_string(),
                ],
                vec![
                    "known_hosts".to_owned(),
                    paths.known_hosts().display().to_string(),
                ],
                vec!["state".to_owned(), paths.state_file().display().to_string()],
            ]);
            Ok(0)
        }
    }
}

/// `goway host list`: the configured pool and each host's cached address.
fn host_list(paths: &Paths, renderer: Renderer) -> Result<u8> {
    let config = Config::load(&paths.config_file())?;
    let state = state::State::load(&paths.state_file())?;
    if config.hosts.is_empty() {
        renderer.note(format_args!(
            "no hosts configured in {}; add one with `goway host add NAME`",
            paths.config_file().display()
        ));
        return Ok(0);
    }
    let mut rows = vec![vec![
        "host".to_owned(),
        "port".to_owned(),
        "user".to_owned(),
        "last address".to_owned(),
    ]];
    for host in &config.hosts {
        rows.push(vec![
            host.name.clone(),
            config.port_of(host).to_string(),
            host.user.clone().unwrap_or_else(|| "-".to_owned()),
            state
                .get(&host.name)
                .map_or_else(|| "-".to_owned(), |s| s.address.clone()),
        ]);
    }
    renderer.table(&rows);
    Ok(0)
}

/// `goway host remove`: drop the host from config, state and `known_hosts`.
fn host_remove(paths: &Paths, renderer: Renderer, name: &str) -> Result<u8> {
    config::remove_host(&paths.config_file(), name)?;
    let mut state = state::State::load(&paths.state_file())?;
    state.forget(name);
    state.save(&paths.state_file())?;
    let alias = config::key_alias(name);
    let kh = paths.known_hosts();
    if kh.exists() {
        match std::process::Command::new("ssh-keygen")
            .arg("-R")
            .arg(&alias)
            .arg("-f")
            .arg(&kh)
            .output()
        {
            Ok(out) if out.status.success() => tracing::info!(alias, "pinned key removed"),
            Ok(out) => renderer.warn(format_args!(
                "could not remove pinned key {alias}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )),
            Err(e) => renderer.warn(format_args!("could not run ssh-keygen: {e}")),
        }
    }
    renderer.ok(format_args!("removed host {name}"));
    Ok(0)
}
