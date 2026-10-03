//! goway binary entry point; all logic lives in the library.

use std::process::ExitCode;

use clap::Parser as _;

fn main() -> ExitCode {
    goway::main_with(&goway::cli::Cli::parse())
}
