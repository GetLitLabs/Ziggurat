//! Tauri 2 shell (M12 host). Starts Accordion.be on 127.0.0.1:7749 inside the app, then shows
//! the origami cockpit from `origami/dist`. The webview gets no Tauri commands or permissions:
//! it reads everything over loopback HTTP, so the cockpit sees exactly what any client sees.
#![forbid(unsafe_code)]

use std::process::ExitCode;

use hcc_daemon::{boot_and_serve_with, BootConfig};

pub fn run() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let config = match BootConfig::from_args(&args) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("ziggurat: {err}");
            return ExitCode::FAILURE;
        }
    };
    let app = tauri::Builder::default().setup(move |_app| {
        tauri::async_runtime::spawn(async move {
            let log = |line: &str| eprintln!("ziggurat: daemon: {line}");
            if let Err(err) = boot_and_serve_with(&config, log).await {
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
