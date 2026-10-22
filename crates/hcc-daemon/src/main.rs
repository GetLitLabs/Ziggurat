//! `cargo run -p hcc-daemon [-- --charter <path>]`
#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use hcc_daemon::{
    bind_loopback, probe_runner, serve, AccordionSwitchboardState, CharterStatus, ACCORDION_ADDR,
    DEFAULT_CHARTER_PATH, RUNNER_ADDR,
};

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let charter_path = match args.iter().position(|a| a == "--charter") {
        Some(i) => match args.get(i + 1) {
            Some(path) => PathBuf::from(path),
            None => {
                eprintln!("hcc-daemon: --charter needs a path");
                return ExitCode::FAILURE;
            }
        },
        None => PathBuf::from(DEFAULT_CHARTER_PATH),
    };

    let charter = CharterStatus::boot(&charter_path, hcc_seal::BAKED_SEAL.as_ref());
    eprintln!(
        "hcc-daemon: charter {}: {}",
        charter_path.display(),
        charter.summary()
    );
    if !charter.is_sealed() {
        eprintln!("hcc-daemon: serving status only; inference stays refused");
    }

    let listener = match bind_loopback(ACCORDION_ADDR).await {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("hcc-daemon: {err}");
            return ExitCode::FAILURE;
        }
    };
    if !probe_runner(RUNNER_ADDR).await {
        eprintln!("hcc-daemon: warn: no model runner on {RUNNER_ADDR} (weather); continuing");
    }
    eprintln!("hcc-daemon: listening on http://{ACCORDION_ADDR}");

    let state = Arc::new(AccordionSwitchboardState {
        charter,
        runner_addr: RUNNER_ADDR,
    });
    match serve(listener, state).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("hcc-daemon: {err}");
            ExitCode::FAILURE
        }
    }
}
