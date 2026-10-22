//! `cargo run -p hcc-daemon [-- --charter <path>] [--db <path>]`
#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::ExitCode;

use hcc_daemon::{boot_and_serve, DEFAULT_CHARTER_PATH, DEFAULT_DB_PATH};

fn path_flag(args: &[String], name: &str, default: &str) -> Result<PathBuf, String> {
    match args.iter().position(|a| a == name) {
        Some(i) => args
            .get(i + 1)
            .map(PathBuf::from)
            .ok_or_else(|| format!("{name} needs a path")),
        None => Ok(PathBuf::from(default)),
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (charter_path, db_path) = match (
        path_flag(&args, "--charter", DEFAULT_CHARTER_PATH),
        path_flag(&args, "--db", DEFAULT_DB_PATH),
    ) {
        (Ok(charter), Ok(db)) => (charter, db),
        (Err(err), _) | (_, Err(err)) => {
            eprintln!("hcc-daemon: {err}");
            return ExitCode::FAILURE;
        }
    };

    match boot_and_serve(&charter_path, &db_path, |line| {
        eprintln!("hcc-daemon: {line}")
    })
    .await
    {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("hcc-daemon: {err}");
            ExitCode::FAILURE
        }
    }
}
