//! Tauri 2 shell (M12 host). Starts Accordion.be on 127.0.0.1:7749 inside the app, then shows
//! the origami cockpit from `origami/dist`. The webview gets no Tauri commands or permissions:
//! it reads everything over loopback HTTP, so the cockpit sees exactly what any client sees.
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

pub fn run() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (charter_path, db_path) = match (
        path_flag(&args, "--charter", DEFAULT_CHARTER_PATH),
        path_flag(&args, "--db", DEFAULT_DB_PATH),
    ) {
        (Ok(charter), Ok(db)) => (charter, db),
        (Err(err), _) | (_, Err(err)) => {
            eprintln!("ziggurat: {err}");
            return ExitCode::FAILURE;
        }
    };
    let app = tauri::Builder::default().setup(move |_app| {
        tauri::async_runtime::spawn(async move {
            let log = |line: &str| eprintln!("ziggurat: daemon: {line}");
            if let Err(err) = boot_and_serve(&charter_path, &db_path, log).await {
                // Usually: a standalone hcc-daemon already holds 127.0.0.1:7749. The cockpit
                // then talks to that one, or shows the daemon as unreachable in red.
                eprintln!("ziggurat: daemon: {err}");
            }
        });
        Ok(())
    });
    match app.run(tauri::generate_context!()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("ziggurat: {err}");
            ExitCode::FAILURE
        }
    }
}
