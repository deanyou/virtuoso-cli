#![allow(dead_code)]

mod async_runtime;
mod auth;
mod capability;
mod client;
mod command_log;
mod commands;
mod config;
mod error;
mod exit_codes;
mod history;
mod mcp;
mod models;
mod ocean;
mod output;
mod plugins;
mod rpc;
mod skill_finder;
mod spectre;
mod streaming;
mod transaction;
mod transport;
mod tui;
mod version;

pub use capability::{Capability, CapabilitySet};
pub use rpc::schema::standard_schema;
pub use transaction::{SchematicDiff, SchematicSnapshot, TransactionManager};

fn main() {
    if let Err(e) = tui::run_tui() {
        eprintln!("vtui error: {e}");
        std::process::exit(1);
    }
}
