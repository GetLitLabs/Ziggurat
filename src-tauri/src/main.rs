//! `cargo run -p ziggurat-desktop [-- --charter <path>] [--db <path>]` (run from the repo root).
#![forbid(unsafe_code)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> std::process::ExitCode {
    ziggurat_desktop::run()
}
