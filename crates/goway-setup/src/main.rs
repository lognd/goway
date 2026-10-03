//! The `goway-setup` executable.

use std::process::ExitCode;

use clap::Parser as _;
use goway_setup::cli::{Cli, run};
use goway_setup::render::Renderer;

fn main() -> ExitCode {
    let cli = Cli::parse();
    goway_setup::init_tracing(cli.verbose);
    let renderer = Renderer::new(cli.color);
    match run(&cli, renderer) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            renderer.error(&e);
            ExitCode::FAILURE
        }
    }
}
