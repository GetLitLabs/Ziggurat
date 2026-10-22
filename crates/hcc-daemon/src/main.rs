//! `cargo run -p hcc-daemon [-- --charter <path>] [--db <path>]`
#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use hcc_daemon::{
    bind_loopback, probe_runner, serve, AccordionSwitchboardState, CharterStatus, SteleBoot,
    ACCORDION_ADDR, DEFAULT_CHARTER_PATH, DEFAULT_DB_PATH, RUNNER_ADDR,
};

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

    let charter = CharterStatus::boot(&charter_path, hcc_seal::BAKED_SEAL.as_ref());
    eprintln!(
        "hcc-daemon: charter {}: {}",
        charter_path.display(),
        charter.summary()
    );
    if !charter.is_sealed() {
        eprintln!("hcc-daemon: serving status only; inference stays refused");
    }
    let royal = hcc_seal::BAKED_SEAL.and_then(|baked| baked.royal);
    let stele = SteleBoot::boot(&db_path, &charter, royal.as_ref());
    eprintln!("hcc-daemon: stele: {}", stele.note);

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
        stele_genesis: stele.genesis,
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
