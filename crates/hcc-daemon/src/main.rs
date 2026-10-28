//! `cargo run -p hcc-daemon [-- --charter <path>] [--db <path>] [--rmk <path>] [--workspace <path>]`
#![forbid(unsafe_code)]

use std::process::ExitCode;

use hcc_daemon::{boot_and_serve_with, BootConfig};

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let config = match BootConfig::from_args(&args) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("hcc-daemon: {err}");
            return ExitCode::FAILURE;
        }
    };
    match boot_and_serve_with(&config, |line| eprintln!("hcc-daemon: {line}")).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("hcc-daemon: {err}");
            ExitCode::FAILURE
        }
    }
}
