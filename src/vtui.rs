#![allow(dead_code)]

mod client;
mod command_log;
mod commands;
mod config;
mod error;
mod exit_codes;
mod models;
mod ocean;
mod output;
mod spectre;
mod transport;
mod tui;

fn main() {
    if let Err(e) = tui::run_tui() {
        eprintln!("vtui error: {e}");
        std::process::exit(1);
    }
}
